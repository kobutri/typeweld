//! The proxy main loop.
//!
//! typeweld-ls is the Rust language server the editor talks to; behind it
//! runs the user's real rust-analyzer ([`crate::ra`]). Every message is
//! forwarded verbatim — opaque JSON, identical ids — except a small,
//! fail-open interception whitelist:
//!
//! - `textDocument/rename` responses gain the TypeScript half of an API
//!   rename (one atomic `WorkspaceEdit`),
//! - `textDocument/references` responses gain the TypeScript usages,
//! - `textDocument/hover` responses gain the contract block,
//! - `publishDiagnostics` notifications gain extraction diagnostics,
//! - document sync is observed in passing to keep the engine snapshot fresh
//!   and the generated package on disk current.
//!
//! "Fail-open" means: when an interception errors or panics, the original
//! message is forwarded untouched — a typeweld bug can cost typeweld
//! features, never Rust editing.
//!
//! Without a usable rust-analyzer the loop degrades to the engine-only
//! sidecar behavior: contract-based definitions, references, hover, rename,
//! and diagnostics.

use std::collections::HashMap;
use std::panic::AssertUnwindSafe;
use std::path::{Path, PathBuf};

use lsp_server::{Connection, ErrorCode, Message, Notification, Request, RequestId, Response};
use lsp_types::notification::{
    DidChangeTextDocument, DidChangeWatchedFiles, DidCloseTextDocument, DidOpenTextDocument,
    DidSaveTextDocument, Exit, Notification as _, PublishDiagnostics,
};
use lsp_types::request::{
    GotoDefinition, HoverRequest, PrepareRenameRequest, References, RegisterCapability, Rename,
    Request as _,
};
use lsp_types::{
    ClientCapabilities, DiagnosticSeverity, DidChangeTextDocumentParams,
    DidChangeWatchedFilesRegistrationOptions, DidCloseTextDocumentParams,
    DidOpenTextDocumentParams, FileSystemWatcher, GlobPattern, GotoDefinitionParams, HoverParams,
    HoverProviderCapability, InitializeParams, Location, NumberOrString, PositionEncodingKind,
    ReferenceParams, Registration, RegistrationParams, RenameOptions, RenameParams,
    ServerCapabilities, TextDocumentPositionParams, TextDocumentSyncCapability,
    TextDocumentSyncKind, WorkDoneProgressOptions, WorkspaceEdit,
};
use typeweld_engine::diag::{Diagnostic, Severity};

use crate::convert::{self, path_to_uri, uri_to_path, Encoding};
use crate::features;
use crate::plugin::{PluginEvent, PluginHub};
use crate::ra::{self, RaChild};
use crate::state::State;

type RequestResult = Result<serde_json::Value, (ErrorCode, String)>;

/// Options resolved before the server starts (CLI flags).
#[derive(Default)]
pub struct Options {
    /// Explicit rust-analyzer binary to wrap.
    pub rust_analyzer: Option<PathBuf>,
}

/// Runs the handshake and the main loop over `connection`.
pub fn run(connection: &Connection, options: &Options) -> Result<(), String> {
    let (request_id, raw_params) = connection
        .initialize_start()
        .map_err(|error| error.to_string())?;
    let params: InitializeParams = serde_json::from_value(raw_params.clone())
        .map_err(|error| format!("invalid initialize params: {error}"))?;
    let root = workspace_root(&params)
        .ok_or_else(|| "initialize params carry no workspace root".to_owned())?;

    let mut spawn_error = None;
    let ra = if wrap_enabled(&params) {
        ra::resolve_binary(options.rust_analyzer.as_deref()).and_then(
            |binary| match RaChild::spawn(&binary, &root, &raw_params) {
                Ok(spawned) => Some(spawned),
                Err(error) => {
                    spawn_error = Some(error);
                    None
                }
            },
        )
    } else {
        None
    };

    let (ra, encoding, initialize_result) = if let Some((child, mut result)) = ra {
        let encoding = encoding_from_result(&result);
        if let Some(object) = result.as_object_mut() {
            object.insert(
                "serverInfo".to_owned(),
                serde_json::json!({
                    "name": "typeweld-ls",
                    "version": env!("CARGO_PKG_VERSION"),
                }),
            );
        }
        (Some(child), encoding, result)
    } else {
        let encoding = negotiate_encoding(&params.capabilities);
        let result = serde_json::json!({
            "capabilities": server_capabilities(encoding),
            "serverInfo": { "name": "typeweld-ls", "version": env!("CARGO_PKG_VERSION") },
        });
        (None, encoding, result)
    };

    // `initialize_finish` also consumes the `initialized` notification.
    connection
        .initialize_finish(request_id, initialize_result)
        .map_err(|error| error.to_string())?;

    register_file_watchers(connection, &params.capabilities, ra.is_some());

    let mut server = Proxy {
        connection,
        state: State::new(root.clone(), encoding),
        ra,
        raw_initialize: raw_params,
        plugin: PluginHub::start(&root),
        pending: HashMap::new(),
        injected_renames: std::collections::HashSet::new(),
        ra_diagnostics: HashMap::new(),
        our_diagnostics: HashMap::new(),
        next_internal: 0,
        pushed_generation: 0,
        pending_saves: HashMap::new(),
        shutting_down: false,
    };
    if let Some(error) = spawn_error {
        server.show_message(
            1,
            &format!("typeweld: failed to start rust-analyzer ({error}); running degraded"),
        );
    }
    let _ = std::panic::catch_unwind(AssertUnwindSafe(|| server.refresh()));
    server.main_loop()
}

/// Whether to wrap a rust-analyzer child: initialization option
/// `{ "rustAnalyzer": "off" }` or `TYPEWELD_RUST_ANALYZER=off` disable it.
fn wrap_enabled(params: &InitializeParams) -> bool {
    if std::env::var("TYPEWELD_RUST_ANALYZER").is_ok_and(|value| value == "off") {
        return false;
    }
    params
        .initialization_options
        .as_ref()
        .and_then(|options| options.get("rustAnalyzer"))
        .and_then(serde_json::Value::as_str)
        != Some("off")
}

/// The encoding rust-analyzer negotiated with the editor; our augmentations
/// must speak the same one.
fn encoding_from_result(result: &serde_json::Value) -> Encoding {
    match result
        .pointer("/capabilities/positionEncoding")
        .and_then(serde_json::Value::as_str)
    {
        Some("utf-8") => Encoding::Utf8,
        _ => Encoding::Utf16,
    }
}

/// One forwarded editor request awaiting the rust-analyzer response.
struct Pending {
    method: String,
    params: serde_json::Value,
}

struct Proxy<'a> {
    connection: &'a Connection,
    state: State,
    ra: Option<RaChild>,
    raw_initialize: serde_json::Value,
    plugin: Option<PluginHub>,
    pending: HashMap<RequestId, Pending>,
    /// Renames we injected into rust-analyzer ourselves (the Rust complement
    /// of a TypeScript-initiated rename): the resulting edit is filtered to
    /// Rust files and applied via `workspace/applyEdit`.
    injected_renames: std::collections::HashSet<RequestId>,
    /// rust-analyzer's last published diagnostics per file URI, merged with
    /// ours on every publish from either side.
    ra_diagnostics: HashMap<String, Vec<serde_json::Value>>,
    our_diagnostics: HashMap<String, Vec<serde_json::Value>>,
    next_internal: u64,
    /// The engine generation last pushed to the plugins.
    pushed_generation: u64,
    /// Outstanding `workspace/applyEdit` requests and the files to ask the
    /// editor extension to save once the editor confirms the edit (editors
    /// auto-save their own rename edits, but not server-applied ones).
    pending_saves: HashMap<RequestId, Vec<String>>,
    shutting_down: bool,
}

impl Proxy<'_> {
    fn main_loop(&mut self) -> Result<(), String> {
        loop {
            let ra_receiver = self
                .ra
                .as_ref()
                .map_or_else(crossbeam_channel::never, |child| child.incoming.clone());
            let plugin_receiver = self
                .plugin
                .as_ref()
                .map_or_else(crossbeam_channel::never, |hub| hub.events.clone());
            let editor_receiver = self.connection.receiver.clone();

            crossbeam_channel::select! {
                recv(editor_receiver) -> message => {
                    let Ok(message) = message else { return Ok(()) };
                    if self.on_editor_message(message) {
                        return Ok(());
                    }
                }
                recv(ra_receiver) -> message => {
                    match message {
                        Ok(message) => self.on_ra_message(message),
                        Err(_) => self.on_ra_exit(),
                    }
                }
                recv(plugin_receiver) -> event => {
                    if let Ok(event) = event {
                        let _ = std::panic::catch_unwind(AssertUnwindSafe(|| {
                            self.on_plugin_event(event);
                        }));
                    }
                }
            }
        }
    }

    /// Handles one message from the editor. Returns `true` on exit.
    fn on_editor_message(&mut self, message: Message) -> bool {
        match message {
            Message::Request(request) => {
                if request.method == "shutdown" {
                    self.shutting_down = true;
                    self.forward_to_ra(&Message::Request(Request::new(
                        RequestId::from("typeweld:shutdown".to_owned()),
                        "shutdown".to_owned(),
                        serde_json::Value::Null,
                    )));
                    self.respond(Response::new_ok(request.id, serde_json::Value::Null));
                    return false;
                }
                if self.ra.is_some() {
                    self.pending.insert(
                        request.id.clone(),
                        Pending {
                            method: request.method.clone(),
                            params: request.params.clone(),
                        },
                    );
                    self.forward_to_ra(&Message::Request(request));
                } else {
                    self.answer_locally(&request);
                }
                false
            }
            Message::Notification(notification) => {
                if notification.method == Exit::METHOD {
                    self.forward_to_ra(&Message::Notification(notification));
                    return true;
                }
                let _ = std::panic::catch_unwind(AssertUnwindSafe(|| {
                    self.observe_notification(&notification);
                }));
                self.forward_to_ra(&Message::Notification(notification));
                false
            }
            Message::Response(response) => {
                // The editor answering a server-to-client request: ours are
                // namespaced, everything else belongs to rust-analyzer.
                if id_text(&response.id).starts_with("typeweld") {
                    if let Some(files) = self.pending_saves.remove(&response.id) {
                        let applied = response
                            .result
                            .as_ref()
                            .and_then(|result| result.get("applied"))
                            .and_then(serde_json::Value::as_bool)
                            .unwrap_or(false);
                        if applied {
                            if let Some(hub) = &self.plugin {
                                hub.request_saves(&files);
                            }
                        }
                    }
                } else {
                    self.forward_to_ra(&Message::Response(response));
                }
                false
            }
        }
    }

    /// Handles one message from the rust-analyzer child.
    fn on_ra_message(&mut self, message: Message) {
        match message {
            Message::Response(mut response) => {
                if let Some(pending) = self.pending.remove(&response.id) {
                    let outcome = std::panic::catch_unwind(AssertUnwindSafe(|| {
                        self.augment_response(&pending, &mut response);
                    }));
                    drop(outcome); // fail-open: forward what we have
                    self.respond(response);
                } else if self.injected_renames.remove(&response.id) {
                    let _ = std::panic::catch_unwind(AssertUnwindSafe(|| {
                        self.on_injected_rename_response(response);
                    }));
                }
            }
            Message::Request(request) => {
                self.send(Message::Request(request));
            }
            Message::Notification(notification) => {
                if notification.method == PublishDiagnostics::METHOD {
                    let merged = self.merge_ra_diagnostics(&notification.params);
                    self.send(Message::Notification(Notification::new(
                        PublishDiagnostics::METHOD.to_owned(),
                        merged,
                    )));
                } else {
                    self.send(Message::Notification(notification));
                }
            }
        }
    }

    /// The rust-analyzer channel disconnected: fail pending requests, then
    /// respawn (replaying the handshake and open documents) or degrade.
    fn on_ra_exit(&mut self) {
        let Some(child) = self.ra.take() else {
            return;
        };
        if self.shutting_down {
            return;
        }
        let pending: Vec<RequestId> = self.pending.drain().map(|(id, _)| id).collect();
        for id in pending {
            self.respond(Response::new_err(
                id,
                ErrorCode::InternalError as i32,
                "rust-analyzer exited; retry shortly".to_owned(),
            ));
        }
        self.injected_renames.clear();

        let open_documents: Vec<(PathBuf, String, i32)> = self
            .state
            .open_documents()
            .into_iter()
            .filter(|(path, _, _)| is_rust(path))
            .collect();
        match child.respawn(&self.raw_initialize, &open_documents) {
            Ok(respawned) => {
                self.ra = Some(respawned);
                self.show_message(2, "typeweld: rust-analyzer crashed and was restarted");
            }
            Err(error) => {
                self.show_message(
                    1,
                    &format!("typeweld: rust-analyzer unavailable ({error}); running degraded"),
                );
            }
        }
    }

    /// Reacts to a plugin event: a TypeScript-initiated rename of an API
    /// symbol whose Rust complement we now owe.
    fn on_plugin_event(&mut self, event: PluginEvent) {
        let PluginEvent::RenameLanded { symbol, new_name } = event;
        self.state.ensure_fresh();
        let Some(plan) = features::rename::rust_complement(&self.state, &symbol, &new_name) else {
            return;
        };
        match (&plan.decl_position, &mut self.ra) {
            (Some((path, position)), Some(_)) => {
                let id = self.fresh_internal_id("rename");
                self.injected_renames.insert(id.clone());
                let params = serde_json::json!({
                    "textDocument": { "uri": path_to_uri(path) },
                    "position": position,
                    "newName": plan.rust_name,
                });
                self.forward_to_ra(&Message::Request(Request::new(
                    id,
                    Rename::METHOD.to_owned(),
                    params,
                )));
            }
            _ => {
                // No semantic backend: apply the contract-known Rust edits.
                self.apply_rust_complement(&plan.fallback_edit);
            }
        }
    }

    fn on_injected_rename_response(&mut self, response: Response) {
        let Some(result) = response.result else {
            return;
        };
        let Ok(edit) = serde_json::from_value::<WorkspaceEdit>(result) else {
            return;
        };
        let rust_only = features::rename::filter_to_rust(edit);
        self.apply_rust_complement(&rust_only);
    }

    /// Applies the Rust complement of a TypeScript-initiated rename: files
    /// the editor has open go through `workspace/applyEdit`; everything else
    /// is written straight to disk so no buffers are left dirty. Regeneration
    /// follows immediately instead of waiting for watcher events.
    fn apply_rust_complement(&mut self, edit: &WorkspaceEdit) {
        let mut open = Vec::new();
        for document in features::rename::document_edits(edit) {
            let Some(path) = uri_to_path(&document.text_document.uri) else {
                continue;
            };
            if self.state.version_of(&path).is_some() {
                open.push(document);
                continue;
            }
            let Some(text) = self.state.read_text(&path) else {
                continue;
            };
            let mut text = text.to_string();
            let mut spans: Vec<(usize, usize, String)> = document
                .edits
                .iter()
                .map(|edit| {
                    let edit = match edit {
                        lsp_types::OneOf::Left(edit) => edit,
                        lsp_types::OneOf::Right(annotated) => &annotated.text_edit,
                    };
                    let start = convert::offset_at(&text, edit.range.start, self.state.encoding());
                    let end = convert::offset_at(&text, edit.range.end, self.state.encoding());
                    (start as usize, end as usize, edit.new_text.clone())
                })
                .collect();
            spans.sort_by_key(|(start, _, _)| std::cmp::Reverse(*start));
            for (start, end, new_text) in spans {
                if start <= end && end <= text.len() {
                    text.replace_range(start..end, &new_text);
                }
            }
            let _ = std::fs::write(&path, text);
        }
        if !open.is_empty() {
            let files: Vec<String> = open
                .iter()
                .filter_map(|document| uri_to_path(&document.text_document.uri))
                .map(|path| path.to_string_lossy().into_owned())
                .collect();
            let id = self.apply_edit(&WorkspaceEdit {
                changes: None,
                document_changes: Some(lsp_types::DocumentChanges::Edits(open)),
                change_annotations: None,
            });
            if let Some(id) = id {
                self.pending_saves.insert(id, files);
            }
        }
        self.state.mark_dirty();
        self.refresh();
    }

    /// Augments a rust-analyzer response in place; any failure leaves it
    /// untouched.
    fn augment_response(&mut self, pending: &Pending, response: &mut Response) {
        if response.error.is_some() {
            return;
        }
        match pending.method.as_str() {
            Rename::METHOD => {
                let Ok(params) = serde_json::from_value::<RenameParams>(pending.params.clone())
                else {
                    return;
                };
                self.state.ensure_fresh();
                let base = response.result.take().unwrap_or(serde_json::Value::Null);
                let parsed = if base.is_null() {
                    Ok(WorkspaceEdit::default())
                } else {
                    serde_json::from_value::<WorkspaceEdit>(base.clone())
                };
                let Ok(mut edit) = parsed else {
                    response.result = Some(base);
                    return;
                };
                let Some(complement) = features::rename::ts_complement(
                    &self.state,
                    self.plugin.as_ref(),
                    &params,
                    Some(&edit),
                ) else {
                    response.result = Some(base);
                    return;
                };
                merge_workspace_edits(&mut edit, complement);
                response.result = serde_json::to_value(edit).ok().or(Some(base));
            }
            References::METHOD => {
                let Ok(params) = serde_json::from_value::<ReferenceParams>(pending.params.clone())
                else {
                    return;
                };
                self.state.ensure_fresh();
                let position = &params.text_document_position;
                let extra = features::references::ts_locations(
                    &self.state,
                    self.plugin.as_ref(),
                    &position.text_document.uri,
                    position.position,
                );
                if extra.is_empty() {
                    return;
                }
                let base = response.result.take().unwrap_or(serde_json::Value::Null);
                let parsed: Result<Vec<Location>, _> = if base.is_null() {
                    Ok(Vec::new())
                } else {
                    serde_json::from_value(base.clone())
                };
                let Ok(mut locations) = parsed else {
                    response.result = Some(base);
                    return;
                };
                for location in extra {
                    if !locations.contains(&location) {
                        locations.push(location);
                    }
                }
                response.result = serde_json::to_value(locations).ok().or(Some(base));
            }
            HoverRequest::METHOD => {
                let Ok(params) = serde_json::from_value::<HoverParams>(pending.params.clone())
                else {
                    return;
                };
                self.state.ensure_fresh();
                let position = &params.text_document_position_params;
                let Some(markdown) = features::hover::contract_markdown(
                    &self.state,
                    &position.text_document.uri,
                    position.position,
                ) else {
                    return;
                };
                let result = response.result.take().unwrap_or(serde_json::Value::Null);
                response.result = Some(append_hover_markdown(result, &markdown));
            }
            _ => {}
        }
    }

    /// Observes a document-sync notification to keep the engine fresh; the
    /// notification is still forwarded to rust-analyzer afterwards.
    fn observe_notification(&mut self, notification: &Notification) {
        match notification.method.as_str() {
            DidOpenTextDocument::METHOD => {
                let Ok(params) = serde_json::from_value::<DidOpenTextDocumentParams>(
                    notification.params.clone(),
                ) else {
                    return;
                };
                let Some(path) = uri_to_path(&params.text_document.uri) else {
                    return;
                };
                self.state.open(
                    path,
                    params.text_document.text,
                    params.text_document.version,
                );
                self.refresh();
            }
            DidChangeTextDocument::METHOD => {
                let Ok(params) = serde_json::from_value::<DidChangeTextDocumentParams>(
                    notification.params.clone(),
                ) else {
                    return;
                };
                let Some(path) = uri_to_path(&params.text_document.uri) else {
                    return;
                };
                // In proxy mode the editor negotiated rust-analyzer's
                // incremental sync, so changes may be ranged edits over the
                // current overlay; degraded mode advertises full sync and
                // arrives here with range-less changes.
                let Some(text) = apply_content_changes(&self.state, &path, params.content_changes)
                else {
                    return;
                };
                self.state.change(path, text, params.text_document.version);
                self.refresh();
            }
            DidCloseTextDocument::METHOD => {
                let Ok(params) = serde_json::from_value::<DidCloseTextDocumentParams>(
                    notification.params.clone(),
                ) else {
                    return;
                };
                let Some(path) = uri_to_path(&params.text_document.uri) else {
                    return;
                };
                self.state.close(&path);
                self.refresh();
            }
            DidSaveTextDocument::METHOD | DidChangeWatchedFiles::METHOD => {
                self.state.mark_dirty();
                self.refresh();
            }
            _ => {}
        }
    }

    /// Rebuilds the snapshot if dirty, publishes merged diagnostics, and
    /// pushes the plugin snapshot.
    fn refresh(&mut self) {
        let publishes = self.state.ensure_fresh();
        for (path, diagnostics) in publishes {
            let text = self.state.read_text(&path);
            let text = text.as_deref().unwrap_or("");
            let rendered: Vec<serde_json::Value> = diagnostics
                .iter()
                .filter_map(|diagnostic| {
                    serde_json::to_value(lsp_diagnostic(text, diagnostic, self.state.encoding()))
                        .ok()
                })
                .collect();
            let uri = path_to_uri(&path);
            let key = uri.as_str().to_owned();
            if rendered.is_empty() {
                self.our_diagnostics.remove(&key);
            } else {
                self.our_diagnostics.insert(key.clone(), rendered);
            }
            self.publish_merged(&key, None);
        }
        if let Some(hub) = &self.plugin {
            if self.pushed_generation != self.state.generation() {
                if let Some(snapshot) = features::plugin_snapshot(&self.state) {
                    hub.set_snapshot(&snapshot);
                    self.pushed_generation = self.state.generation();
                }
            }
        }
    }

    /// Caches rust-analyzer's diagnostics for the file and returns the merged
    /// params to forward.
    fn merge_ra_diagnostics(&mut self, params: &serde_json::Value) -> serde_json::Value {
        let uri = params
            .get("uri")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default()
            .to_owned();
        let from_ra = params
            .get("diagnostics")
            .and_then(serde_json::Value::as_array)
            .cloned()
            .unwrap_or_default();
        let version = params.get("version").cloned();
        if from_ra.is_empty() {
            self.ra_diagnostics.remove(&uri);
        } else {
            self.ra_diagnostics.insert(uri.clone(), from_ra);
        }
        self.merged_params(&uri, version)
    }

    /// Publishes the merged diagnostics for one file URI.
    fn publish_merged(&mut self, uri: &str, version: Option<serde_json::Value>) {
        let params = self.merged_params(uri, version);
        self.send(Message::Notification(Notification::new(
            PublishDiagnostics::METHOD.to_owned(),
            params,
        )));
    }

    fn merged_params(&self, uri: &str, version: Option<serde_json::Value>) -> serde_json::Value {
        let mut diagnostics = self.ra_diagnostics.get(uri).cloned().unwrap_or_default();
        diagnostics.extend(self.our_diagnostics.get(uri).cloned().unwrap_or_default());
        let mut params = serde_json::json!({
            "uri": uri,
            "diagnostics": diagnostics,
        });
        if let (Some(version), Some(object)) = (version, params.as_object_mut()) {
            object.insert("version".to_owned(), version);
        }
        params
    }

    /// Answers a request from the engine snapshot — the degraded path used
    /// when no rust-analyzer runs.
    fn answer_locally(&mut self, request: &Request) {
        let id = request.id.clone();
        let outcome = std::panic::catch_unwind(AssertUnwindSafe(|| {
            self.state.ensure_fresh();
            handle_request(&self.state, self.plugin.as_ref(), request)
        }));
        let response = match outcome {
            Ok(Ok(value)) => Response::new_ok(id, value),
            Ok(Err((code, message))) => Response::new_err(id, code as i32, message),
            Err(_) => Response::new_err(
                id,
                ErrorCode::InternalError as i32,
                "typeweld-ls request handler panicked".to_owned(),
            ),
        };
        self.respond(response);
    }

    /// Applies a workspace edit through the editor.
    fn apply_edit(&mut self, edit: &WorkspaceEdit) -> Option<RequestId> {
        let empty = edit.document_changes.is_none() && edit.changes.is_none();
        if empty {
            return None;
        }
        let id = self.fresh_internal_id("apply");
        self.send(Message::Request(Request::new(
            id.clone(),
            "workspace/applyEdit".to_owned(),
            serde_json::json!({ "edit": edit }),
        )));
        Some(id)
    }

    fn fresh_internal_id(&mut self, kind: &str) -> RequestId {
        self.next_internal += 1;
        RequestId::from(format!("typeweld:{kind}:{}", self.next_internal))
    }

    fn show_message(&self, kind: i32, message: &str) {
        let _ = self
            .connection
            .sender
            .send(Message::Notification(Notification::new(
                "window/showMessage".to_owned(),
                serde_json::json!({ "type": kind, "message": message }),
            )));
    }

    fn forward_to_ra(&mut self, message: &Message) {
        let Some(child) = &mut self.ra else { return };
        if child.send(message).is_err() {
            self.on_ra_exit();
        }
    }

    fn respond(&self, response: Response) {
        let _ = self.connection.sender.send(Message::Response(response));
    }

    fn send(&self, message: Message) {
        let _ = self.connection.sender.send(message);
    }
}

/// Applies a didChange content-change sequence over the current document
/// text. Ranged changes splice into the overlay (or disk text for the first
/// change after a missed didOpen); range-less changes replace it whole.
fn apply_content_changes(
    state: &State,
    path: &Path,
    changes: Vec<lsp_types::TextDocumentContentChangeEvent>,
) -> Option<String> {
    let mut text: Option<String> = state.read_text(path).map(|text| text.to_string());
    for change in changes {
        match change.range {
            None => text = Some(change.text),
            Some(range) => {
                let current = text.as_mut()?;
                let start = convert::offset_at(current, range.start, state.encoding()) as usize;
                let end = convert::offset_at(current, range.end, state.encoding()) as usize;
                let start = start.min(current.len());
                let end = end.clamp(start, current.len());
                current.replace_range(start..end, &change.text);
            }
        }
    }
    text
}

fn id_text(id: &RequestId) -> String {
    // RequestId renders numbers and strings distinctly; strings keep their
    // text, which is all the namespace check needs.
    let rendered = format!("{id}");
    rendered.trim_matches('"').to_owned()
}

fn is_rust(path: &Path) -> bool {
    path.extension().and_then(|extension| extension.to_str()) == Some("rs")
}

/// Merges `extra` into `base`, preserving whichever change representation
/// the base already uses.
fn merge_workspace_edits(base: &mut WorkspaceEdit, extra: WorkspaceEdit) {
    use lsp_types::DocumentChanges;

    let extra_documents = match extra.document_changes {
        Some(DocumentChanges::Edits(documents)) => documents,
        _ => Vec::new(),
    };
    if extra_documents.is_empty() {
        return;
    }
    if let Some(changes) = &mut base.changes {
        for document in extra_documents {
            let edits = changes.entry(document.text_document.uri).or_default();
            for edit in document.edits {
                let edit = match edit {
                    lsp_types::OneOf::Left(edit) => edit,
                    lsp_types::OneOf::Right(annotated) => annotated.text_edit,
                };
                edits.push(edit);
            }
        }
        return;
    }
    match &mut base.document_changes {
        Some(DocumentChanges::Edits(documents)) => documents.extend(extra_documents),
        Some(DocumentChanges::Operations(operations)) => operations.extend(
            extra_documents
                .into_iter()
                .map(lsp_types::DocumentChangeOperation::Edit),
        ),
        None => base.document_changes = Some(DocumentChanges::Edits(extra_documents)),
    }
}

/// Appends the contract block to a hover result, whatever shape it has.
fn append_hover_markdown(result: serde_json::Value, markdown: &str) -> serde_json::Value {
    if result.is_null() {
        return serde_json::json!({
            "contents": { "kind": "markdown", "value": markdown },
        });
    }
    let mut result = result;
    if let Some(value) = result.pointer_mut("/contents/value").and_then(|value| {
        value
            .as_str()
            .map(ToOwned::to_owned)
            .map(|text| (value, text))
    }) {
        let (slot, text) = value;
        *slot = serde_json::Value::String(format!("{text}\n\n---\n\n{markdown}"));
    }
    result
}

#[allow(deprecated)] // `root_uri` is the standard fallback for older clients.
fn workspace_root(params: &InitializeParams) -> Option<PathBuf> {
    if let Some(folders) = &params.workspace_folders {
        if let Some(folder) = folders.first() {
            return uri_to_path(&folder.uri);
        }
    }
    params.root_uri.as_ref().and_then(uri_to_path)
}

fn negotiate_encoding(capabilities: &ClientCapabilities) -> Encoding {
    let encodings = capabilities
        .general
        .as_ref()
        .and_then(|general| general.position_encodings.as_deref())
        .unwrap_or(&[]);
    if !encodings.is_empty()
        && !encodings.contains(&PositionEncodingKind::UTF16)
        && encodings.contains(&PositionEncodingKind::UTF8)
    {
        Encoding::Utf8
    } else {
        Encoding::Utf16
    }
}

fn server_capabilities(encoding: Encoding) -> ServerCapabilities {
    let position_encoding = match encoding {
        Encoding::Utf8 => PositionEncodingKind::UTF8,
        Encoding::Utf16 => PositionEncodingKind::UTF16,
    };
    ServerCapabilities {
        position_encoding: Some(position_encoding),
        text_document_sync: Some(TextDocumentSyncCapability::Kind(TextDocumentSyncKind::FULL)),
        definition_provider: Some(lsp_types::OneOf::Left(true)),
        references_provider: Some(lsp_types::OneOf::Left(true)),
        hover_provider: Some(HoverProviderCapability::Simple(true)),
        rename_provider: Some(lsp_types::OneOf::Right(RenameOptions {
            prepare_provider: Some(true),
            work_done_progress_options: WorkDoneProgressOptions::default(),
        })),
        ..ServerCapabilities::default()
    }
}

/// Registers our own file watchers (generation inputs rust-analyzer does not
/// watch for us). In proxy mode rust-analyzer registers its own Rust
/// watchers; we only add the TypeScript and config patterns.
fn register_file_watchers(
    connection: &Connection,
    capabilities: &ClientCapabilities,
    proxying: bool,
) {
    let dynamic = capabilities
        .workspace
        .as_ref()
        .and_then(|workspace| workspace.did_change_watched_files.as_ref())
        .and_then(|watched| watched.dynamic_registration)
        .unwrap_or(false);
    if !dynamic {
        return;
    }
    let patterns: &[&str] = if proxying {
        &["**/*.ts", "**/typeweld.toml"]
    } else {
        &["**/*.rs", "**/Cargo.toml", "**/typeweld.toml", "**/*.ts"]
    };
    let watchers = patterns
        .iter()
        .map(|pattern| FileSystemWatcher {
            glob_pattern: GlobPattern::String((*pattern).to_owned()),
            kind: None,
        })
        .collect();
    let registration = Registration {
        id: "typeweld-watched-files".to_owned(),
        method: DidChangeWatchedFiles::METHOD.to_owned(),
        register_options: serde_json::to_value(DidChangeWatchedFilesRegistrationOptions {
            watchers,
        })
        .ok(),
    };
    let request = Request::new(
        RequestId::from("typeweld:register-watchers".to_owned()),
        RegisterCapability::METHOD.to_owned(),
        RegistrationParams {
            registrations: vec![registration],
        },
    );
    let _ = connection.sender.send(Message::Request(request));
}

fn lsp_diagnostic(
    text: &str,
    diagnostic: &Diagnostic,
    encoding: Encoding,
) -> lsp_types::Diagnostic {
    let range = diagnostic
        .span
        .as_ref()
        .map(|span| convert::range(text, span.start, span.end, encoding))
        .unwrap_or_default();
    lsp_types::Diagnostic {
        range,
        severity: Some(match diagnostic.severity {
            Severity::Error => DiagnosticSeverity::ERROR,
            Severity::Warning => DiagnosticSeverity::WARNING,
        }),
        code: Some(NumberOrString::String(diagnostic.code.to_owned())),
        source: Some("typeweld".to_owned()),
        message: diagnostic.message.clone(),
        ..lsp_types::Diagnostic::default()
    }
}

/// The degraded-mode request dispatch: engine-snapshot answers only.
fn handle_request(state: &State, plugin: Option<&PluginHub>, request: &Request) -> RequestResult {
    match request.method.as_str() {
        GotoDefinition::METHOD => {
            let params: GotoDefinitionParams = parse_params(request)?;
            to_result(features::definition::handle(state, &params))
        }
        References::METHOD => {
            let params: ReferenceParams = parse_params(request)?;
            to_result(features::references::handle(state, plugin, &params))
        }
        HoverRequest::METHOD => {
            let params: HoverParams = parse_params(request)?;
            to_result(features::hover::handle(state, &params))
        }
        PrepareRenameRequest::METHOD => {
            let params: TextDocumentPositionParams = parse_params(request)?;
            to_result(features::rename::prepare(state, &params))
        }
        Rename::METHOD => {
            let params: RenameParams = parse_params(request)?;
            let edit = features::rename::handle(state, plugin, &params)
                .map_err(|message| (ErrorCode::InvalidParams, message))?;
            to_result(edit)
        }
        other => Err((
            ErrorCode::MethodNotFound,
            format!("unsupported method `{other}`"),
        )),
    }
}

fn parse_params<P: serde::de::DeserializeOwned>(
    request: &Request,
) -> Result<P, (ErrorCode, String)> {
    serde_json::from_value(request.params.clone())
        .map_err(|error| (ErrorCode::InvalidParams, format!("invalid params: {error}")))
}

fn to_result(value: impl serde::Serialize) -> RequestResult {
    serde_json::to_value(value).map_err(|error| {
        (
            ErrorCode::InternalError,
            format!("serialization failed: {error}"),
        )
    })
}

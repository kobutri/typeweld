//! The main loop: handshake, capability negotiation, and message dispatch.

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
    HoverProviderCapability, InitializeParams, NumberOrString, PositionEncodingKind,
    PublishDiagnosticsParams, ReferenceParams, Registration, RegistrationParams, RenameOptions,
    RenameParams, ServerCapabilities, TextDocumentPositionParams, TextDocumentSyncCapability,
    TextDocumentSyncKind, WorkDoneProgressOptions,
};
use typeweld_engine::diag::{Diagnostic, Severity};

use crate::convert::{self, path_to_uri, uri_to_path, Encoding};
use crate::features;
use crate::rust_backend::RustBackend;
use crate::state::State;

type RequestResult = Result<serde_json::Value, (ErrorCode, String)>;

/// Runs the handshake and the main loop over `connection`.
pub fn run(connection: &Connection) -> Result<(), String> {
    let (request_id, params) = connection
        .initialize_start()
        .map_err(|error| error.to_string())?;
    let params: InitializeParams = serde_json::from_value(params)
        .map_err(|error| format!("invalid initialize params: {error}"))?;

    let root = workspace_root(&params)
        .ok_or_else(|| "initialize params carry no workspace root".to_owned())?;
    let encoding = negotiate_encoding(&params.capabilities);
    let result = serde_json::json!({
        "capabilities": server_capabilities(encoding),
        "serverInfo": { "name": "typeweld-ls", "version": env!("CARGO_PKG_VERSION") },
    });
    // `initialize_finish` also consumes the `initialized` notification.
    connection
        .initialize_finish(request_id, result)
        .map_err(|error| error.to_string())?;

    register_file_watchers(connection, &params.capabilities);

    // The private rust-analyzer child is a pure query backend — never exposed
    // to the editor — and spawns lazily on the first semantic rename.
    let mut rust_backend =
        rust_backend_enabled(&params).then(|| RustBackend::new(root.clone(), None));

    let mut state = State::new(root, encoding);
    let _ = std::panic::catch_unwind(AssertUnwindSafe(|| {
        publish_fresh_diagnostics(&mut state, connection);
    }));

    main_loop(connection, &mut state, &mut rust_backend)
}

/// Whether the semantic Rust backend may be used this session: initialization
/// options `{ "rustBackend": "lazy" | "off" }` (default `lazy`), with the
/// `TYPEWELD_RUST_BACKEND=off` environment variable as a hard override.
fn rust_backend_enabled(params: &InitializeParams) -> bool {
    if std::env::var("TYPEWELD_RUST_BACKEND").is_ok_and(|value| value == "off") {
        return false;
    }
    params
        .initialization_options
        .as_ref()
        .and_then(|options| options.get("rustBackend"))
        .and_then(serde_json::Value::as_str)
        != Some("off")
}

fn main_loop(
    connection: &Connection,
    state: &mut State,
    rust_backend: &mut Option<RustBackend>,
) -> Result<(), String> {
    for message in &connection.receiver {
        match message {
            Message::Request(request) => {
                match connection.handle_shutdown(&request) {
                    Ok(true) => return Ok(()),
                    Ok(false) => {}
                    Err(error) => return Err(error.to_string()),
                }
                let id = request.id.clone();
                let outcome = std::panic::catch_unwind(AssertUnwindSafe(|| {
                    publish_fresh_diagnostics(state, connection);
                    handle_request(state, rust_backend, &request)
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
                connection
                    .sender
                    .send(Message::Response(response))
                    .map_err(|error| error.to_string())?;
            }
            Message::Notification(notification) => {
                if notification.method == Exit::METHOD {
                    return Ok(());
                }
                let _ = std::panic::catch_unwind(AssertUnwindSafe(|| {
                    handle_notification(state, rust_backend, connection, notification);
                }));
            }
            Message::Response(_) => {}
        }
    }
    Ok(())
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

/// Registers watchers for the files generation depends on, when the client
/// supports dynamic registration. Clients that do not are served through
/// didOpen/didChange/didSave alone.
fn register_file_watchers(connection: &Connection, capabilities: &ClientCapabilities) {
    let dynamic = capabilities
        .workspace
        .as_ref()
        .and_then(|workspace| workspace.did_change_watched_files.as_ref())
        .and_then(|watched| watched.dynamic_registration)
        .unwrap_or(false);
    if !dynamic {
        return;
    }
    let watchers = ["**/*.rs", "**/Cargo.toml", "**/typeweld.toml", "**/*.ts"]
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
        RequestId::from("typeweld-register-watchers".to_owned()),
        RegisterCapability::METHOD.to_owned(),
        RegistrationParams {
            registrations: vec![registration],
        },
    );
    let _ = connection.sender.send(Message::Request(request));
}

/// Rebuilds the snapshot if dirty and publishes the resulting diagnostics.
fn publish_fresh_diagnostics(state: &mut State, connection: &Connection) {
    for (path, diagnostics) in state.ensure_fresh() {
        let text = state.read_text(&path);
        let text = text.as_deref().unwrap_or("");
        let diagnostics = diagnostics
            .iter()
            .map(|diagnostic| lsp_diagnostic(text, diagnostic, state.encoding()))
            .collect();
        let params = PublishDiagnosticsParams {
            uri: path_to_uri(&path),
            diagnostics,
            version: None,
        };
        let notification = Notification::new(PublishDiagnostics::METHOD.to_owned(), params);
        let _ = connection.sender.send(Message::Notification(notification));
    }
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

fn handle_notification(
    state: &mut State,
    rust_backend: &mut Option<RustBackend>,
    connection: &Connection,
    notification: Notification,
) {
    match notification.method.as_str() {
        DidOpenTextDocument::METHOD => {
            let Ok(params) =
                serde_json::from_value::<DidOpenTextDocumentParams>(notification.params)
            else {
                return;
            };
            let Some(path) = uri_to_path(&params.text_document.uri) else {
                return;
            };
            sync_rust_document(
                rust_backend,
                &path,
                &params.text_document.text,
                params.text_document.version,
            );
            state.open(
                path,
                params.text_document.text,
                params.text_document.version,
            );
            publish_fresh_diagnostics(state, connection);
        }
        DidChangeTextDocument::METHOD => {
            let Ok(params) =
                serde_json::from_value::<DidChangeTextDocumentParams>(notification.params)
            else {
                return;
            };
            let Some(path) = uri_to_path(&params.text_document.uri) else {
                return;
            };
            // Full sync: the last change carries the complete new text.
            let Some(change) = params.content_changes.into_iter().last() else {
                return;
            };
            sync_rust_document(
                rust_backend,
                &path,
                &change.text,
                params.text_document.version,
            );
            state.change(path, change.text, params.text_document.version);
            publish_fresh_diagnostics(state, connection);
        }
        DidCloseTextDocument::METHOD => {
            let Ok(params) =
                serde_json::from_value::<DidCloseTextDocumentParams>(notification.params)
            else {
                return;
            };
            let Some(path) = uri_to_path(&params.text_document.uri) else {
                return;
            };
            if let Some(backend) = rust_backend {
                if is_rust_path(&path) {
                    backend.close_document(&path);
                }
            }
            state.close(&path);
            publish_fresh_diagnostics(state, connection);
        }
        DidSaveTextDocument::METHOD | DidChangeWatchedFiles::METHOD => {
            state.mark_dirty();
            publish_fresh_diagnostics(state, connection);
        }
        _ => {}
    }
}

/// Mirrors an open Rust document's overlay into the semantic backend.
fn sync_rust_document(
    rust_backend: &mut Option<RustBackend>,
    path: &Path,
    text: &str,
    version: i32,
) {
    if let Some(backend) = rust_backend {
        if is_rust_path(path) {
            backend.sync_document(path, text, version);
        }
    }
}

fn is_rust_path(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| extension == "rs")
}

fn handle_request(
    state: &State,
    rust_backend: &mut Option<RustBackend>,
    request: &Request,
) -> RequestResult {
    match request.method.as_str() {
        GotoDefinition::METHOD => {
            let params: GotoDefinitionParams = parse_params(request)?;
            to_result(features::definition::handle(state, &params))
        }
        References::METHOD => {
            let params: ReferenceParams = parse_params(request)?;
            to_result(features::references::handle(state, &params))
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
            let edit = features::rename::handle(state, rust_backend.as_mut(), &params)
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

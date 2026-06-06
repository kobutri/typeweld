//! Language server gateway for Rust API contracts and generated TypeScript.

use std::{
    collections::{BTreeSet, HashMap},
    env, fs,
    io::{BufRead, BufReader as StdBufReader, Read, Write},
    path::{Path, PathBuf},
    process::Stdio,
    time::Duration,
};

use lsp_types::{
    HoverProviderCapability, InitializeParams, InitializeResult, OneOf, PositionEncodingKind,
    RenameOptions, ServerCapabilities, ServerInfo, TextDocumentSyncCapability,
    TextDocumentSyncKind, WorkDoneProgressOptions,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tokio::{
    io::{
        AsyncBufRead, AsyncBufReadExt, AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt,
        BufReader as AsyncBufReader,
    },
    process::{Child, ChildStdin, ChildStdout, Command},
    sync::mpsc,
};

const CONFIG_FILES: [&str; 2] = [".api-ls.json", "api-ls.json"];

/// `api-ls` configuration loaded from the workspace root.
#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize)]
#[serde(default, rename_all = "camelCase")]
pub struct ApiLsConfig {
    pub rust_analyzer: BackendCommand,
    pub typescript: BackendCommand,
    pub effect_language_service_plugin: Option<String>,
    pub generated_cache_dir: PathBuf,
    pub symbol_graph: PathBuf,
    pub usage_index: PathBuf,
    pub unused_endpoint_lints: UsageLintLevel,
    pub log_file: Option<PathBuf>,
}

impl Default for ApiLsConfig {
    fn default() -> Self {
        Self {
            rust_analyzer: BackendCommand {
                command: "rust-analyzer".to_owned(),
                args: Vec::new(),
            },
            typescript: BackendCommand {
                command: "typescript-language-server".to_owned(),
                args: vec!["--stdio".to_owned()],
            },
            effect_language_service_plugin: Some("@effect/language-service".to_owned()),
            generated_cache_dir: PathBuf::from("target/api-contract/effect-v4/packages"),
            symbol_graph: PathBuf::from("target/api-contract/rust-ts-symbols.json"),
            usage_index: PathBuf::from("target/api-contract/graph/effect-usage-index.json"),
            unused_endpoint_lints: UsageLintLevel::Warn,
            log_file: None,
        }
    }
}

/// Unused endpoint diagnostic policy.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum UsageLintLevel {
    Off,
    #[default]
    Warn,
    Deny,
}

/// External language-server command configuration.
#[derive(Clone, Debug, Default, Eq, PartialEq, Deserialize, Serialize)]
#[serde(default, rename_all = "camelCase")]
pub struct BackendCommand {
    pub command: String,
    pub args: Vec<String>,
}

/// Resolved workspace and cache paths used by the gateway.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WorkspaceConfig {
    pub root: PathBuf,
    pub config_path: Option<PathBuf>,
    pub config: ApiLsConfig,
    pub generated_cache_dir: PathBuf,
    pub symbol_graph: PathBuf,
    pub usage_index: PathBuf,
}

/// Discovers the workspace root and loads optional `api-ls` configuration.
///
/// Relative paths in the config are resolved from the discovered workspace root.
pub fn discover_workspace_config(
    root_hint: Option<&Path>,
    cwd: &Path,
) -> Result<WorkspaceConfig, String> {
    let root = discover_workspace_root(root_hint.unwrap_or(cwd), cwd)
        .ok_or_else(|| format!("unable to discover workspace root from `{}`", cwd.display()))?;
    let config_path = CONFIG_FILES
        .iter()
        .map(|name| root.join(name))
        .find(|path| path.is_file());

    let config = match &config_path {
        Some(path) => {
            let contents = fs::read_to_string(path)
                .map_err(|error| format!("failed to read `{}`: {error}", path.display()))?;
            serde_json::from_str::<ApiLsConfig>(&contents)
                .map_err(|error| format!("failed to parse `{}`: {error}", path.display()))?
        }
        None => ApiLsConfig::default(),
    };

    Ok(resolve_workspace_config(root, config_path, config))
}

/// Runs the stdio JSON-RPC language server loop.
pub fn run_stdio() -> Result<(), String> {
    runtime()?.block_on(run_stdio_async())
}

/// Runs the JSON-RPC language server loop over arbitrary streams.
pub fn run(input: impl Read, output: impl Write) -> Result<(), String> {
    let mut reader = StdBufReader::new(input);
    let mut messages = Vec::new();
    while let Some(message) = read_message(&mut reader)? {
        messages.push(message);
    }

    let output_messages = runtime()?.block_on(run_messages_async(messages))?;
    write_output_messages(output, output_messages)
}

#[derive(Debug)]
struct LspMessage {
    id: Option<Value>,
    method: Option<String>,
    params: Value,
    raw: Value,
}

struct AsyncLspServer {
    event_tx: mpsc::UnboundedSender<GatewayEvent>,
    event_rx: mpsc::UnboundedReceiver<GatewayEvent>,
    output_tx: mpsc::UnboundedSender<Value>,
    workspace: Option<WorkspaceConfig>,
    rust_analyzer: Option<AsyncBackend>,
    typescript: Option<AsyncBackend>,
    pending_requests: HashMap<BackendRequestKey, PendingRequest>,
    client_requests: HashMap<String, BackendRequestKey>,
    backend_requests_by_client_id: HashMap<String, BackendRequestKey>,
    backend_request_client_ids: HashMap<BackendRequestKey, Value>,
    pending_shutdown: Option<PendingShutdown>,
    exit_received: bool,
    client_eof: bool,
    next_backend_client_id: u64,
}

impl AsyncLspServer {
    fn new(
        event_tx: mpsc::UnboundedSender<GatewayEvent>,
        event_rx: mpsc::UnboundedReceiver<GatewayEvent>,
        output_tx: mpsc::UnboundedSender<Value>,
    ) -> Self {
        Self {
            event_tx,
            event_rx,
            output_tx,
            workspace: None,
            rust_analyzer: None,
            typescript: None,
            pending_requests: HashMap::new(),
            client_requests: HashMap::new(),
            backend_requests_by_client_id: HashMap::new(),
            backend_request_client_ids: HashMap::new(),
            pending_shutdown: None,
            exit_received: false,
            client_eof: false,
            next_backend_client_id: 1,
        }
    }

    async fn run(mut self) -> Result<(), String> {
        while let Some(event) = self.event_rx.recv().await {
            self.handle_event(event).await?;
            if self.should_finish() {
                self.stop_backends().await;
                return Ok(());
            }
        }

        self.stop_backends().await;
        Ok(())
    }

    async fn handle_event(&mut self, event: GatewayEvent) -> Result<(), String> {
        match event {
            GatewayEvent::ClientMessage(message) => self.handle_client_message(message).await,
            GatewayEvent::ClientEof => {
                self.client_eof = true;
                Ok(())
            }
            GatewayEvent::BackendMessage(kind, message) => {
                self.handle_backend_message(kind, message)
            }
            GatewayEvent::BackendExited(kind) => {
                self.fail_backend(kind, "backend exited".to_owned()).await
            }
            GatewayEvent::BackendFailed(kind, error) => self.fail_backend(kind, error).await,
            GatewayEvent::BackendLog(message) => self.write_notification(
                "window/logMessage",
                json!({
                    "type": 3,
                    "message": message,
                }),
            ),
        }
    }

    fn should_finish(&self) -> bool {
        (self.exit_received && self.pending_shutdown.is_none())
            || (self.client_eof
                && self.pending_requests.is_empty()
                && self.pending_shutdown.is_none())
    }

    async fn handle_client_message(&mut self, message: LspMessage) -> Result<(), String> {
        let Some(method) = message.method.clone() else {
            if let Some(id) = message.id.as_ref() {
                self.forward_client_response_to_backend(id, message.raw)?;
            }
            return Ok(());
        };

        if method == "$/cancelRequest" {
            self.forward_cancel_request(message.params)?;
            return Ok(());
        }

        self.handle_method_message(&method, message).await
    }

    async fn handle_method_message(
        &mut self,
        method: &str,
        message: LspMessage,
    ) -> Result<(), String> {
        match method {
            "initialize" => {
                let Some(id) = message.id else {
                    return Ok(());
                };
                match initialize_workspace(&message.params) {
                    Ok(workspace) => {
                        let log_file = workspace_log_file(&workspace);
                        let rust_backend = match start_required_backend(
                            BackendKind::Rust,
                            "rust-analyzer",
                            &workspace.config.rust_analyzer,
                            message.params.clone(),
                            self.event_tx.clone(),
                            log_file.clone(),
                        )
                        .await
                        {
                            Ok(backend) => backend,
                            Err(error) => {
                                self.write_error(id, -32_602, error)?;
                                return Ok(());
                            }
                        };
                        let typescript_params =
                            typescript_initialize_params(&message.params, &workspace);
                        let typescript_backend = match start_required_backend(
                            BackendKind::TypeScript,
                            "TypeScript/Effect",
                            &workspace.config.typescript,
                            typescript_params,
                            self.event_tx.clone(),
                            log_file,
                        )
                        .await
                        {
                            Ok(backend) => backend,
                            Err(error) => {
                                rust_backend.stop().await;
                                self.write_error(id, -32_602, error)?;
                                return Ok(());
                            }
                        };
                        self.workspace = Some(workspace);
                        self.rust_analyzer = Some(rust_backend);
                        self.typescript = Some(typescript_backend);
                        self.write_response(id, initialize_result())?;
                    }
                    Err(error) => self.write_error(id, -32_602, error)?,
                }
            }
            "initialized" => {
                self.forward_notification_to_rust_analyzer(method, message.params.clone())?;
                self.forward_notification_to_typescript(method, Value::Null)?;
                self.publish_unused_endpoint_diagnostics()?;
            }
            "shutdown" => {
                if let Some(id) = message.id {
                    self.begin_shutdown(id)?;
                }
            }
            "exit" => {
                self.exit_received = true;
            }
            "api-ls/generatedPackageFile" => {
                if let Some(id) = message.id {
                    self.read_generated_package_file(message.params, id)?;
                }
            }
            "textDocument/definition" => {
                if let Some(id) = message.id {
                    if let Some(locations) = self.cross_language_definition(&message.params)? {
                        self.write_response(id, locations)?;
                    } else {
                        self.route_or_default_response(method, message.params, id, Value::Null)?;
                    }
                }
            }
            "textDocument/references" => {
                if let Some(id) = message.id {
                    self.handle_references_request(message.params, id)?;
                }
            }
            "textDocument/hover" => {
                if let Some(id) = message.id {
                    if let Some(hover) = self.cross_language_hover(&message.params)? {
                        self.write_response(id, hover)?;
                    } else {
                        self.route_or_default_response(method, message.params, id, Value::Null)?;
                    }
                }
            }
            "textDocument/prepareRename" => {
                if let Some(id) = message.id {
                    if let Some(rename) = self.cross_language_prepare_rename(&message.params)? {
                        self.write_response(id, rename)?;
                    } else {
                        self.route_or_default_response(method, message.params, id, Value::Null)?;
                    }
                }
            }
            "textDocument/rename" => {
                if let Some(id) = message.id {
                    match self.cross_language_rename(&message.params) {
                        Ok(Some(edit)) => self.write_response(id, edit)?,
                        Ok(None) => {
                            self.route_or_default_response(
                                method,
                                message.params,
                                id,
                                json!({ "changes": {} }),
                            )?;
                        }
                        Err(error) => self.write_error(id, -32_602, error)?,
                    }
                }
            }
            _ => match message.id {
                Some(id) => {
                    if let Some(kind) = self.route_backend(method, &message.params) {
                        self.forward_request_to_backend(
                            kind,
                            method,
                            message.params,
                            id,
                            PendingRequestKind::Forward,
                            true,
                        )?;
                    } else {
                        self.write_error(
                            id,
                            -32_601,
                            format!("api-ls has no handler for `{method}` yet"),
                        )?;
                    }
                }
                None => {
                    if let Some(kind) = self.route_backend(method, &message.params) {
                        self.forward_notification_to_backend(kind, method, message.params)?;
                    } else {
                        self.forward_notification_to_all(method, message.params)?;
                    }
                }
            },
        }

        Ok(())
    }

    fn write_response(&self, id: Value, result: Value) -> Result<(), String> {
        self.emit(json!({
            "jsonrpc": "2.0",
            "id": id,
            "result": result,
        }))
    }

    fn write_notification(&self, method: &str, params: Value) -> Result<(), String> {
        self.emit(json!({
            "jsonrpc": "2.0",
            "method": method,
            "params": params,
        }))
    }

    fn write_error(&self, id: Value, code: i64, message: String) -> Result<(), String> {
        self.emit(json!({
            "jsonrpc": "2.0",
            "id": id,
            "error": {
                "code": code,
                "message": message,
            },
        }))
    }

    fn emit(&self, value: Value) -> Result<(), String> {
        self.output_tx
            .send(value)
            .map_err(|error| format!("failed to queue LSP output: {error}"))
    }

    fn forward_notification_to_rust_analyzer(
        &mut self,
        method: &str,
        params: Value,
    ) -> Result<(), String> {
        self.forward_notification_to_backend(BackendKind::Rust, method, params)
    }

    fn forward_notification_to_typescript(
        &mut self,
        method: &str,
        params: Value,
    ) -> Result<(), String> {
        self.forward_notification_to_backend(BackendKind::TypeScript, method, params)
    }

    fn forward_notification_to_all(&mut self, method: &str, params: Value) -> Result<(), String> {
        self.forward_notification_to_backend(BackendKind::Rust, method, params.clone())?;
        self.forward_notification_to_backend(BackendKind::TypeScript, method, params)
    }

    fn forward_notification_to_backend(
        &mut self,
        kind: BackendKind,
        method: &str,
        params: Value,
    ) -> Result<(), String> {
        let Some(backend) = self.backend_mut(kind) else {
            return Ok(());
        };
        backend.send_notification(method, params)
    }

    fn route_or_default_response(
        &mut self,
        method: &str,
        params: Value,
        id: Value,
        default: Value,
    ) -> Result<(), String> {
        if let Some(kind) = self.route_backend(method, &params) {
            self.forward_request_to_backend(
                kind,
                method,
                params,
                id,
                PendingRequestKind::Forward,
                true,
            )
        } else {
            self.write_response(id, default)
        }
    }

    fn handle_references_request(&mut self, params: Value, id: Value) -> Result<(), String> {
        if let Some((locations, workspace)) = self.cross_language_references_base(&params)? {
            if let Some(kind) = self.route_backend("textDocument/references", &params) {
                self.forward_request_to_backend(
                    kind,
                    "textDocument/references",
                    params,
                    id,
                    PendingRequestKind::References {
                        local_locations: locations,
                        workspace: Box::new(workspace),
                    },
                    true,
                )
            } else {
                self.write_response(id, dedupe_locations(locations, &workspace))
            }
        } else {
            self.route_or_default_response("textDocument/references", params, id, json!([]))
        }
    }

    fn route_backend(&self, method: &str, params: &Value) -> Option<BackendKind> {
        if is_rust_message(method, params) {
            Some(BackendKind::Rust)
        } else if self.is_typescript_message(method, params) {
            Some(BackendKind::TypeScript)
        } else {
            None
        }
    }

    fn forward_request_to_backend(
        &mut self,
        kind: BackendKind,
        method: &str,
        params: Value,
        client_id: Value,
        pending_kind: PendingRequestKind,
        track_client_request: bool,
    ) -> Result<(), String> {
        let Some(backend) = self.backend_mut(kind) else {
            return self.write_error(
                client_id,
                -32_003,
                format!("{} backend is not available", kind.display_name()),
            );
        };
        let backend_id = backend.allocate_id();
        let backend_id_value = json!(backend_id);
        let key = BackendRequestKey::new(kind, &backend_id_value);
        backend.send_request(backend_id, method, params)?;
        if track_client_request {
            self.client_requests
                .insert(rpc_id_key(&client_id), key.clone());
        }
        self.pending_requests.insert(
            key,
            PendingRequest {
                client_id,
                method: method.to_owned(),
                kind: pending_kind,
            },
        );
        Ok(())
    }

    fn forward_cancel_request(&mut self, params: Value) -> Result<(), String> {
        let Some(cancelled_id) = params.get("id") else {
            return Ok(());
        };
        let Some(key) = self.client_requests.get(&rpc_id_key(cancelled_id)).cloned() else {
            return Ok(());
        };
        let mut backend_params = params;
        backend_params["id"] = rpc_id_from_key(&key.id);
        self.forward_notification_to_backend(key.kind, "$/cancelRequest", backend_params)
    }

    fn forward_client_response_to_backend(
        &mut self,
        client_id: &Value,
        mut raw: Value,
    ) -> Result<(), String> {
        let Some(key) = self
            .backend_requests_by_client_id
            .remove(&rpc_id_key(client_id))
        else {
            return Ok(());
        };
        self.backend_request_client_ids.remove(&key);
        raw["id"] = rpc_id_from_key(&key.id);
        let Some(backend) = self.backend_mut(key.kind) else {
            return Ok(());
        };
        backend.send_raw(raw)
    }

    fn handle_backend_message(
        &mut self,
        kind: BackendKind,
        message: LspMessage,
    ) -> Result<(), String> {
        let method = message.method.clone();
        let id = message.id.clone();
        match (method.as_deref(), id.as_ref()) {
            (Some(method), Some(id)) => {
                self.forward_backend_request_to_client(kind, method, id, message.raw)
            }
            (Some("$/cancelRequest"), None) => {
                self.forward_backend_cancel_request(kind, message.raw)
            }
            (Some(_), None) => self.emit(message.raw),
            (None, Some(id)) => self.handle_backend_response(kind, id, message.raw),
            (None, None) => Ok(()),
        }
    }

    fn forward_backend_request_to_client(
        &mut self,
        kind: BackendKind,
        _method: &str,
        backend_id: &Value,
        mut raw: Value,
    ) -> Result<(), String> {
        let key = BackendRequestKey::new(kind, backend_id);
        let client_id = Value::String(format!(
            "api-ls:{}:{}",
            kind.wire_label(),
            self.next_backend_client_id
        ));
        self.next_backend_client_id += 1;
        self.backend_requests_by_client_id
            .insert(rpc_id_key(&client_id), key.clone());
        self.backend_request_client_ids
            .insert(key, client_id.clone());
        raw["id"] = client_id;
        self.emit(raw)
    }

    fn forward_backend_cancel_request(
        &mut self,
        kind: BackendKind,
        mut raw: Value,
    ) -> Result<(), String> {
        let Some(cancelled_id) = raw.get("params").and_then(|params| params.get("id")) else {
            return self.emit(raw);
        };
        let key = BackendRequestKey::new(kind, cancelled_id);
        if let Some(client_id) = self.backend_request_client_ids.get(&key) {
            raw["params"]["id"] = client_id.clone();
        }
        self.emit(raw)
    }

    fn handle_backend_response(
        &mut self,
        kind: BackendKind,
        backend_id: &Value,
        mut raw: Value,
    ) -> Result<(), String> {
        let key = BackendRequestKey::new(kind, backend_id);
        let Some(pending) = self.pending_requests.remove(&key) else {
            return Ok(());
        };
        self.client_requests.remove(&rpc_id_key(&pending.client_id));

        match pending.kind {
            PendingRequestKind::Forward => {
                raw["id"] = pending.client_id;
                self.emit(raw)
            }
            PendingRequestKind::References {
                mut local_locations,
                workspace,
            } => {
                local_locations.extend(
                    raw.get("result")
                        .and_then(Value::as_array)
                        .cloned()
                        .unwrap_or_default(),
                );
                self.write_response(
                    pending.client_id,
                    dedupe_locations(local_locations, workspace.as_ref()),
                )
            }
            PendingRequestKind::Shutdown => self.finish_backend_shutdown(kind),
        }
    }

    fn begin_shutdown(&mut self, id: Value) -> Result<(), String> {
        let mut remaining = BTreeSet::new();
        for kind in [BackendKind::Rust, BackendKind::TypeScript] {
            if self.backend(kind).is_some() {
                remaining.insert(kind);
            }
        }

        if remaining.is_empty() {
            return self.write_response(id, Value::Null);
        }

        self.pending_shutdown = Some(PendingShutdown {
            client_id: id.clone(),
            remaining: remaining.clone(),
        });
        for kind in remaining {
            self.forward_request_to_backend(
                kind,
                "shutdown",
                Value::Null,
                id.clone(),
                PendingRequestKind::Shutdown,
                false,
            )?;
        }
        Ok(())
    }

    fn finish_backend_shutdown(&mut self, kind: BackendKind) -> Result<(), String> {
        let Some(shutdown) = &mut self.pending_shutdown else {
            return Ok(());
        };
        shutdown.remaining.remove(&kind);
        if shutdown.remaining.is_empty() {
            let client_id = shutdown.client_id.clone();
            self.pending_shutdown = None;
            self.write_response(client_id, Value::Null)?;
        }
        Ok(())
    }

    async fn fail_backend(&mut self, kind: BackendKind, error: String) -> Result<(), String> {
        self.write_notification(
            "window/logMessage",
            json!({
                "type": 1,
                "message": format!("api-ls {} backend unavailable: {error}", kind.display_name()),
            }),
        )?;

        let failed_keys = self
            .pending_requests
            .keys()
            .filter(|key| key.kind == kind)
            .cloned()
            .collect::<Vec<_>>();
        for key in failed_keys {
            if let Some(pending) = self.pending_requests.remove(&key) {
                self.client_requests.remove(&rpc_id_key(&pending.client_id));
                match pending.kind {
                    PendingRequestKind::Forward | PendingRequestKind::References { .. } => {
                        self.write_error(
                            pending.client_id,
                            -32_003,
                            format!(
                                "{} backend stopped while handling `{}`: {error}",
                                kind.display_name(),
                                pending.method
                            ),
                        )?;
                    }
                    PendingRequestKind::Shutdown => {
                        self.finish_backend_shutdown(kind)?;
                    }
                }
            }
        }

        match kind {
            BackendKind::Rust => {
                if let Some(backend) = self.rust_analyzer.take() {
                    backend.reap().await;
                }
            }
            BackendKind::TypeScript => {
                if let Some(backend) = self.typescript.take() {
                    backend.reap().await;
                }
            }
        }

        Ok(())
    }

    async fn stop_backends(&mut self) {
        if let Some(backend) = self.rust_analyzer.take() {
            backend.stop().await;
        }
        if let Some(backend) = self.typescript.take() {
            backend.stop().await;
        }
    }

    fn backend(&self, kind: BackendKind) -> Option<&AsyncBackend> {
        match kind {
            BackendKind::Rust => self.rust_analyzer.as_ref(),
            BackendKind::TypeScript => self.typescript.as_ref(),
        }
    }

    fn backend_mut(&mut self, kind: BackendKind) -> Option<&mut AsyncBackend> {
        match kind {
            BackendKind::Rust => self.rust_analyzer.as_mut(),
            BackendKind::TypeScript => self.typescript.as_mut(),
        }
    }

    fn read_generated_package_file(&mut self, params: Value, id: Value) -> Result<(), String> {
        let Some(workspace) = &self.workspace else {
            return self.write_error(id, -32_000, "api-ls is not initialized".to_owned());
        };
        let Some(path) = generated_file_path(&params, workspace) else {
            return self.write_error(
                id,
                -32_602,
                "generated package file must be inside the generated cache".to_owned(),
            );
        };
        match fs::read_to_string(&path) {
            Ok(text) => self.write_response(
                id,
                json!({
                    "uri": path_to_file_uri(&path),
                    "text": text,
                }),
            ),
            Err(error) => self.write_error(
                id,
                -32_602,
                format!(
                    "failed to read generated package file `{}`: {error}",
                    path.display()
                ),
            ),
        }
    }

    fn is_typescript_message(&self, method: &str, params: &Value) -> bool {
        method.starts_with("typescript/")
            || document_uris(params)
                .iter()
                .any(|uri| is_typescript_uri(uri) || self.is_generated_package_uri(uri))
    }

    fn is_generated_package_uri(&self, uri: &str) -> bool {
        let Some(workspace) = &self.workspace else {
            return false;
        };
        file_uri_to_path(uri).is_some_and(|path| path.starts_with(&workspace.generated_cache_dir))
    }

    fn cross_language_definition(&self, params: &Value) -> Result<Option<Value>, String> {
        let Some(workspace) = &self.workspace else {
            return Ok(None);
        };
        let Some(query) = TextPositionQuery::from_lsp_params(params) else {
            return Ok(None);
        };
        let Some(graph) = SymbolGraph::load(&workspace.symbol_graph)? else {
            return Ok(None);
        };
        Ok(graph.definition(&query, workspace))
    }

    fn cross_language_references_base(
        &self,
        params: &Value,
    ) -> Result<Option<(Vec<Value>, WorkspaceConfig)>, String> {
        let Some(workspace) = self.workspace.clone() else {
            return Ok(None);
        };
        let Some(query) = TextPositionQuery::from_lsp_params(params) else {
            return Ok(None);
        };
        let Some(graph) = SymbolGraph::load(&workspace.symbol_graph)? else {
            return Ok(None);
        };
        let Some(locations) = graph.references(&query, &workspace) else {
            return Ok(None);
        };

        Ok(Some((locations, workspace)))
    }

    fn cross_language_hover(&self, params: &Value) -> Result<Option<Value>, String> {
        let Some(workspace) = &self.workspace else {
            return Ok(None);
        };
        let Some(query) = TextPositionQuery::from_lsp_params(params) else {
            return Ok(None);
        };
        let Some(graph) = SymbolGraph::load(&workspace.symbol_graph)? else {
            return Ok(None);
        };
        Ok(graph.hover(&query, workspace))
    }

    fn cross_language_prepare_rename(&self, params: &Value) -> Result<Option<Value>, String> {
        let Some(workspace) = &self.workspace else {
            return Ok(None);
        };
        let Some(query) = TextPositionQuery::from_lsp_params(params) else {
            return Ok(None);
        };
        let Some(graph) = SymbolGraph::load(&workspace.symbol_graph)? else {
            return Ok(None);
        };
        Ok(graph.prepare_rename(&query, workspace))
    }

    fn cross_language_rename(&self, params: &Value) -> Result<Option<Value>, String> {
        let Some(workspace) = &self.workspace else {
            return Ok(None);
        };
        let Some(query) = TextPositionQuery::from_lsp_params(params) else {
            return Ok(None);
        };
        let Some(new_name) = params.get("newName").and_then(Value::as_str) else {
            return Err("rename requires `newName`".to_owned());
        };
        let Some(graph) = SymbolGraph::load(&workspace.symbol_graph)? else {
            return Ok(None);
        };
        graph.rename(&query, new_name, workspace)
    }

    fn publish_unused_endpoint_diagnostics(&mut self) -> Result<(), String> {
        let Some(workspace) = &self.workspace else {
            return Ok(());
        };
        if matches!(workspace.config.unused_endpoint_lints, UsageLintLevel::Off) {
            return Ok(());
        }
        let Some(graph) = SymbolGraph::load(&workspace.symbol_graph)? else {
            return Ok(());
        };
        let Some(index) = EffectUsageIndex::load(&workspace.usage_index)? else {
            return Ok(());
        };
        if index.is_stale_for(&graph) {
            return Ok(());
        }
        let diagnostics = graph.unused_endpoint_diagnostics(&index, workspace);

        for (uri, diagnostics) in diagnostics {
            self.write_notification(
                "textDocument/publishDiagnostics",
                json!({
                    "uri": uri,
                    "diagnostics": diagnostics,
                }),
            )?;
        }

        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
enum BackendKind {
    Rust,
    TypeScript,
}

impl BackendKind {
    const fn display_name(self) -> &'static str {
        match self {
            Self::Rust => "rust-analyzer",
            Self::TypeScript => "TypeScript/Effect",
        }
    }

    const fn wire_label(self) -> &'static str {
        match self {
            Self::Rust => "rust-analyzer",
            Self::TypeScript => "typescript-effect",
        }
    }
}

#[derive(Debug)]
enum GatewayEvent {
    ClientMessage(LspMessage),
    ClientEof,
    BackendMessage(BackendKind, LspMessage),
    BackendExited(BackendKind),
    BackendFailed(BackendKind, String),
    BackendLog(String),
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
struct BackendRequestKey {
    kind: BackendKind,
    id: String,
}

impl BackendRequestKey {
    fn new(kind: BackendKind, id: &Value) -> Self {
        Self {
            kind,
            id: rpc_id_key(id),
        }
    }
}

#[derive(Debug)]
struct PendingRequest {
    client_id: Value,
    method: String,
    kind: PendingRequestKind,
}

#[derive(Debug)]
enum PendingRequestKind {
    Forward,
    References {
        local_locations: Vec<Value>,
        workspace: Box<WorkspaceConfig>,
    },
    Shutdown,
}

#[derive(Debug)]
struct PendingShutdown {
    client_id: Value,
    remaining: BTreeSet<BackendKind>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SymbolGraph {
    #[serde(default)]
    contract_hash: Option<String>,
    #[serde(default)]
    contract_hashes: Vec<String>,
    symbols: Vec<LinkedSymbol>,
}

impl SymbolGraph {
    fn load(path: &Path) -> Result<Option<Self>, String> {
        if !path.is_file() {
            return Ok(None);
        }
        let contents = fs::read_to_string(path).map_err(|error| {
            format!("failed to read symbol graph `{}`: {error}", path.display())
        })?;
        serde_json::from_str(&contents)
            .map(Some)
            .map_err(|error| format!("failed to parse symbol graph `{}`: {error}", path.display()))
    }

    fn contains_contract_hash(&self, contract_hash: &str) -> bool {
        if contract_hash.is_empty() {
            return false;
        }
        self.contract_hash.as_deref() == Some(contract_hash)
            || self
                .contract_hashes
                .iter()
                .any(|candidate| candidate == contract_hash)
    }

    fn definition(&self, query: &TextPositionQuery, workspace: &WorkspaceConfig) -> Option<Value> {
        for symbol in &self.symbols {
            if symbol.rust.matches(query, workspace) {
                let locations = symbol
                    .typescript
                    .iter()
                    .filter(|location| !location.generated)
                    .map(|location| location.to_lsp_location(workspace))
                    .collect::<Vec<_>>();
                if !locations.is_empty() {
                    return Some(Value::Array(locations));
                }
            }

            if symbol
                .typescript
                .iter()
                .any(|location| location.matches(query, workspace))
            {
                return Some(symbol.rust.to_lsp_location(workspace));
            }
        }

        None
    }

    fn references(
        &self,
        query: &TextPositionQuery,
        workspace: &WorkspaceConfig,
    ) -> Option<Vec<Value>> {
        let include_declaration = query.include_declaration;

        for symbol in &self.symbols {
            let rust_match = symbol.rust.matches(query, workspace);
            let typescript_match = symbol
                .typescript
                .iter()
                .any(|location| location.matches(query, workspace));
            if !rust_match && !typescript_match {
                continue;
            }

            let mut locations = Vec::new();
            if include_declaration || typescript_match {
                locations.push(symbol.rust.to_lsp_location(workspace));
            }
            locations.extend(
                symbol
                    .typescript
                    .iter()
                    .filter(|location| !location.generated)
                    .map(|location| location.to_lsp_location(workspace)),
            );

            return Some(locations);
        }

        None
    }

    fn hover(&self, query: &TextPositionQuery, workspace: &WorkspaceConfig) -> Option<Value> {
        for symbol in &self.symbols {
            let matched_range = if symbol.rust.matches(query, workspace) {
                Some(symbol.rust.range)
            } else {
                symbol
                    .typescript
                    .iter()
                    .find(|location| location.matches(query, workspace))
                    .map(|location| location.range)
            };
            let Some(range) = matched_range else {
                continue;
            };

            return Some(json!({
                "contents": {
                    "kind": "markdown",
                    "value": symbol.hover_markdown(workspace),
                },
                "range": range,
            }));
        }

        None
    }

    fn prepare_rename(
        &self,
        query: &TextPositionQuery,
        workspace: &WorkspaceConfig,
    ) -> Option<Value> {
        let (symbol, location) = self.rename_symbol_at(query, workspace)?;
        Some(json!({
            "range": location.range,
            "placeholder": symbol.rename_placeholder(),
        }))
    }

    fn rename(
        &self,
        query: &TextPositionQuery,
        new_name: &str,
        workspace: &WorkspaceConfig,
    ) -> Result<Option<Value>, String> {
        let Some((symbol, _)) = self.rename_symbol_at(query, workspace) else {
            return Ok(None);
        };
        validate_rename_name(new_name)?;
        if symbol
            .metadata
            .reserved_names
            .iter()
            .any(|reserved| reserved == new_name)
        {
            return Err(format!(
                "rename `{new_name}` conflicts with an existing API symbol"
            ));
        }

        let mut changes = serde_json::Map::new();
        for location in symbol.rename_locations() {
            let Some(uri) = location.resolved_uri(workspace) else {
                continue;
            };
            changes
                .entry(uri)
                .or_insert_with(|| Value::Array(Vec::new()))
                .as_array_mut()
                .expect("change entry is an array")
                .push(json!({
                    "range": location.range,
                    "newText": new_name,
                }));
        }

        Ok(Some(json!({ "changes": changes })))
    }

    fn unused_endpoint_diagnostics(
        &self,
        index: &EffectUsageIndex,
        workspace: &WorkspaceConfig,
    ) -> Vec<(String, Vec<Value>)> {
        let mut by_uri = Vec::<(String, Vec<Value>)>::new();

        for symbol in &self.symbols {
            if symbol.kind != "endpoint" || symbol.metadata.allow_unused {
                continue;
            }
            if index.strong_usage_count(&symbol.id) > 0 {
                continue;
            }
            let Some(uri) = symbol.rust.resolved_uri(workspace) else {
                continue;
            };
            let diagnostic = symbol.unused_endpoint_diagnostic(workspace);
            if let Some((_, diagnostics)) = by_uri.iter_mut().find(|(item_uri, _)| item_uri == &uri)
            {
                diagnostics.push(diagnostic);
            } else {
                by_uri.push((uri, vec![diagnostic]));
            }
        }

        by_uri.sort_by(|left, right| left.0.cmp(&right.0));
        for (_, diagnostics) in &mut by_uri {
            diagnostics.sort_by(|left, right| {
                diagnostic_start(left)
                    .cmp(&diagnostic_start(right))
                    .then_with(|| diagnostic_message(left).cmp(&diagnostic_message(right)))
            });
        }
        by_uri
    }

    fn rename_symbol_at<'a>(
        &'a self,
        query: &TextPositionQuery,
        workspace: &WorkspaceConfig,
    ) -> Option<(&'a LinkedSymbol, &'a GraphLocation)> {
        for symbol in &self.symbols {
            if symbol.rust.is_renamable() && symbol.rust.matches(query, workspace) {
                return Some((symbol, &symbol.rust));
            }
            if let Some(location) = symbol
                .typescript
                .iter()
                .find(|location| location.is_renamable() && location.matches(query, workspace))
            {
                return Some((symbol, location));
            }
        }

        None
    }
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct LinkedSymbol {
    #[allow(dead_code)]
    id: String,
    kind: String,
    rust: GraphLocation,
    #[serde(default)]
    typescript: Vec<GraphLocation>,
    #[serde(default)]
    metadata: SymbolMetadata,
}

impl LinkedSymbol {
    fn hover_markdown(&self, workspace: &WorkspaceConfig) -> String {
        let mut lines = Vec::new();
        lines.push(format!("**API {}** `{}`", self.kind, self.id));

        if let (Some(method), Some(route)) = (&self.metadata.method, &self.metadata.route) {
            lines.push(format!("Route: `{method} {route}`"));
        }
        if let Some(effect_signature) = &self.metadata.effect_signature {
            lines.push(format!("Effect: `{effect_signature}`"));
        }
        if !self.metadata.errors.is_empty() {
            lines.push(format!("Errors: `{}`", self.metadata.errors.join(" | ")));
        }
        if !self.metadata.rust_path.is_empty() {
            lines.push(format!("Rust: `{}`", self.metadata.rust_path.join("::")));
        } else if let Some(uri) = self.rust.resolved_uri(workspace) {
            lines.push(format!("Rust source: `{uri}`"));
        }
        if !self.metadata.ts_path.is_empty() {
            lines.push(format!("TypeScript: `{}`", self.metadata.ts_path.join(".")));
        }
        let usage_count = self
            .metadata
            .usage_count
            .map_or_else(|| "pending".to_owned(), |count| count.to_string());
        lines.push(format!("Usage count: `{usage_count}`"));

        lines.join("\n\n")
    }

    fn rename_placeholder(&self) -> String {
        self.metadata
            .rename_placeholder
            .clone()
            .or_else(|| self.metadata.ts_path.last().cloned())
            .or_else(|| self.metadata.rust_path.last().cloned())
            .or_else(|| self.id.split(':').next_back().map(str::to_owned))
            .unwrap_or_else(|| self.id.clone())
    }

    fn rename_locations(&self) -> impl Iterator<Item = &GraphLocation> {
        std::iter::once(&self.rust)
            .chain(self.typescript.iter())
            .filter(|location| location.is_renamable())
    }

    fn unused_endpoint_diagnostic(&self, workspace: &WorkspaceConfig) -> Value {
        let method = self.metadata.method.as_deref().unwrap_or("UNKNOWN");
        let route = self.metadata.route.as_deref().unwrap_or("<unknown route>");
        let accessor = if self.metadata.ts_path.is_empty() {
            "<unknown accessor>".to_owned()
        } else {
            self.metadata.ts_path.join(".")
        };
        let severity = match workspace.config.unused_endpoint_lints {
            UsageLintLevel::Off | UsageLintLevel::Warn => 2,
            UsageLintLevel::Deny => 1,
        };

        json!({
            "range": self.rust.range,
            "severity": severity,
            "code": "api::unused_endpoint",
            "source": "api-ls",
            "message": format!(
                "endpoint is exported but has no strong TypeScript usages\nroute: {method} {route}\ngenerated accessor: {accessor}\nhelp: call it from Effect code, mark #[api(allow_unused)], or remove it"
            ),
        })
    }
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
struct SymbolMetadata {
    method: Option<String>,
    route: Option<String>,
    effect_signature: Option<String>,
    errors: Vec<String>,
    rust_path: Vec<String>,
    ts_path: Vec<String>,
    usage_count: Option<u64>,
    allow_unused: bool,
    rename_placeholder: Option<String>,
    reserved_names: Vec<String>,
}

#[derive(Clone, Debug, Deserialize)]
struct EffectUsageIndex {
    #[serde(default)]
    schema_version: u32,
    #[serde(default)]
    contract_hash: String,
    #[serde(default)]
    endpoints: Vec<EndpointUsageSummary>,
}

impl EffectUsageIndex {
    fn load(path: &Path) -> Result<Option<Self>, String> {
        if !path.is_file() {
            return Ok(None);
        }
        let contents = fs::read_to_string(path)
            .map_err(|error| format!("failed to read usage index `{}`: {error}", path.display()))?;
        serde_json::from_str(&contents)
            .map(Some)
            .map_err(|error| format!("failed to parse usage index `{}`: {error}", path.display()))
    }

    fn is_stale_for(&self, graph: &SymbolGraph) -> bool {
        self.schema_version != 2 || !graph.contains_contract_hash(&self.contract_hash)
    }

    fn strong_usage_count(&self, endpoint_id: &str) -> u64 {
        self.endpoints
            .iter()
            .find(|endpoint| endpoint.endpoint_id == endpoint_id)
            .map_or(0, |endpoint| endpoint.strong)
    }
}

#[derive(Clone, Debug, Deserialize)]
struct EndpointUsageSummary {
    endpoint_id: String,
    #[serde(default)]
    strong: u64,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct GraphLocation {
    uri: Option<String>,
    file: Option<PathBuf>,
    range: GraphRange,
    #[serde(default)]
    generated: bool,
    renamable: Option<bool>,
}

impl GraphLocation {
    fn matches(&self, query: &TextPositionQuery, workspace: &WorkspaceConfig) -> bool {
        self.resolved_uri(workspace)
            .is_some_and(|uri| same_uri(&uri, &query.uri) && self.range.contains(query.position))
    }

    fn to_lsp_location(&self, workspace: &WorkspaceConfig) -> Value {
        json!({
            "uri": self.resolved_uri(workspace).unwrap_or_default(),
            "range": self.range,
        })
    }

    fn resolved_uri(&self, workspace: &WorkspaceConfig) -> Option<String> {
        self.uri.clone().or_else(|| {
            self.file.as_ref().map(|file| {
                let path = if file.is_absolute() {
                    file.clone()
                } else {
                    workspace.root.join(file)
                };
                path_to_file_uri(&path)
            })
        })
    }

    fn is_renamable(&self) -> bool {
        self.renamable.unwrap_or(!self.generated)
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
struct GraphRange {
    start: GraphPosition,
    end: GraphPosition,
}

impl GraphRange {
    fn contains(self, position: GraphPosition) -> bool {
        self.start <= position && position <= self.end
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
struct GraphPosition {
    line: u32,
    character: u32,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct TextPositionQuery {
    uri: String,
    position: GraphPosition,
    include_declaration: bool,
}

impl TextPositionQuery {
    fn from_lsp_params(params: &Value) -> Option<Self> {
        Some(Self {
            uri: params.get("textDocument")?.get("uri")?.as_str()?.to_owned(),
            position: GraphPosition {
                line: u32::try_from(params.get("position")?.get("line")?.as_u64()?).ok()?,
                character: u32::try_from(params.get("position")?.get("character")?.as_u64()?)
                    .ok()?,
            },
            include_declaration: params
                .get("context")
                .and_then(|context| context.get("includeDeclaration"))
                .and_then(Value::as_bool)
                .unwrap_or(false),
        })
    }
}

struct AsyncBackend {
    name: String,
    child: Child,
    stdin_tx: mpsc::UnboundedSender<Value>,
    next_id: u64,
}

async fn start_required_backend(
    kind: BackendKind,
    label: &str,
    command: &BackendCommand,
    initialize_params: Value,
    event_tx: mpsc::UnboundedSender<GatewayEvent>,
    log_file: Option<PathBuf>,
) -> Result<AsyncBackend, String> {
    AsyncBackend::start(kind, label, command, initialize_params, event_tx, log_file)
        .await
        .map_err(|error| {
            format!(
                "api-ls requires the {label} backend to start and initialize; configured command: {}; error: {error}; help: install the backend or update `.api-ls.json` for this workspace",
                backend_command_label(command)
            )
        })
}

impl AsyncBackend {
    async fn start(
        kind: BackendKind,
        name: &str,
        command: &BackendCommand,
        initialize_params: Value,
        event_tx: mpsc::UnboundedSender<GatewayEvent>,
        log_file: Option<PathBuf>,
    ) -> Result<Self, String> {
        if command.command.is_empty() {
            return Err("backend command is empty".to_owned());
        }

        let mut child = Command::new(&command.command)
            .args(&command.args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|error| format!("failed to start `{}`: {error}", command.command))?;
        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| format!("{name} stdin was not piped"))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| format!("{name} stdout was not piped"))?;
        let stderr = child
            .stderr
            .take()
            .ok_or_else(|| format!("{name} stderr was not piped"))?;
        spawn_backend_stderr_task(kind, name.to_owned(), stderr, event_tx.clone(), log_file);

        let mut stdin = stdin;
        let mut stdout = AsyncBufReader::new(stdout);
        let initialize_id = 1;
        if let Err(error) =
            write_request_async(&mut stdin, initialize_id, "initialize", initialize_params).await
        {
            terminate_child(&mut child).await;
            return Err(error);
        }
        if let Err(error) =
            read_initialize_response(name, &mut stdout, initialize_id, kind, &event_tx).await
        {
            terminate_child(&mut child).await;
            return Err(error);
        }

        let (stdin_tx, stdin_rx) = mpsc::unbounded_channel();
        spawn_backend_writer(kind, name.to_owned(), stdin, stdin_rx, event_tx.clone());
        spawn_backend_stdout_reader(kind, name.to_owned(), stdout, event_tx);

        Ok(Self {
            name: name.to_owned(),
            child,
            stdin_tx,
            next_id: initialize_id + 1,
        })
    }

    fn send_request(&self, id: u64, method: &str, params: Value) -> Result<(), String> {
        self.send_raw(json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": params,
        }))
    }

    fn send_notification(&self, method: &str, params: Value) -> Result<(), String> {
        self.send_raw(json!({
                "jsonrpc": "2.0",
                "method": method,
                "params": params,
        }))
    }

    fn send_raw(&self, message: Value) -> Result<(), String> {
        self.stdin_tx
            .send(message)
            .map_err(|error| format!("{} stdin is unavailable: {error}", self.name))
    }

    async fn stop(mut self) {
        let _ = self.send_notification("exit", Value::Null);
        self.wait_or_kill(Duration::from_millis(500)).await;
    }

    async fn reap(mut self) {
        self.wait_or_kill(Duration::from_millis(100)).await;
    }

    async fn wait_or_kill(&mut self, timeout: Duration) {
        if tokio::time::timeout(timeout, self.child.wait())
            .await
            .is_err()
        {
            let _ = self.child.start_kill();
            let _ = self.child.wait().await;
        }
    }

    fn allocate_id(&mut self) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        id
    }
}

async fn terminate_child(child: &mut Child) {
    let _ = child.start_kill();
    let _ = child.wait().await;
}

async fn read_initialize_response(
    name: &str,
    stdout: &mut AsyncBufReader<ChildStdout>,
    initialize_id: u64,
    kind: BackendKind,
    event_tx: &mpsc::UnboundedSender<GatewayEvent>,
) -> Result<(), String> {
    loop {
        let message = read_message_async(stdout)
            .await?
            .ok_or_else(|| format!("{name} exited before responding to initialize"))?;
        if message.id.as_ref() == Some(&json!(initialize_id)) {
            if let Some(error) = message.raw.get("error") {
                return Err(format!("initialize returned error: {error}"));
            }
            return Ok(());
        }
        event_tx
            .send(GatewayEvent::BackendMessage(kind, message))
            .map_err(|error| format!("failed to queue {name} initialize message: {error}"))?;
    }
}

fn spawn_backend_writer(
    kind: BackendKind,
    name: String,
    mut stdin: ChildStdin,
    mut stdin_rx: mpsc::UnboundedReceiver<Value>,
    event_tx: mpsc::UnboundedSender<GatewayEvent>,
) {
    tokio::spawn(async move {
        while let Some(message) = stdin_rx.recv().await {
            if let Err(error) = write_framed_async(&mut stdin, &message).await {
                let _ = event_tx.send(GatewayEvent::BackendFailed(
                    kind,
                    format!("{name} stdin write failed: {error}"),
                ));
                break;
            }
        }
    });
}

fn spawn_backend_stdout_reader(
    kind: BackendKind,
    name: String,
    mut stdout: AsyncBufReader<ChildStdout>,
    event_tx: mpsc::UnboundedSender<GatewayEvent>,
) {
    tokio::spawn(async move {
        loop {
            match read_message_async(&mut stdout).await {
                Ok(Some(message)) => {
                    if event_tx
                        .send(GatewayEvent::BackendMessage(kind, message))
                        .is_err()
                    {
                        break;
                    }
                }
                Ok(None) => {
                    let _ = event_tx.send(GatewayEvent::BackendExited(kind));
                    break;
                }
                Err(error) => {
                    let _ = event_tx.send(GatewayEvent::BackendFailed(
                        kind,
                        format!("{name} stdout read failed: {error}"),
                    ));
                    break;
                }
            }
        }
    });
}

fn spawn_backend_stderr_task(
    kind: BackendKind,
    name: String,
    stderr: tokio::process::ChildStderr,
    event_tx: mpsc::UnboundedSender<GatewayEvent>,
    log_file: Option<PathBuf>,
) {
    tokio::spawn(async move {
        let mut lines = AsyncBufReader::new(stderr).lines();
        loop {
            match lines.next_line().await {
                Ok(Some(line)) => {
                    let message = format!("{name} stderr: {line}");
                    if let Some(path) = &log_file {
                        if let Err(error) = append_log_line(path, &message).await {
                            let _ = event_tx.send(GatewayEvent::BackendLog(format!(
                                "{} stderr log write failed: {error}",
                                kind.display_name()
                            )));
                        }
                    } else if event_tx.send(GatewayEvent::BackendLog(message)).is_err() {
                        break;
                    }
                }
                Ok(None) => break,
                Err(error) => {
                    let _ = event_tx.send(GatewayEvent::BackendLog(format!(
                        "{} stderr read failed: {error}",
                        kind.display_name()
                    )));
                    break;
                }
            }
        }
    });
}

async fn append_log_line(path: &Path, line: &str) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        tokio::fs::create_dir_all(parent)
            .await
            .map_err(|error| format!("failed to create log dir `{}`: {error}", parent.display()))?;
    }
    let mut file = tokio::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .await
        .map_err(|error| format!("failed to open log file `{}`: {error}", path.display()))?;
    file.write_all(line.as_bytes())
        .await
        .map_err(|error| format!("failed to write log file `{}`: {error}", path.display()))?;
    file.write_all(b"\n")
        .await
        .map_err(|error| format!("failed to write log file `{}`: {error}", path.display()))
}

fn backend_command_label(command: &BackendCommand) -> String {
    if command.command.is_empty() {
        return "<empty command>".to_owned();
    }
    std::iter::once(command.command.as_str())
        .chain(command.args.iter().map(String::as_str))
        .collect::<Vec<_>>()
        .join(" ")
}

fn initialize_workspace(params: &Value) -> Result<WorkspaceConfig, String> {
    let params = serde_json::from_value::<InitializeParams>(params.clone())
        .map_err(|error| format!("invalid initialize params: {error}"))?;
    let root_hint = params
        .workspace_folders
        .as_ref()
        .and_then(|folders| folders.first())
        .and_then(|folder| file_uri_to_path(&folder.uri.to_string()))
        .or_else(|| {
            #[allow(deprecated)]
            {
                params
                    .root_uri
                    .as_ref()
                    .and_then(|uri| file_uri_to_path(&uri.to_string()))
            }
        })
        .or_else(|| {
            #[allow(deprecated)]
            {
                params.root_path.as_ref().map(PathBuf::from)
            }
        });
    let cwd = env::current_dir().map_err(|error| format!("failed to read current dir: {error}"))?;

    discover_workspace_config(root_hint.as_deref(), &cwd)
}

fn typescript_initialize_params(params: &Value, workspace: &WorkspaceConfig) -> Value {
    let mut params = params.clone();
    params["initializationOptions"]["hostInfo"] = json!("api-ls");
    params["initializationOptions"]["generatedPackageCacheDir"] =
        json!(workspace.generated_cache_dir.to_string_lossy());
    params["initializationOptions"]["generatedPackageTsconfig"] = json!(workspace
        .generated_cache_dir
        .join("tsconfig.paths.json")
        .to_string_lossy());
    if let Some(plugin) = &workspace.config.effect_language_service_plugin {
        params["initializationOptions"]["plugins"] = json!([{ "name": plugin }]);
    }
    params
}

fn initialize_result() -> Value {
    let capabilities = ServerCapabilities {
        position_encoding: Some(PositionEncodingKind::UTF16),
        text_document_sync: Some(TextDocumentSyncCapability::Kind(
            TextDocumentSyncKind::INCREMENTAL,
        )),
        definition_provider: Some(OneOf::Left(true)),
        references_provider: Some(OneOf::Left(true)),
        hover_provider: Some(HoverProviderCapability::Simple(true)),
        rename_provider: Some(OneOf::Right(RenameOptions {
            prepare_provider: Some(true),
            work_done_progress_options: WorkDoneProgressOptions::default(),
        })),
        ..ServerCapabilities::default()
    };

    serde_json::to_value(InitializeResult {
        capabilities,
        server_info: Some(ServerInfo {
            name: "api-ls".to_owned(),
            version: Some(env!("CARGO_PKG_VERSION").to_owned()),
        }),
    })
    .expect("initialize result serializes")
}

fn discover_workspace_root(start: &Path, cwd: &Path) -> Option<PathBuf> {
    let start = if start.is_absolute() {
        start.to_path_buf()
    } else {
        cwd.join(start)
    };
    let mut current = if start.is_file() {
        start.parent()?.to_path_buf()
    } else {
        start
    };

    loop {
        if current.join("Cargo.toml").is_file()
            || current.join("package.json").is_file()
            || current.join(".git").exists()
        {
            return Some(current);
        }
        if !current.pop() {
            return None;
        }
    }
}

fn resolve_workspace_config(
    root: PathBuf,
    config_path: Option<PathBuf>,
    config: ApiLsConfig,
) -> WorkspaceConfig {
    let generated_cache_dir = absolutize_from(&root, &config.generated_cache_dir);
    let symbol_graph = absolutize_from(&root, &config.symbol_graph);
    let usage_index = absolutize_from(&root, &config.usage_index);

    WorkspaceConfig {
        root,
        config_path,
        config,
        generated_cache_dir,
        symbol_graph,
        usage_index,
    }
}

fn workspace_log_file(workspace: &WorkspaceConfig) -> Option<PathBuf> {
    workspace
        .config
        .log_file
        .as_ref()
        .map(|path| absolutize_from(&workspace.root, path))
}

fn absolutize_from(root: &Path, path: &Path) -> PathBuf {
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        root.join(path)
    }
}

fn is_rust_message(method: &str, params: &Value) -> bool {
    method.starts_with("rust-analyzer/")
        || document_uris(params).iter().any(|uri| {
            uri.strip_prefix("file://")
                .is_some_and(|path| path.ends_with(".rs"))
        })
}

fn is_typescript_uri(uri: &str) -> bool {
    let Some(path) = uri.strip_prefix("file://") else {
        return false;
    };
    [".ts", ".tsx", ".mts", ".cts", ".js", ".jsx", ".mjs", ".cjs"]
        .iter()
        .any(|extension| path.ends_with(extension))
}

fn generated_file_path(params: &Value, workspace: &WorkspaceConfig) -> Option<PathBuf> {
    let path = params
        .get("uri")
        .and_then(Value::as_str)
        .and_then(file_uri_to_path)
        .or_else(|| {
            params
                .get("path")
                .and_then(Value::as_str)
                .map(PathBuf::from)
        })?;
    let path = if path.is_absolute() {
        path
    } else {
        workspace.generated_cache_dir.join(path)
    };

    path.starts_with(&workspace.generated_cache_dir)
        .then_some(path)
}

fn path_to_file_uri(path: &Path) -> String {
    format!("file://{}", path.to_string_lossy())
}

fn same_uri(left: &str, right: &str) -> bool {
    match (file_uri_to_path(left), file_uri_to_path(right)) {
        (Some(left), Some(right)) => left == right,
        _ => left == right,
    }
}

fn dedupe_locations(locations: Vec<Value>, workspace: &WorkspaceConfig) -> Value {
    let mut seen = BTreeSet::new();
    let mut deduped = Vec::new();

    for location in locations {
        let Some(uri) = location.get("uri").and_then(Value::as_str) else {
            continue;
        };
        if file_uri_to_path(uri)
            .is_some_and(|path| path.starts_with(&workspace.generated_cache_dir))
        {
            continue;
        }
        let key = serde_json::to_string(&json!({
            "uri": uri,
            "range": location.get("range").cloned().unwrap_or(Value::Null),
        }))
        .expect("location key serializes");
        if seen.insert(key) {
            deduped.push(location);
        }
    }

    Value::Array(deduped)
}

fn diagnostic_start(diagnostic: &Value) -> GraphPosition {
    diagnostic
        .get("range")
        .and_then(|range| range.get("start"))
        .and_then(|start| serde_json::from_value(start.clone()).ok())
        .unwrap_or(GraphPosition {
            line: u32::MAX,
            character: u32::MAX,
        })
}

fn diagnostic_message(diagnostic: &Value) -> String {
    diagnostic
        .get("message")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned()
}

fn validate_rename_name(new_name: &str) -> Result<(), String> {
    let mut chars = new_name.chars();
    let Some(first) = chars.next() else {
        return Err("rename target cannot be empty".to_owned());
    };
    if !(first == '_' || first.is_ascii_alphabetic()) {
        return Err(format!(
            "rename target `{new_name}` must start with a letter or underscore"
        ));
    }
    if chars.any(|character| !(character == '_' || character.is_ascii_alphanumeric())) {
        return Err(format!(
            "rename target `{new_name}` must be a Rust/TypeScript identifier"
        ));
    }
    Ok(())
}

fn document_uris(value: &Value) -> BTreeSet<String> {
    let mut uris = BTreeSet::new();
    collect_document_uris(value, &mut uris);
    uris
}

fn collect_document_uris(value: &Value, uris: &mut BTreeSet<String>) {
    match value {
        Value::Object(object) => {
            if let Some(uri) = object.get("uri").and_then(Value::as_str) {
                uris.insert(uri.to_owned());
            }
            for value in object.values() {
                collect_document_uris(value, uris);
            }
        }
        Value::Array(values) => {
            for value in values {
                collect_document_uris(value, uris);
            }
        }
        Value::Null | Value::Bool(_) | Value::Number(_) | Value::String(_) => {}
    }
}

fn file_uri_to_path(uri: &str) -> Option<PathBuf> {
    let path = uri.strip_prefix("file://")?;
    Some(PathBuf::from(percent_decode(path)))
}

fn percent_decode(value: &str) -> String {
    let mut output = String::with_capacity(value.len());
    let bytes = value.as_bytes();
    let mut index = 0;

    while index < bytes.len() {
        if bytes[index] == b'%' && index + 2 < bytes.len() {
            let hex = &value[index + 1..index + 3];
            if let Ok(byte) = u8::from_str_radix(hex, 16) {
                output.push(char::from(byte));
                index += 3;
                continue;
            }
        }
        output.push(char::from(bytes[index]));
        index += 1;
    }

    output
}

fn read_message(reader: &mut impl BufRead) -> Result<Option<LspMessage>, String> {
    let mut content_length = None;
    let mut line = String::new();

    loop {
        line.clear();
        let read = reader
            .read_line(&mut line)
            .map_err(|error| format!("failed to read LSP header: {error}"))?;
        if read == 0 {
            return Ok(None);
        }

        let header = line.trim_end_matches(['\r', '\n']);
        if header.is_empty() {
            break;
        }

        let Some((name, value)) = header.split_once(':') else {
            return Err(format!("malformed LSP header `{header}`"));
        };
        if name.eq_ignore_ascii_case("content-length") {
            content_length = Some(
                value
                    .trim()
                    .parse::<usize>()
                    .map_err(|error| format!("invalid Content-Length `{value}`: {error}"))?,
            );
        }
    }

    let content_length =
        content_length.ok_or_else(|| "missing Content-Length header".to_owned())?;
    let mut body = vec![0; content_length];
    reader
        .read_exact(&mut body)
        .map_err(|error| format!("failed to read LSP body: {error}"))?;

    let value = serde_json::from_slice::<Value>(&body)
        .map_err(|error| format!("failed to parse LSP JSON body: {error}"))?;

    Ok(Some(LspMessage {
        id: value.get("id").cloned(),
        method: value
            .get("method")
            .and_then(Value::as_str)
            .map(str::to_owned),
        params: value.get("params").cloned().unwrap_or(Value::Null),
        raw: value,
    }))
}

async fn read_message_async<R>(reader: &mut R) -> Result<Option<LspMessage>, String>
where
    R: AsyncBufRead + Unpin,
{
    let mut content_length = None;
    let mut line = String::new();

    loop {
        line.clear();
        let read = reader
            .read_line(&mut line)
            .await
            .map_err(|error| format!("failed to read LSP header: {error}"))?;
        if read == 0 {
            return Ok(None);
        }

        let header = line.trim_end_matches(['\r', '\n']);
        if header.is_empty() {
            break;
        }

        let Some((name, value)) = header.split_once(':') else {
            return Err(format!("malformed LSP header `{header}`"));
        };
        if name.eq_ignore_ascii_case("content-length") {
            content_length = Some(
                value
                    .trim()
                    .parse::<usize>()
                    .map_err(|error| format!("invalid Content-Length `{value}`: {error}"))?,
            );
        }
    }

    let content_length =
        content_length.ok_or_else(|| "missing Content-Length header".to_owned())?;
    let mut body = vec![0; content_length];
    reader
        .read_exact(&mut body)
        .await
        .map_err(|error| format!("failed to read LSP body: {error}"))?;

    let value = serde_json::from_slice::<Value>(&body)
        .map_err(|error| format!("failed to parse LSP JSON body: {error}"))?;

    Ok(Some(LspMessage {
        id: value.get("id").cloned(),
        method: value
            .get("method")
            .and_then(Value::as_str)
            .map(str::to_owned),
        params: value.get("params").cloned().unwrap_or(Value::Null),
        raw: value,
    }))
}

fn write_framed(writer: &mut impl Write, value: &Value) -> Result<(), String> {
    let body = serde_json::to_vec(value).map_err(|error| error.to_string())?;
    write!(writer, "Content-Length: {}\r\n\r\n", body.len())
        .map_err(|error| format!("failed to write LSP header: {error}"))?;
    writer
        .write_all(&body)
        .map_err(|error| format!("failed to write LSP body: {error}"))?;
    writer
        .flush()
        .map_err(|error| format!("failed to flush LSP body: {error}"))
}

async fn write_framed_async<W>(writer: &mut W, value: &Value) -> Result<(), String>
where
    W: AsyncWrite + Unpin,
{
    let body = serde_json::to_vec(value).map_err(|error| error.to_string())?;
    writer
        .write_all(format!("Content-Length: {}\r\n\r\n", body.len()).as_bytes())
        .await
        .map_err(|error| format!("failed to write LSP header: {error}"))?;
    writer
        .write_all(&body)
        .await
        .map_err(|error| format!("failed to write LSP body: {error}"))?;
    writer
        .flush()
        .await
        .map_err(|error| format!("failed to flush LSP body: {error}"))
}

async fn write_request_async<W>(
    writer: &mut W,
    id: u64,
    method: &str,
    params: Value,
) -> Result<(), String>
where
    W: AsyncWrite + Unpin,
{
    write_framed_async(
        writer,
        &json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": params,
        }),
    )
    .await
}

fn runtime() -> Result<tokio::runtime::Runtime, String> {
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(|error| format!("failed to create async runtime: {error}"))
}

async fn run_stdio_async() -> Result<(), String> {
    let (event_tx, event_rx) = mpsc::unbounded_channel();
    let (output_tx, output_rx) = mpsc::unbounded_channel();
    let client_reader = tokio::spawn(read_client_loop(tokio::io::stdin(), event_tx.clone()));
    let output_writer = tokio::spawn(write_output_loop(tokio::io::stdout(), output_rx));

    let result = AsyncLspServer::new(event_tx, event_rx, output_tx)
        .run()
        .await;
    client_reader.abort();
    let _ = client_reader.await;
    let writer_result = output_writer
        .await
        .map_err(|error| format!("LSP output task failed: {error}"))?;

    result?;
    writer_result
}

async fn run_messages_async(messages: Vec<LspMessage>) -> Result<Vec<Value>, String> {
    let (event_tx, event_rx) = mpsc::unbounded_channel();
    let (output_tx, mut output_rx) = mpsc::unbounded_channel();
    let client_tx = event_tx.clone();
    let server = AsyncLspServer::new(event_tx, event_rx, output_tx);

    for message in messages {
        client_tx
            .send(GatewayEvent::ClientMessage(message))
            .map_err(|error| format!("failed to queue test LSP input: {error}"))?;
    }
    client_tx
        .send(GatewayEvent::ClientEof)
        .map_err(|error| format!("failed to queue test LSP EOF: {error}"))?;
    drop(client_tx);

    server.run().await?;

    let mut output = Vec::new();
    while let Ok(message) = output_rx.try_recv() {
        output.push(message);
    }
    Ok(output)
}

async fn read_client_loop<R>(input: R, event_tx: mpsc::UnboundedSender<GatewayEvent>)
where
    R: AsyncRead + Unpin,
{
    let mut reader = AsyncBufReader::new(input);
    loop {
        match read_message_async(&mut reader).await {
            Ok(Some(message)) => {
                if event_tx.send(GatewayEvent::ClientMessage(message)).is_err() {
                    break;
                }
            }
            Ok(None) => {
                let _ = event_tx.send(GatewayEvent::ClientEof);
                break;
            }
            Err(error) => {
                let _ = event_tx.send(GatewayEvent::BackendLog(format!(
                    "client input read failed: {error}"
                )));
                let _ = event_tx.send(GatewayEvent::ClientEof);
                break;
            }
        }
    }
}

async fn write_output_loop<W>(
    mut writer: W,
    mut output_rx: mpsc::UnboundedReceiver<Value>,
) -> Result<(), String>
where
    W: AsyncWrite + Unpin,
{
    while let Some(message) = output_rx.recv().await {
        write_framed_async(&mut writer, &message).await?;
    }
    Ok(())
}

fn write_output_messages(mut output: impl Write, messages: Vec<Value>) -> Result<(), String> {
    for message in messages {
        write_framed(&mut output, &message)?;
    }
    Ok(())
}

fn rpc_id_key(id: &Value) -> String {
    serde_json::to_string(id).expect("JSON-RPC id serializes")
}

fn rpc_id_from_key(key: &str) -> Value {
    serde_json::from_str(key).unwrap_or_else(|_| Value::String(key.to_owned()))
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicU64, Ordering};

    use serde_json::json;

    use super::*;

    static TEST_ID: AtomicU64 = AtomicU64::new(0);

    #[test]
    fn discovers_workspace_config_and_resolves_cache_paths() {
        let root = test_root("config");
        fs::create_dir_all(&root).expect("create root");
        fs::write(root.join("Cargo.toml"), "[workspace]\n").expect("write manifest");
        fs::write(
            root.join(".api-ls.json"),
            r#"{
  "generatedCacheDir": "target/custom-cache",
  "symbolGraph": "target/symbols.json",
  "rustAnalyzer": { "command": "ra-test", "args": ["--log"] }
}"#,
        )
        .expect("write config");

        let config = discover_workspace_config(Some(&root), &root).expect("discover config");

        assert_eq!(config.root, root);
        assert_eq!(
            config.generated_cache_dir,
            config.root.join("target/custom-cache")
        );
        assert_eq!(config.symbol_graph, config.root.join("target/symbols.json"));
        assert_eq!(
            config.usage_index,
            config
                .root
                .join("target/api-contract/graph/effect-usage-index.json")
        );
        assert_eq!(config.config.unused_endpoint_lints, UsageLintLevel::Warn);
        assert_eq!(config.config.rust_analyzer.command, "ra-test");
        assert_eq!(
            config.config.typescript.command,
            "typescript-language-server"
        );
    }

    #[test]
    fn initialize_and_shutdown_use_lsp_framing() {
        let root = test_root("initialize");
        fs::create_dir_all(&root).expect("create root");
        fs::write(root.join("Cargo.toml"), "[workspace]\n").expect("write manifest");
        write_mock_backend_config(&root);

        let input = [
            framed(&json!({
                "jsonrpc": "2.0",
                "id": 1,
                "method": "initialize",
                "params": {
                    "processId": null,
                    "rootUri": path_to_file_uri(&root),
                    "capabilities": {}
                }
            })),
            framed(&json!({
                "jsonrpc": "2.0",
                "id": 2,
                "method": "shutdown"
            })),
            framed(&json!({
                "jsonrpc": "2.0",
                "method": "exit"
            })),
        ]
        .join("");
        let mut output = Vec::new();

        run(input.as_bytes(), &mut output).expect("run server");
        let output = String::from_utf8(output).expect("utf8 output");

        assert!(output.contains("\"id\":1"));
        assert!(output.contains("\"serverInfo\":{\"name\":\"api-ls\""));
        assert!(output.contains("\"id\":2"));
        assert!(output.contains("\"result\":null"));
    }

    #[test]
    fn invalid_config_reports_clear_initialize_error() {
        let root = test_root("invalid-config");
        fs::create_dir_all(&root).expect("create root");
        fs::write(root.join("Cargo.toml"), "[workspace]\n").expect("write manifest");
        fs::write(root.join(".api-ls.json"), "{ nope").expect("write config");

        let input = framed(&json!({
            "jsonrpc": "2.0",
            "id": "init",
            "method": "initialize",
            "params": {
                "processId": null,
                "rootUri": path_to_file_uri(&root),
                "capabilities": {}
            }
        }));
        let mut output = Vec::new();

        run(input.as_bytes(), &mut output).expect("run server");
        let output = String::from_utf8(output).expect("utf8 output");

        assert!(output.contains("\"id\":\"init\""));
        assert!(output.contains("failed to parse"));
        assert!(output.contains(".api-ls.json"));
    }

    #[test]
    fn missing_rust_backend_fails_initialize_clearly() {
        let root = test_root("missing-rust-backend");
        fs::create_dir_all(&root).expect("create root");
        fs::write(root.join("Cargo.toml"), "[workspace]\n").expect("write manifest");
        fs::write(
            root.join(".api-ls.json"),
            json!({
                "rustAnalyzer": { "command": "" },
                "typescript": { "command": "python3", "args": ["-c", MOCK_BACKEND] }
            })
            .to_string(),
        )
        .expect("write config");

        let output = initialize_output(&root);

        assert!(output.contains("\"id\":\"init\""));
        assert!(output.contains("requires the rust-analyzer backend"));
        assert!(
            output.contains("configured command: &lt;empty command&gt;")
                || output.contains("configured command: <empty command>")
        );
        assert!(output.contains("update `.api-ls.json`"));
    }

    #[test]
    fn missing_typescript_backend_fails_initialize_clearly() {
        let root = test_root("missing-typescript-backend");
        fs::create_dir_all(&root).expect("create root");
        fs::write(root.join("Cargo.toml"), "[workspace]\n").expect("write manifest");
        let backend = root.join("mock_backend.py");
        fs::write(&backend, MOCK_BACKEND).expect("write backend");
        fs::write(
            root.join(".api-ls.json"),
            json!({
                "rustAnalyzer": {
                    "command": "python3",
                    "args": [backend.to_string_lossy()]
                },
                "typescript": { "command": "" }
            })
            .to_string(),
        )
        .expect("write config");

        let output = initialize_output(&root);

        assert!(output.contains("\"id\":\"init\""));
        assert!(output.contains("requires the TypeScript/Effect backend"));
        assert!(
            output.contains("configured command: &lt;empty command&gt;")
                || output.contains("configured command: <empty command>")
        );
        assert!(output.contains("update `.api-ls.json`"));
    }

    #[test]
    fn proxies_rust_requests_and_diagnostics_through_backend() {
        let root = test_root("rust-proxy");
        fs::create_dir_all(&root).expect("create root");
        fs::write(root.join("Cargo.toml"), "[workspace]\n").expect("write manifest");
        write_mock_backend_config(&root);
        let rust_uri = format!("{}/src/lib.rs", path_to_file_uri(&root));

        let input = [
            framed(&json!({
                "jsonrpc": "2.0",
                "id": 1,
                "method": "initialize",
                "params": {
                    "processId": null,
                    "rootUri": path_to_file_uri(&root),
                    "capabilities": {}
                }
            })),
            framed(&json!({
                "jsonrpc": "2.0",
                "method": "textDocument/didOpen",
                "params": {
                    "textDocument": {
                        "uri": rust_uri,
                        "languageId": "rust",
                        "version": 1,
                        "text": "fn main() {}"
                    }
                }
            })),
            framed(&json!({
                "jsonrpc": "2.0",
                "id": 2,
                "method": "textDocument/definition",
                "params": {
                    "textDocument": { "uri": rust_uri },
                    "position": { "line": 0, "character": 3 }
                }
            })),
            framed(&json!({
                "jsonrpc": "2.0",
                "id": 3,
                "method": "shutdown"
            })),
            framed(&json!({
                "jsonrpc": "2.0",
                "method": "exit"
            })),
        ]
        .join("");
        let mut output = Vec::new();

        run(input.as_bytes(), &mut output).expect("run server");
        let output = String::from_utf8(output).expect("utf8 output");

        assert!(output.contains("\"id\":2"));
        assert!(output.contains("\"method\":\"textDocument/publishDiagnostics\""));
        assert!(output.contains("\"message\":\"mock diagnostic\""));
        assert!(output.contains("\"uri\":\"file:///mock-definition.rs\""));
    }

    #[test]
    fn proxies_typescript_requests_and_serves_generated_files() {
        let root = test_root("typescript-proxy");
        let generated_cache_dir = root.join("target/api-contract/effect-v4/packages");
        fs::create_dir_all(&generated_cache_dir).expect("create generated cache");
        fs::write(root.join("package.json"), "{}\n").expect("write package manifest");
        fs::write(
            generated_cache_dir.join("index.ts"),
            "export const generated = true\n",
        )
        .expect("write generated file");
        write_mock_backend_config(&root);
        let ts_uri = format!("{}/src/client.ts", path_to_file_uri(&root));
        let generated_uri = path_to_file_uri(&generated_cache_dir.join("index.ts"));

        let input = [
            framed(&json!({
                "jsonrpc": "2.0",
                "id": 1,
                "method": "initialize",
                "params": {
                    "processId": null,
                    "rootUri": path_to_file_uri(&root),
                    "capabilities": {}
                }
            })),
            framed(&json!({
                "jsonrpc": "2.0",
                "id": 2,
                "method": "textDocument/definition",
                "params": {
                    "textDocument": { "uri": ts_uri },
                    "position": { "line": 0, "character": 10 }
                }
            })),
            framed(&json!({
                "jsonrpc": "2.0",
                "id": 3,
                "method": "api-ls/generatedPackageFile",
                "params": { "uri": generated_uri }
            })),
            framed(&json!({
                "jsonrpc": "2.0",
                "id": 4,
                "method": "shutdown"
            })),
            framed(&json!({
                "jsonrpc": "2.0",
                "method": "exit"
            })),
        ]
        .join("");
        let mut output = Vec::new();

        run(input.as_bytes(), &mut output).expect("run server");
        let output = String::from_utf8(output).expect("utf8 output");

        assert!(output.contains("\"id\":2"));
        assert!(output.contains("\"uri\":\"file:///mock-definition.rs\""));
        assert!(output.contains("\"id\":3"));
        assert!(output.contains("export const generated = true"));
    }

    #[test]
    fn concurrent_rust_and_typescript_requests_do_not_block_each_other() {
        let root = test_root("concurrent-proxy");
        fs::create_dir_all(&root).expect("create root");
        fs::write(root.join("Cargo.toml"), "[workspace]\n").expect("write manifest");
        write_backend_config(&root, SLOW_RUST_BACKEND, FAST_BACKEND, None);
        let rust_uri = path_to_file_uri(&root.join("src/lib.rs"));
        let ts_uri = path_to_file_uri(&root.join("src/client.ts"));

        let input = [
            framed(&json!({
                "jsonrpc": "2.0",
                "id": 1,
                "method": "initialize",
                "params": {
                    "processId": null,
                    "rootUri": path_to_file_uri(&root),
                    "capabilities": {}
                }
            })),
            framed(&json!({
                "jsonrpc": "2.0",
                "id": 2,
                "method": "textDocument/definition",
                "params": {
                    "textDocument": { "uri": rust_uri },
                    "position": { "line": 0, "character": 3 }
                }
            })),
            framed(&json!({
                "jsonrpc": "2.0",
                "id": 3,
                "method": "textDocument/definition",
                "params": {
                    "textDocument": { "uri": ts_uri },
                    "position": { "line": 0, "character": 10 }
                }
            })),
            framed(&json!({
                "jsonrpc": "2.0",
                "id": 4,
                "method": "shutdown"
            })),
            framed(&json!({
                "jsonrpc": "2.0",
                "method": "exit"
            })),
        ]
        .join("");
        let mut output = Vec::new();

        run(input.as_bytes(), &mut output).expect("run server");
        let output = String::from_utf8(output).expect("utf8 output");

        assert!(output.contains("file:///fast-definition.ts"));
        assert!(output.contains("file:///slow-definition.rs"));
        assert_output_order(&output, "\"id\":3", "\"id\":2");
    }

    #[test]
    fn forwards_cancel_request_to_backend_while_request_is_pending() {
        let root = test_root("cancel-proxy");
        fs::create_dir_all(&root).expect("create root");
        fs::write(root.join("Cargo.toml"), "[workspace]\n").expect("write manifest");
        write_backend_config(&root, CANCELLABLE_BACKEND, FAST_BACKEND, None);
        let rust_uri = path_to_file_uri(&root.join("src/lib.rs"));

        let input = [
            framed(&json!({
                "jsonrpc": "2.0",
                "id": 1,
                "method": "initialize",
                "params": {
                    "processId": null,
                    "rootUri": path_to_file_uri(&root),
                    "capabilities": {}
                }
            })),
            framed(&json!({
                "jsonrpc": "2.0",
                "id": "slow",
                "method": "textDocument/definition",
                "params": {
                    "textDocument": { "uri": rust_uri },
                    "position": { "line": 0, "character": 3 }
                }
            })),
            framed(&json!({
                "jsonrpc": "2.0",
                "method": "$/cancelRequest",
                "params": { "id": "slow" }
            })),
        ]
        .join("");
        let mut output = Vec::new();

        run(input.as_bytes(), &mut output).expect("run server");
        let output = String::from_utf8(output).expect("utf8 output");

        assert!(output.contains("cancelled:2"));
        assert!(output.contains("\"id\":\"slow\""));
        assert_output_order(&output, "cancelled:2", "\"id\":\"slow\"");
    }

    #[test]
    fn captures_backend_stderr_to_configured_log_file() {
        let root = test_root("stderr-log");
        fs::create_dir_all(&root).expect("create root");
        fs::write(root.join("Cargo.toml"), "[workspace]\n").expect("write manifest");
        write_backend_config(
            &root,
            STDERR_BACKEND,
            STDERR_BACKEND,
            Some("target/api-ls.log"),
        );

        let input = [
            framed(&json!({
                "jsonrpc": "2.0",
                "id": 1,
                "method": "initialize",
                "params": {
                    "processId": null,
                    "rootUri": path_to_file_uri(&root),
                    "capabilities": {}
                }
            })),
            framed(&json!({
                "jsonrpc": "2.0",
                "id": 2,
                "method": "shutdown"
            })),
            framed(&json!({
                "jsonrpc": "2.0",
                "method": "exit"
            })),
        ]
        .join("");
        let mut output = Vec::new();

        run(input.as_bytes(), &mut output).expect("run server");
        let log = fs::read_to_string(root.join("target/api-ls.log")).expect("read log");

        assert!(log.contains("rust-analyzer stderr: backend stderr hello"));
        assert!(log.contains("TypeScript/Effect stderr: backend stderr hello"));
    }

    #[test]
    fn redirects_generated_typescript_definition_to_rust_source() {
        let root = test_root("cross-definition");
        let generated_cache_dir = root.join("target/api-contract/effect-v4/packages");
        fs::create_dir_all(&generated_cache_dir).expect("create generated cache");
        fs::create_dir_all(root.join("src")).expect("create src");
        fs::write(root.join("Cargo.toml"), "[workspace]\n").expect("write manifest");
        write_mock_backend_config(&root);
        let rust_uri = path_to_file_uri(&root.join("src/lib.rs"));
        let ts_uri = path_to_file_uri(&generated_cache_dir.join("endpoints.ts"));
        fs::write(
            root.join("target/api-contract/rust-ts-symbols.json"),
            json!({
                "symbols": [{
                    "id": "endpoint:get_user",
                    "kind": "endpoint",
                    "rust": {
                        "uri": rust_uri,
                        "range": {
                            "start": { "line": 10, "character": 9 },
                            "end": { "line": 10, "character": 17 }
                        }
                    },
                    "typescript": [{
                        "uri": ts_uri,
                        "generated": true,
                        "range": {
                            "start": { "line": 4, "character": 15 },
                            "end": { "line": 4, "character": 22 }
                        }
                    }]
                }]
            })
            .to_string(),
        )
        .expect("write symbol graph");

        let input = [
            framed(&json!({
                "jsonrpc": "2.0",
                "id": 1,
                "method": "initialize",
                "params": {
                    "processId": null,
                    "rootUri": path_to_file_uri(&root),
                    "capabilities": {}
                }
            })),
            framed(&json!({
                "jsonrpc": "2.0",
                "id": 2,
                "method": "textDocument/definition",
                "params": {
                    "textDocument": { "uri": ts_uri },
                    "position": { "line": 4, "character": 18 }
                }
            })),
            framed(&json!({
                "jsonrpc": "2.0",
                "id": 3,
                "method": "shutdown"
            })),
            framed(&json!({
                "jsonrpc": "2.0",
                "method": "exit"
            })),
        ]
        .join("");
        let mut output = Vec::new();

        run(input.as_bytes(), &mut output).expect("run server");
        let output = String::from_utf8(output).expect("utf8 output");

        assert!(output.contains("\"id\":2"));
        assert!(output.contains(&rust_uri));
        assert!(output.contains("\"line\":10"));
        assert!(!output.contains("\"id\":2,\"error\""));
    }

    #[test]
    fn merges_cross_language_references_without_generated_duplicates() {
        let root = test_root("cross-references");
        let generated_cache_dir = root.join("target/api-contract/effect-v4/packages");
        fs::create_dir_all(&generated_cache_dir).expect("create generated cache");
        fs::create_dir_all(root.join("src")).expect("create src");
        fs::create_dir_all(root.join("client")).expect("create client");
        fs::write(root.join("Cargo.toml"), "[workspace]\n").expect("write manifest");
        write_mock_backend_config(&root);
        let rust_uri = path_to_file_uri(&root.join("src/lib.rs"));
        let generated_uri = path_to_file_uri(&generated_cache_dir.join("endpoints.ts"));
        let usage_uri = path_to_file_uri(&root.join("client/use-api.ts"));
        let usage_location = json!({
            "uri": usage_uri,
            "generated": false,
            "range": {
                "start": { "line": 2, "character": 4 },
                "end": { "line": 2, "character": 11 }
            }
        });
        fs::write(
            root.join("target/api-contract/rust-ts-symbols.json"),
            json!({
                "symbols": [{
                    "id": "endpoint:get_user",
                    "kind": "endpoint",
                    "rust": {
                        "uri": rust_uri,
                        "range": {
                            "start": { "line": 10, "character": 9 },
                            "end": { "line": 10, "character": 17 }
                        }
                    },
                    "typescript": [
                        usage_location,
                        usage_location,
                        {
                            "uri": generated_uri,
                            "generated": true,
                            "range": {
                                "start": { "line": 4, "character": 15 },
                                "end": { "line": 4, "character": 22 }
                            }
                        }
                    ]
                }]
            })
            .to_string(),
        )
        .expect("write symbol graph");

        let input = [
            framed(&json!({
                "jsonrpc": "2.0",
                "id": 1,
                "method": "initialize",
                "params": {
                    "processId": null,
                    "rootUri": path_to_file_uri(&root),
                    "capabilities": {}
                }
            })),
            framed(&json!({
                "jsonrpc": "2.0",
                "id": 2,
                "method": "textDocument/references",
                "params": {
                    "textDocument": { "uri": rust_uri },
                    "position": { "line": 10, "character": 12 },
                    "context": { "includeDeclaration": true }
                }
            })),
            framed(&json!({
                "jsonrpc": "2.0",
                "id": 3,
                "method": "shutdown"
            })),
            framed(&json!({
                "jsonrpc": "2.0",
                "method": "exit"
            })),
        ]
        .join("");
        let mut output = Vec::new();

        run(input.as_bytes(), &mut output).expect("run server");
        let output = String::from_utf8(output).expect("utf8 output");

        assert!(output.contains("\"id\":2"));
        assert!(output.contains(&rust_uri));
        assert_eq!(output.matches(&usage_uri).count(), 1);
        assert!(!output.contains(&generated_uri));
    }

    #[test]
    fn augments_hover_with_api_metadata() {
        let root = test_root("hover");
        let generated_cache_dir = root.join("target/api-contract/effect-v4/packages");
        fs::create_dir_all(&generated_cache_dir).expect("create generated cache");
        fs::create_dir_all(root.join("src")).expect("create src");
        fs::write(root.join("Cargo.toml"), "[workspace]\n").expect("write manifest");
        write_mock_backend_config(&root);
        let rust_uri = path_to_file_uri(&root.join("src/lib.rs"));
        let ts_uri = path_to_file_uri(&generated_cache_dir.join("endpoints.ts"));
        fs::write(
            root.join("target/api-contract/rust-ts-symbols.json"),
            json!({
                "symbols": [{
                    "id": "endpoint:get_user",
                    "kind": "endpoint",
                    "rust": {
                        "uri": rust_uri,
                        "range": {
                            "start": { "line": 10, "character": 9 },
                            "end": { "line": 10, "character": 17 }
                        }
                    },
                    "typescript": [{
                        "uri": ts_uri,
                        "generated": true,
                        "range": {
                            "start": { "line": 4, "character": 15 },
                            "end": { "line": 4, "character": 22 }
                        }
                    }],
                    "metadata": {
                        "method": "GET",
                        "route": "/users/{id}",
                        "effectSignature": "Effect.Effect<User, ApiClientError | GetUserError, ServerApi>",
                        "errors": ["GetUserError"],
                        "rustPath": ["crate", "users", "get_user"],
                        "tsPath": ["users", "getUser"]
                    }
                }]
            })
            .to_string(),
        )
        .expect("write symbol graph");

        let input = [
            framed(&json!({
                "jsonrpc": "2.0",
                "id": 1,
                "method": "initialize",
                "params": {
                    "processId": null,
                    "rootUri": path_to_file_uri(&root),
                    "capabilities": {}
                }
            })),
            framed(&json!({
                "jsonrpc": "2.0",
                "id": 2,
                "method": "textDocument/hover",
                "params": {
                    "textDocument": { "uri": ts_uri },
                    "position": { "line": 4, "character": 18 }
                }
            })),
            framed(&json!({
                "jsonrpc": "2.0",
                "id": 3,
                "method": "shutdown"
            })),
            framed(&json!({
                "jsonrpc": "2.0",
                "method": "exit"
            })),
        ]
        .join("");
        let mut output = Vec::new();

        run(input.as_bytes(), &mut output).expect("run server");
        let output = String::from_utf8(output).expect("utf8 output");

        assert!(output.contains("\"id\":2"));
        assert!(output.contains("GET /users/{id}"));
        assert!(output.contains("Effect.Effect"));
        assert!(output.contains("crate::users::get_user"));
        assert!(output.contains("Usage count"));
        assert!(output.contains("pending"));
    }

    #[test]
    fn prepares_and_builds_cross_language_rename_edit() {
        let root = test_root("rename");
        let generated_cache_dir = root.join("target/api-contract/effect-v4/packages");
        fs::create_dir_all(&generated_cache_dir).expect("create generated cache");
        fs::create_dir_all(root.join("src")).expect("create src");
        fs::create_dir_all(root.join("client")).expect("create client");
        fs::write(root.join("Cargo.toml"), "[workspace]\n").expect("write manifest");
        write_mock_backend_config(&root);
        let rust_uri = path_to_file_uri(&root.join("src/lib.rs"));
        let generated_uri = path_to_file_uri(&generated_cache_dir.join("endpoints.ts"));
        let usage_uri = path_to_file_uri(&root.join("client/use-api.ts"));
        fs::write(
            root.join("target/api-contract/rust-ts-symbols.json"),
            json!({
                "symbols": [{
                    "id": "endpoint:get_user",
                    "kind": "endpoint",
                    "rust": {
                        "uri": rust_uri,
                        "range": {
                            "start": { "line": 10, "character": 9 },
                            "end": { "line": 10, "character": 17 }
                        }
                    },
                    "typescript": [
                        {
                            "uri": usage_uri,
                            "range": {
                                "start": { "line": 2, "character": 4 },
                                "end": { "line": 2, "character": 11 }
                            }
                        },
                        {
                            "uri": generated_uri,
                            "generated": true,
                            "range": {
                                "start": { "line": 4, "character": 15 },
                                "end": { "line": 4, "character": 22 }
                            }
                        }
                    ],
                    "metadata": {
                        "method": "GET",
                        "route": "/users/{id}",
                        "rustPath": ["crate", "users", "get_user"],
                        "tsPath": ["users", "getUser"],
                        "renamePlaceholder": "getUser",
                        "reservedNames": ["deleteUser"]
                    }
                }]
            })
            .to_string(),
        )
        .expect("write symbol graph");

        let input = [
            framed(&json!({
                "jsonrpc": "2.0",
                "id": 1,
                "method": "initialize",
                "params": {
                    "processId": null,
                    "rootUri": path_to_file_uri(&root),
                    "capabilities": {}
                }
            })),
            framed(&json!({
                "jsonrpc": "2.0",
                "id": 2,
                "method": "textDocument/prepareRename",
                "params": {
                    "textDocument": { "uri": usage_uri },
                    "position": { "line": 2, "character": 6 }
                }
            })),
            framed(&json!({
                "jsonrpc": "2.0",
                "id": 3,
                "method": "textDocument/rename",
                "params": {
                    "textDocument": { "uri": rust_uri },
                    "position": { "line": 10, "character": 12 },
                    "newName": "fetchUser"
                }
            })),
            framed(&json!({
                "jsonrpc": "2.0",
                "id": 4,
                "method": "shutdown"
            })),
            framed(&json!({
                "jsonrpc": "2.0",
                "method": "exit"
            })),
        ]
        .join("");
        let mut output = Vec::new();

        run(input.as_bytes(), &mut output).expect("run server");
        let output = String::from_utf8(output).expect("utf8 output");

        assert!(output.contains("\"id\":2"));
        assert!(output.contains("\"placeholder\":\"getUser\""));
        assert!(output.contains("\"id\":3"));
        assert!(output.contains(&rust_uri));
        assert!(output.contains(&usage_uri));
        assert!(!output.contains(&generated_uri));
        assert_eq!(output.matches("\"newText\":\"fetchUser\"").count(), 2);
        assert!(!output.contains("/users/{id}\\\",\\\"newText"));
    }

    #[test]
    fn rejects_cross_language_rename_conflicts() {
        let root = test_root("rename-conflict");
        fs::create_dir_all(root.join("target/api-contract")).expect("create graph dir");
        fs::create_dir_all(root.join("src")).expect("create src");
        fs::write(root.join("Cargo.toml"), "[workspace]\n").expect("write manifest");
        write_mock_backend_config(&root);
        let rust_uri = path_to_file_uri(&root.join("src/lib.rs"));
        fs::write(
            root.join("target/api-contract/rust-ts-symbols.json"),
            json!({
                "symbols": [{
                    "id": "endpoint:get_user",
                    "kind": "endpoint",
                    "rust": {
                        "uri": rust_uri,
                        "range": {
                            "start": { "line": 10, "character": 9 },
                            "end": { "line": 10, "character": 17 }
                        }
                    },
                    "typescript": [],
                    "metadata": {
                        "reservedNames": ["deleteUser"]
                    }
                }]
            })
            .to_string(),
        )
        .expect("write symbol graph");

        let input = [
            framed(&json!({
                "jsonrpc": "2.0",
                "id": 1,
                "method": "initialize",
                "params": {
                    "processId": null,
                    "rootUri": path_to_file_uri(&root),
                    "capabilities": {}
                }
            })),
            framed(&json!({
                "jsonrpc": "2.0",
                "id": 2,
                "method": "textDocument/rename",
                "params": {
                    "textDocument": { "uri": rust_uri },
                    "position": { "line": 10, "character": 12 },
                    "newName": "deleteUser"
                }
            })),
        ]
        .join("");
        let mut output = Vec::new();

        run(input.as_bytes(), &mut output).expect("run server");
        let output = String::from_utf8(output).expect("utf8 output");

        assert!(output.contains("\"id\":2"));
        assert!(output.contains("\"error\""));
        assert!(output.contains("conflicts with an existing API symbol"));
    }

    #[test]
    fn publishes_unused_endpoint_diagnostics_from_usage_index() {
        let root = test_root("unused-diagnostics");
        fs::create_dir_all(root.join("target/api-contract/graph")).expect("create usage dir");
        fs::create_dir_all(root.join("src")).expect("create src");
        fs::write(root.join("Cargo.toml"), "[workspace]\n").expect("write manifest");
        write_mock_backend_config(&root);
        let rust_uri = path_to_file_uri(&root.join("src/lib.rs"));
        fs::write(
            root.join("target/api-contract/rust-ts-symbols.json"),
            json!({
                "schemaVersion": 1,
                "contractHash": "test-contract",
                "symbols": [
                    endpoint_symbol(
                        "endpoint:unused",
                        &rust_uri,
                        10,
                        "GET",
                        "/unused",
                        ["api", "unused"],
                        false,
                    ),
                    endpoint_symbol(
                        "endpoint:used",
                        &rust_uri,
                        20,
                        "GET",
                        "/used",
                        ["api", "used"],
                        false,
                    ),
                    endpoint_symbol(
                        "endpoint:allowed",
                        &rust_uri,
                        30,
                        "POST",
                        "/admin/reindex",
                        ["admin", "reindex"],
                        true,
                    )
                ]
            })
            .to_string(),
        )
        .expect("write symbol graph");
        fs::write(
            root.join("target/api-contract/graph/effect-usage-index.json"),
            json!({
                "schema_version": 2,
                "contract_hash": "test-contract",
                "package_name": "@workspace/server-api",
                "endpoints": [
                    { "endpoint_id": "endpoint:unused", "accessor_path": ["api", "unused"], "strong": 0 },
                    { "endpoint_id": "endpoint:used", "accessor_path": ["api", "used"], "strong": 1 },
                    { "endpoint_id": "endpoint:allowed", "accessor_path": ["admin", "reindex"], "strong": 0 }
                ],
                "usages": []
            })
            .to_string(),
        )
        .expect("write usage index");

        let input = [
            framed(&json!({
                "jsonrpc": "2.0",
                "id": 1,
                "method": "initialize",
                "params": {
                    "processId": null,
                    "rootUri": path_to_file_uri(&root),
                    "capabilities": {}
                }
            })),
            framed(&json!({
                "jsonrpc": "2.0",
                "method": "initialized",
                "params": {}
            })),
            framed(&json!({
                "jsonrpc": "2.0",
                "id": 2,
                "method": "shutdown"
            })),
            framed(&json!({
                "jsonrpc": "2.0",
                "method": "exit"
            })),
        ]
        .join("");
        let mut output = Vec::new();

        run(input.as_bytes(), &mut output).expect("run server");
        let output = String::from_utf8(output).expect("utf8 output");

        assert!(output.contains("\"method\":\"textDocument/publishDiagnostics\""));
        assert!(output.contains("api::unused_endpoint"));
        assert!(output.contains("GET /unused"));
        assert!(output.contains("api.unused"));
        assert!(!output.contains("GET /used"));
        assert!(!output.contains("/admin/reindex"));
        assert!(output.contains("\"severity\":2"));
    }

    #[test]
    fn unused_endpoint_diagnostics_support_deny_and_off_modes() {
        let deny_root = test_root("unused-deny");
        write_unused_endpoint_workspace(&deny_root, "deny");
        let deny_output = run_initialized_workspace(&deny_root);

        assert!(deny_output.contains("\"severity\":1"));
        assert!(deny_output.contains("GET /unused"));

        let off_root = test_root("unused-off");
        write_unused_endpoint_workspace(&off_root, "off");
        let off_output = run_initialized_workspace(&off_root);

        assert!(!off_output.contains("api::unused_endpoint"));
        assert!(!off_output.contains("GET /unused"));
    }

    #[test]
    fn generated_symbol_graph_drives_definition_and_hover_transcript() {
        let root = test_root("generated-transcript");
        fs::create_dir_all(root.join("target/api-contract")).expect("create graph dir");
        fs::create_dir_all(root.join("crates/server/src")).expect("create rust dir");
        fs::write(root.join("Cargo.toml"), "[workspace]\n").expect("write manifest");
        write_mock_backend_config(&root);
        let contract = api_test_fixtures::basic_contract();
        let package = api_gen_effect_v4::render_generated_package(&contract, &root.join("target"));
        let symbol_graph = api_gen_effect_v4::render_symbol_graph(&contract, &package);
        let graph_value: Value = serde_json::from_str(&symbol_graph).expect("parse symbol graph");
        let endpoint_position = graph_value["symbols"]
            .as_array()
            .expect("symbols array")
            .iter()
            .find(|symbol| symbol["id"] == "fixture:endpoint:getUser")
            .and_then(|symbol| symbol["typescript"].as_array())
            .and_then(|locations| locations.first())
            .map(|location| location["range"]["start"].clone())
            .expect("endpoint generated location");
        fs::write(
            root.join("target/api-contract/rust-ts-symbols.json"),
            symbol_graph,
        )
        .expect("write symbol graph");

        let ts_uri = path_to_file_uri(&package.package_dir.join("endpoints.ts"));
        let input = [
            framed(&json!({
                "jsonrpc": "2.0",
                "id": 1,
                "method": "initialize",
                "params": {
                    "processId": null,
                    "rootUri": path_to_file_uri(&root),
                    "capabilities": {}
                }
            })),
            framed(&json!({
                "jsonrpc": "2.0",
                "id": 2,
                "method": "textDocument/definition",
                "params": {
                    "textDocument": { "uri": ts_uri },
                    "position": endpoint_position.clone()
                }
            })),
            framed(&json!({
                "jsonrpc": "2.0",
                "id": 3,
                "method": "textDocument/hover",
                "params": {
                    "textDocument": { "uri": ts_uri },
                    "position": endpoint_position
                }
            })),
            framed(&json!({
                "jsonrpc": "2.0",
                "id": 4,
                "method": "shutdown"
            })),
            framed(&json!({
                "jsonrpc": "2.0",
                "method": "exit"
            })),
        ]
        .join("");
        let mut output = Vec::new();

        run(input.as_bytes(), &mut output).expect("run server");
        let output = String::from_utf8(output).expect("utf8 output");

        assert!(output.contains("\"id\":2"));
        assert!(output.contains("crates/server/src/users.rs"));
        assert!(output.contains("\"line\":17"));
        assert!(output.contains("\"id\":3"));
        assert!(output.contains("**API endpoint** `fixture:endpoint:getUser`"));
        assert!(output.contains("Route: `GET /users/{id}`"));
        assert!(output.contains("TypeScript: `users.getUser`"));
    }

    fn framed(value: &Value) -> String {
        let body = serde_json::to_string(value).expect("serialize message");
        format!("Content-Length: {}\r\n\r\n{body}", body.len())
    }

    fn assert_output_order(output: &str, first: &str, second: &str) {
        let first_index = output.find(first).expect("first output marker");
        let second_index = output.find(second).expect("second output marker");
        assert!(
            first_index < second_index,
            "expected `{first}` before `{second}` in {output}"
        );
    }

    fn initialize_output(root: &Path) -> String {
        let input = framed(&json!({
            "jsonrpc": "2.0",
            "id": "init",
            "method": "initialize",
            "params": {
                "processId": null,
                "rootUri": path_to_file_uri(root),
                "capabilities": {}
            }
        }));
        let mut output = Vec::new();

        run(input.as_bytes(), &mut output).expect("run server");
        String::from_utf8(output).expect("utf8 output")
    }

    fn test_root(name: &str) -> PathBuf {
        let id = TEST_ID.fetch_add(1, Ordering::Relaxed);
        let path = env::temp_dir().join(format!("api-ls-{name}-{}-{id}", std::process::id()));
        let _ = fs::remove_dir_all(&path);
        path
    }

    fn endpoint_symbol(
        id: &str,
        rust_uri: &str,
        line: u32,
        method: &str,
        route: &str,
        ts_path: [&str; 2],
        allow_unused: bool,
    ) -> Value {
        json!({
            "id": id,
            "kind": "endpoint",
            "rust": {
                "uri": rust_uri,
                "range": {
                    "start": { "line": line, "character": 9 },
                    "end": { "line": line, "character": 17 }
                }
            },
            "typescript": [],
            "metadata": {
                "method": method,
                "route": route,
                "tsPath": ts_path,
                "allowUnused": allow_unused
            }
        })
    }

    fn write_unused_endpoint_workspace(root: &Path, lint_level: &str) {
        fs::create_dir_all(root.join("target/api-contract/graph")).expect("create usage dir");
        fs::create_dir_all(root.join("src")).expect("create src");
        fs::write(root.join("Cargo.toml"), "[workspace]\n").expect("write manifest");
        write_mock_backend_config_with_lint(root, Some(lint_level));
        let rust_uri = path_to_file_uri(&root.join("src/lib.rs"));
        fs::write(
            root.join("target/api-contract/rust-ts-symbols.json"),
            json!({
                "schemaVersion": 1,
                "contractHash": "test-contract",
                "symbols": [
                    endpoint_symbol(
                        "endpoint:unused",
                        &rust_uri,
                        10,
                        "GET",
                        "/unused",
                        ["api", "unused"],
                        false,
                    )
                ]
            })
            .to_string(),
        )
        .expect("write symbol graph");
        fs::write(
            root.join("target/api-contract/graph/effect-usage-index.json"),
            json!({
                "schema_version": 2,
                "contract_hash": "test-contract",
                "package_name": "@workspace/server-api",
                "endpoints": [
                    { "endpoint_id": "endpoint:unused", "accessor_path": ["api", "unused"], "strong": 0 }
                ],
                "usages": []
            })
            .to_string(),
        )
        .expect("write usage index");
    }

    fn write_mock_backend_config(root: &Path) {
        write_mock_backend_config_with_lint(root, None);
    }

    fn write_backend_config(
        root: &Path,
        rust_backend_source: &str,
        typescript_backend_source: &str,
        log_file: Option<&str>,
    ) {
        let rust_backend = root.join("rust_backend.py");
        let typescript_backend = root.join("typescript_backend.py");
        fs::write(&rust_backend, rust_backend_source).expect("write rust backend");
        fs::write(&typescript_backend, typescript_backend_source)
            .expect("write typescript backend");
        let mut config = json!({
            "rustAnalyzer": {
                "command": "python3",
                "args": [rust_backend.to_string_lossy()]
            },
            "typescript": {
                "command": "python3",
                "args": [typescript_backend.to_string_lossy()]
            }
        });
        if let Some(log_file) = log_file {
            config["logFile"] = json!(log_file);
        }
        fs::write(root.join(".api-ls.json"), config.to_string()).expect("write config");
    }

    fn write_mock_backend_config_with_lint(root: &Path, lint: Option<&str>) {
        let backend = root.join("mock_backend.py");
        fs::write(&backend, MOCK_BACKEND).expect("write mock backend");
        let mut config = json!({
            "rustAnalyzer": {
                "command": "python3",
                "args": [backend.to_string_lossy()]
            },
            "typescript": {
                "command": "python3",
                "args": [backend.to_string_lossy()]
            }
        });
        if let Some(lint) = lint {
            config["unusedEndpointLints"] = json!(lint);
        }
        fs::write(root.join(".api-ls.json"), config.to_string()).expect("write config");
    }

    fn run_initialized_workspace(root: &Path) -> String {
        let input = [
            framed(&json!({
                "jsonrpc": "2.0",
                "id": 1,
                "method": "initialize",
                "params": {
                    "processId": null,
                    "rootUri": path_to_file_uri(root),
                    "capabilities": {}
                }
            })),
            framed(&json!({
                "jsonrpc": "2.0",
                "method": "initialized",
                "params": {}
            })),
            framed(&json!({
                "jsonrpc": "2.0",
                "id": 2,
                "method": "shutdown"
            })),
            framed(&json!({
                "jsonrpc": "2.0",
                "method": "exit"
            })),
        ]
        .join("");
        let mut output = Vec::new();

        run(input.as_bytes(), &mut output).expect("run server");
        String::from_utf8(output).expect("utf8 output")
    }

    const MOCK_BACKEND: &str = r#"
import json
import sys

def read_message():
    content_length = None
    while True:
        line = sys.stdin.buffer.readline()
        if not line:
            return None
        line = line.decode("ascii").strip()
        if line == "":
            break
        name, value = line.split(":", 1)
        if name.lower() == "content-length":
            content_length = int(value.strip())
    return json.loads(sys.stdin.buffer.read(content_length))

def write_message(message):
    body = json.dumps(message, separators=(",", ":")).encode("utf-8")
    sys.stdout.buffer.write(f"Content-Length: {len(body)}\r\n\r\n".encode("ascii"))
    sys.stdout.buffer.write(body)
    sys.stdout.buffer.flush()

while True:
    message = read_message()
    if message is None:
        break
    method = message.get("method")
    if method == "initialize":
        write_message({"jsonrpc":"2.0","id":message["id"],"result":{"capabilities":{}}})
    elif method == "textDocument/definition":
        write_message({
            "jsonrpc":"2.0",
            "method":"textDocument/publishDiagnostics",
            "params":{
                "uri":message["params"]["textDocument"]["uri"],
                "diagnostics":[{
                    "range":{"start":{"line":0,"character":0},"end":{"line":0,"character":1}},
                    "severity":2,
                    "source":"mock",
                    "message":"mock diagnostic"
                }]
            }
        })
        write_message({
            "jsonrpc":"2.0",
            "id":message["id"],
            "result":[{
                "uri":"file:///mock-definition.rs",
                "range":{"start":{"line":1,"character":0},"end":{"line":1,"character":4}}
            }]
        })
    elif method == "textDocument/references":
        write_message({
            "jsonrpc":"2.0",
            "id":message["id"],
            "result":[{
                "uri":"file:///mock-reference.rs",
                "range":{"start":{"line":3,"character":0},"end":{"line":3,"character":4}}
            }]
        })
    elif method == "shutdown":
        write_message({"jsonrpc":"2.0","id":message["id"],"result":None})
    elif method == "exit":
        break
"#;

    const FAST_BACKEND: &str = r#"
import json
import sys

def read_message():
    content_length = None
    while True:
        line = sys.stdin.buffer.readline()
        if not line:
            return None
        line = line.decode("ascii").strip()
        if line == "":
            break
        name, value = line.split(":", 1)
        if name.lower() == "content-length":
            content_length = int(value.strip())
    return json.loads(sys.stdin.buffer.read(content_length))

def write_message(message):
    body = json.dumps(message, separators=(",", ":")).encode("utf-8")
    sys.stdout.buffer.write(f"Content-Length: {len(body)}\r\n\r\n".encode("ascii"))
    sys.stdout.buffer.write(body)
    sys.stdout.buffer.flush()

while True:
    message = read_message()
    if message is None:
        break
    method = message.get("method")
    if method == "initialize":
        write_message({"jsonrpc":"2.0","id":message["id"],"result":{"capabilities":{}}})
    elif method == "textDocument/definition":
        write_message({
            "jsonrpc":"2.0",
            "id":message["id"],
            "result":[{
                "uri":"file:///fast-definition.ts",
                "range":{"start":{"line":1,"character":0},"end":{"line":1,"character":4}}
            }]
        })
    elif method == "shutdown":
        write_message({"jsonrpc":"2.0","id":message["id"],"result":None})
    elif method == "exit":
        break
"#;

    const SLOW_RUST_BACKEND: &str = r#"
import json
import sys
import time

def read_message():
    content_length = None
    while True:
        line = sys.stdin.buffer.readline()
        if not line:
            return None
        line = line.decode("ascii").strip()
        if line == "":
            break
        name, value = line.split(":", 1)
        if name.lower() == "content-length":
            content_length = int(value.strip())
    return json.loads(sys.stdin.buffer.read(content_length))

def write_message(message):
    body = json.dumps(message, separators=(",", ":")).encode("utf-8")
    sys.stdout.buffer.write(f"Content-Length: {len(body)}\r\n\r\n".encode("ascii"))
    sys.stdout.buffer.write(body)
    sys.stdout.buffer.flush()

while True:
    message = read_message()
    if message is None:
        break
    method = message.get("method")
    if method == "initialize":
        write_message({"jsonrpc":"2.0","id":message["id"],"result":{"capabilities":{}}})
    elif method == "textDocument/definition":
        time.sleep(0.25)
        write_message({
            "jsonrpc":"2.0",
            "id":message["id"],
            "result":[{
                "uri":"file:///slow-definition.rs",
                "range":{"start":{"line":1,"character":0},"end":{"line":1,"character":4}}
            }]
        })
    elif method == "shutdown":
        write_message({"jsonrpc":"2.0","id":message["id"],"result":None})
    elif method == "exit":
        break
"#;

    const CANCELLABLE_BACKEND: &str = r#"
import json
import sys
import threading
import time

write_lock = threading.Lock()

def read_message():
    content_length = None
    while True:
        line = sys.stdin.buffer.readline()
        if not line:
            return None
        line = line.decode("ascii").strip()
        if line == "":
            break
        name, value = line.split(":", 1)
        if name.lower() == "content-length":
            content_length = int(value.strip())
    return json.loads(sys.stdin.buffer.read(content_length))

def write_message(message):
    body = json.dumps(message, separators=(",", ":")).encode("utf-8")
    with write_lock:
        sys.stdout.buffer.write(f"Content-Length: {len(body)}\r\n\r\n".encode("ascii"))
        sys.stdout.buffer.write(body)
        sys.stdout.buffer.flush()

def delayed_definition(request_id):
    time.sleep(0.2)
    write_message({
        "jsonrpc":"2.0",
        "id":request_id,
        "result":[{
            "uri":"file:///cancel-definition.rs",
            "range":{"start":{"line":1,"character":0},"end":{"line":1,"character":4}}
        }]
    })

while True:
    message = read_message()
    if message is None:
        break
    method = message.get("method")
    if method == "initialize":
        write_message({"jsonrpc":"2.0","id":message["id"],"result":{"capabilities":{}}})
    elif method == "textDocument/definition":
        threading.Thread(target=delayed_definition, args=(message["id"],), daemon=True).start()
    elif method == "$/cancelRequest":
        write_message({
            "jsonrpc":"2.0",
            "method":"window/logMessage",
            "params":{"type":3,"message":f"cancelled:{message['params']['id']}"}
        })
    elif method == "shutdown":
        write_message({"jsonrpc":"2.0","id":message["id"],"result":None})
    elif method == "exit":
        break
"#;

    const STDERR_BACKEND: &str = r#"
import json
import sys

def read_message():
    content_length = None
    while True:
        line = sys.stdin.buffer.readline()
        if not line:
            return None
        line = line.decode("ascii").strip()
        if line == "":
            break
        name, value = line.split(":", 1)
        if name.lower() == "content-length":
            content_length = int(value.strip())
    return json.loads(sys.stdin.buffer.read(content_length))

def write_message(message):
    body = json.dumps(message, separators=(",", ":")).encode("utf-8")
    sys.stdout.buffer.write(f"Content-Length: {len(body)}\r\n\r\n".encode("ascii"))
    sys.stdout.buffer.write(body)
    sys.stdout.buffer.flush()

while True:
    message = read_message()
    if message is None:
        break
    method = message.get("method")
    if method == "initialize":
        sys.stderr.write("backend stderr hello\n")
        sys.stderr.flush()
        write_message({"jsonrpc":"2.0","id":message["id"],"result":{"capabilities":{}}})
    elif method == "shutdown":
        write_message({"jsonrpc":"2.0","id":message["id"],"result":None})
    elif method == "exit":
        break
"#;
}

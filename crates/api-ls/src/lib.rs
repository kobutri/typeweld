//! Language server gateway for Rust API contracts and generated TypeScript.

use std::{
    collections::BTreeSet,
    env, fs,
    io::{self, BufRead, BufReader, Read, Write},
    path::{Path, PathBuf},
    process::{Child, ChildStdin, ChildStdout, Command, Stdio},
};

use lsp_types::{
    HoverProviderCapability, InitializeParams, InitializeResult, OneOf, PositionEncodingKind,
    RenameOptions, ServerCapabilities, ServerInfo, TextDocumentSyncCapability,
    TextDocumentSyncKind, WorkDoneProgressOptions,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

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
#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize)]
#[serde(default, rename_all = "camelCase")]
pub struct BackendCommand {
    pub command: String,
    pub args: Vec<String>,
}

impl Default for BackendCommand {
    fn default() -> Self {
        Self {
            command: String::new(),
            args: Vec::new(),
        }
    }
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
    let stdin = io::stdin();
    let stdout = io::stdout();
    run(stdin.lock(), stdout.lock())
}

/// Runs the JSON-RPC language server loop over arbitrary streams.
pub fn run(input: impl Read, output: impl Write) -> Result<(), String> {
    let mut server = LspServer::new(output);
    server.run(input)
}

#[derive(Debug)]
struct LspMessage {
    id: Option<Value>,
    method: Option<String>,
    params: Value,
    raw: Value,
}

struct LspServer<W> {
    writer: W,
    workspace: Option<WorkspaceConfig>,
    rust_analyzer: Option<BackendProcess>,
    typescript: Option<BackendProcess>,
    shutdown_requested: bool,
}

impl<W: Write> LspServer<W> {
    fn new(writer: W) -> Self {
        Self {
            writer,
            workspace: None,
            rust_analyzer: None,
            typescript: None,
            shutdown_requested: false,
        }
    }

    fn run(&mut self, input: impl Read) -> Result<(), String> {
        let mut reader = BufReader::new(input);

        while let Some(message) = read_message(&mut reader)? {
            if self.handle_message(message)? {
                break;
            }
        }

        Ok(())
    }

    fn handle_message(&mut self, message: LspMessage) -> Result<bool, String> {
        let Some(method) = message.method.as_deref() else {
            return Ok(false);
        };

        match method {
            "initialize" => {
                let Some(id) = message.id else {
                    return Ok(false);
                };
                match initialize_workspace(&message.params) {
                    Ok(workspace) => {
                        let rust_backend_result =
                            BackendProcess::start("rust-analyzer", &workspace.config.rust_analyzer)
                                .and_then(|mut backend| {
                                    backend.initialize(message.params.clone())?;
                                    Ok(backend)
                                });
                        let typescript_params =
                            typescript_initialize_params(&message.params, &workspace);
                        let typescript_backend_result =
                            BackendProcess::start("typescript", &workspace.config.typescript)
                                .and_then(|mut backend| {
                                    backend.initialize(typescript_params)?;
                                    Ok(backend)
                                });
                        self.workspace = Some(workspace);
                        match rust_backend_result {
                            Ok(backend) => self.rust_analyzer = Some(backend),
                            Err(error) => self.write_notification(
                                "window/logMessage",
                                json!({
                                    "type": 3,
                                    "message": format!("api-ls rust-analyzer backend unavailable: {error}"),
                                }),
                            )?,
                        }
                        match typescript_backend_result {
                            Ok(backend) => self.typescript = Some(backend),
                            Err(error) => self.write_notification(
                                "window/logMessage",
                                json!({
                                    "type": 3,
                                    "message": format!("api-ls TypeScript backend unavailable: {error}"),
                                }),
                            )?,
                        }
                        self.write_response(id, initialize_result())?;
                    }
                    Err(error) => self.write_error(id, -32_602, error)?,
                }
            }
            "initialized" => {
                self.forward_notification_to_rust_analyzer(method, message.params)?;
                self.forward_notification_to_typescript(method, Value::Null)?;
                self.publish_unused_endpoint_diagnostics()?;
            }
            "shutdown" => {
                if let Some(id) = message.id {
                    self.shutdown_requested = true;
                    if let Some(backend) = &mut self.rust_analyzer {
                        backend.shutdown();
                    }
                    if let Some(backend) = &mut self.typescript {
                        backend.shutdown();
                    }
                    self.write_response(id, Value::Null)?;
                }
            }
            "exit" => {
                if let Some(backend) = &mut self.rust_analyzer {
                    backend.exit();
                }
                if let Some(backend) = &mut self.typescript {
                    backend.exit();
                }
                return Ok(self.shutdown_requested);
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
                    } else if is_rust_message(method, &message.params) {
                        self.forward_request_to_rust_analyzer(method, message.params, id)?;
                    } else if self.is_typescript_message(method, &message.params) {
                        self.forward_request_to_typescript(method, message.params, id)?;
                    } else {
                        self.write_response(id, Value::Null)?;
                    }
                }
            }
            "textDocument/references" => {
                if let Some(id) = message.id {
                    if let Some(locations) = self.cross_language_references(&message.params)? {
                        self.write_response(id, locations)?;
                    } else if is_rust_message(method, &message.params) {
                        self.forward_request_to_rust_analyzer(method, message.params, id)?;
                    } else if self.is_typescript_message(method, &message.params) {
                        self.forward_request_to_typescript(method, message.params, id)?;
                    } else {
                        self.write_response(id, json!([]))?;
                    }
                }
            }
            "textDocument/hover" => {
                if let Some(id) = message.id {
                    if let Some(hover) = self.cross_language_hover(&message.params)? {
                        self.write_response(id, hover)?;
                    } else if is_rust_message(method, &message.params) {
                        self.forward_request_to_rust_analyzer(method, message.params, id)?;
                    } else if self.is_typescript_message(method, &message.params) {
                        self.forward_request_to_typescript(method, message.params, id)?;
                    } else {
                        self.write_response(id, Value::Null)?;
                    }
                }
            }
            "textDocument/prepareRename" => {
                if let Some(id) = message.id {
                    if let Some(rename) = self.cross_language_prepare_rename(&message.params)? {
                        self.write_response(id, rename)?;
                    } else if is_rust_message(method, &message.params) {
                        self.forward_request_to_rust_analyzer(method, message.params, id)?;
                    } else if self.is_typescript_message(method, &message.params) {
                        self.forward_request_to_typescript(method, message.params, id)?;
                    } else {
                        self.write_response(id, Value::Null)?;
                    }
                }
            }
            "textDocument/rename" => {
                if let Some(id) = message.id {
                    match self.cross_language_rename(&message.params) {
                        Ok(Some(edit)) => self.write_response(id, edit)?,
                        Ok(None) if is_rust_message(method, &message.params) => {
                            self.forward_request_to_rust_analyzer(method, message.params, id)?;
                        }
                        Ok(None) if self.is_typescript_message(method, &message.params) => {
                            self.forward_request_to_typescript(method, message.params, id)?;
                        }
                        Ok(None) => self.write_response(id, json!({ "changes": {} }))?,
                        Err(error) => self.write_error(id, -32_602, error)?,
                    }
                }
            }
            _ => {
                if is_rust_message(method, &message.params) {
                    match message.id {
                        Some(id) => {
                            self.forward_request_to_rust_analyzer(method, message.params, id)?;
                        }
                        None => {
                            self.forward_notification_to_rust_analyzer(method, message.params)?;
                        }
                    }
                } else if self.is_typescript_message(method, &message.params) {
                    match message.id {
                        Some(id) => {
                            self.forward_request_to_typescript(method, message.params, id)?;
                        }
                        None => {
                            self.forward_notification_to_typescript(method, message.params)?;
                        }
                    }
                } else if let Some(id) = message.id {
                    self.write_error(
                        id,
                        -32_601,
                        format!("api-ls has no handler for `{method}` yet"),
                    )?;
                }
            }
        }

        Ok(false)
    }

    fn write_response(&mut self, id: Value, result: Value) -> Result<(), String> {
        write_framed(
            &mut self.writer,
            &json!({
                "jsonrpc": "2.0",
                "id": id,
                "result": result,
            }),
        )
    }

    fn write_notification(&mut self, method: &str, params: Value) -> Result<(), String> {
        write_framed(
            &mut self.writer,
            &json!({
                "jsonrpc": "2.0",
                "method": method,
                "params": params,
            }),
        )
    }

    fn write_error(&mut self, id: Value, code: i64, message: String) -> Result<(), String> {
        write_framed(
            &mut self.writer,
            &json!({
                "jsonrpc": "2.0",
                "id": id,
                "error": {
                    "code": code,
                    "message": message,
                },
            }),
        )
    }

    fn forward_request_to_rust_analyzer(
        &mut self,
        method: &str,
        params: Value,
        id: Value,
    ) -> Result<(), String> {
        match &mut self.rust_analyzer {
            Some(backend) => backend.forward_request(method, params, id, &mut self.writer),
            None => self.write_error(
                id,
                -32_003,
                "rust-analyzer backend is not available".to_owned(),
            ),
        }
    }

    fn forward_notification_to_rust_analyzer(
        &mut self,
        method: &str,
        params: Value,
    ) -> Result<(), String> {
        if let Some(backend) = &mut self.rust_analyzer {
            backend.forward_notification(method, params)?;
        }
        Ok(())
    }

    fn forward_request_to_typescript(
        &mut self,
        method: &str,
        params: Value,
        id: Value,
    ) -> Result<(), String> {
        match &mut self.typescript {
            Some(backend) => backend.forward_request(method, params, id, &mut self.writer),
            None => self.write_error(
                id,
                -32_003,
                "TypeScript backend is not available".to_owned(),
            ),
        }
    }

    fn forward_notification_to_typescript(
        &mut self,
        method: &str,
        params: Value,
    ) -> Result<(), String> {
        if let Some(backend) = &mut self.typescript {
            backend.forward_notification(method, params)?;
        }
        Ok(())
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

    fn cross_language_references(&mut self, params: &Value) -> Result<Option<Value>, String> {
        let Some(workspace) = self.workspace.clone() else {
            return Ok(None);
        };
        let Some(query) = TextPositionQuery::from_lsp_params(params) else {
            return Ok(None);
        };
        let Some(graph) = SymbolGraph::load(&workspace.symbol_graph)? else {
            return Ok(None);
        };
        let Some(mut locations) = graph.references(&query, &workspace) else {
            return Ok(None);
        };

        let backend_locations = if is_rust_message("textDocument/references", params) {
            self.backend_reference_locations(BackendKind::Rust, params.clone())?
        } else if self.is_typescript_message("textDocument/references", params) {
            self.backend_reference_locations(BackendKind::TypeScript, params.clone())?
        } else {
            Vec::new()
        };
        locations.extend(backend_locations);
        Ok(Some(dedupe_locations(locations, &workspace)))
    }

    fn backend_reference_locations(
        &mut self,
        backend: BackendKind,
        params: Value,
    ) -> Result<Vec<Value>, String> {
        let response = match backend {
            BackendKind::Rust => self
                .rust_analyzer
                .as_mut()
                .map(|backend| backend.request_raw("textDocument/references", params)),
            BackendKind::TypeScript => self
                .typescript
                .as_mut()
                .map(|backend| backend.request_raw("textDocument/references", params)),
        };
        let Some(response) = response else {
            return Ok(Vec::new());
        };
        let response = response?;
        Ok(response
            .get("result")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default())
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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum BackendKind {
    Rust,
    TypeScript,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SymbolGraph {
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

struct BackendProcess {
    name: String,
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
    next_id: u64,
}

impl BackendProcess {
    fn start(name: &str, command: &BackendCommand) -> Result<Self, String> {
        if command.command.is_empty() {
            return Err("backend command is empty".to_owned());
        }

        let mut child = Command::new(&command.command)
            .args(&command.args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
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

        Ok(Self {
            name: name.to_owned(),
            child,
            stdin,
            stdout: BufReader::new(stdout),
            next_id: 1,
        })
    }

    fn initialize(&mut self, params: Value) -> Result<(), String> {
        let id = self.allocate_id();
        self.write_request(id, "initialize", params)?;
        self.read_until_response(id).map(|_| ())
    }

    fn forward_request(
        &mut self,
        method: &str,
        params: Value,
        client_id: Value,
        writer: &mut impl Write,
    ) -> Result<(), String> {
        let (response, notifications) = self.request_raw_with_notifications(method, params)?;
        for notification in notifications {
            write_framed(writer, &notification)?;
        }
        let mut raw = response;
        raw["id"] = client_id;
        write_framed(writer, &raw)
    }

    fn request_raw(&mut self, method: &str, params: Value) -> Result<Value, String> {
        self.request_raw_with_notifications(method, params)
            .map(|(response, _)| response)
    }

    fn request_raw_with_notifications(
        &mut self,
        method: &str,
        params: Value,
    ) -> Result<(Value, Vec<Value>), String> {
        let backend_id = self.allocate_id();
        self.write_request(backend_id, method, params)?;
        let mut notifications = Vec::new();

        loop {
            let message = read_message(&mut self.stdout)?
                .ok_or_else(|| format!("{} exited while handling `{method}`", self.name))?;
            if message.id.as_ref() == Some(&json!(backend_id)) {
                return Ok((message.raw, notifications));
            }
            if message.id.is_none() {
                notifications.push(message.raw);
            }
        }
    }

    fn forward_notification(&mut self, method: &str, params: Value) -> Result<(), String> {
        write_framed(
            &mut self.stdin,
            &json!({
                "jsonrpc": "2.0",
                "method": method,
                "params": params,
            }),
        )
    }

    fn shutdown(&mut self) {
        let id = self.allocate_id();
        if self.write_request(id, "shutdown", Value::Null).is_ok() {
            let _ = self.read_until_response(id);
        }
    }

    fn exit(&mut self) {
        let _ = self.forward_notification("exit", Value::Null);
    }

    fn write_request(&mut self, id: u64, method: &str, params: Value) -> Result<(), String> {
        write_framed(
            &mut self.stdin,
            &json!({
                "jsonrpc": "2.0",
                "id": id,
                "method": method,
                "params": params,
            }),
        )
    }

    fn read_until_response(&mut self, id: u64) -> Result<Value, String> {
        loop {
            let message = read_message(&mut self.stdout)?
                .ok_or_else(|| format!("{} exited before responding", self.name))?;
            if message.id.as_ref() == Some(&json!(id)) {
                return Ok(message.raw);
            }
        }
    }

    const fn allocate_id(&mut self) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        id
    }
}

impl Drop for BackendProcess {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
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
    fn proxies_rust_requests_and_diagnostics_through_backend() {
        let root = test_root("rust-proxy");
        fs::create_dir_all(&root).expect("create root");
        fs::write(root.join("Cargo.toml"), "[workspace]\n").expect("write manifest");
        let backend = root.join("mock_backend.py");
        fs::write(&backend, MOCK_BACKEND).expect("write backend");
        fs::write(
            root.join(".api-ls.json"),
            format!(
                "{{\"rustAnalyzer\":{{\"command\":\"python3\",\"args\":[{}]}}}}",
                serde_json::to_string(&backend.to_string_lossy()).expect("serialize path")
            ),
        )
        .expect("write config");
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
        let backend = root.join("mock_backend.py");
        fs::write(&backend, MOCK_BACKEND).expect("write backend");
        fs::write(
            root.join(".api-ls.json"),
            format!(
                "{{\"rustAnalyzer\":{{\"command\":\"\"}},\"typescript\":{{\"command\":\"python3\",\"args\":[{}]}}}}",
                serde_json::to_string(&backend.to_string_lossy()).expect("serialize path")
            ),
        )
        .expect("write config");
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
    fn redirects_generated_typescript_definition_to_rust_source() {
        let root = test_root("cross-definition");
        let generated_cache_dir = root.join("target/api-contract/effect-v4/packages");
        fs::create_dir_all(&generated_cache_dir).expect("create generated cache");
        fs::create_dir_all(root.join("src")).expect("create src");
        fs::write(root.join("Cargo.toml"), "[workspace]\n").expect("write manifest");
        fs::write(
            root.join(".api-ls.json"),
            "{\"rustAnalyzer\":{\"command\":\"\"},\"typescript\":{\"command\":\"\"}}\n",
        )
        .expect("write config");
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
        fs::write(
            root.join(".api-ls.json"),
            "{\"rustAnalyzer\":{\"command\":\"\"},\"typescript\":{\"command\":\"\"}}\n",
        )
        .expect("write config");
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
        fs::write(
            root.join(".api-ls.json"),
            "{\"rustAnalyzer\":{\"command\":\"\"},\"typescript\":{\"command\":\"\"}}\n",
        )
        .expect("write config");
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
        fs::write(
            root.join(".api-ls.json"),
            "{\"rustAnalyzer\":{\"command\":\"\"},\"typescript\":{\"command\":\"\"}}\n",
        )
        .expect("write config");
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
        fs::write(
            root.join(".api-ls.json"),
            "{\"rustAnalyzer\":{\"command\":\"\"},\"typescript\":{\"command\":\"\"}}\n",
        )
        .expect("write config");
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
        fs::write(
            root.join(".api-ls.json"),
            "{\"rustAnalyzer\":{\"command\":\"\"},\"typescript\":{\"command\":\"\"}}\n",
        )
        .expect("write config");
        let rust_uri = path_to_file_uri(&root.join("src/lib.rs"));
        fs::write(
            root.join("target/api-contract/rust-ts-symbols.json"),
            json!({
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
    fn fixture_symbol_graph_drives_definition_and_hover_transcript() {
        let root = test_root("fixture-transcript");
        fs::create_dir_all(root.join("target/api-contract")).expect("create graph dir");
        fs::create_dir_all(root.join("app/src")).expect("create ts dir");
        fs::create_dir_all(root.join("crates/server/src")).expect("create rust dir");
        fs::write(root.join("Cargo.toml"), "[workspace]\n").expect("write manifest");
        fs::write(
            root.join(".api-ls.json"),
            r#"{"rustAnalyzer":{"command":""},"typescript":{"command":""}}"#,
        )
        .expect("write config");
        fs::write(
            root.join("target/api-contract/rust-ts-symbols.json"),
            api_test_fixtures::basic_symbol_graph_json(),
        )
        .expect("write symbol graph");

        let ts_uri = path_to_file_uri(&root.join("app/src/client.ts"));
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
                    "position": { "line": 4, "character": 16 }
                }
            })),
            framed(&json!({
                "jsonrpc": "2.0",
                "id": 3,
                "method": "textDocument/hover",
                "params": {
                    "textDocument": { "uri": ts_uri },
                    "position": { "line": 4, "character": 16 }
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
        assert!(output.contains("\"character\":13"));
        assert!(output.contains("\"id\":3"));
        assert!(output.contains("**API endpoint** `fixture:endpoint:getUser`"));
        assert!(output.contains("Route: `GET /users/{id}`"));
        assert!(output.contains("TypeScript: `users.getUser`"));
    }

    fn framed(value: &Value) -> String {
        let body = serde_json::to_string(value).expect("serialize message");
        format!("Content-Length: {}\r\n\r\n{body}", body.len())
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
        fs::write(
            root.join(".api-ls.json"),
            format!(
                "{{\"rustAnalyzer\":{{\"command\":\"\"}},\"typescript\":{{\"command\":\"\"}},\"unusedEndpointLints\":{}}}\n",
                serde_json::to_string(lint_level).expect("serialize lint level")
            ),
        )
        .expect("write config");
        let rust_uri = path_to_file_uri(&root.join("src/lib.rs"));
        fs::write(
            root.join("target/api-contract/rust-ts-symbols.json"),
            json!({
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
}

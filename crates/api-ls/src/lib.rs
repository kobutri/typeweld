//! Language server gateway for Rust API contracts and generated TypeScript.

use std::{
    env, fs,
    io::{self, BufRead, BufReader, Read, Write},
    path::{Path, PathBuf},
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
    pub generated_cache_dir: PathBuf,
    pub symbol_graph: PathBuf,
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
            generated_cache_dir: PathBuf::from("target/api-contract/effect-v4/packages"),
            symbol_graph: PathBuf::from("target/api-contract/rust-ts-symbols.json"),
            log_file: None,
        }
    }
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
}

struct LspServer<W> {
    writer: W,
    workspace: Option<WorkspaceConfig>,
    shutdown_requested: bool,
}

impl<W: Write> LspServer<W> {
    fn new(writer: W) -> Self {
        Self {
            writer,
            workspace: None,
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
                        self.workspace = Some(workspace);
                        self.write_response(id, initialize_result())?;
                    }
                    Err(error) => self.write_error(id, -32_602, error)?,
                }
            }
            "shutdown" => {
                if let Some(id) = message.id {
                    self.shutdown_requested = true;
                    self.write_response(id, Value::Null)?;
                }
            }
            "exit" => return Ok(self.shutdown_requested),
            _ => {
                if let Some(id) = message.id {
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

    WorkspaceConfig {
        root,
        config_path,
        config,
        generated_cache_dir,
        symbol_graph,
    }
}

fn absolutize_from(root: &Path, path: &Path) -> PathBuf {
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        root.join(path)
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

    fn path_to_file_uri(path: &Path) -> String {
        let path = path.to_string_lossy();
        format!("file://{path}")
    }
}

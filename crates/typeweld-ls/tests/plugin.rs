//! Integration tests for the TypeScript plugin channel.
//!
//! Drives a real tsserver with `@typeweld/typescript-plugin` loaded against a
//! fixture workspace, with typeweld-ls running degraded (no rust-analyzer) on
//! an in-memory connection. Covers the full triangle:
//!
//! - TS→Rust goto-definition served by the plugin from the pushed snapshot,
//! - Rust-initiated field rename covering user property accesses via the
//!   daemon→plugin semantic query,
//! - TypeScript-initiated rename: the plugin filters generated locations,
//!   watches the edit land, and the daemon applies the Rust complement via
//!   `workspace/applyEdit`.
//!
//! Skipped (with a note) when the repo-local npm install or the built plugin
//! is missing.

use std::io::{BufRead, BufReader, Write as _};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use lsp_server::{Connection, Message, Notification, Request, RequestId, Response};
use lsp_types::notification::{Exit, Initialized, Notification as _};
use lsp_types::request::{Request as _, Shutdown};
use lsp_types::{DocumentChanges, OneOf, TextDocumentEdit, WorkspaceEdit};
use typeweld_engine::line_index::LineIndex;

const TIMEOUT: Duration = Duration::from_mins(3);

const TYPEWELD_TOML: &str = r#"
[[package]]
cargo = "server"
ts = "@workspace/test-api"

[app]
src = ["app/src/**/*.ts"]
"#;

const ROOT_MANIFEST: &str = "[workspace]\nmembers = [\"server\"]\n";

const SERVER_MANIFEST: &str = "[package]\nname = \"server\"\n";

const LIB_RS: &str = r#"
use serde::{Deserialize, Serialize};
use typeweld::{api, api_router, Api, ApiError, ApiRouter, Json, Path};

/// A user of the system.
#[derive(Clone, Debug, Deserialize, Serialize, Api)]
#[serde(rename_all = "camelCase")]
pub struct User {
    pub id: i32,
    pub display_name: String,
}

#[derive(Clone, Debug, Deserialize, Serialize, ApiError)]
#[serde(tag = "_tag")]
pub enum UserError {
    /// No user with this id exists.
    #[status(404)]
    UserNotFound { id: i32 },
}

/// Fetch one user by id.
#[api(get, "/users/{id}")]
pub async fn get_user(id: Path<i32>) -> Result<Json<User>, UserError> {
    Err(UserError::UserNotFound { id: *id })
}

#[api_router]
pub fn routes() -> ApiRouter {
    ApiRouter::new().endpoint(get_user)
}
"#;

/// The app uses a renameable property access (`user.displayName`) in user
/// code the contract cannot see.
const MAIN_TS: &str = r#"
import { getUser } from "@workspace/test-api/endpoints/users"
import type { User } from "@workspace/test-api"

export const program = getUser({ id: 1 })
export const show = (user: User) => user.displayName
"#;

/// The app project pulls the generated package in through `paths`, exactly
/// like a scaffolded workspace.
const APP_TSCONFIG: &str = r#"
{
  "compilerOptions": {
    "strict": true,
    "noEmit": true,
    "target": "ES2022",
    "module": "ESNext",
    "moduleResolution": "bundler",
    "skipLibCheck": true,
    "types": [],
    "paths": {
      "@workspace/test-api": [
        "../target/typeweld/packages/@workspace/test-api/index.ts"
      ],
      "@workspace/test-api/*": [
        "../target/typeweld/packages/@workspace/test-api/*"
      ]
    }
  },
  "include": ["src", "../target/typeweld/packages"]
}
"#;

fn fixture() -> tempfile::TempDir {
    let dir = tempfile::TempDir::new().expect("tempdir");
    let root = dir.path();
    let write = |path: &str, contents: &str| {
        let path = root.join(path);
        std::fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");
        std::fs::write(path, contents).expect("write fixture");
    };
    write("typeweld.toml", TYPEWELD_TOML);
    write("Cargo.toml", ROOT_MANIFEST);
    write("server/Cargo.toml", SERVER_MANIFEST);
    write("server/src/lib.rs", LIB_RS);
    write("app/src/main.ts", MAIN_TS);
    write("app/tsconfig.json", APP_TSCONFIG);
    dir
}

fn repo_path(relative: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(relative)
}

/// The pieces a real-tsserver test needs; `None` skips the test.
fn tsserver_setup() -> Option<(PathBuf, PathBuf)> {
    let tsserver = repo_path("npm/node_modules/typescript/lib/tsserver.js");
    // The plugin must be resolvable as <probe>/node_modules/<name>; the npm
    // workspace hoists the file: dependency to the npm root.
    let probe = repo_path("npm");
    let built = probe.join("node_modules/@typeweld/typescript-plugin");
    let dist = repo_path("npm/vscode-extension/typescript-plugin/dist/index.js");
    (tsserver.is_file() && built.exists() && dist.is_file()).then_some((tsserver, probe))
}

// --- typeweld-ls client over an in-memory connection ---

struct LsClient {
    connection: Connection,
    next_id: i32,
    handle: Option<std::thread::JoinHandle<Result<(), String>>>,
}

impl LsClient {
    fn start(root: &Path) -> Self {
        let (server_side, client_side) = Connection::memory();
        let handle = std::thread::spawn(move || typeweld_ls::run_with_connection(server_side));
        let mut client = LsClient {
            connection: client_side,
            next_id: 0,
            handle: Some(handle),
        };
        let params = serde_json::json!({
            "rootUri": uri(root),
            "capabilities": {},
            "initializationOptions": { "rustAnalyzer": "off" },
        });
        client
            .request_value("initialize", params)
            .expect("initialize");
        client.notify(Initialized::METHOD, serde_json::json!({}));
        client
    }

    fn notify(&self, method: &str, params: impl serde::Serialize) {
        let notification = Notification::new(method.to_owned(), params);
        self.connection
            .sender
            .send(Message::Notification(notification))
            .expect("send");
    }

    fn open(&self, path: &Path, language: &str, text: &str) {
        self.notify(
            "textDocument/didOpen",
            serde_json::json!({
                "textDocument": {
                    "uri": uri(path),
                    "languageId": language,
                    "version": 1,
                    "text": text,
                },
            }),
        );
    }

    /// Waits for a server-initiated request with `method`, answering
    /// `workspace/applyEdit` with `applied: true` and everything else with
    /// null. Returns the request params.
    fn wait_for_request(&mut self, method: &str, timeout: Duration) -> Option<serde_json::Value> {
        let deadline = Instant::now() + timeout;
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            let Ok(message) = self.connection.receiver.recv_timeout(remaining) else {
                return None;
            };
            if let Message::Request(request) = message {
                let params = request.params.clone();
                let answer = if request.method == "workspace/applyEdit" {
                    serde_json::json!({ "applied": true })
                } else {
                    serde_json::Value::Null
                };
                let _ = self
                    .connection
                    .sender
                    .send(Message::Response(Response::new_ok(request.id, answer)));
                if request.method == method {
                    return Some(params);
                }
            }
        }
    }

    fn request_value(
        &mut self,
        method: &str,
        params: serde_json::Value,
    ) -> Result<serde_json::Value, String> {
        self.next_id += 1;
        let id = RequestId::from(self.next_id);
        let request = Request::new(id.clone(), method.to_owned(), params);
        self.connection
            .sender
            .send(Message::Request(request))
            .expect("send");
        let deadline = Instant::now() + TIMEOUT;
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            let message = self
                .connection
                .receiver
                .recv_timeout(remaining)
                .expect("response");
            match message {
                Message::Response(response) if response.id == id => {
                    if let Some(error) = response.error {
                        return Err(error.message);
                    }
                    return Ok(response.result.unwrap_or(serde_json::Value::Null));
                }
                Message::Request(request) => {
                    let _ = self
                        .connection
                        .sender
                        .send(Message::Response(Response::new_ok(
                            request.id,
                            serde_json::Value::Null,
                        )));
                }
                _ => {}
            }
        }
    }
}

impl Drop for LsClient {
    fn drop(&mut self) {
        self.next_id += 1;
        let request = Request::new(
            RequestId::from(self.next_id),
            Shutdown::METHOD.to_owned(),
            serde_json::Value::Null,
        );
        let _ = self.connection.sender.send(Message::Request(request));
        let deadline = Instant::now() + Duration::from_secs(10);
        while Instant::now() < deadline {
            match self
                .connection
                .receiver
                .recv_timeout(Duration::from_millis(100))
            {
                Ok(Message::Response(Response { .. })) | Err(_) => break,
                Ok(_) => {}
            }
        }
        let notification = Notification::new(Exit::METHOD.to_owned(), serde_json::Value::Null);
        let _ = self
            .connection
            .sender
            .send(Message::Notification(notification));
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

// --- tsserver protocol driver ---

struct TsServer {
    process: Child,
    stdin: std::process::ChildStdin,
    incoming: crossbeam_channel::Receiver<serde_json::Value>,
    next_seq: i64,
}

impl TsServer {
    fn spawn(tsserver_js: &Path, probe: &Path, cwd: &Path) -> Self {
        let mut process = Command::new("node")
            .arg(tsserver_js)
            .arg("--globalPlugins")
            .arg("@typeweld/typescript-plugin")
            .arg("--pluginProbeLocations")
            .arg(probe)
            .current_dir(cwd)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn tsserver");
        let stdin = process.stdin.take().expect("tsserver stdin");
        let stdout = process.stdout.take().expect("tsserver stdout");
        let (sender, incoming) = crossbeam_channel::unbounded();
        std::thread::spawn(move || {
            let reader = BufReader::new(stdout);
            for line in reader.lines() {
                let Ok(line) = line else { break };
                if !line.starts_with('{') {
                    continue;
                }
                let Ok(message) = serde_json::from_str(&line) else {
                    continue;
                };
                if sender.send(message).is_err() {
                    break;
                }
            }
        });
        Self {
            process,
            stdin,
            incoming,
            next_seq: 0,
        }
    }

    fn send(&mut self, command: &str, arguments: &serde_json::Value) -> i64 {
        self.next_seq += 1;
        let message = serde_json::json!({
            "seq": self.next_seq,
            "type": "request",
            "command": command,
            "arguments": arguments,
        });
        let line = format!("{message}\n");
        self.stdin
            .write_all(line.as_bytes())
            .expect("tsserver write");
        self.next_seq
    }

    /// Sends a request and waits for its response.
    fn request(&mut self, command: &str, arguments: &serde_json::Value) -> serde_json::Value {
        let seq = self.send(command, arguments);
        let deadline = Instant::now() + TIMEOUT;
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            let message = self
                .incoming
                .recv_timeout(remaining)
                .expect("tsserver response");
            if message.get("type").and_then(serde_json::Value::as_str) == Some("response")
                && message
                    .get("request_seq")
                    .and_then(serde_json::Value::as_i64)
                    == Some(seq)
            {
                return message;
            }
        }
    }
}

impl Drop for TsServer {
    fn drop(&mut self) {
        let _ = self.process.kill();
        let _ = self.process.wait();
    }
}

// --- helpers ---

fn uri(path: &Path) -> String {
    format!("file://{}", path.display())
}

fn uri_path(uri: &str) -> PathBuf {
    PathBuf::from(uri.strip_prefix("file://").expect("file uri"))
}

/// One-based tsserver line/offset of the start of `needle` in `text`
/// (fixtures are ASCII).
fn ts_position(text: &str, needle: &str) -> (u64, u64) {
    let offset = text.find(needle).expect("needle");
    let prefix = &text[..offset];
    let line = prefix.matches('\n').count() as u64 + 1;
    let column = offset - prefix.rfind('\n').map_or(0, |index| index + 1);
    (line, column as u64 + 1)
}

/// Zero-based UTF-16 LSP position of the start of `needle`.
fn lsp_position(text: &str, needle: &str) -> lsp_types::Position {
    let offset = u32::try_from(text.find(needle).expect("needle")).expect("offset");
    let (line, character) = LineIndex::new(text).line_col_utf16(offset, text);
    lsp_types::Position::new(line, character)
}

fn workspace_edits(edit: &WorkspaceEdit) -> Vec<(PathBuf, String, String)> {
    let Some(DocumentChanges::Edits(documents)) = &edit.document_changes else {
        panic!("expected document changes");
    };
    let mut collected = Vec::new();
    for TextDocumentEdit {
        text_document,
        edits,
    } in documents
    {
        let path = uri_path(text_document.uri.as_str());
        let text = std::fs::read_to_string(&path).expect("read edited file");
        let index = LineIndex::new(&text);
        for edit in edits {
            let OneOf::Left(edit) = edit else {
                panic!("unexpected annotated edit")
            };
            let start =
                index.offset_utf16(edit.range.start.line, edit.range.start.character, &text);
            let end = index.offset_utf16(edit.range.end.line, edit.range.end.character, &text);
            collected.push((
                path.clone(),
                text[start as usize..end as usize].to_owned(),
                edit.new_text.clone(),
            ));
        }
    }
    collected
}

#[test]
#[allow(clippy::too_many_lines)] // One linear scenario: the full triangle.
fn plugin_serves_cross_language_features() {
    let Some((tsserver_js, probe)) = tsserver_setup() else {
        eprintln!(
            "skipping plugin_serves_cross_language_features: repo npm install or built \
             plugin missing (run `npm ci && npm run build -w typeweld-vscode` in npm/)"
        );
        return;
    };

    let dir = fixture();
    let root = dir.path();
    let mut ls = LsClient::start(root);

    // The server wrote the generated package and the discovery file during
    // its initial refresh.
    let discovery = root.join("target/typeweld/ls.json");
    let package_dir = root.join("target/typeweld/packages/@workspace/test-api");
    let deadline = Instant::now() + Duration::from_secs(10);
    while (!discovery.is_file() || !package_dir.join("schemas.ts").is_file())
        && Instant::now() < deadline
    {
        std::thread::sleep(Duration::from_millis(100));
    }
    assert!(discovery.is_file(), "discovery file missing");
    assert!(package_dir.join("schemas.ts").is_file(), "package missing");

    let mut tsserver = TsServer::spawn(&tsserver_js, &probe, root);
    let main_ts = root.join("app/src/main.ts");
    tsserver.send(
        "open",
        &serde_json::json!({ "file": main_ts.to_string_lossy() }),
    );

    // 1. TS→Rust goto-definition: poll until the plugin has connected and
    //    received the snapshot, then the definition lands in lib.rs.
    let (line, offset) = ts_position(MAIN_TS, "getUser({ id: 1 })");
    let lib_rs = root.join("server/src/lib.rs");
    let deadline = Instant::now() + Duration::from_secs(90);
    loop {
        let response = tsserver.request(
            "definitionAndBoundSpan",
            &serde_json::json!({
                "file": main_ts.to_string_lossy(),
                "line": line,
                "offset": offset,
            }),
        );
        let definition_file = response
            .pointer("/body/definitions/0/file")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default()
            .to_owned();
        if definition_file.ends_with("server/src/lib.rs") {
            let target_line = response
                .pointer("/body/definitions/0/start/line")
                .and_then(serde_json::Value::as_u64)
                .unwrap_or_default();
            let expected = ts_position(LIB_RS, "pub async fn get_user").0;
            assert_eq!(
                target_line, expected,
                "definition must land on the get_user declaration line"
            );
            break;
        }
        assert!(
            Instant::now() < deadline,
            "definition never reached lib.rs; last file: {definition_file}"
        );
        std::thread::sleep(Duration::from_secs(1));
    }

    // 2. Rust-initiated field rename: the daemon asks the plugin for the
    //    user property accesses.
    let rename_position = lsp_position(LIB_RS, "display_name");
    let deadline = Instant::now() + Duration::from_mins(1);
    loop {
        let value = ls
            .request_value(
                "textDocument/rename",
                serde_json::json!({
                    "textDocument": { "uri": uri(&lib_rs) },
                    "position": rename_position,
                    "newName": "nickname",
                }),
            )
            .expect("rename");
        let edit: WorkspaceEdit = serde_json::from_value(value).expect("edit");
        let edits = workspace_edits(&edit);
        let has_rust = edits
            .iter()
            .any(|(path, old, new)| path == &lib_rs && old == "display_name" && new == "nickname");
        assert!(
            has_rust,
            "rust ident edit must always be present: {edits:?}"
        );
        assert!(
            edits
                .iter()
                .all(|(path, _, _)| !path.starts_with(&package_dir)),
            "generated files must never be edited: {edits:?}"
        );
        let has_ts = edits
            .iter()
            .any(|(path, old, new)| path == &main_ts && old == "displayName" && new == "nickname");
        if has_ts {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "property-access edit never appeared: {edits:?}"
        );
        std::thread::sleep(Duration::from_secs(1));
    }

    // 3. Field references include the semantic TypeScript property access
    //    (the same plugin channel as rename, shared single source).
    let references = ls
        .request_value(
            "textDocument/references",
            serde_json::json!({
                "textDocument": { "uri": uri(&lib_rs) },
                "position": rename_position,
                "context": { "includeDeclaration": true },
            }),
        )
        .expect("references");
    let rendered = references.to_string();
    assert!(
        rendered.contains("main.ts"),
        "field references must include the user property access: {rendered}"
    );

    // 4. TypeScript-initiated rename: tsserver's locations exclude generated
    //    files; once the edit lands, the daemon applies the Rust complement.
    let (line, offset) = ts_position(MAIN_TS, "displayName");
    let response = tsserver.request(
        "rename",
        &serde_json::json!({
            "file": main_ts.to_string_lossy(),
            "line": line,
            "offset": offset,
            "findInComments": false,
            "findInStrings": false,
        }),
    );
    let rendered = response.to_string();
    assert!(
        response
            .pointer("/body/info/canRename")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false),
        "rename must be allowed: {rendered}"
    );
    assert!(
        !rendered.contains("schemas.ts"),
        "generated locations must be filtered: {rendered}"
    );

    // Simulate the editor applying the rename in main.ts.
    let edited = MAIN_TS.replace("displayName", "handle");
    let last_line = MAIN_TS.matches('\n').count() as u64 + 1;
    tsserver.send(
        "change",
        &serde_json::json!({
            "file": main_ts.to_string_lossy(),
            "line": 1,
            "offset": 1,
            "endLine": last_line,
            "endOffset": 1,
            "insertString": edited,
        }),
    );
    // Nudge the project so the plugin's poller sees the updated program.
    let _ = tsserver.request(
        "quickinfo",
        &serde_json::json!({
            "file": main_ts.to_string_lossy(),
            "line": 1,
            "offset": 1,
        }),
    );

    // lib.rs is not open in the editor, so the daemon writes the Rust
    // complement straight to disk (no dirty buffers) instead of applyEdit.
    let deadline = Instant::now() + Duration::from_secs(45);
    loop {
        let text = std::fs::read_to_string(root.join("server/src/lib.rs")).expect("lib.rs");
        if text.contains("pub handle: String") && !text.contains("display_name") {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "the Rust complement never landed on disk; lib.rs:\n{text}"
        );
        std::thread::sleep(Duration::from_millis(500));
    }
}

/// A raw daemon-socket session for protocol tests.
struct RawSession {
    stream: std::net::TcpStream,
    reader: BufReader<std::net::TcpStream>,
}

impl RawSession {
    fn connect(root: &Path, kind: &str) -> Self {
        let discovery: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(root.join("target/typeweld/ls.json")).expect("ls.json"),
        )
        .expect("discovery json");
        let port = discovery["port"].as_u64().expect("port");
        let token = discovery["token"].as_str().expect("token");
        let stream =
            std::net::TcpStream::connect(("127.0.0.1", u16::try_from(port).expect("port")))
                .expect("connect");
        stream
            .set_read_timeout(Some(Duration::from_secs(30)))
            .expect("timeout");
        let reader = BufReader::new(stream.try_clone().expect("clone"));
        let mut session = Self { stream, reader };
        session.send(&serde_json::json!({
            "method": "hello",
            "kind": kind,
            "token": token,
            "pid": std::process::id(),
        }));
        session
    }

    fn send(&mut self, message: &serde_json::Value) {
        let line = format!("{message}\n");
        self.stream.write_all(line.as_bytes()).expect("write");
    }

    fn read_message(&mut self) -> serde_json::Value {
        let mut line = String::new();
        self.reader.read_line(&mut line).expect("read line");
        serde_json::from_str(&line).expect("json line")
    }
}

/// TypeScript-initiated renames of files the editor has open round-trip
/// through `workspace/applyEdit` and end with a save request to the editor
/// session, mirroring how the editor saves its own rename edits.
#[test]
fn ts_initiated_rename_saves_open_documents() {
    let dir = fixture();
    let root = dir.path();
    let mut ls = LsClient::start(root);
    let lib_rs = root.join("server/src/lib.rs");
    ls.open(&lib_rs, "rust", LIB_RS);

    let deadline = Instant::now() + Duration::from_secs(10);
    while !root.join("target/typeweld/ls.json").is_file() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(50));
    }

    // The tsplugin session receives the snapshot on hello; pick a field mark
    // to impersonate a TypeScript-initiated rename of that field.
    let mut tsplugin = RawSession::connect(root, "tsplugin");
    let snapshot = tsplugin.read_message();
    assert_eq!(snapshot["method"], "snapshot");
    let symbol = snapshot["marks"]
        .as_array()
        .expect("marks")
        .iter()
        .find(|mark| mark["kind"] == "Field")
        .and_then(|mark| mark["symbol"].as_str())
        .expect("a field mark")
        .to_owned();

    let mut editor = RawSession::connect(root, "editor");
    tsplugin.send(&serde_json::json!({
        "method": "renameLanded",
        "symbol": symbol,
        "newName": "nickname",
    }));

    // lib.rs is open, so the Rust complement arrives as applyEdit…
    let apply = ls
        .wait_for_request("workspace/applyEdit", Duration::from_secs(30))
        .expect("applyEdit for the open document");
    let rendered = apply.to_string();
    assert!(rendered.contains("lib.rs"), "complement edit: {rendered}");
    assert!(rendered.contains("nickname"), "complement edit: {rendered}");

    // …and once the editor confirms it, the editor session is asked to save.
    let save = editor.read_message();
    assert_eq!(save["method"], "saveFiles", "got: {save}");
    assert!(
        save["files"].to_string().contains("lib.rs"),
        "save request must cover the edited file: {save}"
    );
}

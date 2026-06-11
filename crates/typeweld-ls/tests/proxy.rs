//! Integration tests for the rust-analyzer proxy.
//!
//! The proxied test drives a real rust-analyzer behind typeweld-ls over a
//! compilable fixture workspace and is skipped (with a note) when the binary
//! is unavailable. Environment-variable–driven tests serialize through a
//! process-wide lock.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use lsp_server::{Connection, Message, Notification, Request, RequestId, Response};
use lsp_types::notification::{
    DidChangeTextDocument, DidOpenTextDocument, Exit, Initialized, Notification as _,
};
use lsp_types::request::{Request as _, Shutdown};
use lsp_types::{
    DidChangeTextDocumentParams, DidOpenTextDocumentParams, DocumentChanges, OneOf, Position,
    TextDocumentContentChangeEvent, TextDocumentEdit, TextDocumentItem,
    VersionedTextDocumentIdentifier, WorkspaceEdit,
};
use typeweld_engine::line_index::LineIndex;

/// Generous: the first rust-analyzer query waits for workspace indexing.
const TIMEOUT: Duration = Duration::from_mins(3);

static ENV_LOCK: Mutex<()> = Mutex::new(());

/// Sets process environment variables for one test, restoring on drop.
struct EnvGuard {
    _lock: MutexGuard<'static, ()>,
    keys: Vec<&'static str>,
}

impl EnvGuard {
    fn set(vars: &[(&'static str, String)]) -> Self {
        let lock = ENV_LOCK.lock().unwrap_or_else(PoisonError::into_inner);
        for (key, value) in vars {
            std::env::set_var(key, value);
        }
        Self {
            _lock: lock,
            keys: vars.iter().map(|(key, _)| *key).collect(),
        }
    }
}

impl Drop for EnvGuard {
    fn drop(&mut self) {
        for key in &self.keys {
            std::env::remove_var(key);
        }
    }
}

const TYPEWELD_TOML: &str = r#"
[[package]]
cargo = "server"
ts = "@workspace/test-api"

[app]
src = ["app/src/**/*.ts"]
"#;

const ROOT_MANIFEST: &str = "[workspace]\nmembers = [\"server\"]\nresolver = \"2\"\n";

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

/// Plain business logic the contract knows nothing about.
pub fn business_logic(user: User) -> User {
    let copy: User = user.clone();
    copy
}
"#;

const MAIN_TS: &str = r#"
import { getUser, User } from "@workspace/test-api"

export const program = getUser({ id: 1 })
export const userSchema = User
"#;

/// The fixture's `server` crate depends on the real `typeweld` crate by path
/// so `cargo metadata` (and therefore rust-analyzer indexing) succeeds.
fn server_manifest() -> String {
    let typeweld_dir = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("crates dir")
        .join("typeweld");
    let typeweld_dir = typeweld_dir.to_string_lossy().replace('\\', "/");
    format!(
        "[package]\n\
         name = \"server\"\n\
         version = \"0.1.0\"\n\
         edition = \"2021\"\n\
         \n\
         [dependencies]\n\
         serde = {{ version = \"1\", features = [\"derive\"] }}\n\
         typeweld = {{ path = \"{typeweld_dir}\" }}\n"
    )
}

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
    write("server/Cargo.toml", &server_manifest());
    write("server/src/lib.rs", LIB_RS);
    write("app/src/main.ts", MAIN_TS);
    dir
}

/// A working rust-analyzer binary, probed without reading the environment
/// (the other test mutates `TYPEWELD_RUST_ANALYZER` under the env lock).
fn rust_analyzer_binary() -> Option<PathBuf> {
    let works = |binary: &Path| {
        Command::new(binary)
            .arg("--version")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .is_ok_and(|status| status.success())
    };
    let candidate = PathBuf::from("rust-analyzer");
    if works(&candidate) {
        return Some(candidate);
    }
    let output = Command::new("rustup")
        .args(["which", "rust-analyzer"])
        .output()
        .ok()?;
    let candidate = PathBuf::from(String::from_utf8(output.stdout).ok()?.trim());
    (output.status.success() && works(&candidate)).then_some(candidate)
}

struct Client {
    connection: Connection,
    next_id: i32,
    handle: Option<std::thread::JoinHandle<Result<(), String>>>,
}

impl Client {
    fn start(root: &Path) -> Self {
        let (server_side, client_side) = Connection::memory();
        let handle = std::thread::spawn(move || typeweld_ls::run_with_connection(server_side));
        let mut client = Client {
            connection: client_side,
            next_id: 0,
            handle: Some(handle),
        };
        let params = serde_json::json!({
            "rootUri": uri(root),
            "capabilities": {},
        });
        let result = client
            .request_value("initialize", params)
            .expect("initialize");
        assert!(
            result.pointer("/capabilities/renameProvider").is_some(),
            "initialize must advertise rename: {result}"
        );
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

    /// Sends one request and waits for its response; protocol-level errors
    /// come back as `Err` so callers can retry while indexing finishes.
    /// Server-to-client requests arriving in the meantime get a null answer.
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
                    let answer = if request.method == "workspace/configuration" {
                        let items = request
                            .params
                            .get("items")
                            .and_then(serde_json::Value::as_array)
                            .map_or(1, Vec::len);
                        serde_json::Value::Array(vec![serde_json::Value::Null; items])
                    } else {
                        serde_json::Value::Null
                    };
                    let _ = self
                        .connection
                        .sender
                        .send(Message::Response(Response::new_ok(request.id, answer)));
                }
                _ => {}
            }
        }
    }

    fn open(&self, path: &Path, language: &str, text: &str) {
        let params = DidOpenTextDocumentParams {
            text_document: TextDocumentItem {
                uri: uri(path).parse().expect("uri"),
                language_id: language.to_owned(),
                version: 1,
                text: text.to_owned(),
            },
        };
        self.notify(DidOpenTextDocument::METHOD, params);
    }

    fn change(&self, path: &Path, version: i32, text: &str) {
        let params = DidChangeTextDocumentParams {
            text_document: VersionedTextDocumentIdentifier {
                uri: uri(path).parse().expect("uri"),
                version,
            },
            content_changes: vec![TextDocumentContentChangeEvent {
                range: None,
                range_length: None,
                text: text.to_owned(),
            }],
        };
        self.notify(DidChangeTextDocument::METHOD, params);
    }
}

impl Drop for Client {
    /// Best-effort shutdown; never panics so failing tests do not abort.
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

fn uri(path: &Path) -> String {
    let text = path.to_string_lossy().replace('\\', "/");
    if text.starts_with('/') {
        format!("file://{text}")
    } else {
        format!("file:///{text}")
    }
}

fn uri_path(uri: &str) -> PathBuf {
    let mut text = uri.strip_prefix("file://").expect("file uri").to_owned();
    if cfg!(windows) && has_windows_drive_prefix(&text) {
        text.remove(0);
    }
    PathBuf::from(text)
}

fn has_windows_drive_prefix(path: &str) -> bool {
    let bytes = path.as_bytes();
    bytes.len() >= 3 && bytes[0] == b'/' && bytes[1].is_ascii_alphabetic() && bytes[2] == b':'
}

/// Zero-based UTF-16 position of the start of `needle` in `text`. Fixtures
/// are ASCII, so UTF-16 == UTF-8.
fn position_of(text: &str, needle: &str) -> Position {
    let offset = u32::try_from(text.find(needle).expect("needle")).expect("offset");
    let (line, character) = LineIndex::new(text).line_col_utf16(offset, text);
    Position::new(line, character)
}

fn rename_params(path: &Path, position: Position, new_name: &str) -> serde_json::Value {
    serde_json::json!({
        "textDocument": { "uri": uri(path) },
        "position": position,
        "newName": new_name,
    })
}

/// Flattens a workspace edit into `(path, old_text, new_text)` triples,
/// resolving ranges against the on-disk files (UTF-16, ASCII fixtures).
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

/// Counts standalone `name` identifier occurrences in `text` (so `User` does
/// not match inside `UserError`).
fn ident_occurrences(text: &str, name: &str) -> usize {
    let bytes = text.as_bytes();
    let is_ident =
        |byte: Option<&u8>| byte.is_some_and(|&byte| byte.is_ascii_alphanumeric() || byte == b'_');
    let mut count = 0;
    let mut offset = 0;
    while let Some(found) = text[offset..].find(name) {
        let start = offset + found;
        let end = start + name.len();
        if !is_ident(start.checked_sub(1).and_then(|index| bytes.get(index)))
            && !is_ident(bytes.get(end))
        {
            count += 1;
        }
        offset = end;
    }
    count
}

/// Renames through the proxy, retrying while the wrapped rust-analyzer is
/// still indexing (errors or incomplete coverage), like a human retrying.
fn rename_until(
    client: &mut Client,
    path: &Path,
    position: Position,
    new_name: &str,
    complete: impl Fn(&[(PathBuf, String, String)]) -> bool,
) -> Vec<(PathBuf, String, String)> {
    let deadline = Instant::now() + TIMEOUT;
    let mut last = Vec::new();
    while Instant::now() < deadline {
        let result = client.request_value(
            "textDocument/rename",
            rename_params(path, position, new_name),
        );
        if let Ok(value) = result {
            if !value.is_null() {
                let edit: WorkspaceEdit = serde_json::from_value(value).expect("edit");
                last = workspace_edits(&edit);
                if complete(&last) {
                    return last;
                }
            }
        }
        std::thread::sleep(Duration::from_secs(2));
    }
    panic!("rename never completed; last edits: {last:?}");
}

#[test]
fn proxied_rename_merges_rust_analyzer_and_typescript_edits() {
    let Some(binary) = rust_analyzer_binary() else {
        eprintln!(
            "skipping proxied_rename_merges_rust_analyzer_and_typescript_edits: \
             rust-analyzer --version failed"
        );
        return;
    };
    let _env = EnvGuard::set(&[(
        "TYPEWELD_RUST_ANALYZER",
        binary.to_string_lossy().into_owned(),
    )]);

    let dir = fixture();
    let mut client = Client::start(dir.path());
    let lib_rs = dir.path().join("server/src/lib.rs");
    client.open(&lib_rs, "rust", LIB_RS);
    let dirty = format!("{LIB_RS}\nfn broken(\n");
    client.change(&lib_rs, 2, &dirty);

    let position = position_of(&dirty, "pub struct User");
    let position = Position::new(position.line, position.character + 11);
    let expected = ident_occurrences(&dirty, "User");
    let main_ts = dir.path().join("app/src/main.ts");
    rename_until(&mut client, &lib_rs, position, "Account", |edits| {
        let in_lib = edits
            .iter()
            .filter(|(path, old, new)| path == &lib_rs && old == "User" && new == "Account")
            .count();
        let in_ts = edits
            .iter()
            .any(|(path, old, new)| path == &main_ts && old == "User" && new == "Account");
        // rust-analyzer covers every Rust ident (including `business_logic`,
        // which the contract cannot see); our augmentation adds the TS usage.
        in_lib == expected && in_ts
    });

    // The same TypeScript augmentation must happen when the Rust rename is
    // initiated from a usage. rust-analyzer still returns the declaration
    // edit, and typeweld infers the contract symbol from that edit.
    let usage_position = position_of(&dirty, "copy: User");
    let usage_position = Position::new(usage_position.line, usage_position.character + 6);
    rename_until(&mut client, &lib_rs, usage_position, "Customer", |edits| {
        let in_lib = edits
            .iter()
            .filter(|(path, old, new)| path == &lib_rs && old == "User" && new == "Customer")
            .count();
        let in_ts = edits
            .iter()
            .any(|(path, old, new)| path == &main_ts && old == "User" && new == "Customer");
        in_lib == expected && in_ts
    });

    // Hover augmentation: rust-analyzer's hover plus the contract block.
    let hover_position = position_of(LIB_RS, "get_user");
    let hover = client
        .request_value(
            "textDocument/hover",
            serde_json::json!({
                "textDocument": { "uri": uri(&lib_rs) },
                "position": hover_position,
            }),
        )
        .expect("hover");
    let rendered = hover.to_string();
    assert!(
        rendered.contains("GET /users/{id}"),
        "hover must include the contract block: {rendered}"
    );

    // References augmentation: the TS usage joins rust-analyzer's results.
    let references = client
        .request_value(
            "textDocument/references",
            serde_json::json!({
                "textDocument": { "uri": uri(&lib_rs) },
                "position": hover_position,
                "context": { "includeDeclaration": true },
            }),
        )
        .expect("references");
    let rendered = references.to_string();
    assert!(
        rendered.contains("main.ts"),
        "references must include the TS usage: {rendered}"
    );
}

#[test]
fn degrades_when_rust_analyzer_cannot_start() {
    let _env = EnvGuard::set(&[("TYPEWELD_RUST_ANALYZER", "/bin/false".to_owned())]);

    let dir = fixture();
    let mut client = Client::start(dir.path());
    let lib_rs = dir.path().join("server/src/lib.rs");

    let position = position_of(LIB_RS, "pub async fn get_user");
    let position = Position::new(position.line, position.character + 14);
    let value = client
        .request_value(
            "textDocument/rename",
            rename_params(&lib_rs, position, "fetch_user"),
        )
        .expect("rename");
    let edit: WorkspaceEdit = serde_json::from_value(value).expect("edit");
    let edits = workspace_edits(&edit);

    let rust_edits = edits
        .iter()
        .filter(|(path, old, new)| path == &lib_rs && old == "get_user" && new == "fetch_user")
        .count();
    assert_eq!(rust_edits, 2, "ident + mount edits: {edits:?}");
    let main_ts = dir.path().join("app/src/main.ts");
    assert!(
        edits
            .iter()
            .any(|(path, old, new)| path == &main_ts && old == "getUser" && new == "fetchUser"),
        "main.ts edit: {edits:?}"
    );
}

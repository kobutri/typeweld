//! Integration tests for the private rust-analyzer rename backend.
//!
//! The semantic test drives a real rust-analyzer over a compilable fixture
//! workspace and is skipped (with a note) when the binary is unavailable.
//! Environment-variable–driven tests serialize through a process-wide lock.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use lsp_server::{Connection, Message, Notification, Request, RequestId, Response};
use lsp_types::notification::{DidOpenTextDocument, Exit, Initialized, Notification as _};
use lsp_types::request::{Rename, Request as _, Shutdown};
use lsp_types::{
    DidOpenTextDocumentParams, DocumentChanges, OneOf, Position, TextDocumentEdit,
    TextDocumentItem, WorkspaceEdit,
};
use typeweld_engine::line_index::LineIndex;

/// Generous: the first rust-analyzer query waits for workspace indexing.
const TIMEOUT: Duration = Duration::from_secs(180);

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
    format!(
        "[package]\n\
         name = \"server\"\n\
         version = \"0.1.0\"\n\
         edition = \"2021\"\n\
         \n\
         [dependencies]\n\
         serde = {{ version = \"1\", features = [\"derive\"] }}\n\
         typeweld = {{ path = \"{}\" }}\n",
        typeweld_dir.display()
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

/// The rust-analyzer binary the backend would use, when it actually runs.
fn rust_analyzer_binary() -> Option<PathBuf> {
    let binary = std::env::var_os("TYPEWELD_RUST_ANALYZER")
        .map_or_else(|| PathBuf::from("rust-analyzer"), PathBuf::from);
    let works = Command::new(&binary)
        .arg("--version")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|status| status.success());
    works.then_some(binary)
}

struct Client {
    connection: Connection,
    next_id: i32,
    handle: Option<std::thread::JoinHandle<Result<(), String>>>,
}

impl Client {
    fn start(root: &Path, initialization_options: &serde_json::Value) -> Self {
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
            "initializationOptions": initialization_options,
        });
        client.request_value("initialize", params);
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

    fn request_value(&mut self, method: &str, params: serde_json::Value) -> serde_json::Value {
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
            if let Message::Response(response) = message {
                if response.id == id {
                    assert!(
                        response.error.is_none(),
                        "request `{method}` failed: {:?}",
                        response.error
                    );
                    return response.result.unwrap_or(serde_json::Value::Null);
                }
            }
        }
    }

    fn request<R: lsp_types::request::Request>(&mut self, params: R::Params) -> R::Result {
        let value = self.request_value(R::METHOD, serde_json::to_value(params).expect("params"));
        serde_json::from_value(value).expect("result")
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
    format!("file://{}", path.display())
}

fn uri_path(uri: &str) -> PathBuf {
    PathBuf::from(uri.strip_prefix("file://").expect("file uri"))
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

#[test]
fn semantic_rename_covers_plain_rust_references() {
    let Some(binary) = rust_analyzer_binary() else {
        eprintln!(
            "skipping semantic_rename_covers_plain_rust_references: rust-analyzer --version failed"
        );
        return;
    };
    let _env = EnvGuard::set(&[
        (
            "TYPEWELD_RUST_ANALYZER",
            binary.to_string_lossy().into_owned(),
        ),
        ("TYPEWELD_RA_TIMEOUT_SECS", "120".to_owned()),
    ]);

    let dir = fixture();
    let mut client = Client::start(dir.path(), &serde_json::json!({ "rustBackend": "lazy" }));
    let lib_rs = dir.path().join("server/src/lib.rs");
    // Exercise the queued document sync: opened before the lazy spawn.
    client.open(&lib_rs, "rust", LIB_RS);

    let position = position_of(LIB_RS, "pub struct User");
    let position = Position::new(position.line, position.character + 11);
    let edit: Option<WorkspaceEdit> = client.request::<Rename>(
        serde_json::from_value(rename_params(&lib_rs, position, "Account")).expect("params"),
    );
    let edits = workspace_edits(&edit.expect("workspace edit"));

    let expected = ident_occurrences(LIB_RS, "User");
    let in_lib = edits
        .iter()
        .filter(|(path, old, new)| path == &lib_rs && old == "User" && new == "Account")
        .count();
    assert_eq!(
        in_lib, expected,
        "rust-analyzer should cover every `User` ident in lib.rs \
         (including `business_logic`), got {edits:?}"
    );

    let main_ts = dir.path().join("app/src/main.ts");
    assert!(
        edits
            .iter()
            .any(|(path, old, new)| path == &main_ts && old == "User" && new == "Account"),
        "expected the TS usage edit: {edits:?}"
    );
}

#[test]
fn rename_survives_a_crashing_backend() {
    let _env = EnvGuard::set(&[("TYPEWELD_RUST_ANALYZER", "/bin/false".to_owned())]);

    let dir = fixture();
    let mut client = Client::start(dir.path(), &serde_json::json!({ "rustBackend": "lazy" }));
    let lib_rs = dir.path().join("server/src/lib.rs");

    let position = position_of(LIB_RS, "pub async fn get_user");
    let position = Position::new(position.line, position.character + 14);
    let edit: Option<WorkspaceEdit> = client.request::<Rename>(
        serde_json::from_value(rename_params(&lib_rs, position, "fetch_user")).expect("params"),
    );
    let edits = workspace_edits(&edit.expect("workspace edit"));

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

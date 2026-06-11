//! Integration tests for the private typescript-language-server rename
//! backend.
//!
//! The semantic tests drive a real typescript-language-server over a fixture
//! workspace and are skipped (with a note) when the binary is unavailable.
//! Environment-variable–driven tests serialize through a process-wide lock.

use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use lsp_server::{Connection, Message, Notification, Request, RequestId, Response};
use lsp_types::notification::{Exit, Initialized, Notification as _};
use lsp_types::request::{Rename, Request as _, Shutdown};
use lsp_types::{DocumentChanges, OneOf, Position, TextDocumentEdit, WorkspaceEdit};
use typeweld_engine::line_index::LineIndex;

/// Generous: the first tsserver query waits for project loading.
const TIMEOUT: Duration = Duration::from_mins(3);

static ENV_LOCK: Mutex<()> = Mutex::new(());

/// Sets process environment variables for one test, restoring on drop.
struct EnvGuard {
    _lock: MutexGuard<'static, ()>,
    previous: Vec<(&'static str, Option<std::ffi::OsString>)>,
}

impl EnvGuard {
    /// Locks the process-wide env mutex, computes the variables to set
    /// *under the lock* (other tests temporarily mutate the same process
    /// environment, e.g. `TYPEWELD_TSLS=/bin/false`), and applies them.
    /// `None` from `vars` releases the lock without touching anything.
    fn try_set(vars: impl FnOnce() -> Option<Vec<(&'static str, String)>>) -> Option<Self> {
        let lock = ENV_LOCK.lock().unwrap_or_else(PoisonError::into_inner);
        let vars = vars()?;
        let previous = vars
            .iter()
            .map(|(key, _)| (*key, std::env::var_os(key)))
            .collect();
        for (key, value) in &vars {
            std::env::set_var(key, value);
        }
        Some(Self {
            _lock: lock,
            previous,
        })
    }

    fn set(vars: &[(&'static str, String)]) -> Self {
        Self::try_set(|| Some(vars.to_vec())).expect("vars provided")
    }
}

impl Drop for EnvGuard {
    fn drop(&mut self) {
        for (key, previous) in &self.previous {
            match previous {
                Some(value) => std::env::set_var(key, value),
                None => std::env::remove_var(key),
            }
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

/// The app uses both a renameable property access (`user.displayName`) and a
/// renameable args-object key (`{ id: 1 }`); both live in user code the
/// contract cannot see.
const MAIN_TS: &str = r#"
import { getUser } from "@workspace/test-api/endpoints/users"
import type { User } from "@workspace/test-api"

export const program = getUser({ id: 1 })
export const show = (user: User) => user.displayName
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
    dir
}

/// The repo-local npm install used to run a real typescript-language-server.
fn npm_node_modules() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../npm/node_modules")
}

/// The typescript-language-server binary, when one is available.
fn tsls_binary() -> Option<PathBuf> {
    let binary = std::env::var_os("TYPEWELD_TSLS").map_or_else(
        || npm_node_modules().join(".bin/typescript-language-server"),
        PathBuf::from,
    );
    binary.exists().then_some(binary)
}

/// The pinned tsserver module next to the repo-local install, when present.
fn tsserver_module() -> Option<PathBuf> {
    let module = std::env::var_os("TYPEWELD_TSSERVER").map_or_else(
        || npm_node_modules().join("typescript/lib/tsserver.js"),
        PathBuf::from,
    );
    module.exists().then_some(module)
}

/// The environment for tests that drive the real backend; `None` skips.
fn real_backend_env() -> Option<Vec<(&'static str, String)>> {
    let binary = tsls_binary()?;
    let mut vars = vec![
        ("TYPEWELD_TSLS", binary.to_string_lossy().into_owned()),
        ("TYPEWELD_TSLS_TIMEOUT_SECS", "60".to_owned()),
    ];
    if let Some(module) = tsserver_module() {
        vars.push(("TYPEWELD_TSSERVER", module.to_string_lossy().into_owned()));
    }
    Some(vars)
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

fn package_dir(root: &Path) -> PathBuf {
    root.join("target/typeweld/packages/@workspace/test-api")
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

fn assert_no_generated_edits(root: &Path, edits: &[(PathBuf, String, String)]) {
    let package = package_dir(root);
    assert!(
        edits.iter().all(|(path, _, _)| !path.starts_with(&package)),
        "generated files must never be edited: {edits:?}"
    );
}

#[test]
fn field_rename_covers_user_property_accesses() {
    let Some(vars) = real_backend_env() else {
        eprintln!(
            "skipping field_rename_covers_user_property_accesses: \
             typescript-language-server unavailable (run `npm ci` in npm/ or set TYPEWELD_TSLS)"
        );
        return;
    };
    let _env = EnvGuard::set(&vars);

    let dir = fixture();
    let mut client = Client::start(
        dir.path(),
        &serde_json::json!({ "rustBackend": "off", "tsBackend": "lazy" }),
    );
    let lib_rs = dir.path().join("server/src/lib.rs");

    let position = position_of(LIB_RS, "display_name");
    let edit: Option<WorkspaceEdit> = client.request::<Rename>(
        serde_json::from_value(rename_params(&lib_rs, position, "nickname")).expect("params"),
    );
    let edits = workspace_edits(&edit.expect("workspace edit"));

    assert!(
        edits
            .iter()
            .any(|(path, old, new)| path == &lib_rs && old == "display_name" && new == "nickname"),
        "rust ident edit: {edits:?}"
    );
    let main_ts = dir.path().join("app/src/main.ts");
    assert!(
        edits
            .iter()
            .any(|(path, old, new)| path == &main_ts && old == "displayName" && new == "nickname"),
        "expected the user property-access edit in main.ts: {edits:?}"
    );
    assert_no_generated_edits(dir.path(), &edits);
}

#[test]
fn param_rename_covers_user_args_object_keys() {
    let Some(vars) = real_backend_env() else {
        eprintln!(
            "skipping param_rename_covers_user_args_object_keys: \
             typescript-language-server unavailable (run `npm ci` in npm/ or set TYPEWELD_TSLS)"
        );
        return;
    };
    let _env = EnvGuard::set(&vars);

    let dir = fixture();
    let mut client = Client::start(
        dir.path(),
        &serde_json::json!({ "rustBackend": "off", "tsBackend": "lazy" }),
    );
    let lib_rs = dir.path().join("server/src/lib.rs");

    let position = position_of(LIB_RS, "id: Path<i32>");
    let edit: Option<WorkspaceEdit> = client.request::<Rename>(
        serde_json::from_value(rename_params(&lib_rs, position, "userId")).expect("params"),
    );
    let edits = workspace_edits(&edit.expect("workspace edit"));

    assert!(
        edits
            .iter()
            .any(|(path, old, new)| path == &lib_rs && old == "id" && new == "userId"),
        "rust ident edit: {edits:?}"
    );
    let main_ts = dir.path().join("app/src/main.ts");
    assert!(
        edits
            .iter()
            .any(|(path, old, new)| path == &main_ts && old == "id" && new == "userId"),
        "expected the `{{ id: 1 }}` args-object key edit in main.ts: {edits:?}"
    );
    assert_no_generated_edits(dir.path(), &edits);
}

#[test]
fn rename_survives_a_crashing_ts_backend() {
    let _env = EnvGuard::set(&[("TYPEWELD_TSLS", "/bin/false".to_owned())]);

    let dir = fixture();
    let mut client = Client::start(
        dir.path(),
        &serde_json::json!({ "rustBackend": "off", "tsBackend": "lazy" }),
    );
    let lib_rs = dir.path().join("server/src/lib.rs");

    let position = position_of(LIB_RS, "display_name");
    let edit: Option<WorkspaceEdit> = client.request::<Rename>(
        serde_json::from_value(rename_params(&lib_rs, position, "nickname")).expect("params"),
    );
    let edits = workspace_edits(&edit.expect("workspace edit"));

    assert!(
        edits
            .iter()
            .any(|(path, old, new)| path == &lib_rs && old == "display_name" && new == "nickname"),
        "the Rust-only plan must survive a dead backend: {edits:?}"
    );
    let main_ts = dir.path().join("app/src/main.ts");
    assert!(
        edits.iter().all(|(path, _, _)| path != &main_ts),
        "no TS edits without a live backend: {edits:?}"
    );
}

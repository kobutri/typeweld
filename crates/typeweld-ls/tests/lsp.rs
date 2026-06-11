//! End-to-end language server tests over an in-memory LSP connection.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use lsp_server::{Connection, Message, Notification, Request, RequestId, Response};
use lsp_types::notification::{
    DidChangeTextDocument, DidOpenTextDocument, Exit, Initialized, Notification as _,
    PublishDiagnostics,
};
use lsp_types::request::{
    GotoDefinition, HoverRequest, References, Rename, Request as _, Shutdown,
};
use lsp_types::{
    DidChangeTextDocumentParams, DidOpenTextDocumentParams, DocumentChanges,
    GotoDefinitionResponse, Hover, HoverContents, Location, OneOf, Position,
    PublishDiagnosticsParams, Range, TextDocumentContentChangeEvent, TextDocumentEdit,
    TextDocumentItem, VersionedTextDocumentIdentifier, WorkspaceEdit,
};
use typeweld_engine::line_index::LineIndex;

const TIMEOUT: Duration = Duration::from_mins(1);

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

/// Delete one user by id.
#[api(delete, "/users/{id}")]
pub async fn delete_user(id: Path<i32>) -> Result<Json<User>, UserError> {
    Err(UserError::UserNotFound { id: *id })
}

#[api_router]
pub fn routes() -> ApiRouter {
    ApiRouter::new().endpoint(get_user).endpoint(delete_user)
}
"#;

const MAIN_TS: &str = r#"
import { getUser } from "@workspace/test-api/endpoints/users"

export const program = getUser({ id: 1 })
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

struct Client {
    connection: Connection,
    next_id: i32,
    pending: Vec<Notification>,
    handle: Option<std::thread::JoinHandle<Result<(), String>>>,
}

impl Client {
    fn start(root: &Path) -> Self {
        let (server_side, client_side) = Connection::memory();
        let handle = std::thread::spawn(move || typeweld_ls::run_with_connection(server_side));
        let mut client = Client {
            connection: client_side,
            next_id: 0,
            pending: Vec::new(),
            handle: Some(handle),
        };
        let params = serde_json::json!({
            "rootUri": uri(root),
            "capabilities": {},
            // These tests exercise the contract-based plans; the semantic
            // Rust and TypeScript backends have their own integration tests.
            "initializationOptions": { "rustBackend": "off", "tsBackend": "off" },
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
        let response = self.request_response(method, params);
        assert!(
            response.error.is_none(),
            "request `{method}` failed: {:?}",
            response.error
        );
        response.result.unwrap_or(serde_json::Value::Null)
    }

    fn request_response(&mut self, method: &str, params: serde_json::Value) -> Response {
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
                Message::Response(response) if response.id == id => return response,
                Message::Response(_) | Message::Request(_) => {}
                Message::Notification(notification) => self.pending.push(notification),
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

    /// Waits for the next publishDiagnostics notification for `path`.
    fn wait_diagnostics(&mut self, path: &Path) -> PublishDiagnosticsParams {
        let wanted = uri(path);
        let deadline = Instant::now() + TIMEOUT;
        let mut index = 0;
        loop {
            while index < self.pending.len() {
                let notification = &self.pending[index];
                if notification.method == PublishDiagnostics::METHOD {
                    let params: PublishDiagnosticsParams =
                        serde_json::from_value(notification.params.clone()).expect("params");
                    if params.uri.as_str() == wanted {
                        self.pending.remove(index);
                        return params;
                    }
                }
                index += 1;
            }
            let remaining = deadline.saturating_duration_since(Instant::now());
            match self.connection.receiver.recv_timeout(remaining) {
                Ok(Message::Notification(notification)) => self.pending.push(notification),
                Ok(_) => {}
                Err(error) => panic!("no diagnostics for {}: {error}", path.display()),
            }
        }
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
                Ok(Message::Response(_)) | Err(_) => break,
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

/// Zero-based UTF-16 position of the start of `needle` (skipping `skip`
/// earlier occurrences) in `text`. Fixtures are ASCII, so UTF-16 == UTF-8.
fn position_of(text: &str, needle: &str, skip: usize) -> Position {
    let mut offset = 0;
    for _ in 0..skip {
        let found = text[offset..].find(needle).expect("needle");
        offset += found + needle.len();
    }
    let found = text[offset..].find(needle).expect("needle");
    let offset = u32::try_from(offset + found).expect("offset");
    let (line, character) = LineIndex::new(text).line_col_utf16(offset, text);
    Position::new(line, character)
}

fn offset_of(text: &str, range: Range) -> (usize, usize) {
    let index = LineIndex::new(text);
    let start = index.offset_utf16(range.start.line, range.start.character, text) as usize;
    let end = index.offset_utf16(range.end.line, range.end.character, text) as usize;
    (start, end)
}

fn position_params(path: &Path, position: Position) -> serde_json::Value {
    serde_json::json!({
        "textDocument": { "uri": uri(path) },
        "position": position,
    })
}

fn location_path(location: &Location) -> PathBuf {
    uri_path(location.uri.as_str())
}

fn uri_path(uri: &str) -> PathBuf {
    let text = uri.strip_prefix("file://").expect("file uri");
    let mut decoded = String::with_capacity(text.len());
    let bytes = text.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' {
            let digits = &text[index + 1..index + 3];
            decoded.push(char::from(u8::from_str_radix(digits, 16).expect("hex")));
            index += 3;
        } else {
            decoded.push(char::from(bytes[index]));
            index += 1;
        }
    }
    PathBuf::from(decoded)
}

fn package_dir(root: &Path) -> PathBuf {
    root.join("target/typeweld/packages/@workspace/test-api")
}

fn definition_locations(response: Option<GotoDefinitionResponse>) -> Vec<Location> {
    match response.expect("definition response") {
        GotoDefinitionResponse::Scalar(location) => vec![location],
        GotoDefinitionResponse::Array(locations) => locations,
        GotoDefinitionResponse::Link(_) => panic!("unexpected links"),
    }
}

/// Asserts that `location` covers exactly `needle` in its file.
fn assert_covers(location: &Location, needle: &str) {
    let path = location_path(location);
    let text = std::fs::read_to_string(&path).expect("read location file");
    let (start, end) = offset_of(&text, location.range);
    assert_eq!(&text[start..end], needle, "location in {}", path.display());
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
        for edit in edits {
            let OneOf::Left(edit) = edit else {
                panic!("unexpected annotated edit")
            };
            let (start, end) = offset_of(&text, edit.range);
            collected.push((
                path.clone(),
                text[start..end].to_owned(),
                edit.new_text.clone(),
            ));
        }
    }
    collected
}

fn hover_text(hover: Option<Hover>) -> String {
    match hover.expect("hover").contents {
        HoverContents::Markup(markup) => markup.value,
        other => panic!("unexpected hover contents: {other:?}"),
    }
}

#[test]
fn initialize_generates_the_package() {
    let dir = fixture();
    let mut client = Client::start(dir.path());
    // Any request synchronizes with the initial build.
    let _: Option<Hover> = client.request::<HoverRequest>(
        serde_json::from_value(position_params(
            &dir.path().join("server/src/lib.rs"),
            Position::new(0, 0),
        ))
        .expect("params"),
    );
    let index = package_dir(dir.path()).join("index.ts");
    assert!(index.is_file(), "missing {}", index.display());
}

#[test]
fn definition_from_ts_usage_lands_on_rust_ident() {
    let dir = fixture();
    let mut client = Client::start(dir.path());
    let main_ts = dir.path().join("app/src/main.ts");
    let position = position_of(MAIN_TS, "getUser({", 0);
    let response: Option<GotoDefinitionResponse> = client.request::<GotoDefinition>(
        serde_json::from_value(position_params(&main_ts, position)).expect("params"),
    );
    let locations = definition_locations(response);
    assert_eq!(locations.len(), 1);
    assert_eq!(
        locations[0],
        rust_ident_location(dir.path(), "pub async fn ", "get_user")
    );
}

fn rust_ident_location(root: &Path, prefix: &str, name: &str) -> Location {
    let lib_rs = root.join("server/src/lib.rs");
    let text = std::fs::read_to_string(&lib_rs).expect("read lib.rs");
    let declaration = format!("{prefix}{name}");
    let start = text.find(&declaration).expect("declaration") + prefix.len();
    let index = LineIndex::new(&text);
    let (start_line, start_col) =
        index.line_col_utf16(u32::try_from(start).expect("offset"), &text);
    let (end_line, end_col) =
        index.line_col_utf16(u32::try_from(start + name.len()).expect("offset"), &text);
    Location::new(
        uri(&lib_rs).parse().expect("uri"),
        Range::new(
            Position::new(start_line, start_col),
            Position::new(end_line, end_col),
        ),
    )
}

#[test]
fn definition_from_generated_accessor_lands_on_rust_ident() {
    let dir = fixture();
    let mut client = Client::start(dir.path());
    // Synchronize with the initial build before reading the generated file.
    let users_ts = package_dir(dir.path()).join("endpoints/users.ts");
    let _: Option<Hover> = client.request::<HoverRequest>(
        serde_json::from_value(position_params(
            &dir.path().join("server/src/lib.rs"),
            Position::new(0, 0),
        ))
        .expect("params"),
    );
    let generated = std::fs::read_to_string(&users_ts).expect("read generated file");
    let position = position_of(&generated, "export const getUser = ", 0);
    let position = Position::new(position.line, position.character + 13);
    let response: Option<GotoDefinitionResponse> = client.request::<GotoDefinition>(
        serde_json::from_value(position_params(&users_ts, position)).expect("params"),
    );
    let locations = definition_locations(response);
    assert!(locations.contains(&rust_ident_location(
        dir.path(),
        "pub async fn ",
        "get_user"
    )));
}

#[test]
fn definition_from_rust_returns_generated_locations() {
    let dir = fixture();
    let mut client = Client::start(dir.path());
    let lib_rs = dir.path().join("server/src/lib.rs");
    let position = position_of(LIB_RS, "pub async fn get_user", 0);
    let position = Position::new(position.line, position.character + 14);
    let response: Option<GotoDefinitionResponse> = client.request::<GotoDefinition>(
        serde_json::from_value(position_params(&lib_rs, position)).expect("params"),
    );
    let locations = definition_locations(response);
    assert!(!locations.is_empty());
    let package = package_dir(dir.path());
    for location in &locations {
        assert!(location_path(location).starts_with(&package));
    }
}

#[test]
fn references_include_usage_and_router_mount() {
    let dir = fixture();
    let mut client = Client::start(dir.path());
    let lib_rs = dir.path().join("server/src/lib.rs");
    let position = position_of(LIB_RS, "pub async fn get_user", 0);
    let position = Position::new(position.line, position.character + 14);
    let mut params: serde_json::Value = position_params(&lib_rs, position);
    params["context"] = serde_json::json!({ "includeDeclaration": true });
    let locations: Option<Vec<Location>> =
        client.request::<References>(serde_json::from_value(params).expect("params"));
    let locations = locations.expect("references");

    let main_ts = dir.path().join("app/src/main.ts");
    let in_main = locations
        .iter()
        .filter(|loc| location_path(loc) == main_ts)
        .count();
    assert!(in_main >= 1, "expected a main.ts reference: {locations:?}");

    let mount = locations
        .iter()
        .filter(|loc| location_path(loc) == lib_rs)
        .find(|loc| {
            let text = std::fs::read_to_string(&lib_rs).expect("read");
            let (start, _) = offset_of(&text, loc.range);
            text[..start].ends_with(".endpoint(")
        });
    assert!(mount.is_some(), "expected the router mount: {locations:?}");
    for location in locations.iter().filter(|loc| location_path(loc) == lib_rs) {
        assert_covers(location, "get_user");
    }
}

#[test]
fn hover_shows_method_and_route() {
    let dir = fixture();
    let mut client = Client::start(dir.path());
    let lib_rs = dir.path().join("server/src/lib.rs");
    let position = position_of(LIB_RS, "pub async fn get_user", 0);
    let position = Position::new(position.line, position.character + 14);
    let hover: Option<Hover> = client.request::<HoverRequest>(
        serde_json::from_value(position_params(&lib_rs, position)).expect("params"),
    );
    let text = hover_text(hover);
    assert!(text.contains("GET"), "hover: {text}");
    assert!(text.contains("/users/"), "hover: {text}");
}

#[test]
fn did_change_regenerates_and_updates_hover() {
    let dir = fixture();
    let mut client = Client::start(dir.path());
    let lib_rs = dir.path().join("server/src/lib.rs");
    client.open(&lib_rs, "rust", LIB_RS);

    let changed = LIB_RS.replace(
        "#[api(get, \"/users/{id}\")]",
        "#[api(get, \"/users/{id}/profile\")]",
    );
    client.change(&lib_rs, 2, &changed);

    let position = position_of(&changed, "pub async fn get_user", 0);
    let position = Position::new(position.line, position.character + 14);
    let hover: Option<Hover> = client.request::<HoverRequest>(
        serde_json::from_value(position_params(&lib_rs, position)).expect("params"),
    );
    let text = hover_text(hover);
    assert!(text.contains("/users/{id}/profile"), "hover: {text}");

    let users_ts = package_dir(dir.path()).join("endpoints/users.ts");
    let generated = std::fs::read_to_string(&users_ts).expect("read generated");
    assert!(
        generated.contains("/users/{id}/profile"),
        "generated: {generated}"
    );
}

#[test]
fn rename_works_in_both_directions() {
    let dir = fixture();
    let mut client = Client::start(dir.path());
    let lib_rs = dir.path().join("server/src/lib.rs");
    let main_ts = dir.path().join("app/src/main.ts");

    // Rust-initiated: get_user -> fetch_user.
    let position = position_of(LIB_RS, "pub async fn get_user", 0);
    let position = Position::new(position.line, position.character + 14);
    let mut params = position_params(&lib_rs, position);
    params["newName"] = serde_json::json!("fetch_user");
    let edit: Option<WorkspaceEdit> =
        client.request::<Rename>(serde_json::from_value(params).expect("params"));
    let edits = workspace_edits(&edit.expect("workspace edit"));

    let rust_edits: Vec<_> = edits
        .iter()
        .filter(|(path, old, new)| path == &lib_rs && old == "get_user" && new == "fetch_user")
        .collect();
    assert_eq!(rust_edits.len(), 2, "ident + mount edits: {edits:?}");
    assert!(
        edits
            .iter()
            .any(|(path, old, new)| path == &main_ts && old == "getUser" && new == "fetchUser"),
        "main.ts edit: {edits:?}"
    );

    // TS-initiated: getUser -> fetchUser2.
    let position = position_of(MAIN_TS, "getUser({", 0);
    let mut params = position_params(&main_ts, position);
    params["newName"] = serde_json::json!("fetchUser2");
    let edit: Option<WorkspaceEdit> =
        client.request::<Rename>(serde_json::from_value(params).expect("params"));
    let edits = workspace_edits(&edit.expect("workspace edit"));
    assert!(
        edits
            .iter()
            .any(|(path, old, new)| path == &lib_rs && old == "get_user" && new == "fetch_user2"),
        "rust edit: {edits:?}"
    );
}

#[test]
fn diagnostics_appear_and_clear() {
    let dir = fixture();
    let mut client = Client::start(dir.path());
    let lib_rs = dir.path().join("server/src/lib.rs");
    client.open(&lib_rs, "rust", LIB_RS);

    let broken = LIB_RS.replace(
        "    pub display_name: String,",
        "    #[serde(flatten)]\n    pub display_name: String,",
    );
    client.change(&lib_rs, 2, &broken);
    let published = client.wait_diagnostics(&lib_rs);
    assert!(
        !published.diagnostics.is_empty(),
        "expected an error: {published:?}"
    );

    client.change(&lib_rs, 3, LIB_RS);
    let published = client.wait_diagnostics(&lib_rs);
    assert!(
        published.diagnostics.is_empty(),
        "expected clearing: {published:?}"
    );
}

#[test]
fn server_survives_absurd_requests() {
    let dir = fixture();
    let mut client = Client::start(dir.path());
    let lib_rs = dir.path().join("server/src/lib.rs");

    let response: Option<GotoDefinitionResponse> = client.request::<GotoDefinition>(
        serde_json::from_value(position_params(&lib_rs, Position::new(99_999, 0))).expect("params"),
    );
    assert!(response.is_none());

    let position = position_of(LIB_RS, "pub async fn get_user", 0);
    let position = Position::new(position.line, position.character + 14);
    let hover: Option<Hover> = client.request::<HoverRequest>(
        serde_json::from_value(position_params(&lib_rs, position)).expect("params"),
    );
    assert!(hover_text(hover).contains("GET"));
}

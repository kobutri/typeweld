use std::{
    env, fs,
    io::{BufRead, BufReader, Write},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::atomic::{AtomicU64, Ordering},
};

use serde_json::{json, Value};

static TEST_ID: AtomicU64 = AtomicU64::new(0);

#[test]
fn typeweld_ls_binary_exercises_editor_facing_cross_language_behavior() {
    let fixture = fixture_workspace();
    let endpoint_symbol = graph_symbol(&fixture.graph, "fixture:endpoint:getUser");
    let field_symbol = graph_symbol(&fixture.graph, "fixture:field:User:displayName");

    let transcript = [
        initialize_request(&fixture.root),
        initialized_notification(),
        framed(&json!({
            "jsonrpc": "2.0",
            "id": 2,
            "method": "typeweld-ls/generatedPackageFile",
            "params": { "uri": fixture.generated_endpoints_uri }
        })),
        framed(&json!({
            "jsonrpc": "2.0",
            "id": 3,
            "method": "textDocument/definition",
            "params": {
                "textDocument": { "uri": fixture.user_ts_uri },
                "position": fixture.endpoint_user_range["start"].clone()
            }
        })),
        references_request(endpoint_symbol, &fixture.root, 4),
        references_request(field_symbol, &fixture.root, 5),
        references_request_at(
            &fixture.user_ts_uri,
            &fixture.error_variant_user_range["start"],
            6,
        ),
        references_request_at(
            &fixture.user_ts_uri,
            &fixture.error_tag_user_range["start"],
            7,
        ),
        framed(&json!({
            "jsonrpc": "2.0",
            "id": 8,
            "method": "textDocument/hover",
            "params": {
                "textDocument": { "uri": fixture.user_ts_uri },
                "position": fixture.endpoint_user_range["start"].clone()
            }
        })),
        framed(&json!({
            "jsonrpc": "2.0",
            "id": 9,
            "method": "textDocument/rename",
            "params": {
                "textDocument": { "uri": rust_uri(endpoint_symbol, &fixture.root) },
                "position": endpoint_symbol["rust"]["range"]["start"].clone(),
                "newName": "fetchUser"
            }
        })),
        shutdown_request(10),
        exit_notification(),
    ]
    .join("");
    let messages = run_typeweld_ls(&fixture.root, &transcript);

    assert_initialize_succeeded(&messages);
    assert_generated_package_file_resolved(&messages, 2);
    assert_definition_targets_rust_symbol(&messages, 3, endpoint_symbol, &fixture.root);
    assert_reference_includes(
        &messages,
        4,
        &fixture.user_ts_uri,
        &fixture.endpoint_user_range,
    );
    assert_reference_includes(
        &messages,
        5,
        &fixture.user_ts_uri,
        &fixture.field_user_range,
    );
    assert_reference_includes(
        &messages,
        6,
        &fixture.user_ts_uri,
        &fixture.error_variant_user_range,
    );
    assert_reference_includes(
        &messages,
        7,
        &fixture.user_ts_uri,
        &fixture.error_tag_user_range,
    );
    assert_hover_describes_endpoint(&messages, 8);
    assert_rename_updates_rust_and_typescript(
        &messages,
        9,
        endpoint_symbol,
        &fixture.root,
        &fixture.user_ts_uri,
    );
    assert_unused_endpoint_diagnostic_after_usage_removed(&messages);
}

struct Fixture {
    root: PathBuf,
    graph: Value,
    user_ts_uri: String,
    generated_endpoints_uri: String,
    endpoint_user_range: Value,
    field_user_range: Value,
    error_variant_user_range: Value,
    error_tag_user_range: Value,
}

fn fixture_workspace() -> Fixture {
    let root = test_root("editor-behavior");
    fs::create_dir_all(&root).expect("create fixture root");
    fs::write(root.join("Cargo.toml"), "[workspace]\n").expect("write workspace manifest");
    write_mock_backend_config(&root);

    let contract = typeweld_test_fixtures::basic_contract();
    let package = typeweld_gen_effect_v4::render_generated_package(&contract, &root.join("target"));
    write_generated_package_files(&package);

    let rust_sources = write_rust_sources(&root);
    let user_ts_path = root.join("client/use-api.ts");
    let user_ts = r#"import { Effect } from "effect"
import { users } from "@workspace/server-api"
import type { GetUserUserNotFound } from "@workspace/server-api/errors"

export const endpointProgram = users.getUser({ id: 1n })
export const displayLabel = (user: { displayName: string }) => user.displayName
export const handled = endpointProgram.pipe(
  Effect.catchTag("UserNotFound", (error: GetUserUserNotFound) => Effect.succeed(error.id))
)
"#;
    write_file(&user_ts_path, user_ts);
    let user_ts_uri = path_to_file_uri(&user_ts_path);

    let mut graph: Value = serde_json::from_str(&typeweld_gen_effect_v4::render_symbol_graph(
        &contract, &package,
    ))
    .expect("parse generated symbol graph");
    add_fixture_user_locations(&mut graph, &rust_sources, user_ts, &user_ts_uri);
    let contract_hash = graph["contractHash"]
        .as_str()
        .expect("generated contract hash")
        .to_owned();
    write_file(
        &root.join("target/api-contract/rust-ts-symbols.json"),
        &serde_json::to_string_pretty(&graph).expect("serialize symbol graph"),
    );
    write_usage_index_after_create_user_usage_removed(&root, &contract_hash);

    Fixture {
        root,
        graph,
        user_ts_uri,
        generated_endpoints_uri: path_to_file_uri(&package.package_dir.join("endpoints.ts")),
        endpoint_user_range: range_for_after(user_ts, "users.", "getUser"),
        field_user_range: range_for_after(user_ts, "user.", "displayName"),
        error_variant_user_range: range_for_after(user_ts, "error: ", "GetUserUserNotFound"),
        error_tag_user_range: range_for_after(user_ts, "catchTag(\"", "UserNotFound"),
    }
}

struct RustSources {
    users: String,
    types: String,
    errors: String,
}

fn write_rust_sources(root: &Path) -> RustSources {
    let users_rs = r#"use typeweld_macros::api;

pub struct Path<T>(pub T);
pub struct Json<T>(pub T);
pub struct User;
pub enum GetUserError {}

#[api(method = "GET", path = "/users/{id}")]
pub async fn get_user(id: Path<i64>) -> Result<Json<User>, GetUserError> {
    todo!()
}
"#;
    let types_rs = r#"use serde::Serialize;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct User {
    pub id: i64,
    pub display_name: String,
}
"#;
    let errors_rs = r"use typeweld_macros::api_error;

#[api_error]
pub enum GetUserError {
    #[status(404)]
    UserNotFound { id: i64 },
    #[status(403)]
    PermissionDenied,
}
";

    write_file(&root.join("crates/server/src/users.rs"), users_rs);
    write_file(&root.join("crates/server/src/types.rs"), types_rs);
    write_file(&root.join("crates/server/src/errors.rs"), errors_rs);

    RustSources {
        users: users_rs.to_owned(),
        types: types_rs.to_owned(),
        errors: errors_rs.to_owned(),
    }
}

fn add_fixture_user_locations(
    graph: &mut Value,
    rust_sources: &RustSources,
    user_ts: &str,
    user_ts_uri: &str,
) {
    let endpoint_range = range_for(&rust_sources.users, "get_user");
    set_rust_location(
        graph_symbol_mut(graph, "fixture:endpoint:getUser"),
        "crates/server/src/users.rs",
        &endpoint_range,
        &line_range_for(&rust_sources.users, "pub async fn get_user"),
    );
    add_user_location(
        graph_symbol_mut(graph, "fixture:endpoint:getUser"),
        user_ts_uri,
        &range_for_after(user_ts, "users.", "getUser"),
    );

    let field_range = range_for(&rust_sources.types, "display_name");
    set_rust_location(
        graph_symbol_mut(graph, "fixture:field:User:displayName"),
        "crates/server/src/types.rs",
        &field_range,
        &line_range_for(&rust_sources.types, "    pub display_name"),
    );
    add_user_location(
        graph_symbol_mut(graph, "fixture:field:User:displayName"),
        user_ts_uri,
        &range_for_after(user_ts, "user.", "displayName"),
    );

    let user_type_range = range_for(&rust_sources.types, "User");
    set_rust_location(
        graph_symbol_mut(graph, "fixture:User"),
        "crates/server/src/types.rs",
        &user_type_range,
        &line_range_for(&rust_sources.types, "pub struct User"),
    );

    let error_type_range = range_for(&rust_sources.errors, "GetUserError");
    set_rust_location(
        graph_symbol_mut(graph, "fixture:GetUserError"),
        "crates/server/src/errors.rs",
        &error_type_range,
        &line_range_for(&rust_sources.errors, "pub enum GetUserError"),
    );

    let error_variant_range = range_for(&rust_sources.errors, "UserNotFound");
    set_rust_location(
        graph_symbol_mut(graph, "fixture:error:GetUserError:UserNotFound"),
        "crates/server/src/errors.rs",
        &error_variant_range,
        &line_range_for(&rust_sources.errors, "    UserNotFound"),
    );
    add_user_location(
        graph_symbol_mut(graph, "fixture:error:GetUserError:UserNotFound"),
        user_ts_uri,
        &range_for_after(user_ts, "error: ", "GetUserUserNotFound"),
    );

    let error_tag = graph_error_tag_symbol_mut(graph, "UserNotFound");
    set_rust_location(
        error_tag,
        "crates/server/src/errors.rs",
        &error_variant_range,
        &line_range_for(&rust_sources.errors, "    UserNotFound"),
    );
    add_user_location(
        graph_error_tag_symbol_mut(graph, "UserNotFound"),
        user_ts_uri,
        &range_for_after(user_ts, "catchTag(\"", "UserNotFound"),
    );
}

fn write_usage_index_after_create_user_usage_removed(root: &Path, contract_hash: &str) {
    write_file(
        &root.join("target/api-contract/graph/effect-usage-index.json"),
        &serde_json::to_string_pretty(&json!({
            "schema_version": 2,
            "contract_hash": contract_hash,
            "package_name": typeweld_test_fixtures::BASIC_PACKAGE_NAME,
            "endpoints": [
                { "endpoint_id": "fixture:endpoint:getUser", "accessor_path": ["users", "getUser"], "strong": 1 },
                { "endpoint_id": "fixture:endpoint:createUser", "accessor_path": ["users", "createUser"], "strong": 0 },
                { "endpoint_id": "fixture:endpoint:watchUsers", "accessor_path": ["events", "watchUsers"], "strong": 1 }
            ],
            "usages": []
        }))
        .expect("serialize usage index"),
    );
}

fn run_typeweld_ls(root: &Path, input: &str) -> Vec<Value> {
    let mut child = Command::new(env!("CARGO_BIN_EXE_typeweld-ls"))
        .current_dir(root)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn typeweld-ls binary");
    child
        .stdin
        .as_mut()
        .expect("typeweld-ls stdin")
        .write_all(input.as_bytes())
        .expect("write typeweld-ls input");
    drop(child.stdin.take());

    let output = child.wait_with_output().expect("wait for typeweld-ls");
    assert!(
        output.status.success(),
        "typeweld-ls exited with {}\nstderr:\n{}",
        output.status,
        String::from_utf8_lossy(&output.stderr)
    );
    parse_framed_messages(&output.stdout)
}

fn assert_initialize_succeeded(messages: &[Value]) {
    let response = response_by_id(messages, 1);
    assert!(
        response.get("error").is_none(),
        "initialize failed: {response}"
    );
    assert_eq!(
        response["result"]["serverInfo"]["name"],
        json!("typeweld-ls")
    );
}

fn assert_generated_package_file_resolved(messages: &[Value], id: u64) {
    let response = response_by_id(messages, id);
    let text = response["result"]["text"]
        .as_str()
        .expect("generated file text");
    assert!(text.contains("export namespace users"));
    assert!(text.contains("export const getUser"));
}

fn assert_definition_targets_rust_symbol(messages: &[Value], id: u64, symbol: &Value, root: &Path) {
    let response = response_by_id(messages, id);
    assert_eq!(response["result"]["uri"], rust_uri(symbol, root));
    assert_eq!(response["result"]["range"], symbol["rust"]["range"]);
}

fn assert_reference_includes(messages: &[Value], id: u64, uri: &str, range: &Value) {
    let response = response_by_id(messages, id);
    let locations = response["result"].as_array().expect("reference locations");
    assert!(
        locations
            .iter()
            .any(|location| location["uri"] == uri && location["range"] == *range),
        "expected {uri} {range} in {locations:?}"
    );
}

fn assert_hover_describes_endpoint(messages: &[Value], id: u64) {
    let response = response_by_id(messages, id);
    let hover = response["result"]["contents"]["value"]
        .as_str()
        .expect("hover markdown");
    assert!(hover.contains("Route: `GET /users/{id}`"));
    assert!(
        hover.contains("Effect: `Effect.Effect<User, ApiClientError | GetUserError, ServerApi>`")
    );
    assert!(hover.contains("TypeScript: `users.getUser`"));
}

fn assert_rename_updates_rust_and_typescript(
    messages: &[Value],
    id: u64,
    symbol: &Value,
    root: &Path,
    user_ts_uri: &str,
) {
    let response = response_by_id(messages, id);
    let changes = &response["result"]["changes"];
    let rust_uri = rust_uri(symbol, root);
    assert_eq!(changes[&rust_uri][0]["newText"], json!("fetch_user"));
    assert_eq!(changes[user_ts_uri][0]["newText"], json!("fetchUser"));
}

fn assert_unused_endpoint_diagnostic_after_usage_removed(messages: &[Value]) {
    let diagnostic = messages
        .iter()
        .find(|message| {
            message["method"] == "textDocument/publishDiagnostics"
                && message["params"]["diagnostics"]
                    .as_array()
                    .is_some_and(|items| {
                        items.iter().any(|diagnostic| {
                            diagnostic["code"] == "typeweld::unused_endpoint"
                                && diagnostic["message"].as_str().is_some_and(|message| {
                                    message.contains("POST /users")
                                        && message.contains("users.createUser")
                                })
                        })
                    })
        })
        .expect("unused endpoint diagnostic for removed createUser usage");
    assert!(diagnostic["params"]["uri"]
        .as_str()
        .is_some_and(|uri| uri.ends_with("crates/server/src/users.rs")));
}

fn response_by_id(messages: &[Value], id: u64) -> &Value {
    messages
        .iter()
        .find(|message| message.get("id") == Some(&json!(id)))
        .unwrap_or_else(|| panic!("missing response id {id}; messages: {messages:?}"))
}

fn references_request(symbol: &Value, root: &Path, id: u64) -> String {
    let uri = rust_uri(symbol, root);
    references_request_at(&uri, &symbol["rust"]["range"]["start"], id)
}

fn references_request_at(uri: &str, position: &Value, id: u64) -> String {
    framed(&json!({
        "jsonrpc": "2.0",
        "id": id,
        "method": "textDocument/references",
        "params": {
            "textDocument": { "uri": uri },
            "position": position,
            "context": { "includeDeclaration": true }
        }
    }))
}

fn rust_uri(symbol: &Value, root: &Path) -> String {
    symbol["rust"]
        .get("uri")
        .and_then(Value::as_str)
        .map_or_else(
            || path_to_file_uri(&root.join(symbol["rust"]["file"].as_str().expect("rust file"))),
            str::to_owned,
        )
}

fn set_rust_location(symbol: &mut Value, file: &str, range: &Value, full_range: &Value) {
    symbol["rust"] = json!({
        "file": file,
        "range": range,
        "nameRange": range,
        "fullRange": full_range,
        "renamable": true
    });
}

fn add_user_location(symbol: &mut Value, uri: &str, range: &Value) {
    symbol["typescript"]
        .as_array_mut()
        .expect("typescript locations")
        .push(json!({
            "uri": uri,
            "range": range,
            "nameRange": range,
            "fullRange": range,
            "generated": false,
            "renamable": true
        }));
}

fn graph_symbol<'a>(graph: &'a Value, id: &str) -> &'a Value {
    graph["symbols"]
        .as_array()
        .expect("symbols array")
        .iter()
        .find(|symbol| symbol["id"] == id)
        .unwrap_or_else(|| panic!("missing graph symbol {id}"))
}

fn graph_symbol_mut<'a>(graph: &'a mut Value, id: &str) -> &'a mut Value {
    graph["symbols"]
        .as_array_mut()
        .expect("symbols array")
        .iter_mut()
        .find(|symbol| symbol["id"] == id)
        .unwrap_or_else(|| panic!("missing graph symbol {id}"))
}

fn graph_error_tag_symbol_mut<'a>(graph: &'a mut Value, ts_name: &str) -> &'a mut Value {
    graph["symbols"]
        .as_array_mut()
        .expect("symbols array")
        .iter_mut()
        .find(|symbol| {
            symbol["kind"] == "errorTag" && symbol["metadata"]["tsName"].as_str() == Some(ts_name)
        })
        .unwrap_or_else(|| panic!("missing error tag symbol {ts_name}"))
}

fn range_for(text: &str, needle: &str) -> Value {
    let offset = text
        .find(needle)
        .unwrap_or_else(|| panic!("missing `{needle}` in fixture text"));
    range_for_offsets(text, offset, offset + needle.len())
}

fn range_for_after(text: &str, prefix: &str, needle: &str) -> Value {
    let prefix_offset = text
        .find(prefix)
        .unwrap_or_else(|| panic!("missing `{prefix}` in fixture text"));
    let offset = text[prefix_offset + prefix.len()..]
        .find(needle)
        .map_or_else(
            || panic!("missing `{needle}` after `{prefix}` in fixture text"),
            |relative| prefix_offset + prefix.len() + relative,
        );
    range_for_offsets(text, offset, offset + needle.len())
}

fn line_range_for(text: &str, line_needle: &str) -> Value {
    let start = text
        .find(line_needle)
        .unwrap_or_else(|| panic!("missing line `{line_needle}` in fixture text"));
    let end = text[start..]
        .find('\n')
        .map_or(text.len(), |relative| start + relative);
    range_for_offsets(text, start, end)
}

fn range_for_offsets(text: &str, start: usize, end: usize) -> Value {
    json!({
        "start": position_for_offset(text, start),
        "end": position_for_offset(text, end)
    })
}

fn position_for_offset(text: &str, offset: usize) -> Value {
    let prefix = &text[..offset];
    let line = prefix.bytes().filter(|byte| *byte == b'\n').count();
    let character = prefix
        .rsplit_once('\n')
        .map_or(prefix, |(_, tail)| tail)
        .chars()
        .count();
    json!({
        "line": line,
        "character": character
    })
}

fn initialize_request(root: &Path) -> String {
    framed(&json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "initialize",
        "params": {
            "processId": null,
            "rootUri": path_to_file_uri(root),
            "capabilities": {}
        }
    }))
}

fn initialized_notification() -> String {
    framed(&json!({
        "jsonrpc": "2.0",
        "method": "initialized",
        "params": {}
    }))
}

fn shutdown_request(id: u64) -> String {
    framed(&json!({
        "jsonrpc": "2.0",
        "id": id,
        "method": "shutdown"
    }))
}

fn exit_notification() -> String {
    framed(&json!({
        "jsonrpc": "2.0",
        "method": "exit"
    }))
}

fn framed(value: &Value) -> String {
    let body = serde_json::to_string(value).expect("serialize JSON-RPC message");
    format!("Content-Length: {}\r\n\r\n{body}", body.len())
}

fn parse_framed_messages(output: &[u8]) -> Vec<Value> {
    let mut reader = BufReader::new(output);
    let mut messages = Vec::new();
    while let Some(message) = read_framed_message(&mut reader) {
        messages.push(message);
    }
    messages
}

fn read_framed_message(reader: &mut impl BufRead) -> Option<Value> {
    let mut content_length = None;
    loop {
        let mut line = String::new();
        let bytes = reader.read_line(&mut line).expect("read LSP header");
        if bytes == 0 {
            return None;
        }
        let line = line.trim_end_matches(['\r', '\n']);
        if line.is_empty() {
            break;
        }
        if let Some(value) = line.strip_prefix("Content-Length:") {
            content_length = Some(value.trim().parse::<usize>().expect("parse content length"));
        }
    }

    let mut body = vec![0; content_length.expect("content length header")];
    reader.read_exact(&mut body).expect("read LSP body");
    Some(serde_json::from_slice::<Value>(&body).expect("parse LSP JSON"))
}

fn write_mock_backend_config(root: &Path) {
    let backend = root.join("mock_backend.py");
    write_file(&backend, MOCK_BACKEND);
    write_file(
        &root.join(".typeweld.json"),
        &json!({
            "rustAnalyzer": {
                "command": "python3",
                "args": [backend.to_string_lossy()]
            },
            "typescript": {
                "command": "python3",
                "args": [backend.to_string_lossy()]
            }
        })
        .to_string(),
    );
}

fn write_generated_package_files(package: &typeweld_gen_effect_v4::GeneratedPackage) {
    for file in &package.files {
        write_file(&package.package_dir.join(&file.path), &file.contents);
    }
}

fn write_file(path: &Path, contents: &str) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).expect("create parent directory");
    }
    fs::write(path, contents).expect("write fixture file");
}

fn path_to_file_uri(path: &Path) -> String {
    let path = path
        .canonicalize()
        .unwrap_or_else(|_| path.to_path_buf())
        .to_string_lossy()
        .replace('\\', "/");
    format!("file://{path}")
}

fn test_root(name: &str) -> PathBuf {
    let id = TEST_ID.fetch_add(1, Ordering::Relaxed);
    let path = env::temp_dir().join(format!(
        "typeweld-ls-integration-{name}-{}-{id}",
        std::process::id()
    ));
    let _ = fs::remove_dir_all(&path);
    path
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
        line = line.decode("utf-8").strip()
        if not line:
            break
        if line.lower().startswith("content-length:"):
            content_length = int(line.split(":", 1)[1].strip())
    body = sys.stdin.buffer.read(content_length)
    return json.loads(body.decode("utf-8"))

def send(payload):
    body = json.dumps(payload, separators=(",", ":")).encode("utf-8")
    sys.stdout.buffer.write(f"Content-Length: {len(body)}\r\n\r\n".encode("utf-8"))
    sys.stdout.buffer.write(body)
    sys.stdout.buffer.flush()

while True:
    message = read_message()
    if message is None:
        break
    method = message.get("method")
    if method == "exit":
        break
    if "id" not in message:
        continue
    if method == "initialize":
        result = {
            "capabilities": {
                "definitionProvider": True,
                "referencesProvider": True,
                "hoverProvider": True,
                "renameProvider": {"prepareProvider": True}
            }
        }
    elif method == "shutdown":
        result = None
    elif method == "textDocument/references":
        result = []
    elif method == "textDocument/rename":
        result = {"changes": {}}
    else:
        result = None
    send({"jsonrpc": "2.0", "id": message["id"], "result": result})
"#;

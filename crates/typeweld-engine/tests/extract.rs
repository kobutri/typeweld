//! Extractor integration tests over in-memory fixture workspaces.

use std::path::{Path, PathBuf};

use typeweld_engine::extract::extract;
use typeweld_engine::vfs::MemoryFileProvider;
use typeweld_engine::workspace;

fn provider(files: &[(&str, &str)]) -> MemoryFileProvider {
    let mut memory = MemoryFileProvider::default();
    for (path, contents) in files {
        memory.insert(PathBuf::from(format!("/ws/{path}")), (*contents).to_owned());
    }
    memory
}

fn extract_fixture(files: &[(&str, &str)], package: &str) -> typeweld_engine::extract::Extraction {
    let provider = provider(files);
    let (workspace, diagnostics) =
        workspace::discover(Path::new("/ws"), &provider).expect("discover");
    assert!(
        diagnostics.is_empty(),
        "workspace diagnostics: {diagnostics:?}"
    );
    extract(&workspace, package, "@workspace/test-api", &provider).expect("extract")
}

fn render_diagnostics(extraction: &typeweld_engine::extract::Extraction) -> Vec<String> {
    extraction
        .diagnostics
        .iter()
        .map(|diagnostic| {
            let severity = match diagnostic.severity {
                typeweld_engine::diag::Severity::Error => "error",
                typeweld_engine::diag::Severity::Warning => "warning",
            };
            match &diagnostic.span {
                Some(span) => format!(
                    "{}:{}..{} {severity}[{}]: {}",
                    span.file, span.start, span.end, diagnostic.code, diagnostic.message
                ),
                None => format!("{severity}[{}]: {}", diagnostic.code, diagnostic.message),
            }
        })
        .collect()
}

const SINGLE_MANIFEST: &[(&str, &str)] = &[("Cargo.toml", "[workspace]\nmembers = [\"server\"]\n")];

fn single_crate(lib_rs: &str) -> Vec<(&'static str, String)> {
    let mut files: Vec<(&str, String)> = SINGLE_MANIFEST
        .iter()
        .map(|(path, contents)| (*path, (*contents).to_owned()))
        .collect();
    files.push((
        "server/Cargo.toml",
        "[package]\nname = \"server\"\n".to_owned(),
    ));
    files.push(("server/src/lib.rs", lib_rs.to_owned()));
    files
}

fn extract_single(lib_rs: &str) -> typeweld_engine::extract::Extraction {
    let files = single_crate(lib_rs);
    let borrowed: Vec<(&str, &str)> = files
        .iter()
        .map(|(path, contents)| (*path, contents.as_str()))
        .collect();
    extract_fixture(&borrowed, "server")
}

#[test]
fn extracts_a_complete_single_crate_api() {
    let extraction = extract_single(
        r#"
use serde::{Deserialize, Serialize};
use typeweld::{api, api_router, Api, ApiError, ApiRouter, Body, Created, Json, Path, Query, Sse};

/// A user of the system.
#[derive(Clone, Debug, Deserialize, Serialize, Api)]
#[serde(rename_all = "camelCase")]
pub struct User {
    pub id: i32,
    /// Public display name.
    pub display_name: String,
    #[serde(default)]
    pub bio: Option<String>,
    pub role: Role,
    pub scores: Vec<f64>,
    pub metadata: std::collections::HashMap<String, String>,
}

#[derive(Clone, Debug, Deserialize, Serialize, Api)]
#[serde(tag = "_tag")]
pub enum Role {
    Admin,
    Member { team: String },
}

#[derive(Clone, Debug, Deserialize, Serialize, Api)]
#[serde(rename_all = "camelCase")]
pub struct CreateUser {
    pub display_name: String,
}

#[derive(Clone, Debug, Deserialize, Serialize, Api)]
#[serde(rename_all = "camelCase")]
pub struct UserFilter {
    pub role: Option<String>,
    pub limit: u32,
}

#[derive(Clone, Debug, Deserialize, Serialize, ApiError)]
#[serde(tag = "_tag", rename_all = "PascalCase")]
pub enum UserError {
    #[status(404)]
    UserNotFound { id: i32 },
    #[status(409)]
    NameTaken,
}

/// Fetch one user by id.
#[api(get, "/users/{id}")]
pub async fn get_user(id: Path<i32>) -> Result<Json<User>, UserError> {
    unimplemented!()
}

#[api(get, "/users")]
pub async fn list_users(filter: Query<UserFilter>) -> Json<Vec<User>> {
    unimplemented!()
}

#[api(post, "/users")]
pub async fn create_user(body: Body<CreateUser>) -> Result<Created<User>, UserError> {
    unimplemented!()
}

#[api(sse, "/events/users")]
pub fn watch_users() -> Sse<User, S> {
    unimplemented!()
}

#[api_router]
pub fn routes() -> ApiRouter {
    ApiRouter::new()
        .endpoint(get_user)
        .endpoint(list_users)
        .endpoint(create_user)
        .endpoint(watch_users)
}
"#,
    );

    assert!(
        !extraction.has_errors(),
        "diagnostics: {:?}",
        render_diagnostics(&extraction)
    );
    insta::assert_json_snapshot!("single_crate_contract", extraction.contract);
}

#[test]
fn resolves_types_across_crates_aliases_and_reexports() {
    let extraction = extract_fixture(
        &[
            (
                "Cargo.toml",
                "[workspace]\nmembers = [\"server\", \"api-types\"]\n",
            ),
            (
                "server/Cargo.toml",
                "[package]\nname = \"server\"\n[dependencies]\napi-types = { path = \"../api-types\" }\n",
            ),
            (
                "server/src/lib.rs",
                r"
mod handlers;

pub use handlers::routes;
",
            ),
            (
                "server/src/handlers.rs",
                r#"
use api_types::models::{Widget, WidgetId};
use typeweld::{api, api_router, ApiRouter, Json, Path};

#[api(get, "/widgets/{id}")]
pub async fn get_widget(id: Path<WidgetId>) -> Json<Widget> {
    unimplemented!()
}

#[api_router]
pub fn routes() -> ApiRouter {
    ApiRouter::new().endpoint(get_widget)
}
"#,
            ),
            (
                "api-types/Cargo.toml",
                "[package]\nname = \"api-types\"\n",
            ),
            (
                "api-types/src/lib.rs",
                "pub mod models;\n",
            ),
            (
                "api-types/src/models.rs",
                r"
use serde::{Deserialize, Serialize};
use typeweld::Api;

pub type WidgetId = i64;

#[derive(Clone, Debug, Deserialize, Serialize, Api)]
pub struct Widget {
    pub id: WidgetId,
    pub part: super::models::Part,
}

#[derive(Clone, Debug, Deserialize, Serialize, Api)]
pub struct Part {
    pub name: String,
}
",
            ),
        ],
        "server",
    );

    // i64 in a JSON position warns but extracts.
    let messages = render_diagnostics(&extraction);
    assert!(!extraction.has_errors(), "unexpected errors: {messages:?}");
    let contract = &extraction.contract;
    assert_eq!(contract.endpoints.len(), 1);
    let type_names: Vec<&str> = contract
        .types
        .iter()
        .map(|decl| decl.ts_name.as_str())
        .collect();
    assert_eq!(type_names, ["Part", "Widget"]);
    // The alias resolved to a primitive in both the param and the field.
    insta::assert_json_snapshot!("cross_crate_contract", contract);
}

#[test]
fn router_nesting_prefixes_routes() {
    let extraction = extract_single(
        r#"
use typeweld::{api, api_router, ApiRouter, NoContent};

#[api(delete, "/sessions/{token}")]
pub async fn end_session(token: Path<String>) -> NoContent {
    unimplemented!()
}

#[api(get, "/health")]
pub async fn health() -> NoContent {
    unimplemented!()
}

mod admin {
    use typeweld::{api, NoContent, Path};

    #[api(post, "/reset")]
    pub async fn reset() -> NoContent {
        unimplemented!()
    }
}

use typeweld::Path;

#[api_router]
pub fn admin_routes() -> ApiRouter {
    ApiRouter::new().endpoint(admin::reset)
}

#[api_router]
pub fn routes() -> ApiRouter {
    ApiRouter::new()
        .endpoint(end_session)
        .endpoint(health)
        .nest("/admin", admin_routes())
}
"#,
    );

    assert!(
        !extraction.has_errors(),
        "diagnostics: {:?}",
        render_diagnostics(&extraction)
    );
    let routes: Vec<(&str, &str)> = extraction
        .contract
        .endpoints
        .iter()
        .map(|endpoint| (endpoint.rust_name.as_str(), endpoint.route.as_str()))
        .collect();
    assert_eq!(
        routes,
        [
            ("health", "/health"),
            ("reset", "/admin/reset"),
            ("end_session", "/sessions/{token}"),
        ]
    );
    // Every endpoint has its mount recorded.
    assert!(extraction
        .contract
        .endpoints
        .iter()
        .all(|endpoint| endpoint.router_mounts.len() == 1));
}

#[test]
fn reports_precise_diagnostics_for_violations() {
    let extraction = extract_single(
        r#"
use serde::{Deserialize, Serialize};
use typeweld::{api, Api, Body, Json, Path};

#[derive(Clone, Debug, Deserialize, Serialize, Api)]
pub struct Bad {
    #[serde(flatten)]
    pub nested: Nested,
    pub missing: DoesNotExist,
    pub plain: NoDerive,
}

#[derive(Clone, Debug, Deserialize, Serialize, Api)]
pub struct Nested {
    pub value: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct NoDerive {
    pub value: String,
}

// Signature violations: extra path arg, body on GET.
#[api(get, "/bad/{id}")]
pub async fn bad(id: Path<i32>, extra: Path<i32>) -> Json<Bad> {
    unimplemented!()
}

#[api(get, "/plain")]
pub async fn plain(body: Body<Nested>) -> Json<Nested> {
    unimplemented!()
}

// Valid endpoint so the Bad struct's field violations are reached.
#[api(get, "/bads")]
pub async fn list_bads() -> Json<Bad> {
    unimplemented!()
}
"#,
    );

    assert!(extraction.has_errors());
    insta::assert_debug_snapshot!("violation_diagnostics", render_diagnostics(&extraction));
}

#[test]
fn unmounted_endpoints_warn() {
    let extraction = extract_single(
        r#"
use typeweld::{api, NoContent};

#[api(get, "/ping")]
pub async fn ping() -> NoContent {
    unimplemented!()
}
"#,
    );

    assert!(!extraction.has_errors());
    let warnings = render_diagnostics(&extraction);
    assert!(
        warnings
            .iter()
            .any(|message| message.contains("unmounted-endpoint")),
        "{warnings:?}"
    );
}

//! Generator golden tests: extract a fixture workspace, generate the Effect
//! package, snapshot every file, and verify mark positions.

use std::path::{Path, PathBuf};

use typeweld_engine::extract::extract;
use typeweld_engine::gen::{generate, MarkKind};
use typeweld_engine::vfs::MemoryFileProvider;
use typeweld_engine::workspace;

const FIXTURE: &str = r#"
use serde::{Deserialize, Serialize};
use typeweld::{api, api_router, Api, ApiError, ApiRouter, Binary, Body, Created, Json, NoContent, Path, Query, Sse};

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
    pub joined_at: chrono::DateTime<chrono::Utc>,
    pub labels: Vec<String>,
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
pub struct UserId(pub uuid::Uuid);

#[derive(Clone, Debug, Deserialize, Serialize, Api)]
#[serde(rename_all = "camelCase")]
pub struct UserFilter {
    pub role: Option<String>,
    pub limit: u32,
}

#[derive(Clone, Debug, Deserialize, Serialize, ApiError)]
#[serde(tag = "_tag", rename_all = "PascalCase")]
pub enum UserError {
    /// No user with this id exists.
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

#[api(delete, "/users/{id}")]
pub async fn delete_user(id: Path<i32>) -> Result<NoContent, UserError> {
    unimplemented!()
}

/// Watch user events.
#[api(sse, "/events/users")]
pub fn watch_users() -> Result<Sse<User, S>, UserError> {
    unimplemented!()
}

#[api(get, "/files/{name}")]
pub async fn download(name: Path<String>) -> Binary<bytes::Bytes> {
    unimplemented!()
}

#[api(post, "/files")]
pub async fn upload(data: Binary<bytes::Bytes>) -> Json<User> {
    unimplemented!()
}

#[api_router]
pub fn routes() -> ApiRouter {
    ApiRouter::new()
        .endpoint(get_user)
        .endpoint(list_users)
        .endpoint(create_user)
        .endpoint(delete_user)
        .endpoint(watch_users)
        .endpoint(download)
        .endpoint(upload)
}
"#;

fn generated() -> (typeweld_engine::gen::GeneratedPackage, Vec<String>) {
    let mut provider = MemoryFileProvider::default();
    provider.insert(
        PathBuf::from("/ws/Cargo.toml"),
        "[workspace]\nmembers = [\"server\"]\n".to_owned(),
    );
    provider.insert(
        PathBuf::from("/ws/server/Cargo.toml"),
        "[package]\nname = \"server\"\n".to_owned(),
    );
    provider.insert(PathBuf::from("/ws/server/src/lib.rs"), FIXTURE.to_owned());

    let (workspace, _) = workspace::discover(Path::new("/ws"), &provider).expect("discover");
    let extraction =
        extract(&workspace, "server", "@workspace/server-api", &provider).expect("extract");
    assert!(
        !extraction.has_errors(),
        "extraction errors: {:?}",
        extraction.diagnostics
    );

    let (package, diagnostics) = generate(&extraction.contract);
    let rendered: Vec<String> = diagnostics.iter().map(ToString::to_string).collect();
    (package, rendered)
}

#[test]
fn generates_the_full_package() {
    let (package, diagnostics) = generated();
    assert!(diagnostics.is_empty(), "{diagnostics:?}");

    let mut paths: Vec<&str> = package
        .files
        .iter()
        .map(|file| file.path.as_str())
        .collect();
    paths.sort_unstable();
    assert_eq!(
        paths,
        [
            "client.ts",
            "endpoints/events.ts",
            "endpoints/files.ts",
            "endpoints/users.ts",
            "errors.ts",
            "index.ts",
            "package.json",
            "schemas.ts",
        ]
    );

    for file in &package.files {
        let name = file.path.replace('/', "_");
        insta::assert_snapshot!(name, file.contents);
    }
}

#[test]
fn marks_point_at_their_symbols() {
    let (package, _) = generated();
    assert!(!package.marks.is_empty());

    let mut seen_kinds = std::collections::HashSet::new();
    for mark in &package.marks {
        let file = package
            .files
            .iter()
            .find(|file| file.path == mark.file)
            .unwrap_or_else(|| panic!("mark file {} missing", mark.file));
        let text = &file.contents[mark.start as usize..mark.end as usize];
        assert!(
            text.chars()
                .next()
                .is_some_and(|first| first.is_ascii_alphabetic() || first == 'S'),
            "mark text looks wrong: {text:?}"
        );
        seen_kinds.insert(format!("{:?}", mark.kind));

        // Endpoint marks must select exactly the accessor name.
        if matches!(mark.kind, MarkKind::Endpoint) {
            assert!(
                [
                    "getUser",
                    "listUsers",
                    "createUser",
                    "deleteUser",
                    "watchUsers",
                    "download",
                    "upload"
                ]
                .contains(&text),
                "unexpected endpoint mark {text:?}"
            );
        }
    }

    for kind in [
        "Endpoint",
        "Type",
        "Error",
        "ErrorVariant",
        "EnumVariant",
        "Field",
        "Param",
    ] {
        assert!(seen_kinds.contains(kind), "no {kind} marks recorded");
    }
}

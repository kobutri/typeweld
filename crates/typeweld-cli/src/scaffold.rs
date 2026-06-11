//! `typeweld new`: project scaffolding.

use std::fs;
use std::path::Path;

/// Creates a new typeweld project: a cargo workspace with a server crate, a
/// TypeScript app wired to the generated bindings, and typeweld.toml.
///
/// # Errors
/// Fails when the target directory already exists or files cannot be written.
pub fn new_project(name: &str) -> anyhow::Result<()> {
    if !name
        .chars()
        .all(|character| character.is_ascii_alphanumeric() || character == '-' || character == '_')
        || name.is_empty()
    {
        anyhow::bail!("project names must be alphanumeric with - or _");
    }
    let root = Path::new(name);
    if root.exists() {
        anyhow::bail!("directory `{name}` already exists");
    }

    let ts_package = format!("@{name}/api");
    let package_path = format!("target/typeweld/packages/@{name}/api");

    write(
        &root.join("Cargo.toml"),
        "[workspace]\nmembers = [\"server\"]\nresolver = \"2\"\n",
    )?;
    write(
        &root.join("typeweld.toml"),
        &format!(
            "[[package]]\ncargo = \"server\"\nts = \"{ts_package}\"\n\n[app]\nsrc = \
             [\"app/src/**/*.ts\"]\n\n[lint]\nunused-endpoints = \"warn\"\n"
        ),
    )?;
    write(&root.join(".gitignore"), "/target\nnode_modules/\n")?;

    // Server crate.
    write(
        &root.join("server/Cargo.toml"),
        "[package]\nname = \"server\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[dependencies]\n\
         axum = \"0.8\"\nserde = { version = \"1\", features = [\"derive\"] }\ntokio = { version \
         = \"1\", features = [\"macros\", \"net\", \"rt-multi-thread\"] }\ntypeweld = \"0.1\"\n",
    )?;
    write(
        &root.join("server/src/main.rs"),
        r#"use serde::{Deserialize, Serialize};
use typeweld::{api, api_router, Api, ApiError, ApiRouter, Body, Created, Json, Path};

/// A user of the system.
#[derive(Clone, Debug, Deserialize, Serialize, Api)]
#[serde(rename_all = "camelCase")]
pub struct User {
    pub id: i32,
    pub display_name: String,
}

#[derive(Clone, Debug, Deserialize, Serialize, Api)]
#[serde(rename_all = "camelCase")]
pub struct CreateUser {
    pub display_name: String,
}

#[derive(Clone, Debug, Deserialize, Serialize, ApiError)]
#[serde(tag = "_tag")]
pub enum UserError {
    #[status(404)]
    UserNotFound { id: i32 },
}

/// Fetch one user by id.
#[api(get, "/users/{id}")]
pub async fn get_user(id: Path<i32>) -> Result<Json<User>, UserError> {
    Ok(Json(User {
        id: *id,
        display_name: "Ada".to_owned(),
    }))
}

/// Create a user.
#[api(post, "/users")]
pub async fn create_user(body: Body<CreateUser>) -> Result<Created<User>, UserError> {
    Ok(Created(User {
        id: 1,
        display_name: body.display_name.clone(),
    }))
}

#[api_router]
pub fn routes() -> ApiRouter {
    ApiRouter::new().endpoint(get_user).endpoint(create_user)
}

#[tokio::main]
async fn main() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:3000")
        .await
        .expect("bind");
    println!("listening on http://127.0.0.1:3000");
    axum::serve(listener, routes().into_router())
        .await
        .expect("serve");
}
"#,
    )?;

    // TypeScript app.
    write(
        &root.join("app/package.json"),
        &format!(
            "{{\n  \"name\": \"{name}-app\",\n  \"private\": true,\n  \"type\": \"module\",\n  \
             \"scripts\": {{\n    \"typecheck\": \"tsc --noEmit\"\n  }},\n  \"dependencies\": \
             {{\n    \"@typeweld/effect-runtime\": \"^0.1.0\",\n    \"effect\": \
             \"4.0.0-beta.78\"\n  }},\n  \"devDependencies\": {{\n    \"typescript\": \"^5.9\"\n  \
             }}\n}}\n"
        ),
    )?;
    write(
        &root.join("app/tsconfig.json"),
        &format!(
            "{{\n  \"compilerOptions\": {{\n    \"strict\": true,\n    \"noEmit\": true,\n    \
             \"target\": \"ES2022\",\n    \"module\": \"ESNext\",\n    \"moduleResolution\": \
             \"bundler\",\n    \"skipLibCheck\": true,\n    \"paths\": {{\n      \
             \"{ts_package}\": [\"../{package_path}/index.ts\"],\n      \"{ts_package}/*\": \
             [\"../{package_path}/*\"],\n      \"@typeweld/effect-runtime\": \
             [\"./node_modules/@typeweld/effect-runtime/src/index.ts\"],\n      \
             \"@typeweld/effect-runtime/compat\": \
             [\"./node_modules/@typeweld/effect-runtime/src/compat.ts\"],\n      \"effect\": \
             [\"./node_modules/effect\"]\n    }}\n  }},\n  \"include\": [\"src\", \
             \"../{package_path}\"]\n}}\n"
        ),
    )?;
    write(
        &root.join("app/src/main.ts"),
        &format!(
            r#"import {{ Effect }} from "effect"
import {{ getUser }} from "{ts_package}/endpoints/users"
import {{ layer }} from "{ts_package}"

const program = getUser({{ id: 1 }}).pipe(
  Effect.tap((user) => Effect.log(user.displayName)),
  Effect.provide(layer({{ baseUrl: "http://127.0.0.1:3000" }})),
)

Effect.runPromise(program).catch(console.error)
"#
        ),
    )?;

    println!("created {name}/");
    println!("next steps:");
    println!("  cd {name}");
    println!("  typeweld generate        # extract + generate the Effect client");
    println!("  cargo run -p server      # start the API server");
    println!("  cd app && npm install && npm run typecheck");
    Ok(())
}

fn write(path: &Path, contents: &str) -> anyhow::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(path, contents)?;
    Ok(())
}

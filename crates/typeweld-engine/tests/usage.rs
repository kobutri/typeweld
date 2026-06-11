//! Usage-analysis tests: extract a fixture contract, scan TypeScript app
//! sources, and verify the recorded refs, strengths, and lookups.

use std::path::{Path, PathBuf};

use typeweld_engine::extract::extract;
use typeweld_engine::ir::{Contract, SymbolId};
use typeweld_engine::usage::{scan_file, scan_files, UsageIndex, UsageKind, UsageStrength};
use typeweld_engine::vfs::MemoryFileProvider;
use typeweld_engine::workspace;

const PKG: &str = "@workspace/server-api";

const FIXTURE: &str = r#"
use serde::{Deserialize, Serialize};
use typeweld::{api, api_router, Api, ApiError, ApiRouter, Body, Json, NoContent, Path, Query};

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
#[serde(tag = "_tag", rename_all = "PascalCase")]
pub enum UserError {
    #[status(404)]
    UserNotFound { id: i32 },
    #[status(409)]
    NameTaken,
}

#[api(get, "/users/{id}")]
pub async fn get_user(id: Path<i32>) -> Result<Json<User>, UserError> {
    unimplemented!()
}

#[api(post, "/users")]
pub async fn create_user(body: Body<CreateUser>) -> Result<Json<User>, UserError> {
    unimplemented!()
}

#[api(delete, "/users/{id}")]
pub async fn delete_user(id: Path<i32>) -> Result<NoContent, UserError> {
    unimplemented!()
}

#[api_router]
pub fn routes() -> ApiRouter {
    ApiRouter::new()
        .endpoint(get_user)
        .endpoint(create_user)
        .endpoint(delete_user)
}
"#;

fn contract() -> Contract {
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
    let extraction = extract(&workspace, "server", PKG, &provider).expect("extract");
    assert!(
        !extraction.has_errors(),
        "extraction errors: {:?}",
        extraction.diagnostics
    );
    extraction.contract
}

fn endpoint_id(contract: &Contract, ts_name: &str) -> SymbolId {
    contract
        .endpoints
        .iter()
        .find(|endpoint| endpoint.ts_name == ts_name)
        .unwrap_or_else(|| panic!("no endpoint {ts_name}"))
        .id
        .clone()
}

fn variant_id(contract: &Contract, tag: &str) -> SymbolId {
    contract
        .errors
        .iter()
        .flat_map(|decl| &decl.variants)
        .find(|variant| variant.tag == tag)
        .unwrap_or_else(|| panic!("no error variant {tag}"))
        .id
        .clone()
}

fn scan(contract: &Contract, source: &str) -> UsageIndex {
    scan_files(contract, &[("app.ts".to_owned(), source.to_owned())])
}

#[test]
fn named_import_and_call_is_strong() {
    let contract = contract();
    let source = format!(
        r#"import {{ getUser }} from "{PKG}/endpoints/users"
const user = getUser({{ id: 1 }})
"#
    );
    let index = scan(&contract, &source);

    let id = endpoint_id(&contract, "getUser");
    let refs = index.refs_for(&id);
    assert_eq!(refs.len(), 2, "import ref + call ref: {refs:?}");
    assert!(refs
        .iter()
        .all(|usage| usage.kind == UsageKind::Endpoint && usage.file == "app.ts"));
    assert!(refs
        .iter()
        .any(|usage| usage.strength == UsageStrength::Strong));
    assert!(index.strongly_used_endpoints().contains(&id));
}

#[test]
fn renamed_import_and_call_is_strong() {
    let contract = contract();
    let source = format!(
        r#"import {{ getUser as fetchUser }} from "{PKG}"
fetchUser({{ id: 1 }})
"#
    );
    let index = scan(&contract, &source);

    let id = endpoint_id(&contract, "getUser");
    assert!(index.strongly_used_endpoints().contains(&id));

    // The strong ref covers the local alias at the call site.
    let strong = index
        .refs_for(&id)
        .into_iter()
        .find(|usage| usage.strength == UsageStrength::Strong)
        .expect("strong ref")
        .clone();
    assert_eq!(
        &source[strong.start as usize..strong.end as usize],
        "fetchUser"
    );
}

#[test]
fn namespace_member_chains_resolve() {
    let contract = contract();
    let source = format!(
        r#"import * as api from "{PKG}"
import {{ users }} from "{PKG}"
api.users.getUser({{ id: 1 }})
users.deleteUser({{ id: 2 }})
"#
    );
    let index = scan(&contract, &source);

    let get_user = endpoint_id(&contract, "getUser");
    let delete_user = endpoint_id(&contract, "deleteUser");
    let used = index.strongly_used_endpoints();
    assert!(used.contains(&get_user), "api.users.getUser not detected");
    assert!(used.contains(&delete_user), "users.deleteUser not detected");

    // Refs select exactly the accessor name, not the whole chain.
    for usage in index.refs_for(&get_user) {
        assert_eq!(&source[usage.start as usize..usage.end as usize], "getUser");
    }
}

#[test]
fn subpath_namespace_import_resolves() {
    let contract = contract();
    let source = format!(
        r#"import * as users from "{PKG}/endpoints/users"
export const make = () => users.createUser({{ body: {{ displayName: "x" }} }})
"#
    );
    let index = scan(&contract, &source);

    let id = endpoint_id(&contract, "createUser");
    assert!(index.strongly_used_endpoints().contains(&id));
}

#[test]
fn type_position_only_reference_is_weak() {
    let contract = contract();
    let source = format!(
        r#"import {{ getUser }} from "{PKG}/endpoints/users"
export type GetUserFn = typeof getUser
"#
    );
    let index = scan(&contract, &source);

    let id = endpoint_id(&contract, "getUser");
    let refs = index.refs_for(&id);
    assert!(!refs.is_empty(), "type-position ref not recorded");
    assert!(refs
        .iter()
        .all(|usage| usage.strength == UsageStrength::Weak));
    assert!(index.strongly_used_endpoints().is_empty());
}

#[test]
fn route_reference_is_strong() {
    let contract = contract();
    let source = format!(
        r#"import {{ getUserRoute }} from "{PKG}"
console.log(getUserRoute.path)
"#
    );
    let index = scan(&contract, &source);

    let id = endpoint_id(&contract, "getUser");
    assert!(index.strongly_used_endpoints().contains(&id));
    assert!(index
        .refs_for(&id)
        .iter()
        .any(|usage| usage.kind == UsageKind::Route && usage.strength == UsageStrength::Strong));
}

#[test]
fn catch_tag_string_literal_is_an_error_tag_usage() {
    let contract = contract();
    let source = format!(
        r#"import {{ getUser }} from "{PKG}/endpoints/users"
import {{ Effect }} from "effect"
const safe = getUser({{ id: 1 }}).pipe(
  Effect.catchTag("UserNotFound", () => Effect.succeed(null)),
  Effect.catchTags({{ NameTaken: () => Effect.succeed(null) }}),
)
"#
    );
    let index = scan(&contract, &source);

    let not_found = variant_id(&contract, "UserNotFound");
    let name_taken = variant_id(&contract, "NameTaken");
    for id in [&not_found, &name_taken] {
        let refs = index.refs_for(id);
        assert!(
            refs.iter().any(|usage| usage.kind == UsageKind::ErrorTag
                && usage.strength == UsageStrength::Strong),
            "no ErrorTag ref for {id:?}: {refs:?}"
        );
    }
}

#[test]
fn type_and_error_references_are_recorded() {
    let contract = contract();
    let source = format!(
        r#"import type {{ User }} from "{PKG}/schemas"
import {{ UserNotFound, UserError }} from "{PKG}/errors"
export const fallback = (id: number): User => {{
  throw new UserNotFound({{ id }})
}}
export type Failure = UserError
"#
    );
    let index = scan(&contract, &source);

    let user = contract
        .types
        .iter()
        .find(|decl| decl.ts_name == "User")
        .expect("User type")
        .id
        .clone();
    let union = contract
        .errors
        .iter()
        .find(|decl| decl.ts_name == "UserError")
        .expect("UserError")
        .id
        .clone();
    let variant = variant_id(&contract, "UserNotFound");

    assert!(index
        .refs_for(&user)
        .iter()
        .all(|usage| usage.kind == UsageKind::Type && usage.strength == UsageStrength::Weak));
    assert!(index
        .refs_for(&union)
        .iter()
        .any(|usage| usage.kind == UsageKind::Error));
    assert!(
        index
            .refs_for(&variant)
            .iter()
            .any(|usage| usage.kind == UsageKind::ErrorVariant
                && usage.strength == UsageStrength::Strong),
        "new UserNotFound(...) should be strong"
    );
}

#[test]
fn reexport_counts_as_weak_usage() {
    let contract = contract();
    let source = format!("export {{ getUser }} from \"{PKG}\"\n");
    let index = scan(&contract, &source);

    let id = endpoint_id(&contract, "getUser");
    let refs = index.refs_for(&id);
    assert_eq!(refs.len(), 1, "{refs:?}");
    assert_eq!(refs[0].strength, UsageStrength::Weak);
    assert!(index.strongly_used_endpoints().is_empty());
}

#[test]
fn unused_endpoints_are_absent_from_strong_set() {
    let contract = contract();
    let source = format!(
        r#"import {{ getUser, deleteUser }} from "{PKG}/endpoints/users"
getUser({{ id: 1 }})
"#
    );
    let index = scan(&contract, &source);

    let used = index.strongly_used_endpoints();
    assert!(used.contains(&endpoint_id(&contract, "getUser")));
    // Imported but never invoked: only a Weak import ref.
    assert!(!used.contains(&endpoint_id(&contract, "deleteUser")));
    // Never mentioned at all.
    assert!(!used.contains(&endpoint_id(&contract, "createUser")));
}

#[test]
fn symbol_at_resolves_offsets_inside_usages() {
    let contract = contract();
    let source = format!(
        r#"import {{ getUser }} from "{PKG}/endpoints/users"
const user = getUser({{ id: 1 }})
"#
    );
    let index = scan(&contract, &source);

    let call_start = u32::try_from(source.rfind("getUser").expect("call site")).unwrap();
    let usage = index
        .symbol_at("app.ts", call_start + 3)
        .expect("symbol at call site");
    assert_eq!(usage.symbol_id, endpoint_id(&contract, "getUser"));
    assert_eq!(usage.kind, UsageKind::Endpoint);
    assert_eq!(usage.strength, UsageStrength::Strong);

    assert!(
        index.symbol_at("app.ts", 0).is_none() || index.symbol_at("other.ts", call_start).is_none()
    );
    assert!(index.symbol_at("other.ts", call_start + 3).is_none());
}

#[test]
fn scan_file_accumulates_across_files() {
    let contract = contract();
    let mut index = UsageIndex::default();
    scan_file(
        &contract,
        "a.ts",
        &format!("import {{ getUser }} from \"{PKG}\"\ngetUser({{ id: 1 }})\n"),
        &mut index,
    );
    scan_file(
        &contract,
        "b.tsx",
        &format!("import {{ deleteUser }} from \"{PKG}\"\nexport const App = () => <button onClick={{() => deleteUser({{ id: 1 }})}} />\n"),
        &mut index,
    );

    let used = index.strongly_used_endpoints();
    assert!(used.contains(&endpoint_id(&contract, "getUser")));
    assert!(used.contains(&endpoint_id(&contract, "deleteUser")));

    let get_user_refs = index.refs_for(&endpoint_id(&contract, "getUser"));
    assert!(get_user_refs.iter().all(|usage| usage.file == "a.ts"));
}

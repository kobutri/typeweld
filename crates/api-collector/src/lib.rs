//! API contract collection.

use std::{
    collections::{BTreeMap, BTreeSet},
    path::Path,
};

use api_core::ApiModule;
use api_ir::{
    ApiContract, Endpoint, ErrorDef, Field, ResponseShape, SourceRange, SymbolId, TypeDef, TypeRef,
    TypeShape,
};
use serde::{Deserialize, Serialize};

#[must_use]
pub fn collect_empty_contract(package_name: impl Into<String>) -> ApiContract {
    ApiContract {
        package_name: package_name.into(),
        ..ApiContract::default()
    }
}

/// Explicit metadata provided to the first collector.
#[derive(Clone, Debug, Default)]
pub struct CollectorInput {
    pub package_name: String,
    pub root_module: ApiModule,
    pub types: Vec<TypeDef>,
    pub errors: Vec<ErrorDef>,
}

/// Cargo workspace summary used by build tools and diagnostics.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WorkspaceDiscovery {
    pub root: String,
    pub packages: Vec<String>,
}

/// TypeScript source file scanned for generated Effect endpoint usages.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TypeScriptSourceFile {
    pub path: String,
    pub contents: String,
}

/// Effect language-service diagnostic associated with a usage, when available.
#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize)]
pub struct EffectUsageDiagnostic {
    pub code: Option<String>,
    pub message: String,
    pub source: SourceRange,
}

/// Usage strength used by unused endpoint lints.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Deserialize, Serialize)]
pub enum UsageStrength {
    Strong,
    Weak,
    Invalid,
    Unknown,
}

/// One generated endpoint accessor reference found in TypeScript.
#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize)]
pub struct EndpointUsage {
    pub endpoint_id: SymbolId,
    pub accessor_path: Vec<String>,
    pub file: String,
    pub source: SourceRange,
    pub strength: UsageStrength,
    pub reason: String,
    pub diagnostics: Vec<EffectUsageDiagnostic>,
}

/// Per-endpoint usage counters.
#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize)]
pub struct EndpointUsageSummary {
    pub endpoint_id: SymbolId,
    pub accessor_path: Vec<String>,
    pub strong: u64,
    pub weak: u64,
    pub invalid: u64,
    pub unknown: u64,
}

/// Serialized `effect-usage-index.json` payload.
#[derive(Clone, Debug, Default, Eq, PartialEq, Deserialize, Serialize)]
pub struct EffectUsageIndex {
    pub package_name: String,
    pub endpoints: Vec<EndpointUsageSummary>,
    pub usages: Vec<EndpointUsage>,
}

#[must_use]
pub fn collect_contract(input: CollectorInput) -> ApiContract {
    let endpoints = input.root_module.endpoint_irs();
    let type_index = input
        .types
        .into_iter()
        .map(|type_def| (type_def.id.clone(), type_def))
        .collect::<BTreeMap<_, _>>();
    let error_index = input
        .errors
        .into_iter()
        .map(|error_def| (error_def.id.clone(), error_def))
        .collect::<BTreeMap<_, _>>();

    let mut needed_types = BTreeSet::new();
    let mut needed_errors = BTreeSet::new();

    for endpoint in &endpoints {
        for field in endpoint
            .request
            .path_params
            .iter()
            .chain(endpoint.request.query_params.iter())
        {
            collect_field_types(field, &mut needed_types);
        }
        if let Some(body) = &endpoint.request.body {
            collect_type_ref(body, &mut needed_types);
        }
        match &endpoint.response {
            ResponseShape::Empty => {}
            ResponseShape::Json(type_ref) | ResponseShape::Stream(type_ref) => {
                collect_type_ref(type_ref, &mut needed_types);
            }
        }
        needed_errors.extend(endpoint.errors.iter().map(|error| error.id.clone()));
    }

    let mut errors = Vec::new();
    for error_id in &needed_errors {
        if let Some(error_def) = error_index.get(error_id) {
            for variant in &error_def.variants {
                for field in &variant.fields {
                    collect_field_types(field, &mut needed_types);
                }
            }
            errors.push(error_def.clone());
        }
    }

    let mut types = Vec::new();
    let mut visited_types = BTreeSet::new();
    let mut pending = needed_types.iter().cloned().collect::<Vec<_>>();

    while let Some(type_id) = pending.pop() {
        if !visited_types.insert(type_id.clone()) {
            continue;
        }
        let Some(type_def) = type_index.get(&type_id) else {
            continue;
        };

        collect_shape_types(&type_def.shape, &mut needed_types);
        pending.extend(
            needed_types
                .iter()
                .filter(|type_id| !visited_types.contains(*type_id))
                .cloned(),
        );
        types.push(type_def.clone());
    }

    ApiContract {
        package_name: input.package_name,
        endpoints,
        types,
        errors,
    }
}

pub fn discover_workspace(
    manifest_path: impl AsRef<Path>,
) -> Result<WorkspaceDiscovery, cargo_metadata::Error> {
    let metadata = cargo_metadata::MetadataCommand::new()
        .manifest_path(manifest_path.as_ref().to_path_buf())
        .exec()?;

    Ok(WorkspaceDiscovery {
        root: metadata.workspace_root.to_string(),
        packages: metadata
            .workspace_packages()
            .iter()
            .map(|package| package.name.to_string())
            .collect(),
    })
}

pub fn contract_to_json(contract: &ApiContract) -> serde_json::Result<String> {
    serde_json::to_string_pretty(contract)
}

#[must_use]
pub fn scan_effect_usages(
    contract: &ApiContract,
    files: &[TypeScriptSourceFile],
) -> EffectUsageIndex {
    scan_effect_usages_with_diagnostics(contract, files, &[])
}

#[must_use]
pub fn scan_effect_usages_with_diagnostics(
    contract: &ApiContract,
    files: &[TypeScriptSourceFile],
    diagnostics: &[EffectUsageDiagnostic],
) -> EffectUsageIndex {
    let endpoint_accessors = contract
        .endpoints
        .iter()
        .map(EndpointAccessor::from_endpoint)
        .collect::<Vec<_>>();
    let mut usages = Vec::new();

    for file in files {
        usages.extend(scan_file_for_endpoint_usages(
            file,
            &endpoint_accessors,
            diagnostics,
        ));
    }
    usages.sort_by(|left, right| {
        left.endpoint_id
            .cmp(&right.endpoint_id)
            .then_with(|| left.file.cmp(&right.file))
            .then_with(|| left.source.start_line.cmp(&right.source.start_line))
            .then_with(|| left.source.start_column.cmp(&right.source.start_column))
    });

    let mut endpoints = endpoint_accessors
        .iter()
        .map(|accessor| {
            let mut summary = EndpointUsageSummary {
                endpoint_id: accessor.endpoint_id.clone(),
                accessor_path: accessor.accessor_path.clone(),
                strong: 0,
                weak: 0,
                invalid: 0,
                unknown: 0,
            };
            for usage in usages
                .iter()
                .filter(|usage| usage.endpoint_id == accessor.endpoint_id)
            {
                match usage.strength {
                    UsageStrength::Strong => summary.strong += 1,
                    UsageStrength::Weak => summary.weak += 1,
                    UsageStrength::Invalid => summary.invalid += 1,
                    UsageStrength::Unknown => summary.unknown += 1,
                }
            }
            summary
        })
        .collect::<Vec<_>>();
    endpoints.sort_by(|left, right| left.endpoint_id.cmp(&right.endpoint_id));

    EffectUsageIndex {
        package_name: contract.package_name.clone(),
        endpoints,
        usages,
    }
}

pub fn effect_usage_index_to_json(index: &EffectUsageIndex) -> serde_json::Result<String> {
    serde_json::to_string_pretty(index)
}

pub fn write_effect_usage_index(
    index: &EffectUsageIndex,
    path: impl AsRef<Path>,
) -> std::io::Result<()> {
    let json = effect_usage_index_to_json(index)
        .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))?;
    std::fs::write(path, json)
}

pub fn write_contract_json(contract: &ApiContract, path: impl AsRef<Path>) -> std::io::Result<()> {
    let json = contract_to_json(contract)
        .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))?;
    std::fs::write(path, json)
}

fn collect_field_types(field: &Field, needed_types: &mut BTreeSet<SymbolId>) {
    collect_type_ref(&field.type_ref, needed_types);
}

fn collect_type_ref(type_ref: &TypeRef, needed_types: &mut BTreeSet<SymbolId>) {
    needed_types.insert(type_ref.id.clone());
}

fn collect_shape_types(shape: &TypeShape, needed_types: &mut BTreeSet<SymbolId>) {
    match shape {
        TypeShape::Primitive(_) | TypeShape::External(_) => {}
        TypeShape::Struct(shape) => {
            for field in &shape.fields {
                collect_field_types(field, needed_types);
            }
        }
        TypeShape::Enum(shape) => {
            for variant in &shape.variants {
                for field in &variant.fields {
                    collect_field_types(field, needed_types);
                }
            }
        }
        TypeShape::Newtype(type_ref) | TypeShape::List(type_ref) | TypeShape::Option(type_ref) => {
            collect_type_ref(type_ref, needed_types);
        }
        TypeShape::Tuple(type_refs) => {
            for type_ref in type_refs {
                collect_type_ref(type_ref, needed_types);
            }
        }
        TypeShape::Map { key, value } => {
            collect_type_ref(key, needed_types);
            collect_type_ref(value, needed_types);
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct EndpointAccessor {
    endpoint_id: SymbolId,
    accessor_path: Vec<String>,
    namespace: String,
    function_name: String,
}

impl EndpointAccessor {
    fn from_endpoint(endpoint: &Endpoint) -> Self {
        let namespace = endpoint_namespace(endpoint);
        let function_name = endpoint_function_name(endpoint, &namespace);

        Self {
            endpoint_id: endpoint.id.clone(),
            accessor_path: vec![namespace.clone(), function_name.clone()],
            namespace,
            function_name,
        }
    }
}

fn scan_file_for_endpoint_usages(
    file: &TypeScriptSourceFile,
    accessors: &[EndpointAccessor],
    diagnostics: &[EffectUsageDiagnostic],
) -> Vec<EndpointUsage> {
    let import_aliases = import_aliases(file);
    let mut usages = Vec::new();

    for (line_index, line) in file.contents.lines().enumerate() {
        let line_number = u32::try_from(line_index + 1).unwrap_or(u32::MAX);

        for accessor in accessors {
            let mut qualifiers = BTreeSet::from([accessor.namespace.as_str()]);
            if let Some(aliases) = import_aliases.get(&accessor.namespace) {
                qualifiers.extend(aliases.iter().map(String::as_str));
            }

            for qualifier in qualifiers {
                let needle = format!("{qualifier}.{}", accessor.function_name);
                for column in match_indices(line, &needle) {
                    let source = usage_source_range(&file.path, line_number, column, needle.len());
                    let (strength, reason) = classify_usage(line, column, needle.len());
                    let diagnostics = diagnostics_for_usage(diagnostics, &file.path, &source);
                    usages.push(EndpointUsage {
                        endpoint_id: accessor.endpoint_id.clone(),
                        accessor_path: accessor.accessor_path.clone(),
                        file: file.path.clone(),
                        source,
                        strength,
                        reason,
                        diagnostics,
                    });
                }
            }
        }
    }

    add_import_only_usages(file, accessors, &import_aliases, &mut usages);
    usages
}

fn classify_usage(line: &str, column: usize, width: usize) -> (UsageStrength, String) {
    let after_reference = line.get(column + width..).unwrap_or_default().trim_start();
    let before_reference = line.get(..column).unwrap_or_default();
    let called = after_reference.starts_with('(');
    let after_call = after_reference
        .find(')')
        .and_then(|end| after_reference.get(end + 1..))
        .unwrap_or_default();
    let trimmed_before = before_reference.trim_start();

    if !called {
        return (
            UsageStrength::Weak,
            "endpoint accessor is referenced without being invoked".to_owned(),
        );
    }
    if before_reference.contains("yield*") {
        return (
            UsageStrength::Strong,
            "endpoint Effect is yielded".to_owned(),
        );
    }
    if trimmed_before.starts_with("return") {
        return (
            UsageStrength::Strong,
            "endpoint Effect is returned".to_owned(),
        );
    }
    if after_call.trim_start().starts_with(".pipe(") {
        return (
            UsageStrength::Strong,
            "endpoint Effect is composed with pipe".to_owned(),
        );
    }
    if before_reference.contains("Layer.effect(")
        || before_reference.contains("Effect.all(")
        || before_reference.contains("Stream.fromEffect(")
    {
        return (
            UsageStrength::Strong,
            "endpoint Effect is passed to an Effect combinator".to_owned(),
        );
    }
    if line[..column].contains("void ") {
        return (
            UsageStrength::Invalid,
            "endpoint Effect is explicitly discarded".to_owned(),
        );
    }

    (
        UsageStrength::Weak,
        "endpoint Effect is invoked without being yielded, returned, or composed".to_owned(),
    )
}

fn add_import_only_usages(
    file: &TypeScriptSourceFile,
    accessors: &[EndpointAccessor],
    import_aliases: &BTreeMap<String, Vec<String>>,
    usages: &mut Vec<EndpointUsage>,
) {
    for (line_index, line) in file.contents.lines().enumerate() {
        if !line.trim_start().starts_with("import ") || !line.contains(" from ") {
            continue;
        }
        let line_number = u32::try_from(line_index + 1).unwrap_or(u32::MAX);
        for accessor in accessors {
            let Some(aliases) = import_aliases.get(&accessor.namespace) else {
                continue;
            };
            if usages
                .iter()
                .any(|usage| usage.endpoint_id == accessor.endpoint_id && usage.file == file.path)
            {
                continue;
            }
            for alias in aliases {
                if let Some(column) = line.find(alias) {
                    usages.push(EndpointUsage {
                        endpoint_id: accessor.endpoint_id.clone(),
                        accessor_path: accessor.accessor_path.clone(),
                        file: file.path.clone(),
                        source: usage_source_range(&file.path, line_number, column, alias.len()),
                        strength: UsageStrength::Weak,
                        reason: "endpoint namespace is imported without a strong accessor usage"
                            .to_owned(),
                        diagnostics: Vec::new(),
                    });
                    break;
                }
            }
        }
    }
}

fn import_aliases(file: &TypeScriptSourceFile) -> BTreeMap<String, Vec<String>> {
    let mut aliases = BTreeMap::<String, Vec<String>>::new();

    for line in file.contents.lines() {
        let trimmed = line.trim();
        if !trimmed.starts_with("import ") || !trimmed.contains(" from ") {
            continue;
        }
        let Some(start) = trimmed.find('{') else {
            continue;
        };
        let Some(end) = trimmed[start + 1..].find('}') else {
            continue;
        };
        let named_imports = &trimmed[start + 1..start + 1 + end];

        for named_import in named_imports.split(',').map(str::trim) {
            if named_import.is_empty() {
                continue;
            }
            let (original, alias) = named_import
                .split_once(" as ")
                .map_or((named_import, named_import), |(original, alias)| {
                    (original.trim(), alias.trim())
                });
            if is_identifier(original) && is_identifier(alias) {
                aliases
                    .entry(original.to_owned())
                    .or_default()
                    .push(alias.to_owned());
            }
        }
    }

    aliases
}

fn match_indices(line: &str, needle: &str) -> Vec<usize> {
    let mut indices = Vec::new();
    let mut offset = 0;

    while let Some(index) = line[offset..].find(needle) {
        let index = offset + index;
        let before = index
            .checked_sub(1)
            .and_then(|index| line.as_bytes().get(index));
        let after = line.as_bytes().get(index + needle.len());
        if before.is_none_or(|byte| !is_identifier_byte(*byte))
            && after.is_none_or(|byte| !is_identifier_byte(*byte))
        {
            indices.push(index);
        }
        offset = index + needle.len();
    }

    indices
}

fn diagnostics_for_usage(
    diagnostics: &[EffectUsageDiagnostic],
    file: &str,
    source: &SourceRange,
) -> Vec<EffectUsageDiagnostic> {
    diagnostics
        .iter()
        .filter(|diagnostic| {
            diagnostic.source.file == file
                && diagnostic.source.start_line <= source.start_line
                && diagnostic.source.end_line >= source.end_line
        })
        .cloned()
        .collect()
}

fn usage_source_range(file: &str, line: u32, column: usize, width: usize) -> SourceRange {
    let start_column = u32::try_from(column + 1).unwrap_or(u32::MAX);
    let end_column = u32::try_from(column + width + 1).unwrap_or(u32::MAX);

    SourceRange {
        file: file.to_owned(),
        start_line: line,
        start_column,
        end_line: line,
        end_column,
    }
}

fn endpoint_namespace(endpoint: &Endpoint) -> String {
    endpoint
        .ts_path
        .first()
        .cloned()
        .unwrap_or_else(|| "api".to_owned())
}

fn endpoint_function_name(endpoint: &Endpoint, namespace: &str) -> String {
    endpoint
        .ts_path
        .last()
        .cloned()
        .filter(|name| name != namespace)
        .unwrap_or_else(|| endpoint.rust_name.clone())
}

fn is_identifier(value: &str) -> bool {
    let mut chars = value.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    (first == '_' || first.is_ascii_alphabetic())
        && chars.all(|character| character == '_' || character.is_ascii_alphanumeric())
}

const fn is_identifier_byte(byte: u8) -> bool {
    byte == b'_' || byte.is_ascii_alphanumeric()
}

#[cfg(test)]
mod tests {
    use api_core::{field, ir::*, ApiType, Endpoint};

    use super::*;

    #[allow(dead_code)]
    struct User {
        id: i64,
    }

    impl ApiType for User {
        const RUST_NAME: &'static str = "User";

        fn type_def() -> TypeDef {
            TypeDef {
                id: Self::type_ref().id,
                rust_path: Self::rust_path(),
                rust_name: Self::RUST_NAME.to_owned(),
                ts_name: Self::TS_NAME.to_owned(),
                shape: TypeShape::Struct(StructShape {
                    fields: vec![field::<i64>(Self::RUST_NAME, "id")],
                }),
                source: SourceRange::default(),
            }
        }
    }

    struct Unused;

    impl ApiType for Unused {
        const RUST_NAME: &'static str = "Unused";
    }

    #[test]
    fn collector_exports_reachable_endpoint_and_transitive_types() {
        let module = ApiModule::new("users").with_endpoint(
            Endpoint::new(HttpMethod::Get, "/users/{id}")
                .named(["crate", "get_user"])
                .response(ResponseShape::Json(User::type_ref())),
        );

        let contract = collect_contract(CollectorInput {
            package_name: "@workspace/server-api".to_owned(),
            root_module: module,
            types: vec![User::type_def(), Unused::type_def()],
            errors: Vec::new(),
        });

        assert_eq!(contract.endpoints.len(), 1);
        assert_eq!(contract.types.len(), 1);
        assert_eq!(contract.types[0].rust_name, "User");
    }

    #[test]
    fn collector_writes_ir_json() {
        let contract = collect_empty_contract("@workspace/server-api");
        let path = std::env::temp_dir().join(format!("api-contract-{}.json", std::process::id()));

        write_contract_json(&contract, &path).expect("write contract");
        let json = std::fs::read_to_string(&path).expect("read contract");
        let round_tripped: ApiContract = serde_json::from_str(&json).expect("parse contract");
        let _ = std::fs::remove_file(path);

        assert_eq!(round_tripped.package_name, "@workspace/server-api");
    }

    #[test]
    fn effect_usage_scanner_classifies_strong_and_weak_usages() {
        let get_user = Endpoint::new(HttpMethod::Get, "/users/{id}")
            .named(["server", "users", "get_user"])
            .ts_path(["users", "getUser"]);
        let create_user = Endpoint::new(HttpMethod::Post, "/users")
            .named(["server", "users", "create_user"])
            .ts_path(["users", "createUser"]);
        let contract = ApiContract {
            package_name: "@workspace/server-api".to_owned(),
            endpoints: vec![get_user.into_ir(), create_user.into_ir()],
            ..ApiContract::default()
        };

        let index = scan_effect_usages(
            &contract,
            &[TypeScriptSourceFile {
                path: "client/use-api.ts".to_owned(),
                contents: r#"
import { users } from "@workspace/server-api"

export const program = Effect.gen(function* () {
  yield* users.getUser({ id: "1" })
  users.createUser({ body })
  users.createUser
})
"#
                .to_owned(),
            }],
        );

        let get_user_summary = index
            .endpoints
            .iter()
            .find(|summary| summary.accessor_path == ["users", "getUser"])
            .expect("get user summary");
        let create_user_summary = index
            .endpoints
            .iter()
            .find(|summary| summary.accessor_path == ["users", "createUser"])
            .expect("create user summary");

        assert_eq!(get_user_summary.strong, 1);
        assert_eq!(get_user_summary.weak, 0);
        assert_eq!(create_user_summary.strong, 0);
        assert_eq!(create_user_summary.weak, 2);
    }

    #[test]
    fn effect_usage_scanner_counts_composed_effects_and_import_only_is_weak() {
        let get_user = Endpoint::new(HttpMethod::Get, "/users/{id}")
            .named(["server", "users", "get_user"])
            .ts_path(["users", "getUser"]);
        let list_users = Endpoint::new(HttpMethod::Get, "/users")
            .named(["server", "users", "list_users"])
            .ts_path(["users", "listUsers"]);
        let contract = ApiContract {
            package_name: "@workspace/server-api".to_owned(),
            endpoints: vec![get_user.into_ir(), list_users.into_ir()],
            ..ApiContract::default()
        };

        let index = scan_effect_usages(
            &contract,
            &[TypeScriptSourceFile {
                path: "client/use-api.ts".to_owned(),
                contents: r#"
import { users as apiUsers } from "@workspace/server-api"

const program = apiUsers.getUser({ id }).pipe(Effect.retry({ times: 1 }))
"#
                .to_owned(),
            }],
        );

        let get_user_summary = index
            .endpoints
            .iter()
            .find(|summary| summary.accessor_path == ["users", "getUser"])
            .expect("get user summary");
        let list_users_summary = index
            .endpoints
            .iter()
            .find(|summary| summary.accessor_path == ["users", "listUsers"])
            .expect("list users summary");

        assert_eq!(get_user_summary.strong, 1);
        assert_eq!(list_users_summary.strong, 0);
        assert_eq!(list_users_summary.weak, 1);
        assert!(index.usages.iter().any(|usage| {
            usage.accessor_path == ["users", "listUsers"]
                && usage.reason.contains("imported without a strong")
        }));
    }

    #[test]
    fn effect_usage_index_writes_json() {
        let contract = ApiContract {
            package_name: "@workspace/server-api".to_owned(),
            endpoints: vec![Endpoint::new(HttpMethod::Get, "/users")
                .named(["server", "users", "list_users"])
                .ts_path(["users", "listUsers"])
                .into_ir()],
            ..ApiContract::default()
        };
        let index = scan_effect_usages(&contract, &[]);
        let path =
            std::env::temp_dir().join(format!("effect-usage-index-{}.json", std::process::id()));

        write_effect_usage_index(&index, &path).expect("write usage index");
        let json = std::fs::read_to_string(&path).expect("read usage index");
        let round_tripped: EffectUsageIndex =
            serde_json::from_str(&json).expect("parse usage index");
        let _ = std::fs::remove_file(path);

        assert_eq!(round_tripped.package_name, "@workspace/server-api");
        assert_eq!(round_tripped.endpoints.len(), 1);
    }
}

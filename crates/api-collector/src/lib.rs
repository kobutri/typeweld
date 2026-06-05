//! API contract collection.

use std::{
    collections::{BTreeMap, BTreeSet},
    path::Path,
};

use api_core::ApiModule;
use api_ir::{ApiContract, ErrorDef, Field, ResponseShape, SymbolId, TypeDef, TypeRef, TypeShape};

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
}

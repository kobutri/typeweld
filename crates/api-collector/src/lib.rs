//! API contract collection.

use std::{
    collections::{BTreeMap, BTreeSet},
    path::Path,
};

use api_core::{ApiModule, IntoApiRoot};
use api_ir::{
    ApiContract, Endpoint, ErrorDef, Field, ResponseShape, SourceRange, SymbolId, TypeDef, TypeRef,
    TypeShape,
};
use serde::{Deserialize, Serialize};

pub const EFFECT_USAGE_INDEX_SCHEMA_VERSION: u32 = 2;
#[cfg(feature = "embedded-semantic-scanner")]
const SEMANTIC_USAGE_SCANNER_BUNDLE: &str =
    include_str!(concat!(env!("OUT_DIR"), "/semantic_usage_scanner.js"));

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
    package_name: String,
    root_module: ApiModule,
    types: Vec<TypeDef>,
    errors: Vec<ErrorDef>,
}

impl CollectorInput {
    #[must_use]
    pub fn from_root<R>(package_name: impl Into<String>, root: R) -> Self
    where
        R: IntoApiRoot,
    {
        Self {
            package_name: package_name.into(),
            root_module: root.into_api_module(),
            types: Vec::new(),
            errors: Vec::new(),
        }
    }
}

/// Cargo workspace summary used by build tools and diagnostics.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WorkspaceDiscovery {
    pub root: String,
    pub packages: Vec<String>,
}

/// TypeScript source file scanned for generated Effect endpoint usages.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
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

/// Configurable diagnostic-code handling for Effect-aware usage classification.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct EffectUsageDiagnosticCodeRules {
    pub invalid: BTreeSet<String>,
    pub unknown: BTreeSet<String>,
}

/// Options for semantic TypeScript/Effect usage scanning.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct EffectUsageScanConfig {
    pub diagnostic_codes: EffectUsageDiagnosticCodeRules,
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

/// One generated error symbol reference found in TypeScript.
#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize)]
pub struct ErrorUsage {
    pub symbol_id: SymbolId,
    pub file: String,
    pub source: SourceRange,
    pub reason: String,
    pub diagnostics: Vec<EffectUsageDiagnostic>,
}

/// Read/write shape of a generated API field reference.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Deserialize, Serialize)]
pub enum FieldUsageAccess {
    Read,
    Write,
}

/// One generated field reference found in TypeScript.
#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize)]
pub struct FieldUsage {
    pub field_id: SymbolId,
    pub file: String,
    pub source: SourceRange,
    pub access: FieldUsageAccess,
    pub reason: String,
    pub diagnostics: Vec<EffectUsageDiagnostic>,
}

/// Source file digest included in a persisted TypeScript usage index.
#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize)]
pub struct TypeScriptSourceSummary {
    pub path: String,
    pub hash: String,
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
    #[serde(default = "default_effect_usage_index_schema_version")]
    pub schema_version: u32,
    #[serde(default)]
    pub contract_hash: String,
    #[serde(default)]
    pub ts_program_hash: String,
    #[serde(default)]
    pub source_files: Vec<TypeScriptSourceSummary>,
    pub package_name: String,
    pub endpoints: Vec<EndpointUsageSummary>,
    pub usages: Vec<EndpointUsage>,
    #[serde(default)]
    pub error_usages: Vec<ErrorUsage>,
    #[serde(default)]
    pub field_usages: Vec<FieldUsage>,
    #[serde(default)]
    pub diagnostics: Vec<EffectUsageDiagnostic>,
}

const fn default_effect_usage_index_schema_version() -> u32 {
    EFFECT_USAGE_INDEX_SCHEMA_VERSION
}

#[must_use]
pub fn collect_contract(input: CollectorInput) -> ApiContract {
    let CollectorInput {
        package_name,
        root_module,
        types,
        errors,
    } = input;

    let endpoints = root_module.endpoint_irs();
    let mut registered_types = root_module.registry().type_defs();
    registered_types.extend(types);
    let type_index = registered_types
        .into_iter()
        .map(|type_def| (type_def.id.clone(), type_def))
        .collect::<BTreeMap<_, _>>();
    let mut registered_errors = root_module.registry().error_defs();
    registered_errors.extend(errors);
    let error_index = registered_errors
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
            ResponseShape::Empty | ResponseShape::Binary { .. } => {}
            ResponseShape::Json(type_ref)
            | ResponseShape::Created(type_ref)
            | ResponseShape::Stream(type_ref) => {
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
        package_name,
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
    try_scan_effect_usages(contract, files).expect("semantic TypeScript usage scan failed")
}

pub fn try_scan_effect_usages(
    contract: &ApiContract,
    files: &[TypeScriptSourceFile],
) -> Result<EffectUsageIndex, String> {
    try_scan_effect_usages_with_diagnostics(contract, files, &[])
}

#[must_use]
pub fn scan_effect_usages_with_diagnostics(
    contract: &ApiContract,
    files: &[TypeScriptSourceFile],
    diagnostics: &[EffectUsageDiagnostic],
) -> EffectUsageIndex {
    try_scan_effect_usages_with_diagnostics(contract, files, diagnostics)
        .expect("semantic TypeScript usage scan failed")
}

pub fn try_scan_effect_usages_with_diagnostics(
    contract: &ApiContract,
    files: &[TypeScriptSourceFile],
    diagnostics: &[EffectUsageDiagnostic],
) -> Result<EffectUsageIndex, String> {
    try_scan_effect_usages_with_config(
        contract,
        files,
        diagnostics,
        &EffectUsageScanConfig::default(),
    )
}

pub fn try_scan_effect_usages_with_config(
    contract: &ApiContract,
    files: &[TypeScriptSourceFile],
    diagnostics: &[EffectUsageDiagnostic],
    config: &EffectUsageScanConfig,
) -> Result<EffectUsageIndex, String> {
    let endpoint_accessors = contract
        .endpoints
        .iter()
        .map(EndpointAccessor::from_endpoint)
        .collect::<Vec<_>>();
    let contract_hash = hash_contract(contract)?;
    let source_files = source_file_summaries(files);
    let ts_program_hash = hash_ts_program(&source_files);
    let mut usages = Vec::new();
    let semantic_output = semantic_endpoint_references(contract, files, &endpoint_accessors)?;
    let mut all_diagnostics = semantic_output.diagnostics;
    all_diagnostics.extend(diagnostics.iter().cloned());

    for reference in semantic_output.references {
        let diagnostics =
            diagnostics_for_usage(&all_diagnostics, &reference.file, &reference.source);
        let (strength, reason) = diagnostic_classification_override(
            reference.strength,
            reference.reason,
            &diagnostics,
            &config.diagnostic_codes,
        );
        usages.push(EndpointUsage {
            endpoint_id: reference.endpoint_id,
            accessor_path: reference.accessor_path,
            file: reference.file,
            source: reference.source,
            strength,
            reason,
            diagnostics,
        });
    }
    let error_usages = semantic_output
        .error_references
        .into_iter()
        .map(|reference| {
            let diagnostics =
                diagnostics_for_usage(&all_diagnostics, &reference.file, &reference.source);
            ErrorUsage {
                symbol_id: reference.symbol_id,
                file: reference.file,
                source: reference.source,
                reason: reference.reason,
                diagnostics,
            }
        })
        .collect::<Vec<_>>();
    let field_usages = semantic_output
        .field_references
        .into_iter()
        .map(|reference| {
            let diagnostics =
                diagnostics_for_usage(&all_diagnostics, &reference.file, &reference.source);
            FieldUsage {
                field_id: reference.field_id,
                file: reference.file,
                source: reference.source,
                access: reference.access,
                reason: reference.reason,
                diagnostics,
            }
        })
        .collect::<Vec<_>>();

    for file in files {
        let import_aliases = import_aliases(file);
        add_import_only_usages(file, &endpoint_accessors, &import_aliases, &mut usages);
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

    Ok(EffectUsageIndex {
        schema_version: EFFECT_USAGE_INDEX_SCHEMA_VERSION,
        contract_hash,
        ts_program_hash,
        source_files,
        package_name: contract.package_name.clone(),
        endpoints,
        usages,
        error_usages,
        field_usages,
        diagnostics: all_diagnostics,
    })
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

fn hash_contract(contract: &ApiContract) -> Result<String, String> {
    serde_json::to_vec(contract)
        .map(|bytes| stable_hash(&bytes))
        .map_err(|error| format!("failed to hash API contract: {error}"))
}

fn source_file_summaries(files: &[TypeScriptSourceFile]) -> Vec<TypeScriptSourceSummary> {
    let mut summaries = files
        .iter()
        .map(|file| TypeScriptSourceSummary {
            path: file.path.clone(),
            hash: stable_hash(file.contents.as_bytes()),
        })
        .collect::<Vec<_>>();
    summaries.sort_by(|left, right| left.path.cmp(&right.path));
    summaries
}

fn hash_ts_program(sources: &[TypeScriptSourceSummary]) -> String {
    let mut bytes = Vec::new();
    for source in sources {
        bytes.extend_from_slice(source.path.as_bytes());
        bytes.push(0);
        bytes.extend_from_slice(source.hash.as_bytes());
        bytes.push(0);
    }
    stable_hash(&bytes)
}

fn stable_hash(bytes: &[u8]) -> String {
    const FNV_OFFSET_BASIS: u64 = 0xcbf2_9ce4_8422_2325;
    const FNV_PRIME: u64 = 0x0000_0100_0000_01b3;

    let mut hash = FNV_OFFSET_BASIS;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(FNV_PRIME);
    }
    format!("fnv1a64:{hash:016x}")
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
    transport: api_ir::Transport,
    request: api_ir::RequestShape,
    response: ResponseShape,
    errors: Vec<api_ir::ErrorRef>,
}

impl EndpointAccessor {
    fn from_endpoint(endpoint: &Endpoint) -> Self {
        let namespace = endpoint_namespace(endpoint);
        let function_name = endpoint_function_name(endpoint, &namespace);

        Self {
            endpoint_id: endpoint.id.clone(),
            accessor_path: vec![namespace.clone(), function_name.clone()],
            namespace,
            transport: endpoint.transport,
            request: endpoint.request.clone(),
            response: endpoint.response.clone(),
            errors: endpoint.errors.clone(),
        }
    }
}

#[derive(Serialize)]
struct SemanticScannerInput<'a> {
    package_name: &'a str,
    endpoints: Vec<SemanticScannerEndpoint<'a>>,
    types: &'a [TypeDef],
    errors: Vec<SemanticScannerError<'a>>,
    files: &'a [TypeScriptSourceFile],
}

#[derive(Serialize)]
struct SemanticScannerEndpoint<'a> {
    endpoint_id: &'a SymbolId,
    accessor_path: &'a [String],
    transport: api_ir::Transport,
    request: &'a api_ir::RequestShape,
    response: &'a ResponseShape,
    errors: &'a [api_ir::ErrorRef],
}

#[derive(Serialize)]
struct SemanticScannerError<'a> {
    error_id: &'a SymbolId,
    ts_name: &'a str,
    variants: Vec<SemanticScannerErrorVariant<'a>>,
}

#[derive(Serialize)]
struct SemanticScannerErrorVariant<'a> {
    variant_id: &'a SymbolId,
    tag_symbol_id: SymbolId,
    rust_name: &'a str,
    tag: &'a str,
    fields: &'a [Field],
}

#[derive(Deserialize)]
struct SemanticScannerOutput {
    references: Vec<SemanticEndpointReference>,
    error_references: Vec<SemanticErrorReference>,
    field_references: Vec<SemanticFieldReference>,
    diagnostics: Vec<EffectUsageDiagnostic>,
}

#[derive(Deserialize)]
struct SemanticEndpointReference {
    endpoint_id: SymbolId,
    accessor_path: Vec<String>,
    file: String,
    source: SourceRange,
    strength: UsageStrength,
    reason: String,
}

#[derive(Deserialize)]
struct SemanticErrorReference {
    symbol_id: SymbolId,
    file: String,
    source: SourceRange,
    reason: String,
}

#[derive(Deserialize)]
struct SemanticFieldReference {
    field_id: SymbolId,
    file: String,
    source: SourceRange,
    access: FieldUsageAccess,
    reason: String,
}

fn semantic_endpoint_references(
    contract: &ApiContract,
    files: &[TypeScriptSourceFile],
    accessors: &[EndpointAccessor],
) -> Result<SemanticScannerOutput, String> {
    if files.is_empty() {
        return Ok(SemanticScannerOutput {
            references: Vec::new(),
            error_references: Vec::new(),
            field_references: Vec::new(),
            diagnostics: Vec::new(),
        });
    }

    let input = SemanticScannerInput {
        package_name: &contract.package_name,
        endpoints: accessors
            .iter()
            .map(|accessor| SemanticScannerEndpoint {
                endpoint_id: &accessor.endpoint_id,
                accessor_path: &accessor.accessor_path,
                transport: accessor.transport,
                request: &accessor.request,
                response: &accessor.response,
                errors: &accessor.errors,
            })
            .collect(),
        types: &contract.types,
        errors: contract
            .errors
            .iter()
            .map(|error| SemanticScannerError {
                error_id: &error.id,
                ts_name: &error.ts_name,
                variants: error
                    .variants
                    .iter()
                    .map(|variant| SemanticScannerErrorVariant {
                        variant_id: &variant.id,
                        tag_symbol_id: error_tag_symbol_id(variant),
                        rust_name: &variant.rust_name,
                        tag: &variant.tag,
                        fields: &variant.fields,
                    })
                    .collect(),
            })
            .collect(),
        files,
    };
    let input = serde_json::to_vec(&input)
        .map_err(|error| format!("failed to serialize semantic TypeScript usage input: {error}"))?;

    let output = run_semantic_usage_scanner(&input)?;

    let output: SemanticScannerOutput = serde_json::from_str(&output).map_err(|error| {
        format!(
            "failed to parse semantic TypeScript usage scanner output: {error}\nstdout:\n{}",
            output
        )
    })?;
    Ok(output)
}

#[cfg(feature = "embedded-semantic-scanner")]
fn run_semantic_usage_scanner(input: &[u8]) -> Result<String, String> {
    let input = std::str::from_utf8(input)
        .map_err(|error| format!("semantic TypeScript usage input was not UTF-8: {error}"))?;
    let mut runtime = deno_core::JsRuntime::new(deno_core::RuntimeOptions::default());
    runtime
        .execute_script(
            "semantic_usage_scanner_bundle.js",
            SEMANTIC_USAGE_SCANNER_BUNDLE,
        )
        .map_err(|error| format!("failed to initialize semantic TypeScript scanner: {error}"))?;

    let input_argument = serde_json::to_string(input)
        .map_err(|error| format!("failed to quote semantic TypeScript scanner input: {error}"))?;
    let script = format!("globalThis.__semanticUsageScanner.scan({input_argument})");
    let value = runtime
        .execute_script("semantic_usage_scanner_call.js", script)
        .map_err(|error| format!("semantic TypeScript usage scanner failed: {error}"))?;

    deno_core::scope!(scope, runtime);
    let local = deno_core::v8::Local::new(scope, value);
    deno_core::serde_v8::from_v8::<String>(scope, local)
        .map_err(|error| format!("semantic TypeScript scanner returned invalid output: {error}"))
}

#[cfg(not(feature = "embedded-semantic-scanner"))]
fn run_semantic_usage_scanner(_input: &[u8]) -> Result<String, String> {
    Err(
        "semantic TypeScript usage scanning requires the `embedded-semantic-scanner` feature"
            .to_owned(),
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
        if trimmed.starts_with("import type ") {
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

fn diagnostic_classification_override(
    strength: UsageStrength,
    reason: String,
    diagnostics: &[EffectUsageDiagnostic],
    rules: &EffectUsageDiagnosticCodeRules,
) -> (UsageStrength, String) {
    if let Some(diagnostic) = diagnostics
        .iter()
        .find(|diagnostic| diagnostic_code_matches(diagnostic, &rules.invalid))
    {
        return (
            UsageStrength::Invalid,
            diagnostic_override_reason("invalid", diagnostic),
        );
    }

    if let Some(diagnostic) = diagnostics
        .iter()
        .find(|diagnostic| diagnostic_code_matches(diagnostic, &rules.unknown))
    {
        return (
            UsageStrength::Unknown,
            diagnostic_override_reason("unknown", diagnostic),
        );
    }

    (strength, reason)
}

fn diagnostic_code_matches(diagnostic: &EffectUsageDiagnostic, codes: &BTreeSet<String>) -> bool {
    diagnostic
        .code
        .as_ref()
        .is_some_and(|code| codes.contains(code))
}

fn diagnostic_override_reason(classification: &str, diagnostic: &EffectUsageDiagnostic) -> String {
    match &diagnostic.code {
        Some(code) => format!(
            "Effect diagnostic {code} marks endpoint usage {classification}: {}",
            diagnostic.message
        ),
        None => format!(
            "Effect diagnostic marks endpoint usage {classification}: {}",
            diagnostic.message
        ),
    }
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
        full_range: None,
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

fn error_tag_symbol_id(variant: &api_ir::ErrorVariant) -> SymbolId {
    SymbolId::from_parts("error_tag", &[variant.id.as_str()])
}

fn is_identifier(value: &str) -> bool {
    let mut chars = value.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    (first == '_' || first.is_ascii_alphabetic())
        && chars.all(|character| character == '_' || character.is_ascii_alphanumeric())
}

#[cfg(test)]
mod tests {
    use api_core::{
        field, ir::*, ApiRouteNode, ApiRouteTree, ApiType, ContractRegistry, Endpoint,
        EndpointDescriptor, MountedEndpoint,
    };

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

    fn noop_register(_registry: &mut ContractRegistry) {}

    fn register_user_and_unused(registry: &mut ContractRegistry) {
        registry.register_type::<User>();
        registry.register_type::<Unused>();
    }

    fn route_tree_with_descriptor(descriptor: EndpointDescriptor) -> ApiRouteTree {
        ApiRouteTree {
            module_name: "users".to_owned(),
            nodes: vec![ApiRouteNode::Endpoint(MountedEndpoint {
                descriptor,
                source: SourceRange::default(),
                effective_method: HttpMethod::Get,
                effective_path: "/users/{id}".to_owned(),
            })],
        }
    }

    #[test]
    fn collector_exports_reachable_endpoint_and_registered_types() {
        let descriptor = EndpointDescriptor::new(
            Endpoint::new(HttpMethod::Get, "/users/{id}")
                .named(["crate", "get_user"])
                .response(ResponseShape::Json(User::type_ref())),
            register_user_and_unused,
        );
        let root = route_tree_with_descriptor(descriptor);

        let contract = collect_contract(CollectorInput::from_root("@workspace/server-api", root));

        assert_eq!(contract.endpoints.len(), 1);
        assert_eq!(contract.types.len(), 1);
        assert_eq!(contract.types[0].rust_name, "User");
    }

    #[test]
    fn collector_filters_registered_types_to_reachable_shapes() {
        let descriptor = EndpointDescriptor::new(
            Endpoint::new(HttpMethod::Get, "/users/{id}")
                .named(["crate", "get_user"])
                .response(ResponseShape::Json(User::type_ref())),
            register_user_and_unused,
        );
        let root = route_tree_with_descriptor(descriptor);

        let contract = collect_contract(CollectorInput::from_root("@workspace/server-api", root));

        assert_eq!(contract.types.len(), 1);
        assert_eq!(contract.types[0].rust_name, "User");
    }

    #[test]
    fn collector_input_accepts_route_tree_roots() {
        let descriptor = EndpointDescriptor::new(
            Endpoint::new(HttpMethod::Get, "/users/{id}").named(["crate", "get_user"]),
            noop_register,
        );
        let child = ApiRouteTree {
            module_name: "users".to_owned(),
            nodes: vec![ApiRouteNode::Endpoint(MountedEndpoint {
                descriptor,
                source: SourceRange::default(),
                effective_method: HttpMethod::Get,
                effective_path: "/users/{id}".to_owned(),
            })],
        };
        let root = ApiRouteTree {
            module_name: "server".to_owned(),
            nodes: vec![ApiRouteNode::Nest {
                path: "/api".to_owned(),
                source: SourceRange::default(),
                child: Box::new(child),
            }],
        };

        let contract = collect_contract(CollectorInput::from_root("@workspace/server-api", root));

        assert_eq!(contract.endpoints.len(), 1);
        assert_eq!(contract.endpoints[0].route.0, "/api/users/{id}");
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
import { Effect } from "effect"
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
    fn effect_usage_scanner_classifies_returned_and_composed_effects_semantically() {
        let get_user = Endpoint::new(HttpMethod::Get, "/users/{id}")
            .named(["server", "users", "get_user"])
            .ts_path(["users", "getUser"]);
        let contract = ApiContract {
            package_name: "@workspace/server-api".to_owned(),
            endpoints: vec![get_user.into_ir()],
            ..ApiContract::default()
        };

        let index = scan_effect_usages(
            &contract,
            &[TypeScriptSourceFile {
                path: "client/use-api.ts".to_owned(),
                contents: r#"
import { Effect, Layer, Stream } from "effect"
import { users } from "@workspace/server-api"

declare const Service: any
declare const UnknownCombinator: { use: (input: unknown) => unknown }

export const returned = () => users.getUser({ id: "1" })
export const piped = users.getUser({ id: "2" }).pipe(Effect.retry({ times: 1 }))
export const all = Effect.all([users.getUser({ id: "3" })])
export const layer = Layer.effect(Service, users.getUser({ id: "4" }))
export const stream = Stream.fromEffect(users.getUser({ id: "5" }))

UnknownCombinator.use(users.getUser({ id: "6" }))
"#
                .to_owned(),
            }],
        );

        let summary = index
            .endpoints
            .iter()
            .find(|summary| summary.accessor_path == ["users", "getUser"])
            .expect("get user summary");

        assert_eq!(summary.strong, 5);
        assert_eq!(summary.weak, 1);
        assert_eq!(summary.invalid, 0);
        assert_eq!(summary.unknown, 0);
        assert!(index.usages.iter().any(|usage| {
            usage.strength == UsageStrength::Strong && usage.reason.contains("returned")
        }));
        assert!(index.usages.iter().any(|usage| {
            usage.strength == UsageStrength::Strong && usage.reason.contains("pipe")
        }));
        assert!(index.usages.iter().any(|usage| {
            usage.strength == UsageStrength::Weak
                && usage
                    .reason
                    .contains("without being yielded, returned, or composed")
        }));
    }

    #[test]
    fn effect_usage_scanner_resolves_semantic_aliases_and_reexports() {
        let get_user = Endpoint::new(HttpMethod::Get, "/users/{id}")
            .named(["server", "users", "get_user"])
            .ts_path(["users", "getUser"]);
        let contract = ApiContract {
            package_name: "@workspace/server-api".to_owned(),
            endpoints: vec![get_user.into_ir()],
            ..ApiContract::default()
        };

        let index = scan_effect_usages(
            &contract,
            &[
                TypeScriptSourceFile {
                    path: "client/api.ts".to_owned(),
                    contents: r#"
export { users as apiUsers } from "@workspace/server-api"
"#
                    .to_owned(),
                },
                TypeScriptSourceFile {
                    path: "client/use-api.ts".to_owned(),
                    contents: r#"
import { apiUsers } from "./api"
import { users as directUsers } from "@workspace/server-api"

const unrelated = { getUser: () => "not an endpoint" }
const { getUser } = apiUsers
const wrapped = directUsers.getUser

export const program = Effect.gen(function* () {
  yield* getUser({ id: "1" })
  return wrapped({ id: "2" })
})

unrelated.getUser()
"#
                    .to_owned(),
                },
            ],
        );

        let summary = index
            .endpoints
            .iter()
            .find(|summary| summary.accessor_path == ["users", "getUser"])
            .expect("get user summary");

        assert_eq!(summary.strong, 2);
        assert_eq!(summary.weak, 1);
        assert_eq!(summary.invalid, 0);
        assert_eq!(summary.unknown, 0);
        assert_eq!(index.usages.len(), 3);
        assert!(index
            .usages
            .iter()
            .any(|usage| usage.file == "client/use-api.ts"
                && usage.source.start_line == 10
                && usage.strength == UsageStrength::Strong));
        assert!(index
            .usages
            .iter()
            .any(|usage| usage.file == "client/use-api.ts"
                && usage.source.start_line == 11
                && usage.strength == UsageStrength::Strong));
        assert!(!index
            .usages
            .iter()
            .any(|usage| usage.source.start_line == 14));
    }

    #[test]
    fn effect_usage_scanner_ignores_type_only_and_unrelated_property_names() {
        let get_user = Endpoint::new(HttpMethod::Get, "/users/{id}")
            .named(["server", "users", "get_user"])
            .ts_path(["users", "getUser"]);
        let contract = ApiContract {
            package_name: "@workspace/server-api".to_owned(),
            endpoints: vec![get_user.into_ir()],
            ..ApiContract::default()
        };

        let index = scan_effect_usages(
            &contract,
            &[TypeScriptSourceFile {
                path: "client/use-api.ts".to_owned(),
                contents: r#"
import type { users } from "@workspace/server-api"

type Getter = typeof users.getUser
const unrelated = { getUser: () => "not an endpoint" }

unrelated.getUser()
"#
                .to_owned(),
            }],
        );

        let summary = index
            .endpoints
            .iter()
            .find(|summary| summary.accessor_path == ["users", "getUser"])
            .expect("get user summary");

        assert_eq!(summary.strong, 0);
        assert_eq!(summary.weak, 0);
        assert_eq!(summary.invalid, 0);
        assert_eq!(summary.unknown, 0);
        assert!(index.usages.is_empty());
    }

    #[test]
    fn effect_usage_scanner_attaches_typescript_backend_diagnostics() {
        let get_user = Endpoint::new(HttpMethod::Get, "/users/{id}")
            .named(["server", "users", "get_user"])
            .ts_path(["users", "getUser"]);
        let contract = ApiContract {
            package_name: "@workspace/server-api".to_owned(),
            endpoints: vec![get_user.into_ir()],
            ..ApiContract::default()
        };

        let index = scan_effect_usages(
            &contract,
            &[TypeScriptSourceFile {
                path: "client/use-api.ts".to_owned(),
                contents: r#"
import { Effect } from "effect"
import { users } from "@workspace/server-api"

export const program = Effect.gen(function* () {
  yield* users.getUser
})
"#
                .to_owned(),
            }],
        );

        let usage = index.usages.first().expect("usage");

        assert_eq!(usage.strength, UsageStrength::Weak);
        assert!(!usage.diagnostics.is_empty());
        assert!(usage
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code.is_some()));
    }

    #[test]
    fn effect_usage_scanner_applies_configured_diagnostic_codes() {
        let get_user = Endpoint::new(HttpMethod::Get, "/users/{id}")
            .named(["server", "users", "get_user"])
            .ts_path(["users", "getUser"]);
        let contract = ApiContract {
            package_name: "@workspace/server-api".to_owned(),
            endpoints: vec![get_user.into_ir()],
            ..ApiContract::default()
        };
        let diagnostics = vec![EffectUsageDiagnostic {
            code: Some("effect/no-floating".to_owned()),
            message: "floating Effect is not live".to_owned(),
            source: usage_source_range("client/use-api.ts", 3, 2, 30),
        }];
        let config = EffectUsageScanConfig {
            diagnostic_codes: EffectUsageDiagnosticCodeRules {
                invalid: std::collections::BTreeSet::from(["effect/no-floating".to_owned()]),
                unknown: std::collections::BTreeSet::new(),
            },
        };

        let index = try_scan_effect_usages_with_config(
            &contract,
            &[TypeScriptSourceFile {
                path: "client/use-api.ts".to_owned(),
                contents: "import { users } from \"@workspace/server-api\"\n\nusers.getUser({ id: \"1\" })\n"
                    .to_owned(),
            }],
            &diagnostics,
            &config,
        )
        .expect("scan usages");
        let summary = index
            .endpoints
            .iter()
            .find(|summary| summary.accessor_path == ["users", "getUser"])
            .expect("get user summary");
        let usage = index.usages.first().expect("usage");

        assert_eq!(summary.strong, 0);
        assert_eq!(summary.weak, 0);
        assert_eq!(summary.invalid, 1);
        assert_eq!(usage.strength, UsageStrength::Invalid);
        assert!(usage.reason.contains("effect/no-floating"));
        assert_eq!(usage.diagnostics.len(), 1);
    }

    #[test]
    fn effect_usage_scanner_indexes_error_handlers_and_typed_fields() {
        let contract = api_test_fixtures::basic_contract();

        let index = scan_effect_usages(
            &contract,
            &[TypeScriptSourceFile {
                path: "client/use-api.ts".to_owned(),
                contents: r#"
import { Effect } from "effect"
import { users } from "@workspace/server-api"

export const program = Effect.gen(function* () {
  const user = yield* users.getUser({ id: 1 })
  const name = user.displayName
  const unrelated = { displayName: "not typed" }
  unrelated.displayName
  return user.displayName
}).pipe(
  Effect.catchTag("UserNotFound", (error) => Effect.succeed(error.id)),
  Effect.catchTags({
    PermissionDenied: () => Effect.succeed(undefined),
  }),
)
"#
                .to_owned(),
            }],
        );

        let display_name = SymbolId::new("fixture:field:User:displayName");
        let route_id = SymbolId::new("fixture:field:getUser:id");
        let error_id = SymbolId::new("fixture:field:GetUserError:UserNotFound:id");
        let user_not_found = SymbolId::new("fixture:error:GetUserError:UserNotFound");
        let permission_denied = SymbolId::new("fixture:error:GetUserError:PermissionDenied");

        assert_eq!(
            index
                .field_usages
                .iter()
                .filter(|usage| usage.field_id == display_name)
                .count(),
            2
        );
        assert!(index.field_usages.iter().any(|usage| {
            usage.field_id == route_id && usage.access == FieldUsageAccess::Write
        }));
        assert!(index
            .field_usages
            .iter()
            .any(|usage| { usage.field_id == error_id && usage.access == FieldUsageAccess::Read }));
        assert!(index
            .error_usages
            .iter()
            .any(|usage| usage.symbol_id == user_not_found));
        assert!(index
            .error_usages
            .iter()
            .any(|usage| usage.symbol_id == permission_denied));
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
        assert_eq!(
            round_tripped.schema_version,
            EFFECT_USAGE_INDEX_SCHEMA_VERSION
        );
        assert!(round_tripped.contract_hash.starts_with("fnv1a64:"));
        assert!(round_tripped.ts_program_hash.starts_with("fnv1a64:"));
        assert_eq!(round_tripped.endpoints.len(), 1);
    }
}

//! Effect v4 TypeScript generator backend.

use std::{
    collections::BTreeSet,
    path::{Path, PathBuf},
};

use api_ir::{
    ApiContract, Endpoint, EnumShape, EnumVariant, ErrorDef, ErrorVariant, ExternalType, Field,
    Optionality, Primitive, RequestShape, ResponseShape, SourceRange, SourceSpan, StructShape,
    SymbolId, Transport, TypeDef, TypeRef, TypeShape,
};
use serde::Serialize;

pub const EFFECT_VERSION: &str = "4.0.0-beta.78";
const EFFECT_COMPAT_IMPORT: &str = "@rust-ts-integration/effect-runtime/compat";

#[must_use]
pub fn render_package_banner(contract: &ApiContract) -> String {
    format!("// Generated API package for {}\n", contract.package_name)
}

/// Renders generated Effect Schema declarations for every exported API type.
#[must_use]
pub fn render_schemas(contract: &ApiContract) -> String {
    render_tracked_schemas(contract).file.contents
}

/// Renders schema-backed domain and generated client errors.
#[must_use]
pub fn render_errors(contract: &ApiContract) -> String {
    render_tracked_errors(contract).file.contents
}

/// Renders generated endpoint accessors and route metadata.
#[must_use]
pub fn render_endpoints(contract: &ApiContract) -> String {
    render_tracked_endpoints(contract).file.contents
}

/// Renders the generated Effect service tag, service interface, and layer helpers.
#[must_use]
pub fn render_layer(contract: &ApiContract) -> String {
    render_tracked_layer(contract).file.contents
}

/// In-memory representation of the hidden generated TypeScript package.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GeneratedPackage {
    pub package_dir: PathBuf,
    pub files: Vec<GeneratedFile>,
    pub marks: Vec<GeneratedMark>,
    pub tsconfig_paths: TsconfigPaths,
}

/// A generated file path relative to [`GeneratedPackage::package_dir`].
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GeneratedFile {
    pub path: String,
    pub contents: String,
}

/// A generated TypeScript range associated with a Rust API symbol.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GeneratedMark {
    pub path: String,
    pub symbol_id: String,
    pub kind: String,
    pub range: GeneratedRange,
}

/// Zero-based UTF-16 range in a generated TypeScript file.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct GeneratedRange {
    pub start: GeneratedPosition,
    pub end: GeneratedPosition,
}

/// Zero-based UTF-16 position in a generated TypeScript file.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct GeneratedPosition {
    pub line: u32,
    pub character: u32,
}

/// TypeScript path mapping metadata for a generated API package.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TsconfigPaths {
    pub package_name: String,
    pub package_dir: PathBuf,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct TrackedFile {
    file: GeneratedFile,
    marks: Vec<GeneratedMark>,
}

#[derive(Debug)]
struct TrackedWriter {
    path: String,
    contents: String,
    marks: Vec<GeneratedMark>,
}

impl TrackedWriter {
    fn new(path: &str) -> Self {
        Self {
            path: path.to_owned(),
            contents: String::new(),
            marks: Vec::new(),
        }
    }

    fn push(&mut self, text: &str) {
        self.contents.push_str(text);
    }

    fn mark(&mut self, symbol_id: &SymbolId, kind: &str, text: &str) {
        let start = self.contents.len();
        self.contents.push_str(text);
        let end = self.contents.len();
        self.marks.push(GeneratedMark {
            path: self.path.clone(),
            symbol_id: symbol_id.as_str().to_owned(),
            kind: kind.to_owned(),
            range: generated_range_for_offsets(&self.contents, start, end),
        });
    }

    fn mark_synthetic(&mut self, symbol_id: String, kind: &str, text: &str) {
        let start = self.contents.len();
        self.contents.push_str(text);
        let end = self.contents.len();
        self.marks.push(GeneratedMark {
            path: self.path.clone(),
            symbol_id,
            kind: kind.to_owned(),
            range: generated_range_for_offsets(&self.contents, start, end),
        });
    }

    fn into_tracked_file(self) -> TrackedFile {
        TrackedFile {
            file: GeneratedFile {
                path: self.path,
                contents: trim_trailing_blank_lines(self.contents),
            },
            marks: self.marks,
        }
    }
}

fn write_effect_compat_import(writer: &mut TrackedWriter, imports: &str) {
    writer.push("import { ");
    writer.push(imports);
    writer.push(" } from \"");
    writer.push(EFFECT_COMPAT_IMPORT);
    writer.push("\"\n");
}

/// Builds the generated package files and resolver metadata without writing them.
#[must_use]
pub fn render_generated_package(contract: &ApiContract, target_dir: &Path) -> GeneratedPackage {
    let package_dir = generated_package_dir(target_dir, &contract.package_name);
    let schemas = render_tracked_schemas(contract);
    let errors = render_tracked_errors(contract);
    let endpoints = render_tracked_endpoints(contract);
    let layer = render_tracked_layer(contract);
    let mut marks = Vec::new();
    marks.extend(schemas.marks.clone());
    marks.extend(errors.marks.clone());
    marks.extend(endpoints.marks.clone());
    marks.extend(layer.marks.clone());
    marks.sort_by(|left, right| {
        left.path
            .cmp(&right.path)
            .then_with(|| left.kind.cmp(&right.kind))
            .then_with(|| left.symbol_id.cmp(&right.symbol_id))
            .then_with(|| left.range.start.line.cmp(&right.range.start.line))
            .then_with(|| left.range.start.character.cmp(&right.range.start.character))
    });

    let files = vec![
        GeneratedFile {
            path: "package.json".to_owned(),
            contents: render_package_json(contract),
        },
        GeneratedFile {
            path: "index.ts".to_owned(),
            contents: render_package_index(contract),
        },
        schemas.file,
        errors.file,
        endpoints.file,
        layer.file,
        GeneratedFile {
            path: "tsconfig.paths.json".to_owned(),
            contents: render_tsconfig_paths(&TsconfigPaths {
                package_name: contract.package_name.clone(),
                package_dir: package_dir.clone(),
            }),
        },
    ];

    GeneratedPackage {
        tsconfig_paths: TsconfigPaths {
            package_name: contract.package_name.clone(),
            package_dir: package_dir.clone(),
        },
        package_dir,
        files,
        marks,
    }
}

/// Renders the cross-language symbol graph consumed by `api-ls` and build lints.
#[must_use]
pub fn render_symbol_graph(contract: &ApiContract, package: &GeneratedPackage) -> String {
    let graph = SymbolGraph::from_generated_package(contract, package);
    serde_json::to_string_pretty(&graph).expect("symbol graph serialization should not fail")
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct SymbolGraph {
    symbols: Vec<LinkedSymbol>,
}

impl SymbolGraph {
    fn from_generated_package(contract: &ApiContract, package: &GeneratedPackage) -> Self {
        let mut symbols = Vec::new();

        for endpoint in &contract.endpoints {
            symbols.push(endpoint_symbol(contract, package, endpoint));
            for field in endpoint.request.path_params.iter() {
                symbols.push(field_symbol(
                    "routeParam",
                    package,
                    field,
                    path_with(&endpoint.rust_path, &[&field.rust_name]),
                    path_with(&endpoint.ts_path, &[&field.ts_name]),
                ));
            }
            for field in &endpoint.request.query_params {
                symbols.push(field_symbol(
                    "queryParam",
                    package,
                    field,
                    path_with(&endpoint.rust_path, &[&field.rust_name]),
                    path_with(&endpoint.ts_path, &[&field.ts_name]),
                ));
            }
        }

        for type_def in &contract.types {
            symbols.push(type_symbol(package, type_def));
            collect_type_shape_symbols(package, type_def, &mut symbols);
        }

        for error in &contract.errors {
            symbols.push(error_type_symbol(package, error));
            for variant in &error.variants {
                symbols.push(error_variant_symbol(package, error, variant));
                symbols.push(error_tag_symbol(package, error, variant));
                let class_name = error_variant_class_name(error, variant);
                for field in &variant.fields {
                    symbols.push(field_symbol(
                        "errorField",
                        package,
                        field,
                        path_with(&error.rust_path, &[&variant.rust_name, &field.rust_name]),
                        path_from(&[&class_name, &field.ts_name]),
                    ));
                }
            }
        }

        symbols.sort_by(|left, right| {
            left.kind
                .cmp(&right.kind)
                .then_with(|| left.id.cmp(&right.id))
        });
        Self { symbols }
    }
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct LinkedSymbol {
    id: String,
    kind: String,
    rust: GraphLocation,
    #[serde(default)]
    typescript: Vec<GraphLocation>,
    #[serde(default)]
    metadata: SymbolMetadata,
}

#[derive(Clone, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
struct SymbolMetadata {
    #[serde(skip_serializing_if = "Option::is_none")]
    method: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    route: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    effect_signature: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    errors: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    rust_path: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    ts_path: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    usage_count: Option<u64>,
    #[serde(default, skip_serializing_if = "is_false")]
    allow_unused: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    rename_placeholder: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    reserved_names: Vec<String>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct GraphLocation {
    #[serde(skip_serializing_if = "Option::is_none")]
    file: Option<String>,
    range: GraphRange,
    #[serde(skip_serializing_if = "Option::is_none")]
    name_range: Option<GraphRange>,
    #[serde(skip_serializing_if = "Option::is_none")]
    full_range: Option<GraphRange>,
    #[serde(default, skip_serializing_if = "is_false")]
    generated: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    renamable: Option<bool>,
}

#[derive(Clone, Copy, Debug, Serialize)]
struct GraphRange {
    start: GraphPosition,
    end: GraphPosition,
}

#[derive(Clone, Copy, Debug, Serialize)]
struct GraphPosition {
    line: u32,
    character: u32,
}

fn endpoint_symbol(
    contract: &ApiContract,
    package: &GeneratedPackage,
    endpoint: &Endpoint,
) -> LinkedSymbol {
    let function_name = endpoint_function_name(endpoint);
    LinkedSymbol {
        id: endpoint.id.as_str().to_owned(),
        kind: "endpoint".to_owned(),
        rust: rust_location(&endpoint.source, Some(true)),
        typescript: generated_locations(package, endpoint.id.as_str(), "endpoint"),
        metadata: SymbolMetadata {
            method: Some(endpoint.method.as_str().to_owned()),
            route: Some(endpoint.route.0.clone()),
            effect_signature: Some(render_endpoint_return_type(endpoint, "ServerApi")),
            errors: endpoint
                .errors
                .iter()
                .map(|error| error.name.clone())
                .collect(),
            rust_path: endpoint.rust_path.clone(),
            ts_path: endpoint.ts_path.clone(),
            usage_count: Some(0),
            allow_unused: endpoint.allow_unused,
            rename_placeholder: Some(function_name),
            reserved_names: sibling_endpoint_names(contract, endpoint),
        },
    }
}

fn type_symbol(package: &GeneratedPackage, type_def: &TypeDef) -> LinkedSymbol {
    LinkedSymbol {
        id: type_def.id.as_str().to_owned(),
        kind: "type".to_owned(),
        rust: rust_location(&type_def.source, Some(true)),
        typescript: generated_locations(package, type_def.id.as_str(), "type"),
        metadata: SymbolMetadata {
            rust_path: type_def.rust_path.clone(),
            ts_path: vec![type_def.ts_name.clone()],
            rename_placeholder: Some(type_def.ts_name.clone()),
            ..SymbolMetadata::default()
        },
    }
}

fn error_type_symbol(package: &GeneratedPackage, error: &ErrorDef) -> LinkedSymbol {
    LinkedSymbol {
        id: error.id.as_str().to_owned(),
        kind: "error".to_owned(),
        rust: rust_location(&error.source, Some(true)),
        typescript: generated_locations(package, error.id.as_str(), "error"),
        metadata: SymbolMetadata {
            rust_path: error.rust_path.clone(),
            ts_path: vec![error.ts_name.clone()],
            rename_placeholder: Some(error.ts_name.clone()),
            ..SymbolMetadata::default()
        },
    }
}

fn error_variant_symbol(
    package: &GeneratedPackage,
    error: &ErrorDef,
    variant: &ErrorVariant,
) -> LinkedSymbol {
    let class_name = error_variant_class_name(error, variant);
    LinkedSymbol {
        id: variant.id.as_str().to_owned(),
        kind: "errorVariant".to_owned(),
        rust: rust_location(&variant.source, Some(true)),
        typescript: generated_locations(package, variant.id.as_str(), "errorVariant"),
        metadata: SymbolMetadata {
            rust_path: error
                .rust_path
                .iter()
                .cloned()
                .chain(std::iter::once(variant.rust_name.clone()))
                .collect(),
            ts_path: vec![class_name.clone()],
            rename_placeholder: Some(class_name),
            ..SymbolMetadata::default()
        },
    }
}

fn error_tag_symbol(
    package: &GeneratedPackage,
    error: &ErrorDef,
    variant: &ErrorVariant,
) -> LinkedSymbol {
    let class_name = error_variant_class_name(error, variant);
    let id = error_tag_symbol_id(variant);
    LinkedSymbol {
        id: id.clone(),
        kind: "errorTag".to_owned(),
        rust: rust_location(&variant.source, Some(true)),
        typescript: generated_locations(package, &id, "errorTag"),
        metadata: SymbolMetadata {
            rust_path: path_with(&error.rust_path, &[&variant.rust_name]),
            ts_path: path_from(&[&class_name, &variant.tag]),
            rename_placeholder: Some(variant.tag.clone()),
            ..SymbolMetadata::default()
        },
    }
}

fn field_symbol(
    kind: &str,
    package: &GeneratedPackage,
    field: &Field,
    rust_path: Vec<String>,
    ts_path: Vec<String>,
) -> LinkedSymbol {
    LinkedSymbol {
        id: field.id.as_str().to_owned(),
        kind: kind.to_owned(),
        rust: rust_location(&field.source, Some(true)),
        typescript: generated_locations(package, field.id.as_str(), kind),
        metadata: SymbolMetadata {
            rust_path,
            ts_path,
            rename_placeholder: Some(field.ts_name.clone()),
            ..SymbolMetadata::default()
        },
    }
}

fn collect_type_shape_symbols(
    package: &GeneratedPackage,
    type_def: &TypeDef,
    symbols: &mut Vec<LinkedSymbol>,
) {
    match &type_def.shape {
        TypeShape::Struct(StructShape { fields }) => {
            for field in fields {
                symbols.push(field_symbol(
                    "field",
                    package,
                    field,
                    path_with(&type_def.rust_path, &[&field.rust_name]),
                    path_from(&[&type_def.ts_name, &field.ts_name]),
                ));
            }
        }
        TypeShape::Enum(EnumShape { variants }) => {
            for variant in variants {
                symbols.push(enum_variant_symbol(package, type_def, variant));
                for field in &variant.fields {
                    symbols.push(field_symbol(
                        "field",
                        package,
                        field,
                        path_with(&type_def.rust_path, &[&variant.rust_name, &field.rust_name]),
                        path_from(&[&type_def.ts_name, &variant.wire_name, &field.ts_name]),
                    ));
                }
            }
        }
        TypeShape::Primitive(_)
        | TypeShape::Newtype(_)
        | TypeShape::Tuple(_)
        | TypeShape::List(_)
        | TypeShape::Map { .. }
        | TypeShape::Option(_)
        | TypeShape::External(_) => {}
    }
}

fn enum_variant_symbol(
    package: &GeneratedPackage,
    type_def: &TypeDef,
    variant: &EnumVariant,
) -> LinkedSymbol {
    LinkedSymbol {
        id: variant.id.as_str().to_owned(),
        kind: "enumVariant".to_owned(),
        rust: rust_location(&variant.source, Some(true)),
        typescript: generated_locations(package, variant.id.as_str(), "enumVariant"),
        metadata: SymbolMetadata {
            rust_path: path_with(&type_def.rust_path, &[&variant.rust_name]),
            ts_path: path_from(&[&type_def.ts_name, &variant.wire_name]),
            rename_placeholder: Some(variant.wire_name.clone()),
            ..SymbolMetadata::default()
        },
    }
}

fn sibling_endpoint_names(contract: &ApiContract, endpoint: &Endpoint) -> Vec<String> {
    let namespace = endpoint_namespace(endpoint);
    let mut names = contract
        .endpoints
        .iter()
        .filter(|candidate| {
            candidate.id != endpoint.id && endpoint_namespace(candidate) == namespace
        })
        .map(endpoint_function_name)
        .collect::<Vec<_>>();
    names.sort();
    names.dedup();
    names
}

fn path_from(parts: &[&str]) -> Vec<String> {
    parts.iter().map(|part| (*part).to_owned()).collect()
}

fn path_with(base: &[String], additions: &[&str]) -> Vec<String> {
    base.iter()
        .cloned()
        .chain(additions.iter().map(|addition| (*addition).to_owned()))
        .collect()
}

fn generated_locations(
    package: &GeneratedPackage,
    symbol_id: &str,
    kind: &str,
) -> Vec<GraphLocation> {
    package
        .marks
        .iter()
        .filter(|mark| mark.symbol_id == symbol_id && mark.kind == kind)
        .map(|mark| {
            let range = graph_range_from_generated(mark.range);
            GraphLocation {
                file: Some(package.package_dir.join(&mark.path).display().to_string()),
                range,
                name_range: Some(range),
                full_range: Some(range),
                generated: true,
                renamable: Some(true),
            }
        })
        .collect()
}

fn error_tag_symbol_id(variant: &ErrorVariant) -> String {
    SymbolId::from_parts("error_tag", &[variant.id.as_str()])
        .as_str()
        .to_owned()
}

fn rust_location(source: &SourceRange, renamable: Option<bool>) -> GraphLocation {
    let range = range_from_source(source);
    let full_range = source
        .full_range
        .as_ref()
        .map(range_from_source_span)
        .unwrap_or(range);
    GraphLocation {
        file: (!source.file.is_empty()).then(|| source.file.clone()),
        range,
        name_range: Some(range),
        full_range: Some(full_range),
        generated: false,
        renamable,
    }
}

fn range_from_source(source: &SourceRange) -> GraphRange {
    GraphRange {
        start: GraphPosition {
            line: source.start_line.saturating_sub(1),
            character: source.start_column.saturating_sub(1),
        },
        end: GraphPosition {
            line: source.end_line.saturating_sub(1),
            character: source.end_column.saturating_sub(1),
        },
    }
}

fn range_from_source_span(source: &SourceSpan) -> GraphRange {
    GraphRange {
        start: GraphPosition {
            line: source.start_line.saturating_sub(1),
            character: source.start_column.saturating_sub(1),
        },
        end: GraphPosition {
            line: source.end_line.saturating_sub(1),
            character: source.end_column.saturating_sub(1),
        },
    }
}

fn graph_range_from_generated(range: GeneratedRange) -> GraphRange {
    GraphRange {
        start: GraphPosition {
            line: range.start.line,
            character: range.start.character,
        },
        end: GraphPosition {
            line: range.end.line,
            character: range.end.character,
        },
    }
}

fn generated_range_for_offsets(contents: &str, start: usize, end: usize) -> GeneratedRange {
    GeneratedRange {
        start: generated_position_for_offset(contents, start),
        end: generated_position_for_offset(contents, end),
    }
}

fn generated_position_for_offset(contents: &str, offset: usize) -> GeneratedPosition {
    let position = position_for_offset(contents, offset);
    GeneratedPosition {
        line: position.line,
        character: position.character,
    }
}

fn position_for_offset(contents: &str, offset: usize) -> GraphPosition {
    let mut line = 0_u32;
    let mut character = 0_u32;
    for (index, ch) in contents.char_indices() {
        if index >= offset {
            break;
        }
        if ch == '\n' {
            line += 1;
            character = 0;
        } else {
            character += u32::try_from(ch.len_utf16()).unwrap_or(1);
        }
    }
    GraphPosition { line, character }
}

const fn is_false(value: &bool) -> bool {
    !*value
}

/// Cache path convention for hidden generated packages.
#[must_use]
pub fn generated_package_dir(target_dir: &Path, package_name: &str) -> PathBuf {
    target_dir
        .join("api-contract")
        .join("effect-v4")
        .join("packages")
        .join(sanitize_package_dir_name(package_name))
}

/// Renders the generated package manifest.
#[must_use]
pub fn render_package_json(contract: &ApiContract) -> String {
    format!(
        "{{\n  \"name\": {name},\n  \"version\": \"0.0.0\",\n  \"private\": true,\n  \"type\": \"module\",\n  \"types\": \"./index.ts\",\n  \"exports\": {{\n    \".\": \"./index.ts\",\n    \"./schemas\": \"./schemas.ts\",\n    \"./errors\": \"./errors.ts\",\n    \"./endpoints\": \"./endpoints.ts\",\n    \"./layer\": \"./layer.ts\"\n  }},\n  \"dependencies\": {{\n    \"@rust-ts-integration/effect-runtime\": \"0.0.0\",\n    \"effect\": {effect_version}\n  }}\n}}\n",
        name = ts_string(&contract.package_name),
        effect_version = ts_string(EFFECT_VERSION),
    )
}

/// Renders the generated public package barrel.
#[must_use]
pub fn render_package_index(contract: &ApiContract) -> String {
    let mut output = render_package_banner(contract);
    output.push_str("export * from \"./schemas\"\n");
    output.push_str("export * from \"./errors\"\n");
    output.push_str("export * from \"./endpoints\"\n");
    output.push_str("export * from \"./layer\"\n");
    output
}

/// Renders the `compilerOptions.paths` snippet for importing the hidden package.
#[must_use]
pub fn render_tsconfig_paths(paths: &TsconfigPaths) -> String {
    let package_dir = normalize_path(&paths.package_dir);
    format!(
        "{{\n  \"compilerOptions\": {{\n    \"paths\": {{\n      {package_name}: [\n        {index_path}\n      ],\n      {package_glob}: [\n        {package_dir_glob}\n      ]\n    }}\n  }}\n}}\n",
        package_name = ts_string(&paths.package_name),
        index_path = ts_string(&format!("{package_dir}/index.ts")),
        package_glob = ts_string(&format!("{}/*", paths.package_name)),
        package_dir_glob = ts_string(&format!("{package_dir}/*")),
    )
}

fn render_tracked_schemas(contract: &ApiContract) -> TrackedFile {
    let mut writer = TrackedWriter::new("schemas.ts");
    writer.push(&render_package_banner(contract));
    write_effect_compat_import(&mut writer, "Schema");
    writer.push("\n");

    let mut types = contract.types.iter().collect::<Vec<_>>();
    types.sort_by(|left, right| {
        left.ts_name
            .cmp(&right.ts_name)
            .then_with(|| left.id.as_str().cmp(right.id.as_str()))
    });

    for type_def in types {
        write_tracked_type_def(&mut writer, type_def);
        writer.push("\n");
    }

    writer.into_tracked_file()
}

fn write_tracked_type_def(writer: &mut TrackedWriter, type_def: &TypeDef) {
    writer.push("export const ");
    writer.mark(&type_def.id, "type", &type_def.ts_name);
    writer.push(" = ");
    write_tracked_type_shape(writer, &type_def.shape, type_def, 0);
    writer.push("\nexport type ");
    writer.mark(&type_def.id, "type", &type_def.ts_name);
    writer.push(" = Schema.Schema.Type<typeof ");
    writer.push(&type_def.ts_name);
    writer.push(">\nexport type ");
    writer.mark(
        &type_def.id,
        "type",
        &format!("{}Encoded", type_def.ts_name),
    );
    writer.push(" = Schema.Codec.Encoded<typeof ");
    writer.push(&type_def.ts_name);
    writer.push(">\n");
}

fn write_tracked_type_shape(
    writer: &mut TrackedWriter,
    shape: &TypeShape,
    owner: &TypeDef,
    indent: usize,
) {
    match shape {
        TypeShape::Struct(shape) => write_tracked_struct(writer, shape, indent),
        TypeShape::Enum(shape) => write_tracked_enum(writer, shape, indent),
        TypeShape::Primitive(_)
        | TypeShape::Newtype(_)
        | TypeShape::Tuple(_)
        | TypeShape::List(_)
        | TypeShape::Map { .. }
        | TypeShape::Option(_)
        | TypeShape::External(_) => writer.push(&render_type_shape(shape, owner)),
    }
}

fn write_tracked_struct(writer: &mut TrackedWriter, shape: &StructShape, indent: usize) {
    if shape.fields.is_empty() {
        writer.push("Schema.Struct({})");
        return;
    }

    writer.push("Schema.Struct({\n");
    for field in &shape.fields {
        writer.push(&render_indent(indent + 2));
        writer.mark(&field.id, "field", &field.ts_name);
        writer.push(": ");
        writer.push(&render_field_schema(field));
        writer.push(",\n");
    }
    writer.push(&render_indent(indent));
    writer.push("})");
}

fn write_tracked_enum(writer: &mut TrackedWriter, shape: &EnumShape, indent: usize) {
    if shape.variants.is_empty() {
        writer.push("Schema.Never");
        return;
    }

    writer.push("Schema.Union([");
    for (index, variant) in shape.variants.iter().enumerate() {
        if index > 0 {
            writer.push(", ");
        }
        write_tracked_enum_variant(writer, variant, indent);
    }
    writer.push("])");
}

fn write_tracked_enum_variant(writer: &mut TrackedWriter, variant: &EnumVariant, indent: usize) {
    if variant.fields.is_empty() {
        writer.push("Schema.Struct({ _tag: Schema.Literal(\"");
        writer.mark(&variant.id, "enumVariant", &variant.wire_name);
        writer.push("\") })");
        return;
    }

    writer.push("Schema.Struct({\n");
    writer.push(&render_indent(indent + 2));
    writer.push("_tag: Schema.Literal(\"");
    writer.mark(&variant.id, "enumVariant", &variant.wire_name);
    writer.push("\"),\n");
    for field in &variant.fields {
        writer.push(&render_indent(indent + 2));
        writer.mark(&field.id, "field", &field.ts_name);
        writer.push(": ");
        writer.push(&render_field_schema(field));
        writer.push(",\n");
    }
    writer.push(&render_indent(indent));
    writer.push("})");
}

fn render_tracked_endpoints(contract: &ApiContract) -> TrackedFile {
    let mut writer = TrackedWriter::new("endpoints.ts");
    writer.push(&render_package_banner(contract));
    if contract_has_stream_endpoints(contract) {
        write_effect_compat_import(&mut writer, "Effect, Stream");
    } else {
        write_effect_compat_import(&mut writer, "Effect");
    }
    writer.push("import { ServerApi } from \"./layer\"\n");

    let schema_imports = collect_endpoint_schema_imports(contract);
    if !schema_imports.is_empty() {
        writer.push("import type { ");
        writer.push(
            &schema_imports
                .iter()
                .map(String::as_str)
                .collect::<Vec<_>>()
                .join(", "),
        );
        writer.push(" } from \"./schemas\"\n");
    }

    let error_imports = collect_endpoint_error_imports(contract);
    writer.push("import type { ");
    writer.push(
        &error_imports
            .iter()
            .map(String::as_str)
            .collect::<Vec<_>>()
            .join(", "),
    );
    writer.push(" } from \"./errors\"\n\n");

    let mut endpoints = contract.endpoints.iter().collect::<Vec<_>>();
    endpoints.sort_by(|left, right| {
        endpoint_namespace(left)
            .cmp(&endpoint_namespace(right))
            .then_with(|| endpoint_function_name(left).cmp(&endpoint_function_name(right)))
            .then_with(|| left.id.as_str().cmp(right.id.as_str()))
    });

    let mut namespace = None::<String>;
    for endpoint in endpoints {
        let current_namespace = endpoint_namespace(endpoint);
        if namespace.as_deref() != Some(current_namespace.as_str()) {
            if namespace.is_some() {
                writer.push("}\n\n");
            }
            writer.push("export namespace ");
            writer.push(&current_namespace);
            writer.push(" {\n");
            namespace = Some(current_namespace);
        }

        write_tracked_endpoint(&mut writer, endpoint);
    }

    if namespace.is_some() {
        writer.push("}\n");
    }

    writer.into_tracked_file()
}

fn write_tracked_endpoint(writer: &mut TrackedWriter, endpoint: &Endpoint) {
    let function_name = endpoint_function_name(endpoint);
    let args_name = endpoint_args_name(endpoint);
    let namespace = endpoint_namespace(endpoint);

    write_tracked_endpoint_args(writer, endpoint);
    writer.push("\n  export const ");
    writer.mark(&endpoint.id, "endpoint", &format!("{function_name}Route"));
    writer.push(" = {\n    method: ");
    writer.push(&ts_string(endpoint.method.as_str()));
    writer.push(",\n    path: ");
    writer.push(&ts_string(&endpoint.route.0));
    writer.push(",\n    transport: ");
    writer.push(&ts_string(render_transport(endpoint.transport)));
    writer.push(",\n  } as const\n\n  export const ");
    writer.mark(&endpoint.id, "endpoint", &function_name);
    writer.push(" = (\n    args: ");
    writer.push(&args_name);
    writer.push("\n  ): ");
    writer.push(&render_endpoint_return_type(endpoint, "ServerApi"));
    writer.push(" =>\n    ServerApi.use((api) => api.");
    writer.push(&namespace);
    writer.push(".");
    writer.mark(&endpoint.id, "endpoint", &function_name);
    writer.push("(args))\n\n");
}

fn write_tracked_endpoint_args(writer: &mut TrackedWriter, endpoint: &Endpoint) {
    let args_name = endpoint_args_name(endpoint);
    let mut has_fields = false;
    writer.push("  export interface ");
    writer.push(&args_name);
    if endpoint.request.path_params.is_empty()
        && endpoint.request.query_params.is_empty()
        && endpoint.request.body.is_none()
    {
        writer.push(" {}\n");
        return;
    }

    writer.push(" {\n");
    for field in &endpoint.request.path_params {
        has_fields = true;
        writer.push("    readonly ");
        mark_property_key(writer, &field.id, "routeParam", &field.ts_name);
        if matches!(
            field.optionality,
            Optionality::Optional | Optionality::OptionalNullable
        ) {
            writer.push("?");
        }
        writer.push(": ");
        writer.push(&render_endpoint_arg_field_type(field));
        writer.push(";\n");
    }
    for field in &endpoint.request.query_params {
        has_fields = true;
        writer.push("    readonly ");
        mark_property_key(writer, &field.id, "queryParam", &field.ts_name);
        if matches!(
            field.optionality,
            Optionality::Optional | Optionality::OptionalNullable
        ) {
            writer.push("?");
        }
        writer.push(": ");
        writer.push(&render_endpoint_arg_field_type(field));
        writer.push(";\n");
    }
    if let Some(body) = &endpoint.request.body {
        has_fields = true;
        writer.push("    readonly body: ");
        writer.push(&render_ts_type_ref(body));
        writer.push(";\n");
    }
    if !has_fields {
        writer.push("  }\n");
        return;
    }
    writer.push("  }\n");
}

fn render_tracked_errors(contract: &ApiContract) -> TrackedFile {
    let mut writer = TrackedWriter::new("errors.ts");
    writer.push(&render_package_banner(contract));
    write_effect_compat_import(&mut writer, "Schema");

    let schema_imports = collect_error_schema_imports(contract);
    if !schema_imports.is_empty() {
        writer.push("import { ");
        writer.push(
            &schema_imports
                .iter()
                .map(String::as_str)
                .collect::<Vec<_>>()
                .join(", "),
        );
        writer.push(" } from \"./schemas\"\n");
    }
    writer.push("\n");
    writer.push(&render_client_errors());

    let mut errors = contract.errors.iter().collect::<Vec<_>>();
    errors.sort_by(|left, right| {
        left.ts_name
            .cmp(&right.ts_name)
            .then_with(|| left.id.as_str().cmp(right.id.as_str()))
    });

    for error in errors {
        writer.push("\n");
        write_tracked_error_def(&mut writer, error);
    }

    writer.into_tracked_file()
}

fn write_tracked_error_def(writer: &mut TrackedWriter, error: &ErrorDef) {
    for variant in &error.variants {
        write_tracked_error_variant(writer, error, variant);
        writer.push("\n");
    }

    writer.push("export type ");
    writer.mark(&error.id, "error", &error.ts_name);
    writer.push(" =\n");
    if error.variants.is_empty() {
        writer.push("  never\n");
    } else {
        for variant in &error.variants {
            writer.push("  | ");
            writer.mark(
                &variant.id,
                "errorVariant",
                &error_variant_class_name(error, variant),
            );
            writer.push("\n");
        }
    }

    writer.push("\n");
    write_tracked_error_status_metadata(writer, error);
    writer.push("\n");
    write_tracked_error_status_by_code_metadata(writer, error);
    writer.push("\n");
    write_tracked_error_schema_by_status_metadata(writer, error);
    writer.push("\n");
    write_tracked_error_symbol_metadata(writer, error);
}

fn write_tracked_error_variant(
    writer: &mut TrackedWriter,
    error: &ErrorDef,
    variant: &ErrorVariant,
) {
    let fields = variant
        .fields
        .iter()
        .map(|field| (field, render_field_schema(field)))
        .collect::<Vec<_>>();
    let class_name = error_variant_class_name(error, variant);
    writer.push("export class ");
    writer.mark(&variant.id, "errorVariant", &class_name);
    writer.push(" extends Schema.TaggedErrorClass<");
    writer.push(&class_name);
    writer.push(">()(\n  ");
    mark_ts_string(
        writer,
        error_tag_symbol_id(variant),
        "errorTag",
        &variant.tag,
    );
    writer.push(",\n");
    if fields.is_empty() {
        writer.push("  {}\n");
    } else {
        writer.push("  {\n");
        for (field, schema) in fields {
            writer.push("    ");
            mark_property_key(writer, &field.id, "errorField", &field.ts_name);
            writer.push(": ");
            writer.push(&schema);
            writer.push(",\n");
        }
        writer.push("  }\n");
    }
    writer.push(") {}\n");
}

fn write_tracked_error_status_metadata(writer: &mut TrackedWriter, error: &ErrorDef) {
    writer.push("export const ");
    writer.mark(&error.id, "error", &format!("{}Status", error.ts_name));
    writer.push(" = {\n");
    for variant in &error.variants {
        writer.push("  ");
        mark_property_key(
            writer,
            &SymbolId::new(error_tag_symbol_id(variant)),
            "errorTag",
            &variant.tag,
        );
        writer.push(": ");
        writer.push(&variant.status.0.to_string());
        writer.push(",\n");
    }
    writer.push("} as const\n");
}

fn write_tracked_error_status_by_code_metadata(writer: &mut TrackedWriter, error: &ErrorDef) {
    writer.push("export const ");
    writer.mark(
        &error.id,
        "error",
        &format!("{}StatusByCode", error.ts_name),
    );
    writer.push(" = {\n");
    for variant in &error.variants {
        writer.push("  ");
        writer.push(&variant.status.0.to_string());
        writer.push(": ");
        mark_ts_string(
            writer,
            error_tag_symbol_id(variant),
            "errorTag",
            &variant.tag,
        );
        writer.push(",\n");
    }
    writer.push("} as const\n");
}

fn write_tracked_error_schema_by_status_metadata(writer: &mut TrackedWriter, error: &ErrorDef) {
    writer.push("export const ");
    writer.mark(
        &error.id,
        "error",
        &format!("{}SchemaByStatus", error.ts_name),
    );
    writer.push(" = {\n");
    for variant in &error.variants {
        writer.push("  ");
        writer.push(&variant.status.0.to_string());
        writer.push(": ");
        writer.mark(
            &variant.id,
            "errorVariant",
            &error_variant_class_name(error, variant),
        );
        writer.push(",\n");
    }
    writer.push("} as const\n");
}

fn write_tracked_error_symbol_metadata(writer: &mut TrackedWriter, error: &ErrorDef) {
    writer.push("export const ");
    writer.mark(&error.id, "error", &format!("{}Symbols", error.ts_name));
    writer.push(" = {\n");
    for variant in &error.variants {
        writer.push("  ");
        mark_property_key(
            writer,
            &SymbolId::new(error_tag_symbol_id(variant)),
            "errorTag",
            &variant.tag,
        );
        writer.push(": {\n");
        writer.push("    symbolId: ");
        writer.push(&ts_string(variant.id.as_str()));
        writer.push(",\n    rustName: ");
        writer.push(&ts_string(&variant.rust_name));
        writer.push(",\n    source: ");
        writer.push(&render_source_range(&variant.source, 4));
        writer.push(",\n  },\n");
    }
    writer.push("} as const\n");
}

fn render_tracked_layer(contract: &ApiContract) -> TrackedFile {
    let mut writer = TrackedWriter::new("layer.ts");
    writer.push(&render_package_banner(contract));
    if contract_has_stream_endpoints(contract) {
        write_effect_compat_import(&mut writer, "Context, Layer, Effect, Stream");
    } else {
        write_effect_compat_import(&mut writer, "Context, Layer, Effect");
    }
    let runtime_client_imports = collect_runtime_client_imports(contract);
    if !runtime_client_imports.is_empty() {
        writer.push("import { ");
        writer.push(
            &runtime_client_imports
                .iter()
                .map(String::as_str)
                .collect::<Vec<_>>()
                .join(", "),
        );
        writer.push(" } from \"@rust-ts-integration/effect-runtime\"\n");
    }

    let endpoint_namespaces = collect_endpoint_namespaces(contract);
    if !endpoint_namespaces.is_empty() {
        writer.push("import { ");
        writer.push(
            &endpoint_namespaces
                .iter()
                .map(String::as_str)
                .collect::<Vec<_>>()
                .join(", "),
        );
        writer.push(" } from \"./endpoints\"\n");
    }

    let schema_imports = collect_service_schema_imports(contract);
    if !schema_imports.is_empty() {
        writer.push("import { ");
        writer.push(
            &schema_imports
                .iter()
                .map(String::as_str)
                .collect::<Vec<_>>()
                .join(", "),
        );
        writer.push(" } from \"./schemas\"\n");
    }

    let error_metadata_imports = collect_endpoint_error_metadata_imports(contract);
    if !error_metadata_imports.is_empty() {
        writer.push("import { ");
        writer.push(
            &error_metadata_imports
                .iter()
                .map(String::as_str)
                .collect::<Vec<_>>()
                .join(", "),
        );
        writer.push(" } from \"./errors\"\n");
    }

    let error_type_imports = collect_endpoint_error_imports(contract);
    writer.push("import type { ");
    writer.push(
        &error_type_imports
            .iter()
            .map(String::as_str)
            .collect::<Vec<_>>()
            .join(", "),
    );
    writer.push(" } from \"./errors\"\n\n");

    writer.push("export interface ServerApiConfig {\n");
    writer.push("  readonly baseUrl: string\n");
    writer.push("  readonly timeoutMs?: number\n");
    writer.push("  readonly fetch?: typeof fetch\n");
    writer.push("}\n\n");

    writer.push(&format!(
        "export class ServerApi extends Context.Service<ServerApi, ServerApi.Service>()({}) {{}}\n\n",
        ts_string(&format!("{}::ServerApi", contract.package_name))
    ));

    writer.push("export namespace ServerApi {\n");
    write_tracked_service_interface(&mut writer, contract);
    writer.push("\n");
    writer.push("  export const layer = (config: ServerApiConfig): Layer.Layer<ServerApi> => {\n");
    writer.push("    const service: Service = {\n");
    write_tracked_fetch_service(&mut writer, contract);
    writer.push("    }\n");
    writer.push("    return Layer.succeed(ServerApi, ServerApi.of(service))\n");
    writer.push("  }\n\n");
    writer.push("  export const mock = (service: Service): Layer.Layer<ServerApi> =>\n");
    writer.push("    Layer.succeed(ServerApi, ServerApi.of(service))\n");
    writer.push("}\n");

    writer.into_tracked_file()
}

fn write_tracked_service_interface(writer: &mut TrackedWriter, contract: &ApiContract) {
    writer.push("  export interface Service {\n");
    let grouped = group_endpoints_by_namespace(contract);

    for (namespace, endpoints) in grouped {
        writer.push("    readonly ");
        writer.push(&namespace);
        writer.push(": {\n");
        for endpoint in endpoints {
            writer.push("      readonly ");
            writer.mark(&endpoint.id, "endpoint", &endpoint_function_name(endpoint));
            writer.push(": (args: ");
            writer.push(&endpoint_namespace(endpoint));
            writer.push(".");
            writer.push(&endpoint_args_name(endpoint));
            writer.push(") => ");
            writer.push(&render_endpoint_return_type(endpoint, "never"));
            writer.push("\n");
        }
        writer.push("    }\n");
    }

    writer.push("  }\n");
}

fn write_tracked_fetch_service(writer: &mut TrackedWriter, contract: &ApiContract) {
    let grouped = group_endpoints_by_namespace(contract);

    for (namespace, endpoints) in grouped {
        writer.push("      ");
        writer.push(&namespace);
        writer.push(": {\n");
        for endpoint in endpoints {
            write_tracked_fetch_service_method(writer, endpoint);
        }
        writer.push("      },\n");
    }
}

fn write_tracked_fetch_service_method(writer: &mut TrackedWriter, endpoint: &Endpoint) {
    let function_name = endpoint_function_name(endpoint);
    let namespace = endpoint_namespace(endpoint);
    let args_name = endpoint_args_name(endpoint);
    let helper = render_runtime_client_helper(endpoint);
    let success_decoder = render_success_decoder(&endpoint.response, helper);
    let error_decoder = render_domain_error_decoder(endpoint, helper);
    let success = render_response_type(&endpoint.response);
    let domain_error = render_endpoint_domain_error_type(endpoint);

    writer.push("        ");
    writer.mark(&endpoint.id, "endpoint", &function_name);
    writer.push(": ");
    writer.push(helper);
    writer.push("<");
    writer.push(&namespace);
    writer.push(".");
    writer.push(&args_name);
    writer.push(", ");
    writer.push(&success);
    writer.push(", ");
    writer.push(&domain_error);
    writer.push(">(config, {\n          method: ");
    writer.push(&ts_string(endpoint.method.as_str()));
    writer.push(",\n          path: ");
    writer.push(&namespace);
    writer.push(".");
    writer.push(&function_name);
    writer.push("Route.path,\n          encode: ");
    writer.push(&render_request_encoder(
        &endpoint.request,
        &format!("{namespace}.{args_name}"),
    ));
    writer.push(",\n          decodeSuccess: ");
    writer.push(&success_decoder);
    writer.push(",\n          decodeError: ");
    writer.push(&error_decoder);
    writer.push(",\n        }),\n");
}

fn render_endpoint_arg_field_type(field: &Field) -> String {
    let mut field_type = render_ts_type_ref(&field.type_ref);
    if matches!(
        field.optionality,
        Optionality::Nullable | Optionality::OptionalNullable
    ) {
        field_type.push_str(" | null");
    }
    field_type
}

fn mark_property_key(writer: &mut TrackedWriter, symbol_id: &SymbolId, kind: &str, property: &str) {
    writer.mark(symbol_id, kind, &render_property_key(property));
}

fn mark_ts_string(writer: &mut TrackedWriter, symbol_id: String, kind: &str, value: &str) {
    writer.mark_synthetic(symbol_id, kind, &ts_string(value));
}

fn collect_endpoint_namespaces(contract: &ApiContract) -> BTreeSet<String> {
    contract.endpoints.iter().map(endpoint_namespace).collect()
}

fn collect_runtime_client_imports(contract: &ApiContract) -> BTreeSet<String> {
    let mut imports = BTreeSet::new();
    for endpoint in &contract.endpoints {
        match endpoint.transport {
            Transport::UnaryHttp => {
                imports.insert("makeUnaryHttpClient".to_owned());
            }
            Transport::ServerSentEvents => {
                imports.insert("makeSseClient".to_owned());
            }
            Transport::WebSocketDuplex | Transport::BinaryDownload | Transport::BinaryUpload => {}
        }
    }
    imports
}

fn contract_has_stream_endpoints(contract: &ApiContract) -> bool {
    contract
        .endpoints
        .iter()
        .any(|endpoint| matches!(endpoint.response, ResponseShape::Stream(_)))
}

fn collect_service_schema_imports(contract: &ApiContract) -> BTreeSet<String> {
    let mut imports = BTreeSet::new();
    for endpoint in &contract.endpoints {
        collect_request_type_imports(&endpoint.request, &mut imports);
        collect_response_type_imports(&endpoint.response, &mut imports);
    }
    imports
}

fn group_endpoints_by_namespace(contract: &ApiContract) -> Vec<(String, Vec<&Endpoint>)> {
    let mut endpoints = contract.endpoints.iter().collect::<Vec<_>>();
    endpoints.sort_by(|left, right| {
        endpoint_namespace(left)
            .cmp(&endpoint_namespace(right))
            .then_with(|| endpoint_function_name(left).cmp(&endpoint_function_name(right)))
            .then_with(|| left.id.as_str().cmp(right.id.as_str()))
    });

    let mut grouped = Vec::<(String, Vec<&Endpoint>)>::new();
    for endpoint in endpoints {
        let namespace = endpoint_namespace(endpoint);
        if grouped.last().map(|(name, _)| name.as_str()) != Some(namespace.as_str()) {
            grouped.push((namespace, Vec::new()));
        }
        grouped
            .last_mut()
            .expect("namespace group exists")
            .1
            .push(endpoint);
    }

    grouped
}

fn collect_endpoint_schema_imports(contract: &ApiContract) -> BTreeSet<String> {
    let mut imports = BTreeSet::new();
    for endpoint in &contract.endpoints {
        collect_request_type_imports(&endpoint.request, &mut imports);
        collect_response_type_imports(&endpoint.response, &mut imports);
    }
    imports
}

fn collect_request_type_imports(request: &RequestShape, imports: &mut BTreeSet<String>) {
    for field in request
        .path_params
        .iter()
        .chain(request.query_params.iter())
    {
        collect_type_ref_import(&field.type_ref, imports);
    }
    if let Some(body) = &request.body {
        collect_type_ref_import(body, imports);
    }
}

fn collect_response_type_imports(response: &ResponseShape, imports: &mut BTreeSet<String>) {
    match response {
        ResponseShape::Empty => {}
        ResponseShape::Json(type_ref)
        | ResponseShape::Created(type_ref)
        | ResponseShape::Stream(type_ref) => {
            collect_type_ref_import(type_ref, imports);
        }
    }
}

fn collect_type_ref_import(type_ref: &TypeRef, imports: &mut BTreeSet<String>) {
    if primitive_from_type_ref(type_ref).is_none() {
        imports.insert(type_ref.name.clone());
    }
}

fn collect_endpoint_error_imports(contract: &ApiContract) -> BTreeSet<String> {
    let mut imports = BTreeSet::from(["ApiClientError".to_owned()]);
    for endpoint in &contract.endpoints {
        imports.extend(endpoint.errors.iter().map(|error| error.name.clone()));
    }
    imports
}

fn collect_endpoint_error_metadata_imports(contract: &ApiContract) -> BTreeSet<String> {
    let mut imports = BTreeSet::new();
    for endpoint in &contract.endpoints {
        for error in &endpoint.errors {
            imports.insert(format!("{}SchemaByStatus", error.name));
        }
    }
    imports
}

fn render_response_type(response: &ResponseShape) -> String {
    match response {
        ResponseShape::Empty => "void".to_owned(),
        ResponseShape::Json(type_ref)
        | ResponseShape::Created(type_ref)
        | ResponseShape::Stream(type_ref) => render_ts_type_ref(type_ref),
    }
}

fn render_endpoint_return_type(endpoint: &Endpoint, requirements: &str) -> String {
    let success = render_response_type(&endpoint.response);
    let error = render_endpoint_error_type(endpoint);

    match &endpoint.response {
        ResponseShape::Stream(_) => {
            format!("Stream.Stream<{success}, {error}, {requirements}>")
        }
        ResponseShape::Empty | ResponseShape::Json(_) | ResponseShape::Created(_) => {
            format!("Effect.Effect<{success}, {error}, {requirements}>")
        }
    }
}

fn render_endpoint_error_type(endpoint: &Endpoint) -> String {
    let domain_error = render_endpoint_domain_error_type(endpoint);
    if domain_error == "never" {
        return "ApiClientError".to_owned();
    }
    format!("ApiClientError | {domain_error}")
}

fn render_endpoint_domain_error_type(endpoint: &Endpoint) -> String {
    let mut errors = endpoint
        .errors
        .iter()
        .map(|error| error.name.clone())
        .collect::<Vec<_>>();
    errors.sort();
    errors.dedup();
    if errors.is_empty() {
        "never".to_owned()
    } else {
        errors.join(" | ")
    }
}

fn render_ts_type_ref(type_ref: &TypeRef) -> String {
    match primitive_from_type_ref(type_ref) {
        Some(Primitive::Bool) => "boolean".to_owned(),
        Some(Primitive::I32 | Primitive::I64 | Primitive::F64) => "number".to_owned(),
        Some(Primitive::String) => "string".to_owned(),
        None => type_ref.name.clone(),
    }
}

fn render_request_encoder(request: &RequestShape, args_type: &str) -> String {
    let mut lines = Vec::new();

    if !request.path_params.is_empty() {
        lines.push("path: {".to_owned());
        for field in &request.path_params {
            lines.push(format!(
                "              {}: args.{},",
                render_property_key(&field.wire_name),
                render_property_key(&field.ts_name)
            ));
        }
        lines.push("            },".to_owned());
    }

    if !request.query_params.is_empty() {
        lines.push("query: {".to_owned());
        for field in &request.query_params {
            lines.push(format!(
                "              {}: args.{},",
                render_property_key(&field.wire_name),
                render_property_key(&field.ts_name)
            ));
        }
        lines.push("            },".to_owned());
    }

    if request.body.is_some() {
        lines.push("body: args.body,".to_owned());
    }

    if lines.is_empty() {
        "() => ({})".to_owned()
    } else {
        format!(
            "(args: {args_type}) => ({{
            {}
          }})",
            lines.join("\n            "),
            args_type = args_type,
        )
    }
}

fn render_success_decoder(response: &ResponseShape, helper: &str) -> String {
    match response {
        ResponseShape::Empty => "() => Effect.void",
        ResponseShape::Json(type_ref)
        | ResponseShape::Created(type_ref)
        | ResponseShape::Stream(type_ref) => {
            return format!("(input) => {helper}.decode(input, {})", type_ref.name);
        }
    }
    .to_owned()
}

fn render_domain_error_decoder(endpoint: &Endpoint, helper: &str) -> String {
    if endpoint.errors.is_empty() {
        return "() => undefined".to_owned();
    }

    let domain_error = render_endpoint_domain_error_type(endpoint);
    let mut output = "(status, input) => {\n".to_owned();
    for error in &endpoint.errors {
        output.push_str("            const ");
        output.push_str(&error.name);
        output.push_str("Schema = ");
        output.push_str(&error.name);
        output.push_str("SchemaByStatus[status as keyof typeof ");
        output.push_str(&error.name);
        output.push_str("SchemaByStatus]\n");
        output.push_str("            if (");
        output.push_str(&error.name);
        output.push_str("Schema !== undefined) {\n");
        output.push_str("              return ");
        output.push_str(helper);
        output.push_str(".decode(input, ");
        output.push_str(&error.name);
        output.push_str("Schema) as Effect.Effect<");
        output.push_str(&domain_error);
        output.push_str(", ApiClientError>\n");
        output.push_str("            }\n");
    }
    output.push_str("            return undefined\n");
    output.push_str("          }");
    output
}

const fn render_runtime_client_helper(endpoint: &Endpoint) -> &'static str {
    match endpoint.transport {
        Transport::UnaryHttp => "makeUnaryHttpClient",
        Transport::ServerSentEvents => "makeSseClient",
        Transport::WebSocketDuplex | Transport::BinaryDownload | Transport::BinaryUpload => {
            "makeUnaryHttpClient"
        }
    }
}

fn endpoint_namespace(endpoint: &Endpoint) -> String {
    endpoint
        .ts_path
        .first()
        .cloned()
        .unwrap_or_else(|| "api".to_owned())
}

fn endpoint_function_name(endpoint: &Endpoint) -> String {
    endpoint
        .ts_path
        .last()
        .cloned()
        .filter(|name| name != &endpoint_namespace(endpoint))
        .unwrap_or_else(|| endpoint.rust_name.clone())
}

fn endpoint_args_name(endpoint: &Endpoint) -> String {
    format!("{}Args", to_pascal_case(&endpoint_function_name(endpoint)))
}

const fn render_transport(transport: Transport) -> &'static str {
    match transport {
        Transport::UnaryHttp => "UnaryHttp",
        Transport::ServerSentEvents => "ServerSentEvents",
        Transport::WebSocketDuplex => "WebSocketDuplex",
        Transport::BinaryDownload => "BinaryDownload",
        Transport::BinaryUpload => "BinaryUpload",
    }
}

fn collect_error_schema_imports(contract: &ApiContract) -> BTreeSet<String> {
    contract
        .errors
        .iter()
        .flat_map(|error| error.variants.iter())
        .flat_map(|variant| variant.fields.iter())
        .filter_map(|field| {
            primitive_from_type_ref(&field.type_ref)
                .is_none()
                .then(|| field.type_ref.name.clone())
        })
        .collect()
}

fn render_client_errors() -> String {
    let client_errors = [
        (
            "NetworkError",
            "NetworkError",
            vec![
                ("message", "Schema.String"),
                ("cause", "Schema.optionalKey(Schema.Unknown)"),
            ],
        ),
        (
            "TimeoutError",
            "TimeoutError",
            vec![
                ("message", "Schema.String"),
                ("timeoutMs", "Schema.optionalKey(Schema.Number)"),
            ],
        ),
        (
            "EncodeError",
            "EncodeError",
            vec![
                ("message", "Schema.String"),
                ("cause", "Schema.optionalKey(Schema.Unknown)"),
            ],
        ),
        (
            "DecodeError",
            "DecodeError",
            vec![
                ("message", "Schema.String"),
                ("cause", "Schema.optionalKey(Schema.Unknown)"),
            ],
        ),
        (
            "UnexpectedStatusError",
            "UnexpectedStatusError",
            vec![
                ("message", "Schema.String"),
                ("status", "Schema.Number"),
                ("body", "Schema.optionalKey(Schema.Unknown)"),
            ],
        ),
        (
            "RemoteProtocolError",
            "RemoteProtocolError",
            vec![
                ("message", "Schema.String"),
                ("body", "Schema.optionalKey(Schema.Unknown)"),
            ],
        ),
    ];

    let mut output = String::new();
    for (class_name, tag, fields) in &client_errors {
        output.push_str(&render_tagged_error_class(class_name, tag, &fields));
        output.push('\n');
    }
    output.push_str("export type ApiClientError =\n");
    output.push_str(
        &client_errors
            .iter()
            .map(|(class_name, _, _)| format!("  | {class_name}"))
            .collect::<Vec<_>>()
            .join("\n"),
    );
    output.push('\n');
    output
}

fn render_tagged_error_class(class_name: &str, tag: &str, fields: &[(&str, &str)]) -> String {
    let mut output = format!(
        "export class {class_name} extends Schema.TaggedErrorClass<{class_name}>()(\n  {},\n",
        ts_string(tag)
    );

    if fields.is_empty() {
        output.push_str("  {}\n");
    } else {
        output.push_str("  {\n");
        for (field_name, schema) in fields {
            output.push_str("    ");
            output.push_str(&render_property_key(field_name));
            output.push_str(": ");
            output.push_str(schema);
            output.push_str(",\n");
        }
        output.push_str("  }\n");
    }

    output.push_str(") {}\n");
    output
}

fn render_source_range(source: &SourceRange, indent: usize) -> String {
    format!(
        "{{\n{indent}  file: {file},\n{indent}  startLine: {start_line},\n{indent}  startColumn: {start_column},\n{indent}  endLine: {end_line},\n{indent}  endColumn: {end_column},\n{indent}}}",
        indent = render_indent(indent),
        file = ts_string(&source.file),
        start_line = source.start_line,
        start_column = source.start_column,
        end_line = source.end_line,
        end_column = source.end_column,
    )
}

fn error_variant_class_name(error: &ErrorDef, variant: &ErrorVariant) -> String {
    let prefix = error
        .ts_name
        .strip_suffix("Error")
        .unwrap_or(&error.ts_name);
    format!("{prefix}{}", variant.rust_name)
}

fn render_type_shape(shape: &TypeShape, owner: &TypeDef) -> String {
    match shape {
        TypeShape::Primitive(primitive) => render_primitive(*primitive).to_owned(),
        TypeShape::Struct(shape) => render_struct(shape, 0),
        TypeShape::Enum(shape) => render_enum(shape, 0),
        TypeShape::Newtype(inner) => format!(
            "{inner}.pipe(Schema.brand(\"{brand}\"))",
            inner = render_type_ref(inner),
            brand = owner.rust_path.join("::")
        ),
        TypeShape::Tuple(items) => {
            let items = items
                .iter()
                .map(render_type_ref)
                .collect::<Vec<_>>()
                .join(", ");
            format!("Schema.Tuple([{items}])")
        }
        TypeShape::List(item) => format!("Schema.Array({})", render_type_ref(item)),
        TypeShape::Map { key, value } => format!(
            "Schema.Record({}, {})",
            render_type_ref(key),
            render_type_ref(value)
        ),
        TypeShape::Option(item) => format!("Schema.NullOr({})", render_type_ref(item)),
        TypeShape::External(external) => render_external(external),
    }
}

fn render_struct(shape: &StructShape, indent: usize) -> String {
    if shape.fields.is_empty() {
        return "Schema.Struct({})".to_owned();
    }

    let mut output = "Schema.Struct({\n".to_owned();
    for field in &shape.fields {
        output.push_str(&render_indent(indent + 2));
        output.push_str(&field.ts_name);
        output.push_str(": ");
        output.push_str(&render_field_schema(field));
        output.push_str(",\n");
    }
    output.push_str(&render_indent(indent));
    output.push_str("})");
    output
}

fn render_enum(shape: &EnumShape, indent: usize) -> String {
    if shape.variants.is_empty() {
        return "Schema.Never".to_owned();
    }

    let variants = shape
        .variants
        .iter()
        .map(|variant| render_enum_variant(variant, indent))
        .collect::<Vec<_>>();

    format!("Schema.Union([{}])", variants.join(", "))
}

fn render_enum_variant(variant: &EnumVariant, indent: usize) -> String {
    let mut fields = vec![format!("_tag: Schema.Literal(\"{}\")", variant.wire_name)];
    fields.extend(
        variant
            .fields
            .iter()
            .map(|field| format!("{}: {}", field.ts_name, render_field_schema(field))),
    );

    if fields.len() == 1 {
        return format!("Schema.Struct({{ {} }})", fields[0]);
    }

    let mut output = "Schema.Struct({\n".to_owned();
    for field in fields {
        output.push_str(&render_indent(indent + 2));
        output.push_str(&field);
        output.push_str(",\n");
    }
    output.push_str(&render_indent(indent));
    output.push_str("})");
    output
}

fn render_field_schema(field: &Field) -> String {
    let schema = render_type_ref(&field.type_ref);
    match field.optionality {
        Optionality::Required => schema,
        Optionality::Optional => format!("Schema.optionalKey({schema})"),
        Optionality::Nullable => format!("Schema.NullOr({schema})"),
        Optionality::OptionalNullable => format!("Schema.optionalKey(Schema.NullOr({schema}))"),
    }
}

fn render_type_ref(type_ref: &TypeRef) -> String {
    match primitive_from_type_ref(type_ref) {
        Some(primitive) => render_primitive(primitive).to_owned(),
        None => type_ref.name.clone(),
    }
}

const fn render_primitive(primitive: Primitive) -> &'static str {
    match primitive {
        Primitive::Bool => "Schema.Boolean",
        Primitive::I32 | Primitive::I64 | Primitive::F64 => "Schema.Number",
        Primitive::String => "Schema.String",
    }
}

fn primitive_from_type_ref(type_ref: &TypeRef) -> Option<Primitive> {
    match type_ref.name.as_str() {
        "bool" | "Bool" | "boolean" => Some(Primitive::Bool),
        "i32" | "I32" => Some(Primitive::I32),
        "i64" | "I64" => Some(Primitive::I64),
        "f64" | "F64" | "number" => Some(Primitive::F64),
        "String" | "string" => Some(Primitive::String),
        _ => None,
    }
}

fn render_external(external: &ExternalType) -> String {
    if external.encoded_ts_name == external.decoded_ts_name {
        format!(
            "Schema.declare<{}>((value): value is {} => true)",
            external.ts_name, external.ts_name
        )
    } else {
        format!(
            "Schema.declare<{}>((value): value is {} => true)",
            external.decoded_ts_name, external.decoded_ts_name
        )
    }
}

fn render_indent(width: usize) -> String {
    " ".repeat(width)
}

fn render_property_key(key: &str) -> String {
    if is_identifier(key) {
        key.to_owned()
    } else {
        ts_string(key)
    }
}

fn is_identifier(value: &str) -> bool {
    let mut chars = value.chars();
    let Some(first) = chars.next() else {
        return false;
    };

    (first == '_' || first == '$' || first.is_ascii_alphabetic())
        && chars.all(|char| char == '_' || char == '$' || char.is_ascii_alphanumeric())
}

fn ts_string(value: &str) -> String {
    let mut output = "\"".to_owned();
    for char in value.chars() {
        match char {
            '"' => output.push_str("\\\""),
            '\\' => output.push_str("\\\\"),
            '\n' => output.push_str("\\n"),
            '\r' => output.push_str("\\r"),
            '\t' => output.push_str("\\t"),
            char => output.push(char),
        }
    }
    output.push('"');
    output
}

fn sanitize_package_dir_name(package_name: &str) -> String {
    package_name
        .chars()
        .map(|char| {
            if char.is_ascii_alphanumeric() || char == '-' || char == '_' || char == '.' {
                char
            } else {
                '_'
            }
        })
        .collect()
}

fn normalize_path(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

fn to_pascal_case(value: &str) -> String {
    let mut output = String::new();
    let mut uppercase_next = true;
    for char in value.chars() {
        if char == '_' || char == '-' || char == ' ' {
            uppercase_next = true;
            continue;
        }

        if uppercase_next {
            output.extend(char.to_uppercase());
            uppercase_next = false;
        } else {
            output.push(char);
        }
    }
    output
}

fn trim_trailing_blank_lines(mut output: String) -> String {
    while output.ends_with("\n\n") {
        output.pop();
    }
    output
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use api_ir::{
        ApiContract, Endpoint, EnumShape, EnumVariant, ErrorDef, ErrorRef, ErrorVariant, Field,
        HttpMethod, HttpStatus, Optionality, Primitive, RequestShape, ResponseShape, RoutePattern,
        SourceRange, StructShape, SymbolId, Transport, TypeDef, TypeRef, TypeShape,
    };

    use super::*;

    #[test]
    fn renders_struct_schemas_with_aliases() {
        let contract = ApiContract {
            package_name: "@workspace/server-api".to_owned(),
            types: vec![TypeDef {
                id: symbol("type", &["User"]),
                rust_path: vec!["server".to_owned(), "users".to_owned(), "User".to_owned()],
                rust_name: "User".to_owned(),
                ts_name: "User".to_owned(),
                shape: TypeShape::Struct(StructShape {
                    fields: vec![
                        field("id", "id", type_ref("UserId"), Optionality::Required),
                        field(
                            "display_name",
                            "displayName",
                            type_ref("String"),
                            Optionality::Required,
                        ),
                        field(
                            "avatar_url",
                            "avatarURL",
                            type_ref("String"),
                            Optionality::Nullable,
                        ),
                        field(
                            "nickname",
                            "nickname",
                            type_ref("String"),
                            Optionality::OptionalNullable,
                        ),
                        field(
                            "timezone",
                            "timezone",
                            type_ref("String"),
                            Optionality::Optional,
                        ),
                    ],
                }),
                source: source(),
            }],
            ..ApiContract::default()
        };

        let rendered = render_schemas(&contract);

        assert_eq!(
            rendered,
            r#"// Generated API package for @workspace/server-api
import { Schema } from "@rust-ts-integration/effect-runtime/compat"

export const User = Schema.Struct({
  id: UserId,
  displayName: Schema.String,
  avatarURL: Schema.NullOr(Schema.String),
  nickname: Schema.optionalKey(Schema.NullOr(Schema.String)),
  timezone: Schema.optionalKey(Schema.String),
})
export type User = Schema.Schema.Type<typeof User>
export type UserEncoded = Schema.Codec.Encoded<typeof User>
"#
        );
    }

    #[test]
    fn renders_newtypes_and_enums() {
        let contract = ApiContract {
            package_name: "example-api".to_owned(),
            types: vec![
                TypeDef {
                    id: symbol("type", &["UserId"]),
                    rust_path: vec!["server".to_owned(), "users".to_owned(), "UserId".to_owned()],
                    rust_name: "UserId".to_owned(),
                    ts_name: "UserId".to_owned(),
                    shape: TypeShape::Newtype(Box::new(type_ref("String"))),
                    source: source(),
                },
                TypeDef {
                    id: symbol("type", &["UserEvent"]),
                    rust_path: vec![
                        "server".to_owned(),
                        "users".to_owned(),
                        "UserEvent".to_owned(),
                    ],
                    rust_name: "UserEvent".to_owned(),
                    ts_name: "UserEvent".to_owned(),
                    shape: TypeShape::Enum(EnumShape {
                        variants: vec![
                            EnumVariant {
                                id: symbol("variant", &["UserEvent", "Created"]),
                                rust_name: "Created".to_owned(),
                                wire_name: "created".to_owned(),
                                fields: Vec::new(),
                                source: source(),
                            },
                            EnumVariant {
                                id: symbol("variant", &["UserEvent", "Renamed"]),
                                rust_name: "Renamed".to_owned(),
                                wire_name: "renamed".to_owned(),
                                fields: vec![field(
                                    "display_name",
                                    "displayName",
                                    type_ref("String"),
                                    Optionality::Required,
                                )],
                                source: source(),
                            },
                        ],
                    }),
                    source: source(),
                },
            ],
            ..ApiContract::default()
        };

        let rendered = render_schemas(&contract);

        assert!(rendered.contains(
            "export const UserId = Schema.String.pipe(Schema.brand(\"server::users::UserId\"))"
        ));
        assert!(rendered.contains(
            "export const UserEvent = Schema.Union([Schema.Struct({ _tag: Schema.Literal(\"created\") }), Schema.Struct({\n  _tag: Schema.Literal(\"renamed\"),\n  displayName: Schema.String,\n})])"
        ));
    }

    #[test]
    fn renders_collection_shapes() {
        let owner = TypeDef {
            id: symbol("type", &["Lookup"]),
            rust_path: vec!["Lookup".to_owned()],
            rust_name: "Lookup".to_owned(),
            ts_name: "Lookup".to_owned(),
            shape: TypeShape::Map {
                key: Box::new(type_ref("String")),
                value: Box::new(type_ref("User")),
            },
            source: source(),
        };

        assert_eq!(
            render_type_shape(&owner.shape, &owner),
            "Schema.Record(Schema.String, User)"
        );

        let owner = TypeDef {
            shape: TypeShape::List(Box::new(type_ref("User"))),
            ..owner
        };
        assert_eq!(
            render_type_shape(&owner.shape, &owner),
            "Schema.Array(User)"
        );

        let owner = TypeDef {
            shape: TypeShape::Option(Box::new(type_ref("User"))),
            ..owner
        };
        assert_eq!(
            render_type_shape(&owner.shape, &owner),
            "Schema.NullOr(User)"
        );
    }

    #[test]
    fn renders_types_in_deterministic_name_order() {
        let contract = ApiContract {
            package_name: "example-api".to_owned(),
            types: vec![simple_type("Zed"), simple_type("Alpha")],
            ..ApiContract::default()
        };

        let rendered = render_schemas(&contract);

        let alpha = rendered
            .find("export const Alpha")
            .expect("Alpha is rendered");
        let zed = rendered.find("export const Zed").expect("Zed is rendered");
        assert!(alpha < zed);
    }

    #[test]
    fn renders_domain_errors_with_status_and_symbol_metadata() {
        let contract = ApiContract {
            package_name: "@workspace/server-api".to_owned(),
            errors: vec![ErrorDef {
                id: symbol("error", &["GetUserError"]),
                rust_path: vec![
                    "server".to_owned(),
                    "users".to_owned(),
                    "GetUserError".to_owned(),
                ],
                rust_name: "GetUserError".to_owned(),
                ts_name: "GetUserError".to_owned(),
                variants: vec![
                    ErrorVariant {
                        id: symbol("error_variant", &["GetUserError", "NotFound"]),
                        rust_name: "NotFound".to_owned(),
                        tag: "notFound".to_owned(),
                        status: HttpStatus(404),
                        fields: vec![field("id", "id", type_ref("UserId"), Optionality::Required)],
                        source: source_file("src/users.rs", 42),
                    },
                    ErrorVariant {
                        id: symbol("error_variant", &["GetUserError", "PermissionDenied"]),
                        rust_name: "PermissionDenied".to_owned(),
                        tag: "permission-denied".to_owned(),
                        status: HttpStatus(403),
                        fields: Vec::new(),
                        source: source_file("src/users.rs", 47),
                    },
                ],
                source: source_file("src/users.rs", 39),
            }],
            ..ApiContract::default()
        };

        let rendered = render_errors(&contract);

        assert!(rendered.contains("import { UserId } from \"./schemas\""));
        assert!(rendered.contains(
            "export class GetUserNotFound extends Schema.TaggedErrorClass<GetUserNotFound>()(\n  \"notFound\","
        ));
        assert!(rendered.contains("    id: UserId,"));
        assert!(rendered.contains(
            "export type GetUserError =\n  | GetUserNotFound\n  | GetUserPermissionDenied"
        ));
        assert!(rendered.contains("export const GetUserErrorStatus = {\n  notFound: 404,\n  \"permission-denied\": 403,\n} as const"));
        assert!(rendered.contains("export const GetUserErrorSymbols = {"));
        assert!(rendered.contains("file: \"src/users.rs\""));
        assert!(rendered.contains("startLine: 42"));
    }

    #[test]
    fn renders_generated_client_error_union() {
        let rendered = render_errors(&ApiContract {
            package_name: "example-api".to_owned(),
            ..ApiContract::default()
        });

        assert!(rendered.contains(
            "export class NetworkError extends Schema.TaggedErrorClass<NetworkError>()("
        ));
        assert!(rendered.contains(
            "export class UnexpectedStatusError extends Schema.TaggedErrorClass<UnexpectedStatusError>()("
        ));
        assert!(rendered.contains(
            "export type ApiClientError =\n  | NetworkError\n  | TimeoutError\n  | EncodeError\n  | DecodeError\n  | UnexpectedStatusError\n  | RemoteProtocolError"
        ));
    }

    #[test]
    fn renders_endpoint_accessors_with_effect_signatures() {
        let contract = ApiContract {
            package_name: "@workspace/server-api".to_owned(),
            endpoints: vec![Endpoint {
                id: symbol("endpoint", &["users", "get_user"]),
                rust_path: vec![
                    "server".to_owned(),
                    "users".to_owned(),
                    "get_user".to_owned(),
                ],
                rust_name: "get_user".to_owned(),
                ts_path: vec!["users".to_owned(), "getUser".to_owned()],
                route: RoutePattern("/users/{id}".to_owned()),
                method: HttpMethod::Get,
                transport: Transport::UnaryHttp,
                request: RequestShape {
                    path_params: vec![field("id", "id", type_ref("UserId"), Optionality::Required)],
                    query_params: vec![field(
                        "include_posts",
                        "includePosts",
                        type_ref("bool"),
                        Optionality::Optional,
                    )],
                    body: None,
                },
                response: ResponseShape::Json(type_ref("User")),
                errors: vec![ErrorRef {
                    id: symbol("error", &["GetUserError"]),
                    name: "GetUserError".to_owned(),
                }],
                source: source(),
                allow_unused: false,
            }],
            ..ApiContract::default()
        };

        let rendered = render_endpoints(&contract);

        assert!(rendered
            .contains("import { Effect } from \"@rust-ts-integration/effect-runtime/compat\""));
        assert!(rendered.contains("import { ServerApi } from \"./layer\""));
        assert!(rendered.contains("import type { User, UserId } from \"./schemas\""));
        assert!(rendered.contains("import type { ApiClientError, GetUserError } from \"./errors\""));
        assert!(rendered.contains("export namespace users {"));
        assert!(rendered.contains(
            "  export interface GetUserArgs {\n    readonly id: UserId;\n    readonly includePosts?: boolean;\n  }"
        ));
        assert!(rendered.contains(
            "  export const getUserRoute = {\n    method: \"GET\",\n    path: \"/users/{id}\",\n    transport: \"UnaryHttp\",\n  } as const"
        ));
        assert!(rendered.contains(
            "  export const getUser = (\n    args: GetUserArgs\n  ): Effect.Effect<User, ApiClientError | GetUserError, ServerApi> =>\n    ServerApi.use((api) => api.users.getUser(args))"
        ));
    }

    #[test]
    fn renders_endpoint_body_and_empty_response() {
        let endpoint = Endpoint {
            id: symbol("endpoint", &["users", "create_user"]),
            rust_path: vec![
                "server".to_owned(),
                "users".to_owned(),
                "create_user".to_owned(),
            ],
            rust_name: "create_user".to_owned(),
            ts_path: vec!["users".to_owned(), "createUser".to_owned()],
            route: RoutePattern("/users".to_owned()),
            method: HttpMethod::Post,
            transport: Transport::UnaryHttp,
            request: RequestShape {
                path_params: Vec::new(),
                query_params: Vec::new(),
                body: Some(type_ref("CreateUser")),
            },
            response: ResponseShape::Empty,
            errors: Vec::new(),
            source: source(),
            allow_unused: false,
        };

        let rendered = render_endpoints(&ApiContract {
            package_name: "example-api".to_owned(),
            endpoints: vec![endpoint],
            ..ApiContract::default()
        });

        assert!(rendered
            .contains("  export interface CreateUserArgs {\n    readonly body: CreateUser;\n  }"));
        assert!(rendered.contains("): Effect.Effect<void, ApiClientError, ServerApi> =>"));
    }

    #[test]
    fn renders_sse_endpoint_accessors_with_stream_signatures() {
        let contract = ApiContract {
            package_name: "@workspace/server-api".to_owned(),
            endpoints: vec![Endpoint {
                id: symbol("endpoint", &["events", "events"]),
                rust_path: vec![
                    "server".to_owned(),
                    "events".to_owned(),
                    "events".to_owned(),
                ],
                rust_name: "events".to_owned(),
                ts_path: vec!["events".to_owned(), "events".to_owned()],
                route: RoutePattern("/events".to_owned()),
                method: HttpMethod::Get,
                transport: Transport::ServerSentEvents,
                request: RequestShape::default(),
                response: ResponseShape::Stream(type_ref("UserEvent")),
                errors: vec![ErrorRef {
                    id: symbol("error", &["EventError"]),
                    name: "EventError".to_owned(),
                }],
                source: source(),
                allow_unused: false,
            }],
            ..ApiContract::default()
        };

        let rendered = render_endpoints(&contract);

        assert!(rendered.contains(
            "import { Effect, Stream } from \"@rust-ts-integration/effect-runtime/compat\""
        ));
        assert!(rendered.contains(
            "  export const eventsRoute = {\n    method: \"GET\",\n    path: \"/events\",\n    transport: \"ServerSentEvents\",\n  } as const"
        ));
        assert!(rendered.contains(
            "  export const events = (\n    args: EventsArgs\n  ): Stream.Stream<UserEvent, ApiClientError | EventError, ServerApi> =>\n    ServerApi.use((api) => api.events.events(args))"
        ));
    }

    #[test]
    fn renders_server_api_service_and_layers() {
        let contract = ApiContract {
            package_name: "@workspace/server-api".to_owned(),
            endpoints: vec![Endpoint {
                id: symbol("endpoint", &["users", "get_user"]),
                rust_path: vec![
                    "server".to_owned(),
                    "users".to_owned(),
                    "get_user".to_owned(),
                ],
                rust_name: "get_user".to_owned(),
                ts_path: vec!["users".to_owned(), "getUser".to_owned()],
                route: RoutePattern("/users/{id}".to_owned()),
                method: HttpMethod::Get,
                transport: Transport::UnaryHttp,
                request: RequestShape {
                    path_params: vec![field("id", "id", type_ref("UserId"), Optionality::Required)],
                    query_params: Vec::new(),
                    body: None,
                },
                response: ResponseShape::Json(type_ref("User")),
                errors: vec![ErrorRef {
                    id: symbol("error", &["GetUserError"]),
                    name: "GetUserError".to_owned(),
                }],
                source: source(),
                allow_unused: false,
            }],
            ..ApiContract::default()
        };

        let rendered = render_layer(&contract);

        assert!(rendered.contains(
            "import { Context, Layer, Effect } from \"@rust-ts-integration/effect-runtime/compat\""
        ));
        assert!(rendered.contains(
            "import { makeUnaryHttpClient } from \"@rust-ts-integration/effect-runtime\""
        ));
        assert!(rendered.contains("import { users } from \"./endpoints\""));
        assert!(rendered.contains("import { User, UserId } from \"./schemas\""));
        assert!(rendered.contains("import type { ApiClientError, GetUserError } from \"./errors\""));
        assert!(rendered.contains(
            "export class ServerApi extends Context.Service<ServerApi, ServerApi.Service>()(\"@workspace/server-api::ServerApi\") {}"
        ));
        assert!(rendered.contains(
            "      readonly getUser: (args: users.GetUserArgs) => Effect.Effect<User, ApiClientError | GetUserError, never>"
        ));
        assert!(rendered.contains(
            "  export const layer = (config: ServerApiConfig): Layer.Layer<ServerApi> =>"
        ));
        assert!(rendered.contains(
            "getUser: makeUnaryHttpClient<users.GetUserArgs, User, GetUserError>(config,"
        ));
        assert!(
            rendered.contains("decodeSuccess: (input) => makeUnaryHttpClient.decode(input, User)")
        );
        assert!(rendered
            .contains("return makeUnaryHttpClient.decode(input, GetUserErrorSchema) as Effect.Effect<GetUserError, ApiClientError>"));
        assert!(rendered
            .contains("  export const mock = (service: Service): Layer.Layer<ServerApi> =>"));
    }

    #[test]
    fn renders_sse_server_api_service_and_layer_client() {
        let contract = ApiContract {
            package_name: "@workspace/server-api".to_owned(),
            endpoints: vec![Endpoint {
                id: symbol("endpoint", &["events", "events"]),
                rust_path: vec![
                    "server".to_owned(),
                    "events".to_owned(),
                    "events".to_owned(),
                ],
                rust_name: "events".to_owned(),
                ts_path: vec!["events".to_owned(), "events".to_owned()],
                route: RoutePattern("/events".to_owned()),
                method: HttpMethod::Get,
                transport: Transport::ServerSentEvents,
                request: RequestShape::default(),
                response: ResponseShape::Stream(type_ref("UserEvent")),
                errors: vec![ErrorRef {
                    id: symbol("error", &["EventError"]),
                    name: "EventError".to_owned(),
                }],
                source: source(),
                allow_unused: false,
            }],
            ..ApiContract::default()
        };

        let rendered = render_layer(&contract);

        assert!(rendered.contains(
            "import { Context, Layer, Effect, Stream } from \"@rust-ts-integration/effect-runtime/compat\""
        ));
        assert!(rendered
            .contains("import { makeSseClient } from \"@rust-ts-integration/effect-runtime\""));
        assert!(rendered.contains(
            "      readonly events: (args: events.EventsArgs) => Stream.Stream<UserEvent, ApiClientError | EventError, never>"
        ));
        assert!(rendered.contains(
            "events: makeSseClient<events.EventsArgs, UserEvent, EventError>(config,"
        ));
        assert!(
            rendered.contains("decodeSuccess: (input) => makeSseClient.decode(input, UserEvent)")
        );
    }

    #[test]
    fn renders_generated_package_manifest_and_index() {
        let contract = ApiContract {
            package_name: "@workspace/server-api".to_owned(),
            ..ApiContract::default()
        };

        assert_eq!(
            render_package_json(&contract),
            r#"{
  "name": "@workspace/server-api",
  "version": "0.0.0",
  "private": true,
  "type": "module",
  "types": "./index.ts",
  "exports": {
    ".": "./index.ts",
    "./schemas": "./schemas.ts",
    "./errors": "./errors.ts",
    "./endpoints": "./endpoints.ts",
    "./layer": "./layer.ts"
  },
  "dependencies": {
    "@rust-ts-integration/effect-runtime": "0.0.0",
    "effect": "4.0.0-beta.78"
  }
}
"#
        );
        assert_eq!(
            render_package_index(&contract),
            r#"// Generated API package for @workspace/server-api
export * from "./schemas"
export * from "./errors"
export * from "./endpoints"
export * from "./layer"
"#
        );
    }

    #[test]
    fn generated_effect_version_matches_runtime_package() {
        let manifest = std::fs::read_to_string(
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../npm/effect-runtime/package.json"),
        )
        .expect("read runtime package manifest");
        let value: serde_json::Value =
            serde_json::from_str(&manifest).expect("parse runtime package manifest");

        assert_eq!(
            value["dependencies"]["effect"].as_str(),
            Some(EFFECT_VERSION)
        );
    }

    #[test]
    fn renders_generated_package_with_cache_path_and_tsconfig_mapping() {
        let contract = ApiContract {
            package_name: "@workspace/server-api".to_owned(),
            ..ApiContract::default()
        };

        let package = render_generated_package(&contract, Path::new("target"));

        assert_eq!(
            package.package_dir,
            Path::new("target")
                .join("api-contract")
                .join("effect-v4")
                .join("packages")
                .join("_workspace_server-api")
        );
        assert_eq!(
            package
                .files
                .iter()
                .map(|file| file.path.as_str())
                .collect::<Vec<_>>(),
            vec![
                "package.json",
                "index.ts",
                "schemas.ts",
                "errors.ts",
                "endpoints.ts",
                "layer.ts",
                "tsconfig.paths.json"
            ]
        );

        let paths = render_tsconfig_paths(&package.tsconfig_paths);
        assert!(paths.contains("\"@workspace/server-api\": ["));
        assert!(paths
            .contains("\"target/api-contract/effect-v4/packages/_workspace_server-api/index.ts\""));
        assert!(paths.contains("\"@workspace/server-api/*\": ["));
        assert!(
            paths.contains("\"target/api-contract/effect-v4/packages/_workspace_server-api/*\"")
        );
    }

    #[test]
    fn renders_symbol_graph_for_generated_package() {
        let mut contract = api_test_fixtures::basic_contract();
        contract.types.push(TypeDef {
            id: SymbolId::new("fixture:type:AccountState"),
            rust_path: vec![
                "fixture".to_owned(),
                "users".to_owned(),
                "AccountState".to_owned(),
            ],
            rust_name: "AccountState".to_owned(),
            ts_name: "AccountState".to_owned(),
            shape: TypeShape::Enum(EnumShape {
                variants: vec![EnumVariant {
                    id: SymbolId::new("fixture:enum:AccountState:Active"),
                    rust_name: "Active".to_owned(),
                    wire_name: "active".to_owned(),
                    fields: Vec::new(),
                    source: source_file("crates/server/src/types.rs", 30),
                }],
            }),
            source: source_file("crates/server/src/types.rs", 29),
        });
        let package = render_generated_package(&contract, Path::new("target"));

        let graph = render_symbol_graph(&contract, &package);
        assert_eq!(graph, render_symbol_graph(&contract, &package));

        let value: serde_json::Value = serde_json::from_str(&graph).expect("parse symbol graph");
        let symbols = value["symbols"].as_array().expect("symbols array");
        let endpoint = find_symbol(symbols, "fixture:endpoint:getUser");
        assert_eq!(endpoint["kind"], "endpoint");
        assert_eq!(endpoint["metadata"]["method"], "GET");
        assert_eq!(endpoint["metadata"]["route"], "/users/{id}");
        assert_eq!(
            endpoint["metadata"]["tsPath"],
            serde_json::json!(["users", "getUser"])
        );
        assert!(endpoint["rust"]["nameRange"].is_object());
        assert!(endpoint["rust"]["fullRange"].is_object());
        assert!(endpoint["typescript"]
            .as_array()
            .expect("endpoint ts locations")
            .iter()
            .any(|location| location["generated"] == true
                && location["file"]
                    .as_str()
                    .is_some_and(|file| file.ends_with("endpoints.ts"))));
        assert!(endpoint["typescript"]
            .as_array()
            .expect("endpoint ts locations")
            .iter()
            .any(|location| location["file"]
                .as_str()
                .is_some_and(|file| file.ends_with("layer.ts"))));

        assert_eq!(
            find_symbol(symbols, "fixture:field:getUser:id")["kind"],
            "routeParam"
        );
        assert_eq!(
            find_symbol(symbols, "fixture:type:AccountState")["kind"],
            "type"
        );
        assert_eq!(
            find_symbol(symbols, "fixture:enum:AccountState:Active")["kind"],
            "enumVariant"
        );
        assert_eq!(
            find_symbol(symbols, "fixture:field:User:displayName")["kind"],
            "field"
        );
        assert_eq!(
            find_symbol(symbols, "fixture:error:GetUserError:UserNotFound")["kind"],
            "errorVariant"
        );
        assert!(symbols.iter().any(|symbol| symbol["kind"] == "errorTag"));
        for symbol in symbols {
            assert!(
                !symbol["typescript"]
                    .as_array()
                    .expect("typescript locations")
                    .is_empty(),
                "missing generated TypeScript range for {}",
                symbol["id"]
            );
        }
    }

    #[test]
    fn fixture_generated_endpoints_snapshot_is_deterministic() {
        let contract = api_test_fixtures::basic_contract();
        let package = render_generated_package(&contract, Path::new("target"));
        let endpoints = package
            .files
            .iter()
            .find(|file| file.path == "endpoints.ts")
            .expect("endpoints file");

        assert_eq!(
            package
                .files
                .iter()
                .map(|file| file.path.as_str())
                .collect::<Vec<_>>(),
            vec![
                "package.json",
                "index.ts",
                "schemas.ts",
                "errors.ts",
                "endpoints.ts",
                "layer.ts",
                "tsconfig.paths.json"
            ]
        );
        assert_eq!(
            endpoints.contents,
            r#"// Generated API package for @workspace/server-api
import { Effect, Stream } from "@rust-ts-integration/effect-runtime/compat"
import { ServerApi } from "./layer"
import type { CreateUserRequest, User, UserEvent } from "./schemas"
import type { ApiClientError, GetUserError } from "./errors"

export namespace events {
  export interface WatchUsersArgs {}

  export const watchUsersRoute = {
    method: "GET",
    path: "/users/events",
    transport: "ServerSentEvents",
  } as const

  export const watchUsers = (
    args: WatchUsersArgs
  ): Stream.Stream<UserEvent, ApiClientError | GetUserError, ServerApi> =>
    ServerApi.use((api) => api.events.watchUsers(args))

}

export namespace users {
  export interface CreateUserArgs {
    readonly body: CreateUserRequest;
  }

  export const createUserRoute = {
    method: "POST",
    path: "/users",
    transport: "UnaryHttp",
  } as const

  export const createUser = (
    args: CreateUserArgs
  ): Effect.Effect<User, ApiClientError | GetUserError, ServerApi> =>
    ServerApi.use((api) => api.users.createUser(args))

  export interface GetUserArgs {
    readonly id: number;
  }

  export const getUserRoute = {
    method: "GET",
    path: "/users/{id}",
    transport: "UnaryHttp",
  } as const

  export const getUser = (
    args: GetUserArgs
  ): Effect.Effect<User, ApiClientError | GetUserError, ServerApi> =>
    ServerApi.use((api) => api.users.getUser(args))

}
"#
        );
    }

    fn simple_type(name: &str) -> TypeDef {
        TypeDef {
            id: symbol("type", &[name]),
            rust_path: vec![name.to_owned()],
            rust_name: name.to_owned(),
            ts_name: name.to_owned(),
            shape: TypeShape::Primitive(Primitive::String),
            source: source(),
        }
    }

    fn field(rust_name: &str, ts_name: &str, type_ref: TypeRef, optionality: Optionality) -> Field {
        Field {
            id: symbol("field", &[rust_name]),
            rust_name: rust_name.to_owned(),
            wire_name: ts_name.to_owned(),
            ts_name: ts_name.to_owned(),
            type_ref,
            optionality,
            source: source(),
        }
    }

    fn type_ref(name: &str) -> TypeRef {
        TypeRef {
            id: symbol("type", &[name]),
            name: name.to_owned(),
        }
    }

    fn symbol(namespace: &str, parts: &[&str]) -> SymbolId {
        SymbolId::from_parts(namespace, parts)
    }

    fn source() -> SourceRange {
        source_file("", 0)
    }

    fn source_file(file: &str, line: u32) -> SourceRange {
        SourceRange {
            file: file.to_owned(),
            start_line: line,
            start_column: 0,
            end_line: line,
            end_column: 0,
            full_range: None,
        }
    }

    fn find_symbol<'a>(symbols: &'a [serde_json::Value], id: &str) -> &'a serde_json::Value {
        symbols
            .iter()
            .find(|symbol| symbol["id"] == id)
            .unwrap_or_else(|| panic!("missing symbol `{id}`"))
    }
}

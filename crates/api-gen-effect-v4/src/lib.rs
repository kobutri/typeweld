//! Effect v4 TypeScript generator backend.

use std::collections::BTreeSet;

use api_ir::{
    ApiContract, Endpoint, EnumShape, EnumVariant, ErrorDef, ErrorVariant, ExternalType, Field,
    Optionality, Primitive, RequestShape, ResponseShape, SourceRange, StructShape, Transport,
    TypeDef, TypeRef, TypeShape,
};

#[must_use]
pub fn render_package_banner(contract: &ApiContract) -> String {
    format!("// Generated API package for {}\n", contract.package_name)
}

/// Renders generated Effect Schema declarations for every exported API type.
#[must_use]
pub fn render_schemas(contract: &ApiContract) -> String {
    let mut output = render_package_banner(contract);
    output.push_str("import { Schema } from \"effect\"\n\n");

    let mut types = contract.types.iter().collect::<Vec<_>>();
    types.sort_by(|left, right| {
        left.ts_name
            .cmp(&right.ts_name)
            .then_with(|| left.id.as_str().cmp(right.id.as_str()))
    });

    for type_def in types {
        output.push_str(&render_type_def(type_def));
        output.push('\n');
    }

    trim_trailing_blank_lines(output)
}

/// Renders schema-backed domain and generated client errors.
#[must_use]
pub fn render_errors(contract: &ApiContract) -> String {
    let mut output = render_package_banner(contract);
    output.push_str("import { Schema } from \"effect\"\n");

    let schema_imports = collect_error_schema_imports(contract);
    if !schema_imports.is_empty() {
        output.push_str("import { ");
        output.push_str(
            &schema_imports
                .iter()
                .map(String::as_str)
                .collect::<Vec<_>>()
                .join(", "),
        );
        output.push_str(" } from \"./schemas\"\n");
    }
    output.push('\n');

    output.push_str(&render_client_errors());

    let mut errors = contract.errors.iter().collect::<Vec<_>>();
    errors.sort_by(|left, right| {
        left.ts_name
            .cmp(&right.ts_name)
            .then_with(|| left.id.as_str().cmp(right.id.as_str()))
    });

    for error in errors {
        output.push('\n');
        output.push_str(&render_error_def(error));
    }

    trim_trailing_blank_lines(output)
}

/// Renders generated endpoint accessors and route metadata.
#[must_use]
pub fn render_endpoints(contract: &ApiContract) -> String {
    let mut output = render_package_banner(contract);
    output.push_str("import { Effect } from \"effect\"\n");
    output.push_str("import { ServerApi } from \"./layer\"\n");

    let schema_imports = collect_endpoint_schema_imports(contract);
    if !schema_imports.is_empty() {
        output.push_str("import type { ");
        output.push_str(
            &schema_imports
                .iter()
                .map(String::as_str)
                .collect::<Vec<_>>()
                .join(", "),
        );
        output.push_str(" } from \"./schemas\"\n");
    }

    let error_imports = collect_endpoint_error_imports(contract);
    output.push_str("import type { ");
    output.push_str(
        &error_imports
            .iter()
            .map(String::as_str)
            .collect::<Vec<_>>()
            .join(", "),
    );
    output.push_str(" } from \"./errors\"\n\n");

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
                output.push_str("}\n\n");
            }
            output.push_str("export namespace ");
            output.push_str(&current_namespace);
            output.push_str(" {\n");
            namespace = Some(current_namespace);
        }

        output.push_str(&render_endpoint(endpoint));
    }

    if namespace.is_some() {
        output.push_str("}\n");
    }

    trim_trailing_blank_lines(output)
}

/// Renders the generated Effect service tag, service interface, and layer helpers.
#[must_use]
pub fn render_layer(contract: &ApiContract) -> String {
    let mut output = render_package_banner(contract);
    output.push_str("import { Context, Layer, Effect } from \"effect\"\n");

    let endpoint_namespaces = collect_endpoint_namespaces(contract);
    if !endpoint_namespaces.is_empty() {
        output.push_str("import type { ");
        output.push_str(
            &endpoint_namespaces
                .iter()
                .map(String::as_str)
                .collect::<Vec<_>>()
                .join(", "),
        );
        output.push_str(" } from \"./endpoints\"\n");
    }

    let schema_imports = collect_service_schema_imports(contract);
    if !schema_imports.is_empty() {
        output.push_str("import type { ");
        output.push_str(
            &schema_imports
                .iter()
                .map(String::as_str)
                .collect::<Vec<_>>()
                .join(", "),
        );
        output.push_str(" } from \"./schemas\"\n");
    }

    let error_imports = collect_endpoint_error_imports(contract);
    output.push_str("import type { ");
    output.push_str(
        &error_imports
            .iter()
            .map(String::as_str)
            .collect::<Vec<_>>()
            .join(", "),
    );
    output.push_str(" } from \"./errors\"\n\n");

    output.push_str("export interface ServerApiConfig {\n");
    output.push_str("  readonly baseUrl: string\n");
    output.push_str("  readonly timeoutMs?: number\n");
    output.push_str("  readonly fetch?: typeof fetch\n");
    output.push_str("}\n\n");

    output.push_str(&format!(
        "export class ServerApi extends Context.Tag({})<ServerApi, ServerApi.Service>() {{}}\n\n",
        ts_string(&format!("{}::ServerApi", contract.package_name))
    ));

    output.push_str("export namespace ServerApi {\n");
    output.push_str(&render_service_interface(contract));
    output.push('\n');
    output.push_str(
        "  export const layer = (config: ServerApiConfig): Layer.Layer<ServerApi, never, never> => {\n",
    );
    output.push_str("    void config\n");
    output.push_str("    return Layer.succeed(ServerApi, {} as Service)\n");
    output.push_str("  }\n\n");
    output.push_str(
        "  export const mock = (service: Service): Layer.Layer<ServerApi, never, never> =>\n",
    );
    output.push_str("    Layer.succeed(ServerApi, service)\n");
    output.push_str("}\n");

    trim_trailing_blank_lines(output)
}

fn render_type_def(type_def: &TypeDef) -> String {
    let schema = render_type_shape(&type_def.shape, type_def);
    format!(
        "export const {name} = {schema}\nexport type {name} = typeof {name}.Type\nexport type {name}Encoded = typeof {name}.Encoded\n",
        name = type_def.ts_name,
    )
}

fn collect_endpoint_namespaces(contract: &ApiContract) -> BTreeSet<String> {
    contract.endpoints.iter().map(endpoint_namespace).collect()
}

fn collect_service_schema_imports(contract: &ApiContract) -> BTreeSet<String> {
    let mut imports = BTreeSet::new();
    for endpoint in &contract.endpoints {
        collect_response_type_imports(&endpoint.response, &mut imports);
    }
    imports
}

fn render_service_interface(contract: &ApiContract) -> String {
    let mut output = "  export interface Service {\n".to_owned();
    let grouped = group_endpoints_by_namespace(contract);

    for (namespace, endpoints) in grouped {
        output.push_str("    readonly ");
        output.push_str(&namespace);
        output.push_str(": {\n");
        for endpoint in endpoints {
            output.push_str(&render_service_method(endpoint));
        }
        output.push_str("    }\n");
    }

    output.push_str("  }\n");
    output
}

fn render_service_method(endpoint: &Endpoint) -> String {
    format!(
        "      readonly {function_name}: (args: {namespace}.{args_name}) => Effect.Effect<{success}, {error}, never>\n",
        function_name = endpoint_function_name(endpoint),
        namespace = endpoint_namespace(endpoint),
        args_name = endpoint_args_name(endpoint),
        success = render_response_type(&endpoint.response),
        error = render_endpoint_error_type(endpoint),
    )
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
        ResponseShape::Json(type_ref) | ResponseShape::Stream(type_ref) => {
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

fn render_endpoint(endpoint: &Endpoint) -> String {
    let function_name = endpoint_function_name(endpoint);
    let args_name = endpoint_args_name(endpoint);
    let success = render_response_type(&endpoint.response);
    let error = render_endpoint_error_type(endpoint);

    format!(
        "{args_interface}\n  export const {function_name}Route = {{\n    method: {method},\n    path: {path},\n    transport: {transport},\n  }} as const\n\n  export const {function_name} = (\n    args: {args_name}\n  ): Effect.Effect<{success}, {error}, ServerApi> =>\n    Effect.flatMap(ServerApi, (api) => api.{namespace}.{function_name}(args))\n\n",
        args_interface = render_endpoint_args(endpoint),
        method = ts_string(endpoint.method.as_str()),
        path = ts_string(&endpoint.route.0),
        transport = ts_string(render_transport(endpoint.transport)),
        namespace = endpoint_namespace(endpoint),
    )
}

fn render_endpoint_args(endpoint: &Endpoint) -> String {
    let args_name = endpoint_args_name(endpoint);
    let mut fields = Vec::new();

    fields.extend(
        endpoint
            .request
            .path_params
            .iter()
            .map(render_endpoint_arg_field),
    );
    fields.extend(
        endpoint
            .request
            .query_params
            .iter()
            .map(render_endpoint_arg_field),
    );
    if let Some(body) = &endpoint.request.body {
        fields.push(format!("readonly body: {};", render_ts_type_ref(body)));
    }

    if fields.is_empty() {
        return format!("  export interface {args_name} {{}}\n");
    }

    let mut output = format!("  export interface {args_name} {{\n");
    for field in fields {
        output.push_str("    ");
        output.push_str(&field);
        output.push('\n');
    }
    output.push_str("  }\n");
    output
}

fn render_endpoint_arg_field(field: &Field) -> String {
    let optional_marker = match field.optionality {
        Optionality::Optional => "?",
        Optionality::Required | Optionality::Nullable => "",
    };
    let mut field_type = render_ts_type_ref(&field.type_ref);
    if matches!(field.optionality, Optionality::Nullable) {
        field_type.push_str(" | null");
    }

    format!(
        "readonly {}{}: {};",
        render_property_key(&field.ts_name),
        optional_marker,
        field_type
    )
}

fn render_response_type(response: &ResponseShape) -> String {
    match response {
        ResponseShape::Empty => "void".to_owned(),
        ResponseShape::Json(type_ref) => render_ts_type_ref(type_ref),
        ResponseShape::Stream(type_ref) => {
            format!("ReadonlyArray<{}>", render_ts_type_ref(type_ref))
        }
    }
}

fn render_endpoint_error_type(endpoint: &Endpoint) -> String {
    let mut errors = endpoint
        .errors
        .iter()
        .map(|error| error.name.clone())
        .collect::<Vec<_>>();
    errors.push("ApiClientError".to_owned());
    errors.sort();
    errors.dedup();
    errors.join(" | ")
}

fn render_ts_type_ref(type_ref: &TypeRef) -> String {
    match primitive_from_type_ref(type_ref) {
        Some(Primitive::Bool) => "boolean".to_owned(),
        Some(Primitive::I32 | Primitive::I64 | Primitive::F64) => "number".to_owned(),
        Some(Primitive::String) => "string".to_owned(),
        None => type_ref.name.clone(),
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
                ("cause", "Schema.optional(Schema.Unknown)"),
            ],
        ),
        (
            "TimeoutError",
            "TimeoutError",
            vec![
                ("message", "Schema.String"),
                ("timeoutMs", "Schema.optional(Schema.Number)"),
            ],
        ),
        (
            "EncodeError",
            "EncodeError",
            vec![
                ("message", "Schema.String"),
                ("cause", "Schema.optional(Schema.Unknown)"),
            ],
        ),
        (
            "DecodeError",
            "DecodeError",
            vec![
                ("message", "Schema.String"),
                ("cause", "Schema.optional(Schema.Unknown)"),
            ],
        ),
        (
            "UnexpectedStatusError",
            "UnexpectedStatusError",
            vec![
                ("message", "Schema.String"),
                ("status", "Schema.Number"),
                ("body", "Schema.optional(Schema.Unknown)"),
            ],
        ),
        (
            "RemoteProtocolError",
            "RemoteProtocolError",
            vec![
                ("message", "Schema.String"),
                ("body", "Schema.optional(Schema.Unknown)"),
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

fn render_error_def(error: &ErrorDef) -> String {
    let mut output = String::new();

    for variant in &error.variants {
        output.push_str(&render_error_variant(error, variant));
        output.push('\n');
    }

    output.push_str(&format!("export type {} =\n", error.ts_name));
    if error.variants.is_empty() {
        output.push_str("  never\n");
    } else {
        output.push_str(
            &error
                .variants
                .iter()
                .map(|variant| format!("  | {}", error_variant_class_name(error, variant)))
                .collect::<Vec<_>>()
                .join("\n"),
        );
        output.push('\n');
    }

    output.push('\n');
    output.push_str(&render_error_status_metadata(error));
    output.push('\n');
    output.push_str(&render_error_symbol_metadata(error));

    output
}

fn render_error_variant(error: &ErrorDef, variant: &ErrorVariant) -> String {
    let fields = variant
        .fields
        .iter()
        .map(|field| (field.ts_name.as_str(), render_field_schema(field)))
        .collect::<Vec<_>>();
    let fields = fields
        .iter()
        .map(|(name, schema)| (*name, schema.as_str()))
        .collect::<Vec<_>>();

    render_tagged_error_class(
        &error_variant_class_name(error, variant),
        &variant.tag,
        &fields,
    )
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

fn render_error_status_metadata(error: &ErrorDef) -> String {
    let mut output = format!("export const {}Status = {{\n", error.ts_name);
    for variant in &error.variants {
        output.push_str("  ");
        output.push_str(&render_property_key(&variant.tag));
        output.push_str(": ");
        output.push_str(&variant.status.0.to_string());
        output.push_str(",\n");
    }
    output.push_str("} as const\n");
    output
}

fn render_error_symbol_metadata(error: &ErrorDef) -> String {
    let mut output = format!("export const {}Symbols = {{\n", error.ts_name);
    for variant in &error.variants {
        output.push_str("  ");
        output.push_str(&render_property_key(&variant.tag));
        output.push_str(": {\n");
        output.push_str("    symbolId: ");
        output.push_str(&ts_string(variant.id.as_str()));
        output.push_str(",\n    rustName: ");
        output.push_str(&ts_string(&variant.rust_name));
        output.push_str(",\n    source: ");
        output.push_str(&render_source_range(&variant.source, 4));
        output.push_str(",\n  },\n");
    }
    output.push_str("} as const\n");
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
            format!("Schema.Tuple({items})")
        }
        TypeShape::List(item) => format!("Schema.Array({})", render_type_ref(item)),
        TypeShape::Map { key, value } => format!(
            "Schema.Record({{ key: {}, value: {} }})",
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

    format!("Schema.Union({})", variants.join(", "))
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
        Optionality::Optional => format!("Schema.optional({schema})"),
        Optionality::Nullable => format!("Schema.NullOr({schema})"),
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
        format!("Schema.declare<{}>((value) => true)", external.ts_name)
    } else {
        format!(
            "Schema.declare<{}, {}>((value) => true)",
            external.decoded_ts_name, external.encoded_ts_name
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
                            "nickname",
                            "nickname",
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
import { Schema } from "effect"

export const User = Schema.Struct({
  id: UserId,
  displayName: Schema.String,
  nickname: Schema.optional(Schema.String),
})
export type User = typeof User.Type
export type UserEncoded = typeof User.Encoded
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
            "export const UserEvent = Schema.Union(Schema.Struct({ _tag: Schema.Literal(\"created\") }), Schema.Struct({\n  _tag: Schema.Literal(\"renamed\"),\n  displayName: Schema.String,\n}))"
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
            "Schema.Record({ key: Schema.String, value: User })"
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

        assert!(rendered.contains("import { Effect } from \"effect\""));
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
            "  export const getUser = (\n    args: GetUserArgs\n  ): Effect.Effect<User, ApiClientError | GetUserError, ServerApi> =>\n    Effect.flatMap(ServerApi, (api) => api.users.getUser(args))"
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

        assert!(rendered.contains("import { Context, Layer, Effect } from \"effect\""));
        assert!(rendered.contains("import type { users } from \"./endpoints\""));
        assert!(rendered.contains("import type { User } from \"./schemas\""));
        assert!(rendered.contains("import type { ApiClientError, GetUserError } from \"./errors\""));
        assert!(rendered.contains(
            "export class ServerApi extends Context.Tag(\"@workspace/server-api::ServerApi\")<ServerApi, ServerApi.Service>() {}"
        ));
        assert!(rendered.contains(
            "      readonly getUser: (args: users.GetUserArgs) => Effect.Effect<User, ApiClientError | GetUserError, never>"
        ));
        assert!(rendered.contains(
            "  export const layer = (config: ServerApiConfig): Layer.Layer<ServerApi, never, never> =>"
        ));
        assert!(rendered.contains(
            "  export const mock = (service: Service): Layer.Layer<ServerApi, never, never> =>"
        ));
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
        }
    }
}

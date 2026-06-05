//! Effect v4 TypeScript generator backend.

use api_ir::{
    ApiContract, EnumShape, EnumVariant, ExternalType, Field, Optionality, Primitive, StructShape,
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

fn render_type_def(type_def: &TypeDef) -> String {
    let schema = render_type_shape(&type_def.shape, type_def);
    format!(
        "export const {name} = {schema}\nexport type {name} = typeof {name}.Type\nexport type {name}Encoded = typeof {name}.Encoded\n",
        name = type_def.ts_name,
    )
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

fn trim_trailing_blank_lines(mut output: String) -> String {
    while output.ends_with("\n\n") {
        output.pop();
    }
    output
}

#[cfg(test)]
mod tests {
    use api_ir::{
        ApiContract, EnumShape, EnumVariant, Field, Optionality, Primitive, SourceRange,
        StructShape, SymbolId, TypeDef, TypeRef, TypeShape,
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

    const fn source() -> SourceRange {
        SourceRange {
            file: String::new(),
            start_line: 0,
            start_column: 0,
            end_line: 0,
            end_column: 0,
        }
    }
}

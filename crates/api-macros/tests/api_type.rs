use api_core::{
    ir::{Optionality, TypeShape},
    ApiType,
};

#[derive(api_macros::ApiType)]
#[allow(dead_code)]
struct User {
    id: i64,
    name: String,
}

#[derive(api_macros::ApiType)]
#[allow(dead_code)]
struct UserId(i64);

#[derive(api_macros::ApiType)]
#[allow(dead_code)]
enum UserEvent {
    Created,
    Renamed { name: String },
}

#[test]
fn simple_struct_derives_api_type_metadata() {
    let type_def = User::type_def();

    assert_eq!(type_def.rust_name, "User");
    assert_eq!(type_def.ts_name, "User");
    assert!(type_def.source.start_line > 0);

    let TypeShape::Struct(shape) = type_def.shape else {
        panic!("expected struct shape");
    };

    assert_eq!(shape.fields.len(), 2);
    assert_eq!(shape.fields[0].rust_name, "id");
    assert_eq!(shape.fields[0].wire_name, "id");
    assert_eq!(shape.fields[0].optionality, Optionality::Required);
    assert_eq!(shape.fields[1].rust_name, "name");
}

#[test]
fn derive_uses_primitive_field_mappings() {
    let TypeShape::Struct(shape) = User::type_def().shape else {
        panic!("expected struct shape");
    };

    assert_eq!(shape.fields[0].type_ref.name, "i64");
    assert_eq!(shape.fields[1].type_ref.name, "String");
}

#[test]
fn newtype_derives_api_type_metadata() {
    let type_def = UserId::type_def();

    assert_eq!(type_def.rust_name, "UserId");
    assert!(matches!(type_def.shape, TypeShape::Newtype(_)));
}

#[test]
fn enums_derive_variant_metadata() {
    let TypeShape::Enum(shape) = UserEvent::type_def().shape else {
        panic!("expected enum shape");
    };

    assert_eq!(shape.variants.len(), 2);
    assert_eq!(shape.variants[0].rust_name, "Created");
    assert_eq!(shape.variants[0].wire_name, "Created");
    assert!(shape.variants[0].fields.is_empty());
    assert_eq!(shape.variants[1].rust_name, "Renamed");
    assert_eq!(shape.variants[1].fields[0].rust_name, "name");
    assert!(shape.variants[1].source.start_line > 0);
}

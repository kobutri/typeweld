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

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

#[derive(api_macros::ApiType)]
#[serde(rename_all = "camelCase")]
#[allow(dead_code)]
struct Profile {
    display_name: String,
    #[serde(rename = "avatarURL")]
    avatar_url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    nickname: Option<String>,
}

#[derive(api_macros::ApiType)]
#[serde(rename_all = "snake_case")]
#[allow(dead_code)]
enum RenamedEvent {
    UserCreated,
    #[serde(rename = "userRenamed")]
    UserRenamed {
        display_name: String,
    },
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

#[test]
fn serde_rename_lowering_updates_wire_names() {
    let TypeShape::Struct(shape) = Profile::type_def().shape else {
        panic!("expected struct shape");
    };

    assert_eq!(shape.fields[0].rust_name, "display_name");
    assert_eq!(shape.fields[0].wire_name, "displayName");
    assert!(shape.fields[0].source.start_line > 0);
    assert_eq!(shape.fields[1].wire_name, "avatarURL");
    assert_eq!(shape.fields[2].wire_name, "nickname");
    assert_eq!(shape.fields[2].optionality, Optionality::Optional);
}

#[test]
fn serde_rename_lowering_updates_variant_tags() {
    let TypeShape::Enum(shape) = RenamedEvent::type_def().shape else {
        panic!("expected enum shape");
    };

    assert_eq!(shape.variants[0].wire_name, "user_created");
    assert_eq!(shape.variants[1].wire_name, "userRenamed");
}

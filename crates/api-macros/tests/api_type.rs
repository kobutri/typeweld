use api_core::{
    ir::{ApiContract, Optionality, TypeShape},
    ApiType,
};
use serde::{Deserialize, Serialize};
use serde_json::json;

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
#[serde(tag = "_tag")]
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
#[allow(dead_code)]
struct Team {
    members: Vec<User>,
    owner: Option<Profile>,
}

#[derive(api_macros::ApiType)]
#[serde(tag = "_tag", rename_all = "snake_case")]
#[allow(dead_code)]
enum RenamedEvent {
    UserCreated,
    #[serde(rename = "userRenamed")]
    UserRenamed {
        display_name: String,
    },
}

#[derive(api_macros::ApiType, Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(tag = "_tag", rename_all = "PascalCase")]
#[allow(dead_code)]
enum TaggedUserEvent {
    UserCreated,
    #[serde(rename_all = "camelCase")]
    UserRenamed {
        display_name: String,
    },
}

#[derive(api_macros::ApiType, Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[allow(dead_code)]
struct AliasedProfile {
    #[serde(alias = "display_name")]
    display_name: String,
}

#[derive(api_macros::ApiType, Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(transparent)]
#[allow(dead_code)]
struct TransparentUserId {
    value: String,
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

#[test]
fn serde_internal_tagging_matches_generated_schema_expectations() {
    let value = TaggedUserEvent::UserRenamed {
        display_name: "Ada".to_owned(),
    };
    let json = serde_json::to_value(&value).expect("serialize event");

    assert_eq!(
        json,
        json!({
            "_tag": "UserRenamed",
            "displayName": "Ada",
        })
    );
    assert_eq!(
        serde_json::from_value::<TaggedUserEvent>(json).expect("deserialize event"),
        value
    );

    let TypeShape::Enum(shape) = TaggedUserEvent::type_def().shape else {
        panic!("expected enum shape");
    };
    assert_eq!(shape.variants[0].wire_name, "UserCreated");
    assert_eq!(shape.variants[1].wire_name, "UserRenamed");
    assert_eq!(shape.variants[1].fields[0].wire_name, "displayName");

    let schemas = api_gen_effect_v4::render_schemas(&ApiContract {
        package_name: "@workspace/test-api".to_owned(),
        endpoints: Vec::new(),
        types: vec![TaggedUserEvent::type_def()],
        errors: Vec::new(),
    });

    assert!(schemas.contains("_tag: Schema.Literal(\"UserRenamed\")"));
    assert!(schemas.contains("displayName: Schema.String"));
}

#[test]
fn serde_alias_and_deny_unknown_fields_keep_canonical_wire_name() {
    let value = AliasedProfile {
        display_name: "Ada".to_owned(),
    };

    assert_eq!(
        serde_json::to_value(&value).expect("serialize profile"),
        json!({ "displayName": "Ada" })
    );
    assert_eq!(
        serde_json::from_value::<AliasedProfile>(json!({ "display_name": "Ada" }))
            .expect("deserialize alias"),
        value
    );
    assert!(serde_json::from_value::<AliasedProfile>(
        json!({ "displayName": "Ada", "extra": true })
    )
    .is_err());

    let TypeShape::Struct(shape) = AliasedProfile::type_def().shape else {
        panic!("expected struct shape");
    };
    assert_eq!(shape.fields[0].wire_name, "displayName");
}

#[test]
fn serde_transparent_named_newtype_uses_inner_wire_shape() {
    let value = TransparentUserId {
        value: "user_123".to_owned(),
    };

    assert_eq!(
        serde_json::to_value(&value).expect("serialize transparent newtype"),
        json!("user_123")
    );
    assert!(matches!(
        TransparentUserId::type_def().shape,
        TypeShape::Newtype(_)
    ));
}

#[test]
fn derive_registers_nested_container_types_once() {
    let mut registry = api_core::TypeRegistry::new();

    Team::register_types(&mut registry);
    Team::register_types(&mut registry);

    let rust_names = registry
        .type_defs()
        .into_iter()
        .map(|type_def| type_def.rust_name)
        .collect::<Vec<_>>();

    assert_eq!(rust_names, ["Team", "User", "Profile"]);
}

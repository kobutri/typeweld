use api_core::{ApiError, ApiType};
use serde::{Deserialize, Serialize};
use serde_json::json;

#[derive(api_macros::ApiType, Clone, Debug, Deserialize, PartialEq, Serialize)]
#[allow(dead_code)]
struct ValidationIssue {
    code: String,
}

#[derive(api_macros::ApiError, Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(tag = "_tag", rename_all = "camelCase")]
#[allow(dead_code)]
enum CreateUserError {
    #[api_error(status = 404)]
    NotFound,
    #[api_error(status = 409)]
    Conflict { field_name: String },
    #[serde(rename = "validationFailed")]
    #[api_error(status = 422)]
    Validation { issue: ValidationIssue },
}

#[test]
fn api_error_derive_emits_error_def() {
    let error = CreateUserError::error_def();

    assert_eq!(error.rust_name, "CreateUserError");
    assert_eq!(error.variants.len(), 3);
    assert!(error.source.start_line > 0);
}

#[test]
fn api_error_derive_maps_statuses_and_tags() {
    let error = CreateUserError::error_def();

    assert_eq!(error.variants[0].rust_name, "NotFound");
    assert_eq!(error.variants[0].tag, "notFound");
    assert_eq!(error.variants[0].status.0, 404);
    assert_eq!(error.variants[1].tag, "conflict");
    assert_eq!(error.variants[1].status.0, 409);
    assert_eq!(error.variants[2].tag, "validationFailed");
    assert_eq!(error.variants[2].status.0, 422);
}

#[test]
fn api_error_serde_tag_matches_generated_error_tags() {
    let value = CreateUserError::Validation {
        issue: ValidationIssue {
            code: "missing".to_owned(),
        },
    };
    let json = serde_json::to_value(&value).expect("serialize error");

    assert_eq!(
        json,
        json!({
            "_tag": "validationFailed",
            "issue": {
                "code": "missing",
            },
        })
    );
    assert_eq!(
        serde_json::from_value::<CreateUserError>(json).expect("deserialize error"),
        value
    );
    assert_eq!(
        CreateUserError::error_def().variants[2].tag,
        "validationFailed"
    );
}

#[test]
fn api_error_derive_emits_payload_fields_and_type_metadata() {
    let error = CreateUserError::error_def();

    assert_eq!(error.variants[1].fields[0].rust_name, "field_name");
    assert_eq!(error.variants[1].fields[0].type_ref.name, "String");
    assert!(CreateUserError::type_def().source.start_line > 0);
    assert_eq!(CreateUserError::error_ref().name, "CreateUserError");
}

#[test]
fn api_error_registers_error_and_payload_types() {
    let mut registry = api_core::ContractRegistry::new();

    CreateUserError::register_error(&mut registry);
    CreateUserError::register_error(&mut registry);

    let errors = registry.error_defs();
    let types = registry.type_defs();

    assert_eq!(errors.len(), 1);
    assert_eq!(errors[0].rust_name, "CreateUserError");
    assert_eq!(types.len(), 1);
    assert_eq!(types[0].rust_name, "ValidationIssue");
}

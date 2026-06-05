use api_core::{ApiError, ApiType};

#[derive(api_macros::ApiError)]
#[serde(rename_all = "camelCase")]
#[allow(dead_code)]
enum CreateUserError {
    #[api_error(status = 404)]
    NotFound,
    #[api_error(status = 409)]
    Conflict { field_name: String },
    #[serde(rename = "validationFailed")]
    #[api_error(status = 422)]
    Validation { message: String },
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
fn api_error_derive_emits_payload_fields_and_type_metadata() {
    let error = CreateUserError::error_def();

    assert_eq!(error.variants[1].fields[0].rust_name, "field_name");
    assert_eq!(error.variants[1].fields[0].type_ref.name, "String");
    assert!(CreateUserError::type_def().source.start_line > 0);
    assert_eq!(CreateUserError::error_ref().name, "CreateUserError");
}

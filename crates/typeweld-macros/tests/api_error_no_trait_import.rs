#[derive(typeweld_macros::ApiError, serde::Serialize)]
#[serde(tag = "_tag")]
#[allow(dead_code)]
enum TraitImportError {
    #[api_error(status = 400)]
    BadRequest,
}

#[test]
fn api_error_derive_does_not_require_api_type_import() {
    let mut registry = typeweld_core::ContractRegistry::new();
    registry.register_error::<TraitImportError>();

    assert_eq!(registry.error_defs()[0].rust_name, "TraitImportError");
}

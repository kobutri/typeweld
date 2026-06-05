//! Compiler-backed API contract collection.

use api_ir::ApiContract;

#[must_use]
pub fn collect_empty_contract(package_name: impl Into<String>) -> ApiContract {
    ApiContract {
        package_name: package_name.into(),
        ..ApiContract::default()
    }
}

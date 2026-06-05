//! Effect v4 TypeScript generator backend.

use api_ir::ApiContract;

#[must_use]
pub fn render_package_banner(contract: &ApiContract) -> String {
    format!("// Generated API package for {}\n", contract.package_name)
}

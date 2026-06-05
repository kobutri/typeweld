//! Axum integration adapter placeholder.

use api_core::Endpoint;

#[must_use]
pub fn endpoint_path(endpoint: &Endpoint) -> &str {
    &endpoint.route.0
}

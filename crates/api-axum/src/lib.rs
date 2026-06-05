//! Axum integration adapter placeholder.

use api_core::Endpoint;

#[must_use]
pub const fn endpoint_path(endpoint: Endpoint) -> &'static str {
    endpoint.path
}

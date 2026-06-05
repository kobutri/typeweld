//! Framework-neutral public API primitives.

/// Marker trait for Rust types that can cross the generated API boundary.
pub trait ApiType {}

/// Marker trait for declared, typed API errors.
pub trait ApiError: ApiType {}

/// Placeholder endpoint descriptor used until the contract IR exists.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Endpoint {
    pub method: &'static str,
    pub path: &'static str,
}

impl Endpoint {
    #[must_use]
    pub const fn new(method: &'static str, path: &'static str) -> Self {
        Self { method, path }
    }
}

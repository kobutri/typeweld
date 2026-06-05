//! Shared API contract intermediate representation.

/// Minimal placeholder contract until T-002 defines the serializable IR.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ApiContract {
    pub package_name: String,
}

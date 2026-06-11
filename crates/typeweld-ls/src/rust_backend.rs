//! Seam for a future private rust-analyzer child process.
//!
//! A later phase will spawn a dedicated rust-analyzer over the workspace so
//! Rust-initiated renames become fully semantic (updating call sites the
//! contract cannot see). The server currently performs contract-based renames
//! only; this module is the integration point where that child process will
//! be spawned, initialized, and queried.

/// Placeholder handle for the future rust-analyzer backend.
#[derive(Debug, Default)]
pub struct RustBackend {}

impl RustBackend {
    /// Whether a semantic Rust backend is available; always `false` for now.
    // An instance method on purpose: a real backend holds a child process.
    #[allow(clippy::unused_self)]
    pub fn available(&self) -> bool {
        false
    }
}

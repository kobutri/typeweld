//! The typeweld engine.
//!
//! Everything the CLI and the language server share lives here:
//!
//! - [`workspace`]: cargo workspace discovery from `Cargo.toml` files alone
//! - [`extract`]: the static extractor (Rust sources → [`ir::Contract`])
//! - [`gen`]: the TypeScript generator (contract → Effect client package)
//! - [`usage`]: TypeScript usage analysis for generated bindings
//! - [`config`]: `typeweld.toml` project configuration
//!
//! Nothing in this crate invokes cargo or any other external tool; extraction
//! and generation are pure functions over source text, fast enough to run on
//! every keystroke.

pub mod diag;
pub mod extract;
pub mod ir;
pub mod line_index;
pub mod vfs;
pub mod workspace;

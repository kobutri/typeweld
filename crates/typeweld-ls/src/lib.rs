//! The typeweld language server.
//!
//! A standalone *sidecar* server: the editor keeps running its normal
//! rust-analyzer and tsserver untouched, while typeweld-ls attaches to both
//! Rust and TypeScript files and contributes only the cross-language features
//! the contract makes possible — goto-definition and references across the
//! Rust/TypeScript boundary, contract hovers, bidirectional rename, live
//! regeneration of the client package, and extraction diagnostics.

mod convert;
mod features;
mod rust_backend;
mod server;
mod state;
mod ts_backend;

/// Runs the language server on stdio.
///
/// # Errors
/// Returns an error description when the handshake or transport fails.
pub fn run_stdio() -> Result<(), String> {
    let (connection, io_threads) = lsp_server::Connection::stdio();
    let result = server::run(&connection);
    // The writer thread only finishes once the connection's channel closes.
    drop(connection);
    io_threads.join().map_err(|error| error.to_string())?;
    result
}

/// Runs the language server over an existing connection, performing the
/// initialize handshake itself. Tests drive this over
/// [`lsp_server::Connection::memory`].
///
/// # Errors
/// Returns an error description when the handshake or transport fails.
// By value on purpose: the server owns the connection for its whole lifetime.
#[allow(clippy::needless_pass_by_value)]
pub fn run_with_connection(connection: lsp_server::Connection) -> Result<(), String> {
    server::run(&connection)
}

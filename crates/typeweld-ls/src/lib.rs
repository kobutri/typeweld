//! The typeweld language server.
//!
//! typeweld-ls is the Rust language server the editor runs: behind it lives
//! the user's real rust-analyzer, transparently proxied, while typeweld
//! contributes the cross-language features the contract makes possible —
//! TypeScript halves of renames, TypeScript usages in references, contract
//! hovers, live regeneration of the client package, and extraction
//! diagnostics. The TypeScript side is served by `@typeweld/typescript-plugin`
//! running inside the user's own tsserver, connected back over the discovery
//! file `target/typeweld/ls.json`.
//!
//! Without a usable rust-analyzer the server degrades to engine-only
//! answers; Rust editing then falls back to whatever the editor has.

mod convert;
mod features;
mod plugin;
mod proxy;
mod ra;
mod state;

pub use proxy::Options;

/// Runs the language server on stdio.
///
/// # Errors
/// Returns an error description when the handshake or transport fails.
pub fn run_stdio(options: &Options) -> Result<(), String> {
    let (connection, io_threads) = lsp_server::Connection::stdio();
    let result = proxy::run(&connection, options);
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
    proxy::run(&connection, &Options::default())
}

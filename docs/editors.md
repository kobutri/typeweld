# Editor setup

Typeweld's language server is the Rust language server your editor runs:
behind it, typeweld spawns and transparently proxies your real
rust-analyzer, adding the cross-language features in-band — renames whose
single WorkspaceEdit covers both languages, references that include
TypeScript usages, contract hovers, extraction diagnostics, and live
regeneration of the bindings on every edit.

The TypeScript side needs no extra language server: the
`@typeweld/typescript-plugin` tsserver plugin runs inside whatever already
powers your TypeScript editing (VS Code's built-in TypeScript,
typescript-language-server, vtsls) and connects to the typeweld server
through `target/typeweld/ls.json`. It serves goto-definition from bindings
into Rust, contract hovers, and the TypeScript side of renames.

rust-analyzer is resolved from `--rust-analyzer`, `TYPEWELD_RUST_ANALYZER`,
`PATH`, then `rustup which rust-analyzer`. If none works, typeweld still
serves its own features and Rust editing degrades until one appears.

## VS Code

Install the typeweld extension. In a workspace containing `typeweld.toml`
it asks once whether to route rust-analyzer through typeweld; accepting
updates `rust-analyzer.server.path` in workspace settings (your original
server keeps running behind the proxy, and the rust-analyzer extension
keeps all of its UI). Revert any time with *Typeweld: Disable rust-analyzer
Integration*. The TypeScript plugin is bundled with the extension and loads
into VS Code's TypeScript automatically — no settings needed.

## Neovim (0.11+)

Point your Rust language server at typeweld instead of rust-analyzer — one
server, not two; rust-analyzer settings pass through unchanged:

```lua
vim.lsp.config("typeweld", {
  cmd = { "typeweld", "lsp" },
  root_markers = { "typeweld.toml", "Cargo.toml" },
  filetypes = { "rust" },
  settings = { ["rust-analyzer"] = {} }, -- your usual rust-analyzer settings
})
vim.lsp.enable("typeweld")
```

For TypeScript, load the plugin into typescript-language-server (install it
into your app first: `npm i -D @typeweld/typescript-plugin`):

```lua
vim.lsp.config("ts_ls", {
  init_options = {
    plugins = {
      {
        name = "@typeweld/typescript-plugin",
        location = vim.fn.getcwd() .. "/app/node_modules/@typeweld/typescript-plugin",
      },
    },
  },
})
```

## Helix

```toml
[language-server.typeweld]
command = "typeweld"
args = ["lsp"]

[[language]]
name = "rust"
language-servers = ["typeweld"]
```

For TypeScript, configure typescript-language-server with the same
`plugins` initialization option as the Neovim snippet:

```toml
[language-server.typescript-language-server.config.plugins]
# helix passes initializationOptions through config; see your distro docs.
```

Renames work in-band in every editor: rename a Rust API symbol and the
response already contains the TypeScript edits; rename in TypeScript and
typeweld applies the Rust half right after the editor applies the
TypeScript side — straight to disk for files the editor does not have
open, via `workspace/applyEdit` for the rest. In VS Code the extension
then saves the edited documents, matching how the editor saves its own
rename refactorings.

# Editor setup

The typeweld language server is a sidecar: keep rust-analyzer and your
TypeScript server exactly as they are, and run `typeweld lsp` alongside them
for both Rust and TypeScript files. It contributes cross-language
goto-definition, references, hover, rename, diagnostics, and regenerates
bindings on every edit.

## VS Code

Install the typeweld extension. It activates in workspaces containing
`typeweld.toml`, finds the `typeweld` binary (configurable via
`typeweld.server.path`), and starts the language server for Rust and
TypeScript documents. The bundled tsserver plugin hides generated files from
TypeScript's own navigation results so you land in Rust, not in
`target/typeweld/`.

## Neovim (0.11+)

```lua
vim.lsp.config("typeweld", {
  cmd = { "typeweld", "lsp" },
  root_markers = { "typeweld.toml" },
  filetypes = { "rust", "typescript", "typescriptreact" },
})
vim.lsp.enable("typeweld")
```

Definition, references, and hover results from multiple servers are merged
automatically. For rename on Rust API symbols, route to typeweld:

```lua
vim.keymap.set("n", "<leader>rn", function()
  vim.lsp.buf.rename(nil, { filter = function(c) return c.name == "typeweld" end })
end)
```

## Helix

```toml
[language-server.typeweld]
command = "typeweld"
args = ["lsp"]

[[language]]
name = "rust"
language-servers = ["typeweld", "rust-analyzer"]

[[language]]
name = "typescript"
language-servers = ["typescript-language-server", "typeweld"]
```

Helix routes each request to the first server that supports it, so listing
typeweld first for Rust gives it rename and definition priority; typeweld
returns nothing for non-API symbols and helix falls through to
rust-analyzer.

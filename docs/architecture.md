# Architecture

## The static extractor (`typeweld-engine::extract`)

Generation never invokes cargo. The extractor parses workspace sources with
syn, walks module trees (following `mod` declarations, `#[path]`, inline
modules), and resolves type paths structurally: per-module symbol tables built
from `use` imports (including renames, re-export chains, and globs of
workspace modules), workspace type aliases, a builtin prelude, and a fixed
registry of external types (uuid, chrono `DateTime<Utc>`, rust_decimal,
serde_json). The result is a serializable `Contract` with byte-offset spans
for every declaration — the single source of truth for the generator, the
CLI lints, and the language server.

### Macro / extractor parity

Anything that compiles must extract, and vice versa:

1. Every local validation rule (the serde subset, endpoint signature rules,
   rename rules) lives once in `typeweld-syntax`, used by both the proc
   macros (as compile errors) and the extractor (as diagnostics).
2. What macros cannot see — cross-file type resolution — is enforced at
   compile time by `ApiBound` assertions emitted per field/param/payload.
3. CI runs every macro UI fixture through the extractor and asserts
   identical accept/reject decisions.

## The generator (`typeweld-engine::gen`)

A minimal typed TypeScript AST (~15 node kinds) plus a printer that records
the byte and UTF-16 range of every marked node while printing. The marks —
Rust symbol to generated TypeScript span — are therefore correct by
construction; the language server consumes them directly. Output is
deterministic, topologically sorted, and tree-shakeable (`sideEffects:
false`, no namespaces, config-only service layer).

## Usage analysis (`typeweld-engine::usage`)

An oxc-based scanner resolves imports of the generated package's module
specifiers through user TypeScript code (named imports, renames, namespace
members, re-exports) and records every reference with a strength
(Strong/Weak). It powers the unused-endpoint lint and the language server's
cross-language references; `Effect.catchTag("Tag", ...)` literals are
recognized as error-variant usages.

## The language server (`typeweld-ls`)

A standalone sidecar — it never proxies rust-analyzer or tsserver. The
editor runs its usual servers; typeweld-ls attaches to Rust and TypeScript
files and contributes only cross-language features. All state (contract,
marks, usage index, diagnostics) lives in memory and is recomputed on every
relevant edit — extraction is milliseconds, so there are no caches to go
stale. Generated bindings are written to `target/typeweld/` only so the
user's tsserver can resolve imports; they are content-diffed and never read
back.

Rebuilds run on short-lived worker threads (proc-macro2 keeps a thread-local
source map; dropping the thread reclaims it). Requests are answered after an
on-demand rebuild, so responses are never stale.

## CLI (`typeweld-cli`)

`new` (scaffold), `generate [--watch]` (extract + emit; watch uses the same
engine code as the LSP), `check [--unused]` (diagnostics + usage lints, CI
exit codes), `lsp` (the language server). One binary, shipped on npm as
`typeweld` with per-platform optional dependencies.

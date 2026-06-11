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

typeweld-ls is the Rust language server the editor runs; behind it lives
the user's real rust-analyzer, spawned by typeweld and transparently
proxied. Every message is forwarded verbatim — opaque JSON, identical ids —
except a small fail-open interception whitelist: rename responses gain the
TypeScript half of an API rename (one atomic WorkspaceEdit), references
gain TypeScript usages, hovers gain the contract block, publishDiagnostics
gains extraction diagnostics, and document sync is observed in passing for
live regeneration. Fail-open means an interception error forwards the
original message untouched: a typeweld bug can cost typeweld features,
never Rust editing. A crashed rust-analyzer is respawned with the handshake
and open documents replayed; an unusable one degrades the server to
engine-only answers.

The TypeScript side runs *inside* the user's own tsserver:
`@typeweld/typescript-plugin` discovers the server through
`target/typeweld/ls.json` and connects over localhost TCP (token
authenticated). The server pushes a snapshot of every generated-file mark
(with Rust target and hover text); all synchronous plugin hooks answer from
that local replica — no I/O inside a hook. Over the same socket the server
asks the plugin for semantic TypeScript answers (the property accesses a
field rename must cover), answered on the node event loop with the
project's real LanguageService — so TypeScript analysis is never duplicated
and always agrees with what the editor shows. TypeScript-initiated renames
of API symbols cannot carry Rust edits through the tsserver protocol (the
new name never reaches tsserver), so the plugin filters the generated
locations from the editor's rename, watches the edit land, and reports the
observed new name; the server then applies the Rust complement via
`workspace/applyEdit` using its rust-analyzer.

All engine state (contract, marks, usage index, diagnostics) lives in
memory and is recomputed on every relevant edit — extraction is
milliseconds, so there are no caches to go stale. Generated bindings are
written to `target/typeweld/` only so tsserver can resolve imports; they
are content-diffed and never read back.

Rebuilds run on short-lived worker threads (proc-macro2 keeps a thread-local
source map; dropping the thread reclaims it). Requests are answered after an
on-demand rebuild, so responses are never stale.

## CLI (`typeweld-cli`)

`new` (scaffold), `generate [--watch]` (extract + emit; watch uses the same
engine code as the LSP), `check [--unused]` (diagnostics + usage lints, CI
exit codes), `lsp` (the language server). One binary, shipped on npm as
`typeweld` with per-platform optional dependencies.

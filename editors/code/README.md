# Honk Hoon for VS Code

This extension launches `honk-lsp` directly over stdio. It does not route LSP
traffic through the gRPC daemon. The current feature set includes unsaved-buffer
diagnostics, hierarchical symbols for Hoon arms and molds, and structural hover
augmented with inferred types after the matching compiler check completes.
Workspace symbol search indexes arms and molds in unopened configured sources
and the standard-library prelude.
Go-to-definition follows compiler-resolved core arms and imported gates, with a
conservative structural fallback for unambiguous arms and molds in the current
document, its import graph, and the configured prelude. Rune tokens navigate to
their canonical tagged alternatives in the prelude's `hoon` mold; `++`, `+$`,
and `+|` navigate to the corresponding hoon-138 parser arms. Lexical faces have
scope- and shadow-aware definition, references, and safe rename support.
Completion suggests visible faces, named gate imports, and unambiguous local,
imported, and standard-library arms and molds, and reports each candidate's
provenance. Compiler-resolved imported arms and gates have cross-file references
within the latest successfully checked import graph. Structurally resolved molds
and standard-library symbols have references across every open or configured
root's import graph.

In VS Code, use F12 for definitions, Shift+F12 for references, F2 to rename,
Control+Space to request completion explicitly, and Command+T on macOS or
Control+T elsewhere to search workspace symbols. References for lexical faces
are same-document; compiler-resolved arms and gates can span files, as can
unambiguous imported molds and standard-library symbols. Safe structural rename
can edit unopened configured sources and declines ambiguous, colliding, or
capturing changes.

## Development setup

From the nockchain repository root:

```sh
cargo build --release -p honk-lsp
cd editors/code
npm ci
npm test
```

Open the repository in VS Code and install the extension from this directory or
run its extension-development launch configuration. With an empty
`honk.server.path`, the extension checks `target/release/honk-lsp`, then
`target/debug/honk-lsp`, then `PATH`.

The default repository layout resolves the dependency root to `hoon` and the
prelude to `hoon/common/hoon.hoon`. Other projects can set
`honk.dependenciesPath` and `honk.preludePath`. Set `honk.entryPath` when every
edit should check a stable application or kernel entry rather than the most
recently edited Hoon file.

The first semantic check initializes the persistent compiler and can take a
while. Later checks reuse compiler state. Newer editor generations supersede
older work, so a slow check cannot publish stale diagnostics.

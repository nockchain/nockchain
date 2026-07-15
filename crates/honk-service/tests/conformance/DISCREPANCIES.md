# Known discrepancies

None for the behavior covered by the current editor-session contract.

The service accepts versioned full document snapshots. `honk-lsp` advertises
LSP full-document synchronization, so no UTF-16 ranged-edit translation occurs
between the client and this contract. Content-only changes use dependency-graph
invalidation; configuration, prelude, subject-type, and workspace-layout
changes deliberately fall back to a fresh compiler epoch.

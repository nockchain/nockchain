# Known LSP divergences

## DISC-001: Full document synchronization only

- **Reference:** LSP 3.17 permits full or incremental synchronization.
- **Implementation:** Honk advertises `TextDocumentSyncKind.Full`; ranged changes are rejected and logged.
- **Impact:** Clients must honor the negotiated sync mode. VS Code does.
- **Resolution:** ACCEPTED until an adapter-side text edit implementation has independent UTF-16 conformance coverage.
- **Tests affected:** ranged `textDocument/didChange` (not advertised).
- **Review date:** 2026-07-15.

## DISC-002: Cooperative request cancellation is not implemented

- **Reference:** LSP permits `$/cancelRequest`; cancellation still requires a response.
- **Implementation:** Honk ignores cancellation notifications. Compiler checks are isolated on a worker and stale results are suppressed by document generation.
- **Impact:** CPU work already in progress completes, but obsolete diagnostics are not published and the protocol loop remains responsive.
- **Resolution:** ACCEPTED pending safe compiler cancellation points.
- **Tests affected:** `$/cancelRequest` during a honk check.
- **Review date:** 2026-07-15.

## DISC-003: Diagnostics-first capability surface

- **Reference:** LSP defines many optional language features.
- **Implementation:** Honk currently advertises only document synchronization and push diagnostics.
- **Impact:** No completion, navigation, hover, formatting, symbols, semantic tokens, or pull diagnostics yet.
- **Resolution:** INVESTIGATING; add features only when the compiler exposes a stable semantic query API.
- **Tests affected:** all unadvertised optional feature requests.
- **Review date:** 2026-07-15.

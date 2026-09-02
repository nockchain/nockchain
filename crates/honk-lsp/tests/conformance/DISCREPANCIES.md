# Known LSP divergences

## DISC-001: Full document synchronization only

- **Reference:** LSP 3.17 permits full or incremental synchronization.
- **Implementation:** Honk advertises `TextDocumentSyncKind.Full`; ranged changes are rejected and logged.
- **Impact:** Clients must honor the negotiated sync mode. VS Code does.
- **Resolution:** ACCEPTED until an adapter-side text edit implementation has independent UTF-16 conformance coverage.
- **Tests affected:** ranged `textDocument/didChange` (not advertised).
- **Review date:** 2026-07-15.

## DISC-002: Whole-parse cancellation is cooperative

- **Reference:** LSP permits `$/cancelRequest`; cancellation still requires a response.
- **Implementation:** Honk immediately completes a cancelled semantic request with `RequestCanceled`, marks its worker job cancelled, and suppresses any later result. Cancellation is checked before and after the existing whole-document parser call; that call has no internal cancellation points.
- **Impact:** A parse already in progress may consume CPU until it returns, but it cannot block JSON-RPC handling or publish a cancelled result.
- **Resolution:** INVESTIGATING; add internal cancellation points only with the future editor-specific incremental parser path.
- **Tests affected:** `cancellation_and_other_requests_remain_responsive_during_semantic_indexing` and `client_cancellation_completes_the_semantic_request_once`.
- **Review date:** 2026-07-15.

## DISC-003: Limited workspace-wide semantic provenance

- **Reference:** LSP defines many optional language features.
- **Implementation:** Honk advertises document and workspace symbols, hover, definition, completion, references, and rename. An editor-only parsed side table owns lexical scope and structural symbol identity; compiler side tables remain authoritative for inferred types and resolved core arms/gates. Structural definition, completion, references, and rename traverse the native import graph and decline ambiguous names. Open-document overlays shadow disk. References preserve exact same-document lexical identity, use a generation-stamped declaration-keyed compiler fact index for resolved arms and gates, and use a lightweight structural graph for molds and prelude symbols across open or configured roots. Workspace symbol search enumerates arms and molds from the same structural declaration index.
- **Impact:** Local faces support definition, references, rename, and completion. Compiler-resolved imported arms and gates, structurally resolved molds, and standard-library symbols support cross-file references. Unambiguous local, imported, and standard-library arms and molds support definition and completion, rune tokens navigate to their canonical prelude implementation, and safe structural rename can edit unopened configured sources. Files outside every open or configured root's import graph are not searched. Formatting, semantic tokens, and pull diagnostics remain unadvertised.
- **Resolution:** PARTIAL; the configured workspace-root model, complete structural graph, collision checks, and workspace symbol search are implemented without changing the compiler AST or ordinary CLI/batch results. Rich compiler provenance for every structural declaration remains future work.
- **Tests affected:** references, rename, and workspace symbols across unopened configured roots.
- **Review date:** 2026-07-17.

## DISC-004: Whole-document semantic snapshot rebuilds

- **Reference:** Interactive language servers should keep edit-to-query latency low as documents and workspaces grow.
- **Implementation:** Identical path/version/content requests hit the semantic snapshot cache. The workspace structural index tracks per-file revisions separately from import-layout revisions: ordinary edits reload only the affected source and retain every unchanged source/declaration table, while create/delete events recompute import edges. A changed document is still reparsed in full and its traced AST is serialized once to build editor side tables; stable IDs are then reconciled with the preceding snapshot.
- **Impact:** Unrelated editor generations no longer reread, rescan, or reparse unchanged workspace files. Results and invalidation remain correct and the protocol thread remains responsive, but work for the changed document itself still scales with that document's size.
- **Resolution:** INVESTIGATING; replace full rebuilds with dependency- and node-granular invalidation without changing the CLI/compiler path.
- **Tests affected:** `structural_graph_refresh_reuses_unchanged_source_indexes`, watcher conformance, and semantic-query latency under sustained edits.
- **Review date:** 2026-07-17.

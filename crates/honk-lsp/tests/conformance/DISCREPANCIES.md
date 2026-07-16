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

## DISC-003: Limited semantic capability surface

- **Reference:** LSP defines many optional language features.
- **Implementation:** Honk advertises document symbols and hover backed by an editor-only parsed side table. After a successful compiler check, hover selects the narrowest current debug spot and displays its owned inferred-type summary. Go-to-definition uses a second owned compiler side table for resolved core arms and imported gates, then conservatively falls back to unique arm or mold headers in the same document, the dependency graph returned by Honk's native import resolver, and the configured prelude. Two-glyph gene runes resolve to their canonical tagged alternative in the prelude's `hoon` mold. Core declaration syntax maps explicitly to its hoon-138 parser arm: `++` to `++bola`, `+$` to `++boba`, and `+|` to `++whip`. Open-document overlays shadow disk during these lookups. Duplicate names at the same import depth return null rather than guessing. Native types do not yet retain source provenance for local face or binding declarations.
- **Impact:** Standard-library molds and types declared as arms, including `list`, transitively imported molds such as `tip5-hash-atom`, gene runes such as `^-`, and core declaration runes such as `++` navigate to their canonical Hoon source. Definition requests for local faces and bindings still return null. No completion, references, formatting, semantic tokens, or pull diagnostics yet.
- **Resolution:** INVESTIGATING; add declaration provenance at the native resolution boundary without changing the compiler AST or ordinary CLI/batch results.
- **Tests affected:** all unadvertised optional feature requests.
- **Review date:** 2026-07-15.

## DISC-004: Whole-document semantic snapshot rebuilds

- **Reference:** Interactive language servers should keep edit-to-query latency low as documents and workspaces grow.
- **Implementation:** Identical path/version/content requests hit the semantic snapshot cache. A changed document is currently reparsed in full and its traced AST is serialized once to build editor side tables; stable IDs are then reconciled with the preceding snapshot.
- **Impact:** Results and invalidation are correct and the protocol thread remains responsive, but changed-document work still scales with document size.
- **Resolution:** INVESTIGATING; replace full rebuilds with dependency- and node-granular invalidation without changing the CLI/compiler path.
- **Tests affected:** semantic-query latency under sustained edits; functional conformance is unaffected.
- **Review date:** 2026-07-15.

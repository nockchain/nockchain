# Honk LSP conformance coverage

Specification baseline: Language Server Protocol 3.17. The server intentionally
implements a narrow diagnostics-and-structural-semantics capability set and remains compatible with
clients that implement later, capability-negotiated LSP versions.

| Area | Requirement | Level | Test | Status |
|---|---|---:|---|---|
| Base protocol | `Content-Length` framed UTF-8 JSON-RPC 2.0 messages | MUST | `stdio_lifecycle_uses_lsp_framing_and_json_rpc_responses` | Passing |
| Lifecycle | `initialize` is the first request and receives capabilities | MUST | stdio lifecycle | Passing |
| Lifecycle | `initialized` completes initialization | MUST | stdio lifecycle | Passing |
| Lifecycle | `shutdown` receives a null success response and `exit` terminates | MUST | stdio lifecycle | Passing |
| Document sync | Server advertises open/close and full-content changes | MUST for advertised capability | stdio lifecycle | Passing |
| Positions | Server advertises and emits UTF-16 positions | MUST for negotiated positions | stdio lifecycle + unit range conversion | Passing |
| Diagnostics | Diagnostics use `textDocument/publishDiagnostics` | MUST for push diagnostics | `unsaved_parse_error_is_published_for_the_current_document_version` | Passing |
| Diagnostics | Published diagnostics identify the checked document version | SHOULD | unsaved parse-error integration test | Passing |
| Diagnostics | Closed documents have their diagnostics cleared | SHOULD | unsaved parse-error integration test | Passing |
| Freshness | Results from an older document generation are not published | implementation invariant | `stale_worker_generation_does_not_publish_diagnostics` | Passing |
| Document symbols | Advertised symbols use hierarchical `DocumentSymbol` responses and valid enclosing/selection ranges | MUST for advertised capability | `document_symbols_hover_and_definition_use_current_unsaved_snapshot` | Passing |
| Hover | Advertised hover responds from the current unsaved document snapshot and adds compiler-owned inferred types when the matching check completes | MUST for advertised capability | `document_symbols_hover_and_definition_use_current_unsaved_snapshot` | Passing |
| Definition | Advertised go-to-definition follows compiler-resolved core arms in the current unsaved snapshot | MUST for advertised capability | `document_symbols_hover_and_definition_use_current_unsaved_snapshot` | Passing |
| Definition | Imported gate definitions resolve to another file using compiler-owned provenance | MUST for advertised capability | `definition_navigates_to_an_imported_gate` | Passing |
| Definition | An unambiguous same-document arm or mold resolves structurally when compiler provenance is unavailable | implementation fallback | `definition_navigates_to_a_hyphenated_mold_arm` | Passing |
| Definition | Real `miner.hoon` same-file mold uses resolve despite large atom literals in its parsed AST | real-source regression | `real_miner_definitions_resolve_local_transitive_prelude_and_rune_symbols` | Passing |
| Definition | A mold inherited through the real `zeke` → `ztd-eight` → … → `ztd-four` import chain resolves to its source file | implementation fallback | `real_miner_definitions_resolve_local_transitive_prelude_and_rune_symbols` (`tip5-hash-atom`) | Passing |
| Definition | A standard-library mold resolves into the configured prelude | implementation fallback | `real_miner_definitions_resolve_local_transitive_prelude_and_rune_symbols` (`list`) | Passing |
| Definition | F12 on either glyph of a rune resolves to its canonical tagged alternative or core-declaration parser arm in the configured hoon-138 prelude | implementation fallback | `real_miner_definitions_resolve_local_transitive_prelude_and_rune_symbols` (`^-`, `=/`, `\|=`, `?:`, `++`, `+$`) | Passing |
| Completion | Advertised completion replaces the whole current term and includes only lexical faces visible at the current unsaved cursor position | MUST for advertised capability | `completion_respects_lexical_scope_and_includes_the_standard_library` | Passing |
| Completion | Local symbols shadow imports, and imports shadow the configured prelude using the same nearest-unambiguous import traversal as definition | implementation invariant | imported-gate completion test + synthetic completion test + real `miner.hoon` cases (`dig`, `tip5-hash-atom`, `list`) | Passing |
| References | Advertised references preserve lexical binding identity across nested shadowing and honor `includeDeclaration` | MUST for advertised capability | `local_references_and_rename_preserve_binding_identity` | Passing |
| References | Real-source references exclude cord contents and references captured by an inner same-named face | real-source regression | real `miner.hoon` `cause` case | Passing |
| References | Compiler-resolved imported arms return uses and their declaration across files, preserve exact declaration identity when another module exports the same name, and honor `includeDeclaration` | MUST for advertised capability | `definition_navigates_to_an_imported_gate` (`add-two:math`) | Passing |
| References | A changed unsaved generation immediately invalidates old compiler references and publishes the refreshed repeated qualified uses only after the matching check | implementation freshness invariant | `definition_navigates_to_an_imported_gate` | Passing |
| References | Real `miner.hoon` references for `check-target:mine` include its declaration in `common/pow.hoon` | real-source regression | `real_miner_definitions_resolve_local_transitive_prelude_and_rune_symbols` | Passing |
| References | Structurally resolved molds follow the nearest-unique transitive import identity, work from both a use and an opened declaration, and exclude cords and lexically captured terms | implementation fallback | `structural_references_preserve_import_identity_across_open_roots` + real `miner.hoon` `tip5-hash-atom` cases + service external-reference filtering | Passing |
| References | Standard-library symbols include uses in the open-root import graph and their declaration in the configured prelude | implementation fallback | real `miner.hoon` `list` case | Passing |
| Rename | Advertised prepare-rename and rename return versioned edits for the exact lexical binding identity | MUST for advertised capability | `local_references_and_rename_preserve_binding_identity` | Passing |
| Rename | Rename rejects invalid terms and changes that would capture or collide with another visible face | implementation safety invariant | service unit coverage + protocol collision case | Passing |
| Rename | Renaming a shorthand sample preserves its mold by expanding `=old` to `new=old` | real-source regression | real `miner.hoon` `kernel-state` case | Passing |
| Positions | Semantic queries translate UTF-16 positions without splitting surrogate pairs | MUST for negotiated positions | `semantic_positions_round_trip_as_utf16` | Passing |
| Cancellation | `$/cancelRequest` completes the pending request exactly once and suppresses its result | base protocol contract | unit cancellation test + process responsiveness test | Passing |
| Responsiveness | Semantic indexing does not block unrelated JSON-RPC handling | implementation invariant | `cancellation_and_other_requests_remain_responsive_during_semantic_indexing` | Passing |
| Freshness | Semantic results are validated against the current version and content before publication | implementation invariant | `stale_semantic_results_are_server_cancelled` | Passing |

Coverage claim applies only to the rows above, not to the full LSP feature set.
Formatting, semantic tokens, pull diagnostics, notebooks, and workspace
folders are not advertised. Document symbols remain structural. Hover combines
structural syntax with an owned type summary at compiler debug spots.
Definition and completion cover lexical faces plus unambiguous arm and mold
headers in the current document, its resolved Hoon import graph, and the
configured prelude. Gene rune tokens resolve to their canonical tagged
alternatives in the prelude's `hoon` mold; core declaration tokens `++`, `+$`,
and `+|` resolve to their parser arms. References preserve same-document lexical
binding identity and use compiler-owned declaration identity across the latest
successfully checked import graph for resolved arms and gates. Structural molds
and standard-library symbols use the same nearest-unique import resolver as
definition across every open or configured root. Rename is intentionally
limited to lexical faces until workspace-wide provenance can guarantee complete
edits.

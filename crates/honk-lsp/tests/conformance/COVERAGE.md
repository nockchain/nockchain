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
| Positions | Semantic queries translate UTF-16 positions without splitting surrogate pairs | MUST for negotiated positions | `semantic_positions_round_trip_as_utf16` | Passing |
| Cancellation | `$/cancelRequest` completes the pending request exactly once and suppresses its result | base protocol contract | unit cancellation test + process responsiveness test | Passing |
| Responsiveness | Semantic indexing does not block unrelated JSON-RPC handling | implementation invariant | `cancellation_and_other_requests_remain_responsive_during_semantic_indexing` | Passing |
| Freshness | Semantic results are validated against the current version and content before publication | implementation invariant | `stale_semantic_results_are_server_cancelled` | Passing |

Coverage claim applies only to the rows above, not to the full LSP feature set.
Completion, references, formatting, semantic tokens, pull diagnostics,
notebooks, and workspace folders are not advertised. Document symbols remain
structural. Hover combines structural syntax with an owned type summary at
compiler debug spots. Definition support currently covers compiler-resolved
core arms and gates, but not local face or binding declarations.

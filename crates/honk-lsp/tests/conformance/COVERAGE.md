# Honk LSP conformance coverage

Specification baseline: Language Server Protocol 3.17. The server intentionally
implements a narrow diagnostics-first capability set and remains compatible with
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
| Cancellation | `$/cancelRequest` may be ignored by a synchronous server | MAY | documented behavior | Accepted divergence |

Coverage claim applies only to the rows above, not to the full LSP feature set.
Completion, navigation, hover, formatting, symbols, semantic tokens, pull
diagnostics, notebooks, and workspace folders are not advertised.

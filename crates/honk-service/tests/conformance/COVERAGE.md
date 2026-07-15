# Editor service conformance coverage

| Requirement | Test | Method |
|---|---|---|
| Filesystem behavior remains the reference when no document is open | `open_documents_shadow_disk_and_close_restores_it` | Differential artifact comparison before open and after close |
| Unsaved dependency text participates in parsing, type checking, and artifacts | same | Replaces a transitive leaf's `add` with `mul` in memory and requires a different artifact without a disk write |
| Overlay and filesystem paths are artifact-equivalent for identical text | same | Reopens the dependency with disk-equivalent text and requires byte-identical output |
| Content-only edits invalidate the changed dependency and its transitive dependents | same | Uses `entry -> helper -> leaf` and requires at least two invalidated cached paths plus a miss |
| Content-only edits preserve unrelated dependency results | same | Imports a second unchanged library and requires a path-cache hit after editing the leaf |
| Editor checks do not evaluate, shape, or serialize artifacts | same | Calls the artifact-free `check` operation and requires dependency cache hits |
| Unsaved new files participate in import resolution | `pipeline::tests::overlay_resolves_import_that_does_not_exist_on_disk` | Resolves `/+` against an open `lib/*.hoon` document absent from disk |
| Unsaved malformed entry text produces a structured parse diagnostic | same | Compiles a malformed open buffer while verifying the on-disk entry is unchanged |
| Document versions are monotonic | same plus `document_versions_are_strictly_monotonic` | Duplicate versions must return `StaleDocumentVersion` |
| Results identify their document snapshot | same | Checks the global document revision on compile and check replies |
| Closing documents restores filesystem semantics | same | Closes all buffers and requires byte-identical output to the initial disk compile |
| Editor semantic state is separate from compiler noun ownership | `semantic_snapshot_indexes_arms_and_hover` | Builds traced syntax/arm side tables without a compiler arena or noun-bearing result |
| Semantic IDs survive source movement and body edits | `symbol_ids_survive_position_and_body_changes` | Reconciles the same arm across two document revisions and requires the ID to remain stable |

The gRPC adapter remains covered separately by
`crates/honk-grpc/tests/daemon_conformance.rs`; it exercises the same service
handle through protobuf conversion, health, reflection, invalidation, and
bounded-lifetime behavior.

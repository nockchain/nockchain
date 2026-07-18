# Honk daemon architecture

`honkd` is the persistent, loopback-only gRPC compiler process for tool
integration. `honk-lsp` is the direct stdio JSON-RPC adapter for editors. Both
reuse the same native workspace service as the established `honk` single-entry
and batch commands. The ordinary CLI remains the reference path: editor support
does not alter parser behavior, Hoon ASTs, type checking, minting, evaluation,
or artifact serialization.

## Running it

With Cargo:

```text
cargo run --release -p honk-grpc --bin honkd -- \
  --prelude hoon/common/hoon.hoon \
  --deps-dir hoon
```

With Bazel:

```text
bazel run //:honkd -- \
  --prelude hoon/common/hoon.hoon \
  --deps-dir hoon
```

For an editor/LSP client:

```text
cargo run --release -p honk-lsp -- \
  --prelude hoon/common/hoon.hoon \
  --deps-dir hoon
```

The VS Code extension lives in `editors/code`. It launches `honk-lsp` over
stdio, contributes the Hoon language/grammar, forwards file-watch events, and
offers compiler path, prelude, dependency-root, entry, debounce, and process
rotation settings. It never launches or connects to `honkd`.

The default listener is `127.0.0.1:0`. On startup, the process writes one JSON
readiness record to stdout containing the selected address, protocol name, and
protocol version. Logs go to stderr. `honkd` rejects wildcard and routable bind
addresses.

The protocol is `honk.compiler.v1`, implemented in
`crates/honk-grpc-proto/proto/honk/compiler/v1/compiler.proto`. Clients should
call `GetServerInfo` before compiling and send the returned protocol version on
each `Compile` request. Compilation failures are successful RPC responses with
structured diagnostics; malformed protocol requests use gRPC status errors.
The server also exposes standard gRPC health and reflection services.

## One compiler service, two adapters

There should not be an LSP-to-gRPC network hop. The intended endpoint shape is:

```text
gRPC client ── tonic adapter ──┐
                              ├── workspace domain service ── compiler actor
editor ── stdio LSP adapter ──┘
```

The shared actor and session API live in `honk-service`; the compiler domain
model lives in `honk::workspace`. Together they define workspace configuration,
artifact mode, compile/check request/results, document updates and revisions,
source locations, diagnostics, and cache statistics. Protobuf messages are
converted at the gRPC boundary. LSP documents, paths, ranges, and diagnostics
are converted in `honk-lsp`, which invokes `honk_service::CompilerHandle`
directly. Protobuf and LSP types do not depend on each other; their semantics,
tests, and transport rules differ even when they describe the same operation.

The compiler remains on one dedicated OS thread because its noun and `Rc`
state is intentionally not `Send`. Async RPC handlers only own a bounded,
Send-safe request channel.

The shared service (not the current compile-only gRPC surface) supports open
editor documents as full-text, monotonically versioned overlays. Each accepted
update advances a workspace revision; compile and check results identify the
revision they used. The LSP adapter independently versions its input and
never publishes a result for an older generation. Open documents shadow disk
reads, including imports that do not exist on disk yet. Closing a document
restores filesystem semantics. Notifications received during a long check are
coalesced into the latest full snapshot. The first snapshot seeds compiler
construction directly, avoiding a throwaway filesystem-only epoch before the
first unsaved check.

The service exposes a separate artifact-free `check` operation. It parses,
resolves imports, and type-checks/mints the requested entry, but does not
evaluate, shape, or jam an artifact. `compile` and all CLI/batch paths retain
their established artifact behavior.

That editor check also enables a scoped semantic observer in `Ut`. Existing
`dbug` spots report their inferred native type as an owned source location and
bounded structural string. Exact core-arm resolution records a second owned
fact containing the use and definition locations. The observer is disabled by
default, is switched off before the check returns (including compile errors),
and never exports an `NTy`, noun, `Rc`, or arena reference. Artifact-producing
service calls and ordinary CLI/batch compilation never enable it.

`honk-service::semantic` is a second, deliberately noun-free editor path. It
parses an open document with the existing debug-spot annotations and derives a
revisioned side table of syntax nodes, arm/mold symbols, byte ranges, hierarchy,
and stable session-local node IDs. Matching symbols and unchanged traced
fragments retain their IDs across document revisions. A dedicated semantic
worker owns this cache independently of the compiler actor, so the LSP protocol
thread never parses and hover or document-symbol requests do not queue behind a
long type check.

Snapshot cache hits avoid parsing when path, version, and content are unchanged.
A changed document is still reparsed wholesale and its traced AST is serialized
once to populate the side tables. That is correct invalidation, but not yet
fine-grained incremental parsing. Requests carry cancellation tokens, stale
versions and content are revalidated before publication, and cancellation
always completes the JSON-RPC request exactly once. The existing parser call is
not internally interruptible, so work already inside that call may finish in
the background before its result is discarded.

Document symbols identify `++`, `+$`, `+*`, and `+|` arms. Hover identifies
those structural definitions and traced Hoon syntax, then merges the narrowest
compiler type fact for the same open-document version when one is available.
An edit immediately drops the prior facts; failed or stale checks cannot
publish them. Go-to-definition follows compiler-resolved core arms and imported
gates, including definitions in other files, and uses the structural import
graph for arms, molds, standard-library terms, and runes that do not yet have a
compiler resolution fact. Find-references preserves lexical binding identity
locally and exact declaration/import identity across every Hoon source under
the configured dependency root. The same complete graph powers rename: local
faces retain their existing scope-aware edits, while arms and molds produce a
multi-document edit that includes unopened sources. Structural rename is
declined when the graph is incomplete, a competing declaration would win or
make resolution ambiguous, an existing occurrence would change meaning, or a
lexical face would capture a renamed use.

## Invalidation and lifetime

The old batch cache assumes a finite, immutable input set. `honkd` uses a
separate workspace mode:

- Every source and data file reached by a request is content-fingerprinted.
- Dependency-directory layout is fingerprinted so create/delete/rename changes,
  including a new higher-precedence import candidate, invalidate the context.
- The LSP separately discovers every `.hoon` source under its configured
  dependency root at initialization. VS Code file-watch create, change, and
  delete events update that source set and advance the semantic generation, so
  the cached structural graph never survives a disk-layout change. Open buffers
  remain authoritative overlays and carry document versions in workspace edits.
- An unchanged request can reuse path caches.
- A successful editor check also caches its noun-free semantic and resolution
  facts by canonical entry. Repeated checks and same-content version updates
  return those detached facts without re-minting the root.
- The editor-only source-overlay path records direct and reverse dependency
  edges. A content-only edit to an existing file invalidates that path and its
  transitive dependents while preserving unrelated cached dependency vases.
- Cache statistics report dependency-path and root-check hits, misses, and
  invalidations per successful operation; the differential editor test requires
  both levels.
- Prelude, subject-type, compiler-option, create/delete/rename, and import-layout
  changes still build a fresh compiler context before checking again.
- Content-only cache reuse across different paths is disabled in workspace mode
  because path-derived spots and import context can differ.

Fine-grained noun-bearing reuse is constrained to cached dependency products.
Successful editor root checks reuse only detached facts; a changed root is
still rebuilt as one type-checking unit. Cross-call `miss` memo persistence
stays disabled, and per-mint transient `Ut` state is cleared through the
existing compiler boundary. The ordinary CLI and batch paths never enable
mutable source snapshots, root-check caching, or dependency-graph invalidation
and retain their prior cache behavior.

`WorkspaceArena` owns the noun slab for an epoch and lends it to the compiler
through a closure, so dropping an invalidated epoch reclaims all of its nouns
without allowing noun-bearing state to escape. `honkd` still bounds process
lifetime to 256 accepted compile requests by default as an operational defense
against other long-lived allocator/cache growth. The final response sets
`restart_required`, then the server shuts down gracefully so a client or editor
host can relaunch it. `--max-compiles 0` disables rotation for controlled use.

## Toward LSP and annotated trees

The implemented first layer uses the current immutable `Hoon` tree plus stable
session-local node IDs and side tables for spans, structural symbols, and
editor-only state. This preserves the exact AST consumed by the shipping
compiler and lets editor annotations have independent lifetimes. Inferred types
and core-arm resolutions now cross the compiler boundary through owned,
location-keyed side tables rather than by mutating the legacy tree. Local
binding provenance and references should follow the same rule.

If later phases genuinely need several tree shapes with one shared grammar, the
Haskell idea worth borrowing is Trees That Grow (or an HKD-style extension
family): parameterize node-specific extension fields and define aliases such as
parsed, resolved, and typed Hoon trees. In Rust, this can be expressed with an
extension trait whose associated types supply per-node annotations. A fixed-point
functor/recursion-scheme representation is another principled option when many
whole-tree transformations become necessary, but it is a larger migration.

Neither representation should replace the current AST in-place. The safe route
is a parallel editor/compiler tree module, an explicit lowering boundary, and
differential tests proving that lowering produces the same legacy AST, type
results, and artifact bytes. That decision is deferred; this daemon slice adds
no parser or AST behavior.

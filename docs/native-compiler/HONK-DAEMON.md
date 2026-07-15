# Honk daemon architecture

`honkd` is the persistent, loopback-only compiler process for editor and tool
integration. It reuses the same native workspace compiler as the established
`honk` single-entry and batch commands. The ordinary CLI remains the reference
path: this change does not alter parser behavior, Hoon ASTs, type checking,
minting, evaluation, or artifact serialization.

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
editor ── LSP JSON-RPC adapter ┘
```

The shared layer is the Rust domain model in `honk::workspace`: workspace
configuration, artifact mode, compile request/result, source location, and
diagnostic. Protobuf messages are converted at the gRPC boundary. A future LSP
adapter should convert LSP documents, ranges, and diagnostics at its own
boundary and invoke the same in-process compiler actor. Protobuf and LSP types
should not depend on each other; their semantics, tests, and transport rules are
different even when they describe the same compile operation.

The compiler remains on one dedicated OS thread because its noun and `Rc`
state is intentionally not `Send`. Async RPC handlers only own a bounded,
Send-safe request channel.

## Invalidation and lifetime

The old batch cache assumes a finite, immutable input set. `honkd` uses a
separate workspace mode:

- Every source and data file reached by a request is content-fingerprinted.
- Dependency-directory layout is fingerprinted so create/delete/rename changes,
  including a new higher-precedence import candidate, invalidate the context.
- An unchanged request can reuse path caches.
- If an observed entry, direct dependency, transitive dependency, data file,
  prelude, or supplied subject-type jam changes, the daemon builds a fresh
  compiler context before compiling again.
- Content-only cache reuse across different paths is disabled in workspace mode
  because path-derived spots and import context can differ.

Rebuilding the whole context is conservative but principled: no undocumented
source-dependent `Ut` state crosses an edit boundary. The ordinary CLI and batch
paths never enable workspace mode and retain their prior cache behavior.

The existing compiler context intentionally leaks its slab for noun lifetime
safety. Until that ownership model is replaced, `honkd` bounds process lifetime
to 256 accepted compile requests by default. The final response sets
`restart_required`, then the server shuts down gracefully so a client or editor
host can relaunch it and let the OS reclaim the arena. `--max-compiles 0`
disables rotation for controlled use.

## Toward LSP and annotated trees

For initial LSP work, the least invasive representation is the current immutable
`Hoon` tree plus stable node IDs and side tables for spans, resolved names,
inferred types, references, and editor-only state. This preserves the exact AST
consumed by the shipping compiler and lets editor annotations have independent
lifetimes.

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

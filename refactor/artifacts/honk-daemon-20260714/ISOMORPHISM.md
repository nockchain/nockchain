# Honk daemon isomorphism contract

## Scope

Extract the existing file/workspace compiler from the `honk` binary, expose it
through an in-process workspace API, and add a loopback gRPC daemon. The
ordinary single-entry and batch CLI paths remain the reference behavior.

## Preserved observations

- CLI arguments, exit status, stderr/stdout behavior, and output file padding.
- Parsed Hoon trees and source-location construction.
- Type-checking, minting, evaluation, and artifact serialization.
- Batch cache behavior and shared-prelude behavior.
- Cargo and Bazel entry points.

## Allowed additions

- A transport-neutral workspace request/result/diagnostic model.
- Daemon-only file revalidation and cache invalidation.
- A loopback-only, versioned gRPC adapter.
- Bounded daemon lifetime so the existing leaked compiler arena cannot grow for
  the lifetime of a desktop session.

## Explicit non-goals

- No parser or `Hoon` AST representation changes.
- No LSP wire implementation in this slice.
- No gRPC-to-JSON-RPC proxy hop; a future LSP adapter must call the same
  in-process workspace service as gRPC.

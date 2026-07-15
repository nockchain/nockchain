# Conformance provenance

- Specification: Microsoft Language Server Protocol 3.17 specification.
- Specification URL: <https://microsoft.github.io/language-server-protocol/specifications/lsp/3.17/specification/>
- Reviewed: 2026-07-15.
- Wire implementation: `lsp-server` 0.9.0.
- Protocol types: `lsp-types` 0.97.0.
- Process command: `cargo test -p honk-lsp --test stdio_conformance`.
- Compiler behavior oracle: `honk-service/tests/editor_conformance.rs`, which compares disk, overlay, stale-version, parse-error, and close/reopen outcomes against the ordinary compiler path.

There are no opaque golden fixtures in this slice. The process test generates
typed request frames and parses typed responses in the same test invocation.

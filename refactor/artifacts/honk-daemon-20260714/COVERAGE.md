# Honk daemon conformance coverage

## Baseline (2026-07-14)

- `cargo test --release -p hatch --lib`: 270 passed.
- `cargo test --release -p honk --lib`: 128 passed, 1 pre-existing ignored.
- Reference command:
  `target/release/honk --new --arbitrary --output /tmp/honk-daemon-baseline-auras.jam --prelude hoon/common/hoon.hoon crates/honk/test-assets/type-probes/auras.hoon hoon`
- Reference artifact: 564,368 bytes.
- Reference SHA-256:
  `8535630fa4fd1464ecc398ab4d8882ed122b8e8e12435d103518faf64096d378`.

## Required post-change checks

- Reference CLI artifact: byte-identical (`cmp`) with the same SHA-256.
- `cargo test --release -p honk --lib`: 131 passed, 1 pre-existing ignored.
- `cargo test -p honk --test cli_batch_parity`: passed; two shared-prelude
  batch artifacts equal isolated CLI artifacts byte-for-byte.
- `cargo test -p honk-grpc`: passed (2 unit tests and 1 end-to-end conformance
  test). This covers protocol negotiation, health, the CLI golden, unchanged
  reuse, imported-source invalidation, structured parse diagnostics, and
  bounded-lifetime restart signaling.
- `cargo clippy -p honk-grpc -p honk-grpc-proto --all-targets --no-deps -- -D warnings`:
  passed. The same command without `--no-deps` reaches an unrelated existing
  `nockvm` `type_complexity` warning.
- `cargo fmt --all -- --check`: passed.
- `bazel test //crates/honk-grpc:honk_grpc_unit_tests`: passed.
- `bazel build //crates/honk-grpc:honkd //crates/honk:honk`: passed.

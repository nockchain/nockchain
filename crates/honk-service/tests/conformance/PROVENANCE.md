# Conformance provenance

The filesystem compiler is the executable reference implementation. Tests
create source files at runtime, compile them through the filesystem-only domain
path, then compare editor-overlay and post-close results against that reference.
No copied golden artifact is maintained for this contract.

Protocol behavior is defined by
`crates/honk-grpc-proto/proto/honk/compiler/v1/compiler.proto`; parser and
artifact behavior remains defined by the existing `honk` CLI/batch parity
harnesses and their checked-in kernel/hoon-138 references.

## 2026-07-15 validation

The seven authoritative Bazel parity targets were run with cached test results
disabled. All seven passed strict byte comparison in 239.01 seconds.

Each cold compile benchmark was then run once under macOS `/usr/bin/time -l`.
Peak RSS is the maximum resident-set size in bytes for the complete wrapper,
which runs cold hoonc, then cold honk, then compares their artifacts.

| Artifact | hoonc | honk | Total wall | Peak RSS | Peak GiB | Result |
|---|---:|---:|---:|---:|---:|---|
| hoon-138 | 199.4s | 53.9s | 253.73s | 9,464,692,736 B | 8.815 | byte-identical |
| bridge | 451.9s | 101.4s | 553.74s | 12,173,246,464 B | 11.337 | byte-identical |
| dumb | 341.6s | 92.3s | 434.38s | 10,242,555,904 B | 9.539 | byte-identical |
| miner | 89.6s | 14.0s | 104.03s | 3,940,139,008 B | 3.670 | byte-identical |
| peek | 313.0s | 80.0s | 393.43s | 8,318,386,176 B | 7.747 | byte-identical |
| roswell | 676.4s | 150.8s | 827.71s | 18,675,351,552 B | 17.393 | byte-identical |
| wal | 429.4s | 100.9s | 530.72s | 12,616,515,584 B | 11.750 | byte-identical |

All seven measured cases reported zero swaps.

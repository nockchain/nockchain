# Post-rebase Honk validation and profile

## Outcome

The arena/seminoun branch rebased cleanly onto `origin/master` commit
`a0251be1f8c285177c005db62ec8783b89e344e9`. The rebased branch commit is
`268f33b983c13754707b776d53251ca2c73b36da` before the review-cleanup working
changes documented here.

Honk's hoon-138 arbitrary native build now completes in bounded memory and is
byte-identical to hoonc. All six production kernels also pass Bazel's strict
`cmp` gates. No compatibility fix was required after the rebase.

## Correctness and integration matrix

| Gate | Result |
| --- | --- |
| `cargo build --release -p honk --bin honk -p hoonc --bin hoonc` | pass |
| `cargo test --release -p honk` | 228 passed across library, CLI, mint, and rejection suites |
| `cargo test --release -p hatch --lib` | 278 passed |
| `cargo test --release -p nockvm --lib` | 183 passed; 4 pre-existing ignored |
| Honk Clippy, all targets, no dependency linting, warnings denied | pass |
| `bazel test //crates/honk/test-assets:hoon_138_arbitrary_parity_test` | pass; exact byte equality |
| `bazel test //assets/native:kernel_parity_test` | 6/6 pass; Dumbnet, Wallet, Miner, Peek, Bridge, Roswell |
| `git diff --check` | pass |

`cargo fmt --all -- --check` reports three formatting differences in the
unchanged upstream `crates/bridge/src/main.rs`. The Rust files touched by this
cleanup pass `rustfmt --check`; this branch has no diff from `origin/master` in
the reported Bridge file.

## Review cleanup

- Removed the unused `Context::reset()`. Resetting the arena while a `TypeRef`
  survives would invalidate its stable slot pointer; context and handles must
  instead be dropped together.
- Replaced stale Phase-0/Rc architecture comments with the implemented arena
  ownership model.
- Kept the migration's zero-cost `TypeRef::clone()` call shape behind one
  documented module-level Clippy allowance, avoiding hundreds of cosmetic
  edits in the semantic migration.
- Documented the intentional `dbug=false` prelude-parity configuration and the
  remaining wrapper-preservation constraint for any future debug-enabled use.
- Corrected active documentation to use real repository paths and strict Bazel
  acceptance targets; historical planning documents are labelled as such.

## Performance baseline

| Workload | Wall time | Peak RSS | Artifact SHA-256 |
| --- | ---: | ---: | --- |
| Dumbnet, 20-run warm baseline | mean 32.783 s; p95 35.632 s | 4.92 GiB in an isolated run | `79dad9653af051ba335243581c14010ddc49eba265fd7fcf417bbba94f042d95` |
| hoon-138 native self-mint | 58.48 s | 10.29 GiB | `b156034a5d5d158c133fad6591a71b96def4339e49faba4773b9f9b68e1c1741` |
| Roswell production build | 63.71 s | 8.93 GiB | `8c47a2b9af208d190f2f151bcc299a494a67431afef21221a203538ca3b08cef` |

The hoon-138 hash is unchanged from the pre-rebase successful result. Roswell
is much faster than the obsolete 71–76 s note but still misses the existing
60-second target by 3.71 seconds.

The symbolicated hoon-138 profile ranks noun/value transport and copying first
at about 28.5% of disjoint self samples, map/cache growth and hashing second at
about 10–12%, and type recursion/miss paths third at at least 8.6%.
`TypeTable::intern_node` itself is only 0.416% self time. The next optimization
should therefore measure copy volume and retained allocation at noun/value
boundaries before considering another arena rewrite. Map pre-sizing is the
lower-risk second experiment.

Raw measurements, the host fingerprint, checksums, hotspot ranking, and testable
hypotheses are in
`tests/artifacts/perf/20260807T021543Z-post-rebase/`.

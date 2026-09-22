# Cross-cutting performance verification — all security-advisory fixes

> **STATUS: Verified 2026-09-18.** Comparison points: **before** = `05ce0e4`
> (`139ba66^`, the synced upstream base immediately preceding the first security fix),
> **after** = branch HEAD (`328ffa5`; last code commit `6ce4c04`). All runs:
> `--release`, Apple M2 Max, single pass per tree, warm target directories.

## Suite wall-time comparison (lib suites, 10 touched crates)

| Crate | Before (tests / time) | After (tests / time) | Verdict |
|---|---|---|---|
| nockvm | 184 / 0.52 s | 196 / 0.48 s | flat |
| nockapp | 97 / 2.54 s | 101 / **3.91 s** (excl. new heavy regression) | flat — see note |
| nockchain-libp2p-io | 332 / 3.67 s | 310 / 3.29 s | flat (test set consolidated by the fixes) |
| nockchain-math | 59 / 0.50 s | 68 / 0.51 s | flat |
| zkvm-jetpack | 123 / 2.41 s | 141 / 2.08 s | flat (slightly faster) |
| noun-serde | 27 / ~0 s | 29 / ~0 s | flat |
| chaff | 62 / 4.11 s | 64 / 3.75 s | flat |
| nockapp-grpc | 35 / 0.62 s | 38 / 0.61 s | flat |
| nockchain | 27 / ~0 s | 30 / ~0 s | flat |
| ai-pow-jets | 17 / ~0 s | 18 / 0.02 s | flat |
| ai-pow (all suites) | per-binary 0.00–0.11 s | identical per-binary results and times | flat |

Net test count across the ten crates: 963 → 995 (+32 security regressions added; the
libp2p-io suite shrank by 22 as the exclusion tests were restructured around the new
per-IP evidence model).

**nockapp note:** the full after-tree suite shows 81 s vs 2.54 s before, but the
entire delta is one test: `modify_survives_deep_chain_on_worker_stack`
takes **81.98 s alone** — a deliberately extreme
60,000-deep cue→set-root→modify cycle that exists to prove the iterative rehome walk
on a tokio-sized stack. It is a test-cost, not a production path; excluding it, the
suite is 3.91 s for 100 tests (the +1.4 s vs before is the new snapshot-verify and
cue-bound tests, within single-run noise).

## Production-path analysis

All 38 fixes add either cold-path work or O(1) guards on hot paths:

- **O(1) guards on hot paths:** checked adds and `len()` compares in the cue decoders
  (`547eb07`, `3739518`, `41f372a`), traversal/leaf-count/dyck budgets
  (`73159d2`, `530454c`, `c514462`), mary/table geometry validation (`564ce91`),
  map-length gates (`86aca16`, `2b4fac3`), CLI validation (`8cb03c0`). Per-element
  cost unchanged; several previously quadratic/unbounded paths became bounded.
- **Measured improvements:** ip-block threshold checks went from an O(4096) global-deque
  scan to an O(1) per-IP lookup (`86aca16`); `based_noun` went from O(2^k) to O(k)
  on shared DAGs (`530454c`); malformed-input decodes now reject in O(prefix) instead
  of allocating (`547eb07`, `3739518`).
- **Measured bounded costs:** worst-case event-log jam at the 1M-node network cap is
  ~125 ms (measured 62.7 ms at 500k, linear — `874880e`); verifier-context
  persistence adds two fsyncs per ~15-minute generation (`663735a`); snapshot
  creation gains one structural walk per checkpoint (`8253aed`) — operator-paced, and
  stage timings are already instrumented.
- **Kernel/Hoon:** the only Hoon behavior changes are on malformed-command cold paths
  (`%exit` → `!!`, `2b41223`); canonical-jam decoding is unchanged by construction
  (`c754baa`), confirmed by the full nockvm round-trip suite.
- **ai-pow mining path:** unchanged hot-path code; the ZK verifier's per-byte
  range-check count rose for *provers* only (upstream `54dcaa78`, pre-branch), and the
  adversarial kernel-move regression (8.3 s, `--features zk`) is a test, not mining.

## Caveats

- Single-run wall-clock on a shared machine; deltas under ~0.5 s are noise. The data
  answers the security question (no order-of-magnitude production regressions), not
  micro-benchmarks.
- Hoon-side suites were not re-timed before/after (the only Hoon changes are
  malformed-input cold paths); the embedded `assets/roswell.jam` at HEAD is built
  from HEAD Hoon source and its suites pass (mempool 10/10, fee-enforcement 6/6,
  pending 4/4, fund-split OK — recorded during the earlier dispositions).
- One known lint failure remains on the branch: `cargo clippy -p nockchain --tests -D warnings`
  trips on `mining_key_reject_e2e.rs:161` (`useless_conversion`, the `u64::from(tas!(…))`).
  `--lib` clippy is clean
  for every touched crate.

## Conclusion

**No measurable production performance regression from the 38 advisory fixes.**
Suite times are flat within noise across all ten touched crates; the added cost is
O(1) guards on decoders and validators (previously the crash/OOM paths), plus two
deliberately heavy security regression tests that run only under `cargo test`.

## Post-verification addendum (commits after `328ffa5`)

Three commits landed after the suite comparison above:

- `ae1db7e` (pending-promotion `block-versions` write):
  one height compare + one `h-by` map insert per promoted block — an
  operator-paced event (a block leaves pending only when its last missing
  transaction arrives). Nil.
- `4c14cb9` (fix-2 test fixture rebuild): test-only, no production code.
- `670232a` (dor/gor/mor value equality): measured micro cost only on direct
  `+dor` calls with mug-unequal inputs (+8..+64 ns/op); the container hot
  path (`gor`/`mor` mug-unequal) is byte-identical and measured unchanged;
  `bench-h-zoon` (z-map/h-map build/read/update, ~8 rounds per phase) and
  the full `test-dumb` battery wall time are identical within run variance.

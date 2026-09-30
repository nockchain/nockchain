# t1 (tooling) coverage ledger

This package covers `crates/honk/src/{artifact,build_cache,nasm_bridge,arm_map}.rs`,
tested by `crates/honk/src/cov_t1.rs` (unit) and
`crates/honk/tests/cov_t1_cache_parity.rs` (CLI integration); it has no probes.

Uncovered: 1282 lines and 206 branch outcomes. Unit only: 0 lines, 0
branches. Parity only: 1267 lines, 203 branches. Both: 15 lines, 3 branches.

## Parity gaps

Every line and branch outcome in the four files is uncovered by the parity
corpus, and all of them share one reason: this is build tooling that never
changes compiled output, and no parity-checked compile reaches it.

- The parity replay compiles without `--cache-dir`, so `build_cache.rs` and
  `nasm_bridge.rs` (reached only through
  `NativeBuildContext::read_cache_object` and `flush_build_cache` in
  `bin/honk.rs`) never run.
- `artifact.rs` and `build_cache::run_cli` run only under the `honk nockasm`
  and `honk cache` subcommands.
- `arm_map.rs` is used only by the library API
  (`NativeCompiler::compile_expr` in `native/mod.rs`), never by the `honk`
  binary.

The one parity-relevant property, that a build served from the cache is
byte-identical to an uncached build, is checked by
`tests/cov_t1_cache_parity.rs` in every output mode, in batch builds, and
after recovery from damaged cache objects.

## Unit gaps (also parity gaps)

build_cache.rs has no unit gaps.

arm_map.rs
- `peg_axis` L295, B294 true, B294 cond2 true (zero-axis error): unreachable.
  Callers start from axis 1 or 2 and peg only with the literals 2, 3, 6, and
  7, and pegging two non-zero axes never yields 0.

artifact.rs
- `export` L194, L198, L204 (`)?` of `fs::write` for `manifest.json`,
  `tree/root.ref`, and `formula-root.nasm-dag`): I/O failure only. The files
  go into a pending directory named with the process ID and a nanosecond
  timestamp, so a test cannot make one write fail short of a full disk or a
  read-only filesystem.
- `diff` L326 (`)?` of `writeln!` into the `String` summary): unreachable,
  `fmt::Write` for `String` never fails.
- `diff` L332, L338 (`)?` of `write_fragment`): propagate only an I/O
  failure, `lift_dag`'s `TooManyNodes` (more than u32::MAX nodes), or the
  unreachable invalid-axis case in `noun_at_axis`.
- `noun_at_axis` L390, B389 false (the walk hits an atom): unreachable. `diff`
  builds the axis by descending only through pairs of `DagNode::Cell` nodes,
  and the walk follows the noun lowered from the same graph, so every step
  lands on a cell.
- `write_fragment` L408 (`)?` of `fs::write` for a `.ref` fragment): I/O
  failure only.
- `write_node_line` L632 (`)?` of `write!` for a cell line into the shard's
  `BufWriter<File>`): I/O failure only.

nasm_bridge.rs
- `convert` L77-78 (`as_cell` fails after `as_atom` failed): unreachable, a
  noun is either an atom or a cell.
- `intern` L126-127 and `wide_atom` L155 (more than u32::MAX distinct nouns
  in one bridge): defensive, needs over four billion interned nouns.

## Known bugs (tests are `#[ignore]`d)

These are cache behavior bugs, not parity divergences; compiled output stays
correct in both.

1. A damaged pack is not repaired when the rebuilt pack is byte-identical.
   `BuildCache::write_pack_inner` (build_cache.rs:193) skips the pack write
   whenever a file exists at the pack path, even one that failed its hash
   check, so every later session misses again. The README's claim that a
   hash-mismatched object is repaired by the successful build does not hold
   across sessions. Test: `cov_t1::cache_damaged_pack_is_repaired_across_sessions`.
2. A batch manifest that lists one entry twice with `--cache-dir` fails.
   `compile_entry` (bin/honk.rs:2329-2371) has no in-memory memo for entries,
   so it queues a second `PendingCacheObject` with the same key, and
   `flush_build_cache` (bin/honk.rs:2288) fails in `lift_bundle` with
   "duplicate root name", exiting non-zero after the outputs were written.
   Test: `cov_t1_batch_with_a_repeated_entry_populates_the_cache` in
   `tests/cov_t1_cache_parity.rs`.

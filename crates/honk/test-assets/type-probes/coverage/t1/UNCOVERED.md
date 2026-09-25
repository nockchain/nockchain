# t1 (tooling) coverage ledger

Scope: `crates/honk/src/{artifact,build_cache,nasm_bridge,arm_map}.rs`.
Tests: `crates/honk/src/cov_t1.rs` (unit) and
`crates/honk/tests/cov_t1_cache_parity.rs` (CLI integration). No parity
probes: none of these files can be reached by a probe (see "Parity" below).

## Unit coverage (last `unit-cov.sh t1 honk cov_t1 honk/src/`)

| file | baseline left | now left |
| --- | --- | --- |
| arm_map.rs | 97 lines, 22 branches | 1 line, 2 branches |
| artifact.rs | 210 lines, 22 branches | 9 lines, 1 branch |
| build_cache.rs | 100 lines, 47 branches | 0 |
| nasm_bridge.rs | 8 lines, 1 branch | 5 lines, 0 branches |
| total | 415 lines, 92 branches | 15 lines, 3 branches |

### Still uncovered by unit tests

arm_map.rs
- L295, B294 true, B294 cond2 true (`peg_axis` zero-axis error): unreachable.
  Every caller passes a non-zero literal axis (1, 2, 3, 6, 7) or one derived
  from them by `peg_axis`, which never returns 0.

artifact.rs
- L194, L198, L204 (`)?` of `fs::write` for `manifest.json`, `tree/root.ref`,
  `formula-root.nasm-dag` in `export`): defensive, I/O failure only. The
  files go into a pending directory whose name the test cannot predict, so a
  write failure cannot be arranged without a full disk or read-only
  filesystem.
- L408 (`)?` of `fs::write` for a `.ref` fragment in `write_fragment`): same,
  I/O failure only.
- L632 (`)?` of `write!` for a cell line in `write_node_line`): I/O failure
  only (`BufWriter<File>`).
- L326 (`)?` of `writeln!` into the `String` summary in `diff`): unreachable,
  `fmt::Write` for `String` never fails.
- L332, L338 (`)?` of `write_fragment` in `diff`): propagate only the I/O
  failures above, `lift_dag`'s node-count overflow (more than u32::MAX
  nodes), or the unreachable invalid-axis case below.
- L390, B389 false (`noun_at_axis` hits an atom): unreachable. The axis is
  built by descending only through (cell, cell) node pairs of the same graph
  whose lowered noun is walked, so every step lands on a cell.

nasm_bridge.rs
- L77-78 (`as_cell` failure after `as_atom` failed): unreachable, a noun is
  either an atom or a cell.
- L126-127, L155 (more than u32::MAX distinct nouns in one bridge):
  defensive; needs over four billion interned nouns.

build_cache.rs: fully covered (lines and branches).

## Parity coverage

All 1282 lines and 206 branch outcomes of the four files stay uncovered by the
parity corpus, and no probe can change that. The parity harness runs
`honk --arbitrary` on a single file with empty deps and no `--cache-dir`, so:
- build_cache.rs and nasm_bridge.rs run only when `--cache-dir` is set
  (`NativeBuildContext::read_cache_object` / `flush_build_cache`);
- artifact.rs and build_cache's `run_cli` run only under the `honk nockasm`
  and `honk cache` subcommands;
- arm_map.rs is used only by the library API (`NativeCompiler::compile_expr`
  in `native/mod.rs`), never by the `honk` binary.

None of these can change a compiled type, formula, or accept/reject verdict.
The one parity-relevant property, that a build served from the cache is
byte-identical to an uncached build, is asserted by
`tests/cov_t1_cache_parity.rs` for both cached object kinds (entry products
and dependency vases), in all four output modes (standard, arbitrary, dynock,
dynock-typed), across modes (a product cached by an arbitrary build and served
to the other three), in batch builds, and after recovering from damaged
metadata, damaged packs, and well-formed packs whose payload the compiler
rejects.

## Known bugs found (not parity divergences; tests are `#[ignore]`d)

1. A damaged pack never self-heals when the rebuilt pack is byte-identical.
   `BuildCache::write_pack_inner` (build_cache.rs:193) skips the pack write
   whenever a file exists at the pack path, even when that file failed its
   hash check on read. Rebuilding the same objects produces the same pack
   bytes and hash, so the damaged file stays and every later build misses
   (`corrupt=4` on every run for the cache fixture). Output stays correct;
   the README's "repaired by the successful build" claim does not hold.
   Test: `cov_t1::cache_damaged_pack_is_repaired_across_sessions`.
2. A batch manifest that lists one entry twice (e.g. in two output modes)
   with `--cache-dir` fails. `compile_entry` (bin/honk.rs:2175-2214) has no
   in-memory memo for entries and queues a second `PendingCacheObject` with
   the same key, and `flush_build_cache` (bin/honk.rs:2157) then fails in
   `lift_bundle` with "duplicate root name", exiting non-zero after the
   outputs were written.
   Test: `cov_t1_batch_with_a_repeated_entry_populates_the_cache`.

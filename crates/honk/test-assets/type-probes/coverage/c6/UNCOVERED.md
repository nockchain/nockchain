# c6 coverage ledger: honk pipeline, library entry points, and CLI

Range: `crates/honk/src/pipeline.rs`, `lib.rs`, `errors.rs`, `types.rs`, and
`crates/honk/src/bin/honk.rs`. Line numbers match this worktree.

Tests added:

- `crates/honk/src/cov_c6.rs`: pipeline import parsing and resolution,
  whole-graph parsing, spot paths, Urbit scope mode, errors, `Compiler`,
  `TypeNoun`.
- `crates/honk/src/bin_tests/cov_c6_bin.rs`: CLI helpers (modes, manifests,
  wrapper sources and batteries, pack hydration, subject-type and trace
  decoding, paths, directory hash, softed-constraints checks).
- `crates/honk/tests/cov_c6_cli.rs`: the built binary end to end (argument
  errors, subcommands, dynock modes, `--sut-jam`, `--no-dbug`, batch
  manifests with `--cache-dir`, non-canonical preludes, wrapper asset dumps,
  hoonc delegation for changed softed constraints).

Parity pairings (all PASS; `coverage_parity_test` runs them):

- `c6_bare_fas`, `c6_fas_path`: single-file probes, a `/` line that is Hoon,
  not an import rune.
- `c6_imports` (`imports/app/entry.hoon`, arbitrary): `/=` and `/*` imports
  with `*` and named faces, a transitive and repeated import, identical-content
  dependencies, continuation lines (indented comment, whitespace-only line),
  inline and between-rune comments, data leaves with trailing and all-zero
  bytes.
- `c6_imports_kernel` (standard kernel build with imports and data) and
  `c6_outside_kernel` (entry outside the dependency root).
- `c6_dynock`, `c6_dynock_typed`: genrule pairings for `--dynock` and
  `--dynock-typed`.
- `c6_batch_{arb,kernel,typed}`: one batch manifest built cold then warm
  against `--cache-dir`; the warm pass reads cached entry products and cached
  dependency vases, and the kernel line carries an explicit directory list.

## Fixed divergences (DIVERGENCES.md rows 28-32)

The former `divergent/` pairings now run in `coverage_parity_test`:

- `c6_sur_lib_spot`, `regressions:c6_ford_pin_spot`: honk parses the import
  header with a port of hoonc's `+pile-rule` header (`pipeline.rs`
  `parse_import_header`) and blanks it before hatch parses the body, so the
  body's `%spot` starts after the header as in hoonc. hatch itself now skips
  an import block only at the top of a file (`runes/fas.rs`
  `import_header`), outside the traced body, so it no longer needs
  `should_skip_outer`, and a tall `/-1` or `/=/foo` elsewhere parses as a
  path (`regressions:p3_tall_path_after_import_rune`).
- `c6_dat_eager`, `c6_dat_core`: a node keyed under `/dat` is kicked while it
  is built and its value kept in an eval-vase trap (`honk.rs`
  `is_hoonc_dat_node`, `kick_vase_trap_value`).
- `c6_comment_after_comma`, `c6_blank_continuation`, `c6_double_hyphen`,
  `c6_bar_mark`, `c6_bar_hoon`: the header port accepts what hoonc accepts;
  `/*` takes any mark and a `.hoon` target compiles as Hoon (`+is-hoon`);
  `+segments` keeps a name with an empty hyphen part whole.
- `c6_hoon_root_marker`: the `open|closed/hoon` root-marker matching is gone;
  an entry outside the dependency root is keyed by its canonical path, as in
  hoonc. It came from the monorepo layout (separate `open/hoon` checkouts in
  sandboxes); nothing in this repository builds under an `open/hoon` root.

Both-reject trees under `reject/` (`c6_reject_*_test`, rejection_tree_test.sh):
`trailing-comma`, `tab-continuation`, `bar-star-face`, `bar-bare-mark`,
`lib-before-sur`, `raw-after-dat` (runes out of order), `one-space-gap`,
`tab-in-comment`, `skipped-dir` (hoonc's walk skips `packages` and friends),
and, for hoonc's whole-tree pass (`honk.rs` `check_dependency_tree`),
`unreachable-broken-file`, `unreachable-missing-import`, `unreachable-cycle`
and `unreachable-bad-path` (a path `+stab` rejects). An empty file anywhere in
the tree hangs hoonc; honk rejects it (unit-tested only, since hoonc does not
terminate).

Other findings (not parity verdicts):

- A batch manifest that lists the same entry twice fails under `--cache-dir`:
  `compile_entry` queues one pending object per occurrence with the same key
  (`honk.rs` ~2212) and `flush_build_cache` rejects the duplicate root name
  (~2157, "invalid Nockasm root: duplicate root name").
- An empty `/*` data file panics the honk worker ("Set root of NounSlab to
  noun from outside slab", reported as "native compiler worker thread
  panicked", `main` ~604). hoonc also fails: any empty file in the dependency
  tree panics its serf. Both fail, so there is no probe.
- A truncated `--sut-jam` makes nockvm's cue panic (bitvec index) instead of
  returning an error; `cue_subject_type_to_slab`'s error mapping is unused.
- For a `=<` prelude, `NATIVE_HOON_NO_CHUNK=1` gives a different artifact
  from the default chunked mint (the whole-prelude path burps only the final
  type). Only the chunked path is parity-checked (hoon-138 self-mint).

## Uncovered by unit tests (after `cov_c6*`)

### bin/honk.rs

- L446, 509-512, 3329, 4106-4107, 2665-2666 (field expressions in
  `info!`/`debug!` events): diagnostics-only.
- `hoon_log_path` L530 B523F, `build_entry_wer` L878 B873F,
  `entry_path_for_hoon` L4734-4735 B4729F B4730F: only when `current_dir()`
  fails.
- `main` L604, 606: worker panic or thread spawn failure; defensive (L604 is
  reached by the empty-data-file panic above, which hoonc shares).
- `run` L677: the canonical native-dump builder cannot fail; defensive.
- `parse_prelude_hoon` L731, 733-738 B725F: dead, both callers pass
  `docs=true`.
- `parse_inline_wrapper` L746; `compile_exact_wrapper_gates` L1588-1741
  (`?` lines); `extract_exact_wrapper_batteries` L1802, 1817;
  `compile_wrapper_gate` L1868; `compile_wrapper_expression` L1880, 1894;
  `compile_wrapper_gate_exact` L1925; `initialize_exact_wrappers` L1498:
  error propagation for constant wrapper sources that always compile against
  hoon.hoon; unreachable.
- B1850F, 1877F, 1907F (wrapper dbug off): only `--dump-wrapper-assets
  --no-dbug`, an unsupported combination whose batteries could not match the
  native ones.
- `initialize_exact_wrappers` L1517 B1514F B1514c2F: needs a non-canonical
  prelude that still provides hoonc's wrapper stdlib (`vase`, `trap`, `ut`),
  or `HONK_NATIVE_PARITY` with the dump (a full native self-mint of hoon-138,
  ~14GB RSS). Not worth a unit test.
- `build_context_with_shared_prelude` L1034-1042 B1014c2F B1033c2T: the cold
  jam is always embedded; unreachable. L1101, 1104 B1095F B1098F: type
  interning statistics cannot fail or be empty for a real type;
  diagnostics-only. B1119c2F B1124c2F (and B1234c2F B1239c2F in the dynamic
  builder): build.rs makes the embedded `$octs` asset mandatory.
- `build_context_with_dynamic_wrapper_prelude` L1212, 1215-1216 B1170c2F
  B1204F B1206F B1209F: same embedded-asset invariants and diagnostics.
- `initialize_native_wrappers` B1475F, `initialize_exact_wrappers` B1527F
  B1537F: the swet trap is built as `[battery [gun sample]]`, so its payload
  type always decodes.
- `compile_batch_with_shared_prelude` L1295; `jam_product` L2859-2884
  (error closures): a product from `compile_entry` always keeps its vase
  trap; unreachable.
- Dead code, because `compile_entry` passes `evaluate_value=false` and
  `needs_subject=false`, so `need_eval`/`evaluate_value` are always false and
  `standard_jam` is never set: `compile_path` L2234-2235, 2242-2243, 2249,
  2251-2252 B2233T B2233c2T/F B2241T B2241c2T/F B2248T B2250T B2267c2T/F;
  `compile_path_uncached` L2384-2393, 2419-2423 B2336T B2383T B2418T;
  `data_vase` L2509-2512 B2508T; `value_trap` L2635; `jam_product` L2868
  B2867T; `eval_subject_value`, `ensure_subject_values`,
  `eval_formula_with_subject`, `eval_standard_formula_to_jam`,
  `jam_standard_gate_value`, `jam_standard_kernel_trap_native`,
  `slam_gate_with_atom_sample_noun`,
  `slam_gate_with_atom_sample_noun_in_frame`,
  `jam_standard_kernel_product_trap_in_frame` (L2685-2841, 4024-4044,
  4267-4329).
- `compile_entry` L2205-2209, `compile_path` L2289-2294: the uncached compile
  returns a product exactly when `keep_product`; unreachable.
- `compile_path_uncached` L2333: every import-resolution error is raised
  first by the softed-constraints pre-scan in `run`; unreachable. L2344,
  2361: `label_vase` and `subject_trap` cannot fail.
- `compile_path_uncached` L2367-2372 B2358F, L2409-2416 B2408T,
  `eval_vase_trap` L2552-2557: the pinned softed-constraints override. The
  dependency case is parity-covered; a unit test would compile
  `/common/zeke` natively (minutes); compiling the constraints module as an
  entry is not a supported build.
- Cache defensive paths: `decode_cached_vase` L2038, 2042-2043 B2037T,
  `decode_cached_product` L2074, 2078-2084 B2073T, `reject_cache_payload`
  L2108-2116, `compile_entry` L2198, `compile_path` L2273-2277 B2267F. Packs
  are content-addressed under a compiler-fingerprint namespace, so a
  well-formed pack with another payload version is never read; corrupt packs
  fail pack verification first (`tests/cache_cli.rs`).
- `read_cache_object` L2104, `flush_build_cache` B2135c2T: only called or
  queued when a cache exists; unreachable.
- `dump_*` L2962, 2973, 3001; `evaluate_honc_isolated` L3236: I/O or
  prelude-evaluation errors; defensive.
- `mint_honc_prelude_chunked` B3162F: the untraced chunked route;
  diagnostics-only (the traced route is tested).
- `hydrate_pack_root` B3767c2T: the reachability walk stops at hydrated
  nodes; unreachable.
- `eval_formula_noun_in_context` L4125 B4124T, `trace_interpret_error` L4141,
  4143, 4152 B4151F: tracing of interpreter failures during wrapper
  evaluation, non-deterministic and scry errors, `mook` failure;
  diagnostics-only.
- `directory_mug_with_files` L4410-4414: WalkDir yields paths under the
  walked root; unreachable.
- `noun_eq` L4456, 4466-4467 B4452c2F B4455T B4460F B4460c2F: need a mug
  collision; unreachable in practice.
- `lexical_absolute_path` L4756: `Component::Prefix`, Windows only.

### pipeline.rs

- L307-308, 481: `unreachable!()` arms.
- `resolve_import` L321, 327 B318c2F B319F; `vendored_hoon_sys_path` L411
  B408F: the vendored hoon-138.hoon always exists, and Urbit-mode `Sys`
  imports are synthesized only for files that exist.
- `suffix_path_candidates` L629 B628T B635F, `hyphen_segment_variants_inner`
  L670 B669T: the variant builder never yields an empty segment list.
- `parse_native_hoon_source_with_wer_dbug_and_docs` B722F: chumsky reports at
  least one error on failure.
- `hoon_path_for_any` L803, 822-823 B789F B817F B818F: only when
  `current_dir()` fails.
- `parse_roots_for_mode` L855-856 B849F B850F B852F B852c2F: a
  `#[cfg(test)]` helper private to pipeline's own test module.
- L925, 1166, 1168, 1190, 1276, 1278, 1287 B1286F: panic and cleanup arms
  inside pipeline's existing tests.

### errors.rs, lib.rs, types.rs

- errors.rs L214: panic arm in the existing errors test. lib.rs and types.rs:
  nothing left.

## Uncovered by the parity corpus (after the c6 pairings)

Parity coverage for the pairings was measured with the instrumented parity
honk run over each tree (the same flags as the Bazel rules).

- lib.rs (all), types.rs (all), errors.rs accessors and formatting: library
  API for tools and tests; the honk binary never calls it. Unit-covered.
- pipeline.rs `CompileRequest::new`, `build_jam`: hoonc delegation, covered
  by `test-assets:forked_softed_constraints_{parity,batch_parity}_test`
  (not in the probe corpus) and `c6_cli_changed_softed_constraints_*`.
- pipeline.rs whole-graph parse API and Urbit scope mode (`parse_native_hoon*`,
  `NativeImportResolver::parse`/`parse_uncached`, `data_import_expr`,
  `synthetic_urbit_*`, `urbit_sys_file_exists`, `vendored_hoon_sys_path`,
  `sanitize_urbit_sys_header`, `hoon_path_for_any`, `hoon_path_for_absolute`,
  `wer_base_dir_for_mode` L841, `resolve_urbit_arvo_root`,
  `find_urbit_arvo_root`, `resolve_imports_for` B226T): the binary resolves
  each leaf with `ScopeMode::Standard` and never parses the graph as one AST.
  Unit-covered.
- pipeline.rs `/+`, `/-`, `/?` and `/#` handling: covered by
  `c6_sur_lib_spot`, `regressions:c6_ford_pin_spot`, `c6_dat_eager` and
  `c6_dat_core` since the rows 28-32 fixes (line numbers in this ledger
  predate them).
- pipeline.rs rejections, which hoonc also rejects (checked against hoonc):
  `/%` (L477-479), a non-import rune (B440T is covered by `c6_fas_path`, but
  `/~` rejects), malformed `/=` `/*` `/+` clauses (L491-496, 503, 521-537,
  554, 560-562 B495T B502T B525T B533T B553T B559T B573T B573c2T), missing
  imports (L279, 298-299, 331-334 B269F B273F B277F B287F B296F), cycles,
  B447F (import block to EOF with no body). Unit-covered. The header edge
  cases hoonc rejects are the `reject/` trees above.
- bin/honk.rs CLI plumbing (`from_flags` errors, `CompileMode::parse`,
  `usage`, `parse_args`, `parse_batch_manifest` errors, `main` subcommands
  and help, `hoon_log_path`, timing helpers): not salient; unit-covered.
- bin/honk.rs `run` B629T L630-634 (batch delegated to hoonc) and B636F:
  covered by the test-assets softed-fallback pairings. B655F (`--no-dbug`)
  and B712F (bare output name): hoonc has no `--no-dbug`; plumbing.
- Wrapper asset dumps (`build_context_with_dynamic_wrapper_prelude`,
  `initialize_exact_wrappers`, `compile_exact_wrapper_gates`,
  `extract_exact_wrapper_batteries`, `compile_wrapper_*`, `exact_mint_gun`,
  `slam_wrapper_gate`, `evaluate_honc_isolated`,
  `eval_formula_noun_in_context`, `trace_interpret_error`, spot-trace
  decoding, `dump_*`, `hoonc_*_source`, `hoonc_wrapper_wer`): maintenance
  tooling that regenerates the wrapper and cold-state assets; the dynamic and
  native batteries are compared by `test-assets/wrapper_asset_parity_test.sh`
  and `c6_cli_wrapper_asset_dumps_agree`.
- Non-canonical and native-parity preludes (`build_context_with_shared_prelude`
  B1014F B1029T B1029c2F B1033F B1044F B1060T L1061, `seed_honc_type_with_ut`
  B3033F, `peel_transparent`, `prelude_variant_name`, `mint_honc_formula_with_ut`
  B3191F, `mint_honc_prelude_chunked` L3101 B3100F, `data_vase` B2490F and
  `local_octs_type`): hoonc has only its own hoon.hoon, so a non-canonical
  prelude has no reference; the chunked native-parity mint is covered by
  `test-assets:hoon_138_arbitrary_parity_test`. Unit-covered.
- IR round trip and tracing branches (`HONK_IR_ROUNDTRIP`,
  `NATIVE_HOON_TRACE`, `NATIVE_HOON_SKIP_BURP`, `trace_native`,
  `trace_timed`, `mint_with_sut` B2469T): diagnostics-only; unit-covered.
- `--sut-jam` decoding (`extract_subject_type_root` B3940T B3943F B3945F,
  `type_cell_parts`, `looks_like_type_noun`, `type_tag_atom_text` error
  branches): with the canonical subject type the output equals the default
  build (`c6_cli_subject_type_override_and_relative_output`), which is
  parity-checked; the rest are error paths. Unit-covered.
- Path fallbacks: `build_entry_wer` B865T, `path_is_inside_dir` B891T,
  `build_import_wer` B911F B919T/F, `entry_path_for_hoon` B4716T B4731T,
  `hoon_path_from_relative` B4741T: symlinked roots. A symlink-only match
  cannot be arranged with Bazel-declared inputs; an entry equal to the root
  is not buildable. Unit-covered.
- Cache internals not reached by `c6_batch_*`: the defensive paths listed in
  the unit section, `compile_path` cycle B2281T and
  `dependency_merkle_for_path` B1937T (rejections: hoonc rejects cyclic
  graphs), and `build_pack_node` op arms L3813-3873 (packs are written in
  noun mode, so only atom and cell nodes occur; the op arms guard untrusted
  packs). Unit-covered.
- `directory_mug_with_files` B4417T and `hoonc_directory_allowed_paths`
  B4658T: a directory list that omits files changes the hash, and hoonc has
  no file-subset option. The suffix-matching branches of
  `hoonc_manifest_relative_path` run only with symlinked inputs (Bazel
  sandbox; the `c6_batch_kernel` pairing) and are unit-covered.
- `dor`, `gor_mug`/`mor_mug` Equal arms, `map_put_mug` B4613F, `noun_eq`
  collision arms: need equal mugs or a repeated path with different
  contents, which a real directory walk never produces. Unit-covered.
- `softed_constraints_pins_match` B3291T/F, `native_value_override` B3264F,
  `compile_entry_with_hoonc` B3344F L3349: plumbing and defensive checks
  (an unpinned module is always delegated before native compilation).
  Unit-covered.
- Dead code (the eval path, see the unit section): unreachable.

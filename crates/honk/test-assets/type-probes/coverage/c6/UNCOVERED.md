# c6 coverage ledger: honk pipeline, library entry points, and CLI

Range: `crates/honk/src/pipeline.rs`, `lib.rs`, `errors.rs`, `types.rs` and
`crates/honk/src/bin/honk.rs`, excluding `#[cfg(test)]` modules. Gaps: 2578
lines (45 unit-only, 2168 parity-only, 365 both) and 446 branch outcomes (4
unit-only, 344 parity-only, 98 both). Tags: `U` = not covered by unit tests
(including `cov_c6.rs`, `bin_tests/cov_c6_bin.rs` and `tests/cov_c6_cli.rs`),
`P` = not covered by the parity corpus, `UP` = both. Line numbers match the
current source.

## bin/honk.rs

### Unit-only gaps [U] (covered by parity)

- `main` L604: the worker-panic arm. One both-reject probe in the parity
  corpus panics the honk worker; no unit test panics it.
- `compile_entry` L2359-2363: unreachable, because `compile_path_uncached`
  with `keep_product` always returns a product. Its single parity hit is a
  counter artifact: LLVM derives this arm's count by subtraction, and the
  compile that panics the worker (L604) unwinds through the match.
- `compile_path_uncached` L2498: an import without a face (`*`). Parity
  covers it; no unit test compiles a faceless import yet.
- `compile_path_uncached` L2515, 2521-2526, 2571-2578; B2512F B2570T: the
  pinned `/dat/softed-constraints.hoon` override (`native_value_override`).
  Parity reaches it through corpus builds that import the pinned module; no
  unit test compiles that module natively yet.
- `compile_path_uncached` L2536-2537, B2533T, `kick_vase_trap_value` L2729,
  2731-2734, 2736-2744, 2746, and `eval_vase_trap` L2748-2753: a `/dat` node
  kicked while it is built (`eval_vase_trap` also serves the override above).
  `c6_dat_eager` and `c6_dat_core` cover it in parity; no unit test builds a
  `/dat` node yet.
- `mint_honc_prelude_chunked` B3358F: the untraced chunked prelude mint. A
  native-parity build in the corpus covers it; the unit test runs it with
  `NATIVE_HOON_TRACE` set.

### Not yet covered [UP]

- `run` L652: a batch entry whose dependency tree fails
  `check_dependency_tree`. Uncovered: no test exercises it yet (the
  single-entry route is tested).
- `run` L700: `--dump-native-wrapper-assets` with a prelude that fails to
  build (the canonical prelude always builds). Uncovered: no test exercises
  it yet.
- `parse_prelude_hoon` L925: a prelude that fails to parse. Uncovered: no
  test exercises it yet.
- `jam_product` L3075, 3077, 3079-3080 and `compile_batch_with_shared_prelude`
  L1449: a standard-mode entry whose product cannot be slammed with the
  `@uvI` directory hash (not a gate). hoonc's `shot` wrapper mints the same
  slam, so both compilers reject it. Uncovered: no test exercises it yet.
- `initialize_exact_wrappers` L1671; B1668F B1668c2F: `--dump-wrapper-assets`
  with a non-canonical prelude that still provides hoonc's wrapper stdlib
  (`vase`, `trap`, `ut`), or with `HONK_NATIVE_PARITY` (a full native
  self-mint of hoon-138, about 14GB RSS). Uncovered: no test exercises it
  yet.
- `compile_wrapper_gate` B2004F, `compile_wrapper_expression` L2034, B2031F,
  and `compile_wrapper_gate_exact` B2061F: wrapper compiles with dbug off,
  reached only by `--dump-wrapper-assets --no-dbug`, an unsupported
  combination whose batteries could not match the native ones. Uncovered: no
  test exercises it yet.

### Unreachable, dead, defensive, or diagnostics-only [UP]

- Dead or unreachable because `need_eval` and `evaluate_value` are always
  false (`compile_entry` passes `evaluate_value=false` and
  `needs_subject=false`, so every import compiles with `need_eval=false`)
  and `standard_jam` is never set: `compile_path` L2388-2389, 2396-2397,
  2403, 2405-2406, 2427-2429; B2387T B2387c2T/F B2395T B2395c2T/F B2402T
  B2404T B2421F B2421c2T/F; `compile_path_uncached` L2546-2549, 2552-2555,
  2581-2585; B2490T B2545T B2580T; `data_vase` L2671-2674, B2670T;
  `jam_product` L3064, B3063T; and every gap in `eval_subject_value`
  (L2881-2899), `ensure_subject_values` (L2901-2908, B2903T/F),
  `eval_formula_with_subject` (L2910-2928), `eval_standard_formula_to_jam`
  (L2930-3032, all outcomes of B2958-B3025), `jam_standard_gate_value`
  (L3034-3037), `jam_standard_kernel_trap_native` (L4220-4240, B4231T/F),
  `slam_gate_with_atom_sample_noun` (L4463-4495, B4484T/F),
  `slam_gate_with_atom_sample_noun_in_frame` (L4497-4517) and
  `jam_standard_kernel_product_trap_in_frame` (L4519-4525).
- Other dead code: `parse_prelude_hoon` L927-932, B919F (both callers pass
  `docs=true`); `value_trap` L2831 (only called for arbitrary output).
- Unreachable:
  - `compile_path` L2443-2448 (a dependency compile returns a vase) and
    `jam_product` L3055, 3057, 3059, 3068, 3070, 3072 (a product from
    `compile_entry` always keeps its vase trap).
  - `compile_path` L2436, B2435T and `dependency_merkle_for_path` L2092,
    B2091T: `check_dependency_tree` rejects an import cycle before any
    compile.
  - `compile_path_uncached` L2487: `run` resolves the imports of every
    reachable file first (`entry_uses_unpinned_softed_constraints`,
    `check_dependency_tree`), so a resolution error never reaches a compile.
  - `check_dependency_tree` L784, 786, 788 and `directory_mug_with_files`
    L4606, 4608, 4610: WalkDir yields paths under the walked root.
    `check_dependency_tree` L897, B891F: the walk visits only graph keys.
  - `run` L656, B654F: `parse_args` always yields an entry, a batch manifest
    or a dump directory.
  - Embedded assets (build.rs embeds the cold state and the `$octs` type):
    `build_context_with_shared_prelude` L1188, 1190-1191, 1193-1196;
    B1168c2F B1187c2T B1273c2F B1278c2F;
    `build_context_with_dynamic_wrapper_prelude` B1324c2F B1388c2F B1393c2F.
  - `initialize_native_wrappers` B1629F and `initialize_exact_wrappers`
    B1681F B1691F: the swet trap is built as `[battery [gun sample]]`, so
    its payload type always decodes.
  - Constant wrapper sources always parse, compile and evaluate against
    hoon.hoon: `parse_inline_wrapper` L940; `initialize_exact_wrappers`
    L1652; `compile_exact_wrapper_gates` L1742, 1755, 1760, 1772, 1788,
    1801, 1816, 1832, 1848, 1868, 1873, 1884, 1895;
    `extract_exact_wrapper_batteries` L1956, 1971; `compile_wrapper_gate`
    L2022; `compile_wrapper_expression` L2048; `compile_wrapper_gate_exact`
    L2079; `slam_wrapper_gate` L2866-2868, B2865T B2866T/F.
  - Cache: `read_cache_object` L2258, `reject_cache_payload` B2263F and
    `flush_build_cache` B2289c2T are only called or queued when a cache
    exists; `hydrate_pack_root` B3963c2T: the reachability walk stops at
    hydrated nodes.
  - `noun_eq` L4652, 4662-4663; B4648c2F B4651T B4656F B4656c2F: two
    distinct nouns with equal mugs; unreachable in practice.
- Defensive:
  - `decode_cached_vase` L2196-2197 and `decode_cached_product` L2228,
    2232-2233, 2236, 2238, B2227T: payload version and flag checks. Packs
    are content-addressed under a compiler-fingerprint namespace, corrupt
    packs fail pack verification first, and honk never writes an eval value
    or a product without a trap.
  - `current_dir()` failure: `hoon_log_path` L530, B523F; `build_entry_wer`
    L1072, B1067F; `entry_path_for_hoon` L4924-4925, B4919F B4920F.
  - `main` L606 (worker thread spawn failure); `dump_exact_wrapper_assets`
    L3158, 3169 and `dump_native_wrapper_assets` L3197 (write errors);
    `evaluate_honc_isolated` L3432 (evaluation failure after a successful
    mint); `lexical_absolute_path` L4946 (`Component::Prefix`, Windows only).
- Diagnostics-only:
  - Fields of `info!`/`debug!` events, evaluated only when that level is
    enabled: `TimedHoonPathLog::drop` L446, `report_native_timing_totals`
    L509-512, `run` L658-660, `slam_wrapper_gate` L2861-2862,
    `compile_entry_with_hoonc` L3525, `eval_formula_noun_in_context`
    L4302-4303.
  - Type-intern statistics under `HONK_IR_ROUNDTRIP`, which cannot fail or
    be empty for a real type: `build_context_with_shared_prelude` L1255,
    1258, B1249F B1252F; `build_context_with_dynamic_wrapper_prelude` L1366,
    1369-1370, B1358F B1360F B1363F.
  - Tracing of interpreter failures: `eval_formula_noun_in_context` L4321,
    B4320T; `trace_interpret_error` L4337, 4339, 4348, B4347F
    (non-deterministic and scry errors, and a `mook` failure).

### Parity-only gaps [P] (unit-covered)

- CLI plumbing. The parity replay runs only compile invocations with Bazel's
  workspace-relative paths: `from_flags` L107-109, B106T (mutually exclusive
  modes); `usage` L141-142, 152; `parse_args` L194-197, 200-203, 206-209,
  212-215, 218-221, 225, 234-237, 241-254, 256-268, 273-287, 289-301,
  306-317, 319-331, 336; B224T B233T B233c2T/F B240T B241T/F B241c2T/F
  B247T/F B252T/F B272T B273T/F B273c2T/F B279T/F B285T/F B305T B306T/F
  B309T/F B315T/F B335T (`--sut-jam`, `--cache-dir`, `--batch-manifest`,
  the dump flags, and argument errors); `main` L542-545, 547-549, 554-557,
  559-561, 567-569, 571-574; B541T B553T B567T/F (`nockasm` and `cache`
  subcommands, `--help`, argument errors); `run` B678F (`--no-dbug`, which
  hoonc does not have); `run` L740, B735F and `compile_entry_with_hoonc`
  L3545, B3540F (an output path with no directory).
- Dynock and batch modes. The `c6_dynock`, `c6_dynock_typed` and `c6_batch_*`
  genrule pairings are verified by Bazel but not replayed for coverage:
  `from_flags` L114, 116, B113T B115T; `CompileMode::parse` L122-128, 130;
  `parse_batch_manifest` L356-374, 376, 378-382, 385-389; B361T/F B365T/F
  B373T/F B385T/F; `run` L624-625, 627, 629-632, 634-635, 646-651, 705-712;
  B623T B629T/F B645T B704T (including a whole batch delegated to hoonc);
  `check_dependency_tree` L790-792, 832, 908; B789T B790T/F B907T (manifest
  directory lists and the memo shared by batch entries);
  `compile_batch_with_shared_prelude` L1427-1436, 1441-1448, 1450-1454,
  1456-1458, 1460-1462; B1451T/F; `jam_product` L3047-3052;
  `jam_dynock_output_native` L3562-3568, B3563T/F; and the manifest
  directory lists in `directory_mug_with_files` L4613-4615, B4612T
  B4613T/F, `hoonc_directory_allowed_paths` L4851-4857, 4859-4860,
  B4854T/F, `hoonc_manifest_relative_path` L4862-4870, 4872-4883,
  4887-4892; B4863T/F B4864T/F B4868T/F B4878T/F B4880T/F, and
  `normal_path_components` L4894-4901.
- Persistent cache. Only the batch genrule passes `--cache-dir`:
  `dependency_merkle_for_path` L2086-2091, 2093-2113, 2115-2124, 2127-2129,
  2131-2132, 2134-2138, 2140-2144; B2088T/F B2091F; `cache_object_key`
  L2146-2156, 2158-2160; `decode_cache_noun` L2162-2176; `cached_vase_noun`
  L2178-2183, 2185; `decode_cached_vase` L2187-2195, 2199-2204, B2191T/F;
  `cached_product_noun` L2206-2219, 2221; `decode_cached_product`
  L2223-2227, 2229-2231, 2235, 2237, 2240-2249, B2227F; `read_cache_object`
  L2251-2257, 2260; `reject_cache_payload` L2262-2266, 2268, 2270, B2263T;
  `queue_cache_object` L2272-2286; `flush_build_cache` L2291-2292,
  2295-2299, 2301-2326; B2289F B2289c2F; `compile_entry` L2340-2342,
  2347-2348, 2350-2352, 2354, 2367-2368; B2339T B2346T B2347T/F B2366T;
  `compile_path` L2409-2412, 2417-2418, 2420-2425, 2431, 2433, 2457-2463;
  B2408T B2416T B2417T/F B2421T B2456T; `NativeBuildContext::drop`
  L3105-3109, B3104T; `hydrate_pack_root` L3942-3954, 3956-3960, 3962-3966,
  3968-3969; B3948T/F B3953T/F B3956T/F B3956c2T/F B3963T/F B3963c2F;
  `push_pack_children` L3971, 3973-3990, 3992; `build_pack_node` (every gap
  in L3999-4073; packs are written in noun mode, so its op arms only guard
  untrusted packs); `nasm_atom_to_slab` L4075-4082, B4076T/F B4077T/F;
  `cache_tuple_fields` L4438-4449, 4451-4453, B4439T/F; `atom_u64`
  L4434-4436 (also used by trace decoding).
- Wrapper asset dumps: maintenance tooling that regenerates the wrapper and
  cold-state assets; `test-assets/wrapper_asset_parity_test.sh` and
  `c6_cli_wrapper_asset_dumps_agree` compare the dynamic and native
  batteries. `run` L640, 663, 683-691, 695-699, 701; B636F B642F B642c2F
  B682T B694T; `parse_inline_wrapper` L936-939, 941; `hoonc_wrapper_source`
  L968-975, B971T/F; `hoonc_standard_output_source` (every gap in
  L977-1007); `hoonc_dir_hash_source` (every gap in L1009-1045);
  `hoonc_wrapper_wer` L1054-1056; `build_context_with_dynamic_wrapper_prelude`
  (every P gap in L1312-1425); `initialize_exact_wrappers` L1635-1641,
  1643-1651, 1653-1655, 1657-1659, 1661-1662, 1667-1669, 1674-1676,
  1678-1693, 1695, 1697-1698; B1668T B1668c2T B1681T B1691T;
  `compile_wrapper_gate_maybe_exact` L1700-1708, 1710;
  `compile_exact_wrapper_gates` (every P gap in L1712-1897);
  `extract_exact_wrapper_batteries` (every P gap in L1899-2001);
  `compile_wrapper_gate` L2003-2006, 2008, 2010-2021, 2023-2027; B2004T
  B2004c2T/F B2005T/F; `compile_wrapper_expression` L2029-2032, 2036-2047,
  2049-2053, B2031T; `compile_wrapper_gate_exact` L2055-2063, 2065,
  2067-2078, 2080-2084; B2061T B2061c2T/F B2062T/F; `mint_with_subject_type`
  L2599-2602; `exact_mint_gun` L2774-2784; `slam_wrapper_gate` L2840-2846,
  2848-2858, 2865, 2869-2871, 2874-2879, B2865F; `dump_exact_wrapper_assets`
  (every P gap in L3119-3171); `dump_native_wrapper_assets` L3173-3196,
  3199-3200; `jam_noun_in_fresh_slab` L3202-3206; `evaluate_honc_isolated`
  L3403-3409, 3411-3416, 3418-3430, 3433-3436, 3438; `trap_battery`
  L3628-3634; `jam_slab_noun` L3926-3929; `slam_gate_formula` L4527-4533.
- Non-canonical preludes and `--sut-jam`. hoonc has only its own hoon.hoon,
  so a non-canonical prelude has no reference, and `--sut-jam` with the
  canonical subject type gives the default build
  (`c6_cli_subject_type_override_and_relative_output`): `run` L729 (a
  prelude that fails to build); `build_context_with_shared_prelude` L1178,
  1285; B1168F B1183T B1183c2F B1187F B1198F B1273T B1278F; `data_vase`
  L2661-2665, 2667-2668, B2652F; `local_octs_type` L3582-3588;
  `ty_atom_local` B3910T; `seed_honc_type_with_ut` L3233, B3229F;
  `peel_transparent` L3252-3253, B3251F (prelude shapes other than hoon-138
  parsed with dbug off); `mint_honc_prelude_chunked` L3297, B3296F (a
  prelude that is not `=<`); `mint_honc_formula_with_ut` L3391-3400, B3387F
  (the whole-prelude route for a non-`=<` prelude or `NATIVE_HOON_NO_CHUNK`,
  whose artifact differs from the chunked one, an open bug listed in
  DIVERGENCES.md); `extract_subject_type_root` L4137, 4143-4145; B4136T
  B4139F B4141F; `prelude_type_from_subject_type` L4150-4151;
  `type_cell_parts` L4162, B4161T; `looks_like_type_noun` L4173, 4176,
  4185-4193; B4172T B4175F; `type_tag_atom_text` L4204, 4210; B4203T B4206T
  B4208F B4208c2T/F.
- Debug environment variables and tracing (diagnostics-only):
  `build_context_with_shared_prelude` L1215, 1225-1229, 1246-1250,
  1252-1253, 1257, 1259; B1214T B1224T B1227T/F B1245T B1247T/F B1249T
  B1252T (`NATIVE_HOON_SKIP_BURP`, `HONK_IR_ROUNDTRIP`, `NATIVE_HOON_TRACE`);
  `mint_with_sut` L2632-2633, B2631T; `mint_honc_prelude_chunked` L3359,
  B3358T; `prelude_variant_name` L3261-3276 (names printed in a log line);
  `eval_formula_noun_in_context` L4282-4285, 4290-4293, 4309-4312; B4281T
  B4289T B4308T; `trace_interpret_error` L4334-4336, 4338, 4341-4347,
  4350-4356; B4342T/F B4347T; spot-trace decoding, every gap in L4358-4432
  (`normalize_interpret_trace`, `decode_spot_trace`, `decode_spot_noun`,
  `decode_path_noun`, `decode_pint_noun`, `atom_text`); `trace_native`
  L4561, B4560T; `trace_timed` L4568, 4570-4577, 4579, B4566F;
  `hoon_log_path` L526, 529, 532; B524F B525T (log paths equal to or
  outside the working directory).
- Rejections: `check_dependency_tree` L804-808, B800T (a path `+stab`
  rejects) and L867-871, B866T (an import cycle) are the
  `unreachable-bad-path` and `unreachable-cycle` trees under `reject/`,
  verified by Bazel against hoonc but not replayed for coverage.
  `check_dependency_tree` L823, B822T: an empty file in the tree, which
  hangs hoonc, so there is no hoonc verdict. `eval_formula_noun_in_context`
  L4319-4320, 4322-4323, B4320F: an evaluation that crashes (no parity probe
  has a crashing `/dat` node).
- Defensive checks: `catch_nock_panic` L4539-4554 (a panic inside Nock
  evaluation); `native_value_override` L3462-3467, B3460F (an unpinned
  softed-constraints module that reached native compilation; `run` delegates
  it to hoonc first); `softed_constraints_pins_match` L3487-3488, B3487T/F
  (a pinned file that is missing or unreadable).
- Path fallbacks: `build_entry_wer` B1059T (the binary passes
  `absolute_entry_wer=true` for every entry); `build_entry_wer` L1071, 1073,
  B1069F, `path_components_for_dbug` L1117 and `entry_path_for_hoon` L4923,
  4927, B4921F (an entry outside both the dependency root and the working
  directory); `build_import_wer` L1097, 1099-1106; B1095F B1103T/F and
  `entry_path_for_hoon` L4913, B4912T (a root reached through a symlink; the
  replay's inputs sit lexically under their roots); `hoon_path_from_relative`
  L4932, B4931T (an entry equal to the root, which is not buildable);
  `lexical_absolute_path` L4948-4951 (`.` and `..` components).
- Mug ties: `dor` (every gap in L4715-4762), `gor_mug` L4769 and `mor_mug`
  L4782 (distinct keys with equal mugs, or equal mugs of mugs), and
  `map_put_mug` L4811-4813, B4809F (one path with two contents). A real
  directory walk produces neither.

## pipeline.rs

### Not yet covered [UP]

- `vul` B511c2F: a DEL (127) byte inside a header comment. Uncovered: no
  test exercises it yet.
- `stap` L582, B581c2T: an import path with a trailing `/`. Uncovered: no
  test exercises it yet.
- `reject_leftover_import` L706, B705F: a file that ends in a lone `/` right
  after its import header. Uncovered: no test exercises it yet.

### Unreachable, defensive, or test-only [UP]

- `resolve_import` L276-277: `unreachable!` arms.
- `resolve_import` L290, 296, B287c2F B288F and `vendored_hoon_sys_path`
  L443, B440F: the vendored hoon-138.hoon always exists, and Urbit-mode
  `Sys` imports are synthesized only for files that exist.
- `rune_clause` L658: it is called only with the five import runes.
- `suffix_path_candidates` L780, B779T B786F and
  `hyphen_segment_variants_inner` L810, 822, B809T B821T: the variant
  builder never yields an empty segment list.
- `parse_native_hoon_source_with_wer_dbug_and_docs` B879F: chumsky reports
  at least one error on failure.
- `hoon_path_for_any` L960, 979-980; B946F B974F B975F: only when
  `current_dir()` fails. Defensive.
- `parse_roots_for_mode` L1012-1013; B1006F B1007F B1009F B1009c2F: a
  `#[cfg(test)]` helper, and pipeline's own tests do not take these
  outcomes. Test-only code.

### Parity-only gaps [P] (unit-covered)

- Test-only code absent from the parity binary:
  `parse_native_hoon_with_wer_and_dbug` L838-845 and `parse_roots_for_mode`
  L1004-1011, 1014-1015, B1006T B1007T B1009T B1009c2T are `#[cfg(test)]`.
- `CompileRequest::new` L30-40: the parity profile has no records for these
  lines, although the corpus's one delegated build runs `build_jam`
  (L43-50) through it. `c6_cli_changed_softed_constraints_are_delegated_to_hoonc`
  covers it.
- Whole-graph parse API and Urbit scope mode. The binary resolves each leaf
  with `ScopeMode::Standard` and never parses the graph as one AST:
  `parse_native_hoon` L52-54; `parse_native_hoon_with_mode` L56-65;
  `parse_native_hoon_leaf` L67-69; `parse_native_hoon_leaf_with_mode`
  L71-84; `NativeImportResolver::parse` L172-184, 186-192; B174T/F B177T/F
  B188T/F; `parse_uncached` L194-200, 202-206, 208-212, 214-222, 225-226,
  B204T/F; `resolve_imports_for` L234, B233T; `resolve_import` L274,
  288-289; B287T B287c2T B288T; `synthetic_urbit_scope_faces` (every gap in
  L328-350); `synthetic_urbit_scope_imports` L355-357, 359, 362-364,
  366-367, 369, 374-383; B353F B356T/F B366T/F; `urbit_sys_file_exists`
  L428-433, B429T/F; `vendored_hoon_sys_path` L435-441, 445, B440T;
  `data_import_expr` L754-774; `parse_native_hoon_source_with_wer_and_dbug`
  L847-854; `sanitize_urbit_sys_header` (every gap in L900-937);
  `hoon_path_for_any` (every P gap in L941-983); `hoon_path_for_absolute`
  L985-992; `wer_base_dir_for_mode` L998; `resolve_urbit_arvo_root`
  L1017-1022, B1018T/F; `find_urbit_arvo_root` L1024-1033, 1035-1039,
  1041-1042; B1026T/F B1029T/F B1037T/F.
- Import rejections, which hoonc also rejects. The multi-file trees under
  `reject/` are verified by Bazel against hoonc but not replayed for
  coverage:
  - Missing imports (`unreachable-missing-import`, `skipped-dir`):
    `resolve_import` L263, 286-287, 291-295, 297-298, 300-303; B262T B284F
    B287F; `hoonc_tree_file` L314, 322; B310T B321T B321c2T (`.` or `..`
    knots, a skipped directory, or a suffix hoonc does not load);
    `path_knots` L422, B421T and `stap` B581T B581c2F (the root path `/`).
  - Malformed headers: `gaq` L533, B531F (`one-space-gap`); `sym` L551,
    B550T (`bar-star-face`); `reject_leftover_import` L710, 713-730; B711F
    B720T/F (a clause the header rule cannot take, as in `trailing-comma`,
    `tab-continuation`, `bar-bare-mark`, `lib-before-sur` and
    `raw-after-dat`; `/%` is reported as unsupported).

## errors.rs, lib.rs, types.rs

- lib.rs (`Compiler` and `Compiled`: L34-37, 39-47, 49-57, 68-70, 72-74,
  80-82, 84-87, 94-100, 105-110, 116-119) and types.rs (`TypeNoun`: every
  gap in L10-94, B37T/F B46T/F B51T/F B52T/F B60T/F B62T/F) [P]: library
  API for tools and tests; the honk binary never calls it. Unit-covered.
- errors.rs [P]: `CompilerErrorKind` display L21-22 (the `Backend` and
  `Decode` kinds); `CompilerErrorLocation` display L58-60, 62, 64-66; B54F
  B60T/F (locations without a file, or with a zero-width span);
  `CompilerErrorMetadata` display L75, B73F (metadata without a location);
  the accessors `kind` L110-118, 120, `message` L122-130, 132, `metadata`
  L134-137, 139 and `is_unsupported_expr` L141-143; `with_metadata`
  L157-166, 177 (the `Backend`, `Decode`, `Noun` and `Io` arms). No parity
  rejection produces these shapes, and the binary never calls the
  accessors. Diagnostics and library API; unit-covered.

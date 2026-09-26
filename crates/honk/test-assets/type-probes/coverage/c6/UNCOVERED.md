# c6 coverage ledger: honk pipeline, library entry points, and CLI

Range: `crates/honk/src/pipeline.rs`, `lib.rs`, `errors.rs`, `types.rs` and
`crates/honk/src/bin/honk.rs`, excluding `#[cfg(test)]` modules. Gaps: 2478
lines (0 unit-only, 2125 parity-only, 353 both) and 427 branch outcomes (0
unit-only, 337 parity-only, 90 both). Tags: `U` = not covered by unit tests
(including `cov_c6.rs`, `bin_tests/cov_c6_bin.rs` and `tests/cov_c6_cli.rs`),
`P` = not covered by the parity corpus, `UP` = both. Line numbers match the
current source.

## bin/honk.rs

### Unreachable, dead, defensive, or diagnostics-only [UP]

- Dead or unreachable because `need_eval` and `evaluate_value` are always
  false (`compile_entry` passes `evaluate_value=false` and
  `needs_subject=false`, so every import compiles with `need_eval=false`)
  and `standard_jam` is never set: `compile_path` L2391-2392, 2399-2400,
  2406, 2408-2409, 2430-2432; B2390T B2390c2T/F B2398T B2398c2T/F B2405T
  B2407T B2424F B2424c2T/F; `compile_path_uncached` L2549-2552, 2555-2558,
  2584-2588; B2493T B2548T B2583T; `data_vase` L2674-2677, B2673T;
  `jam_product` L3067, B3066T; and every gap in `eval_subject_value`
  (L2884-2902), `ensure_subject_values` (L2904-2911, B2906T/F),
  `eval_formula_with_subject` (L2913-2931), `eval_standard_formula_to_jam`
  (L2933-3035, all outcomes of B2961-B3025), `jam_standard_gate_value`
  (L3037-3040), `jam_standard_kernel_trap_native` (L4277-4297, B4288T/F),
  `slam_gate_with_atom_sample_noun` (L4520-4552, B4541T/F),
  `slam_gate_with_atom_sample_noun_in_frame` (L4554-4574) and
  `jam_standard_kernel_product_trap_in_frame` (L4576-4582).
- Other dead code: `parse_prelude_hoon` L930-935, B922F (both callers pass
  `docs=true`); `value_trap` L2834 (only called for arbitrary output).
- Unreachable:
  - `compile_path` L2446-2451 (a dependency compile returns a vase) and
    `jam_product` L3058, 3060, 3062, 3071, 3073, 3075 (a product from
    `compile_entry` always keeps its vase trap).
  - `compile_path` L2439, B2438T and `dependency_merkle_for_path` L2095,
    B2094T: `check_dependency_tree` rejects an import cycle before any
    compile.
  - `compile_path_uncached` L2490: `run` resolves the imports of every
    reachable file first (`entry_uses_unpinned_softed_constraints`,
    `check_dependency_tree`), so a resolution error never reaches a compile.
  - `check_dependency_tree` L787, 789, 791 and `directory_mug_with_files`
    L4663, 4665, 4667: WalkDir yields paths under the walked root.
    `check_dependency_tree` L900, B894F: the walk visits only graph keys.
  - `run` L659, B657F: `parse_args` always yields an entry, a batch manifest
    or a dump directory.
  - Embedded assets (build.rs embeds the cold state and the `$octs` type):
    `build_context_with_shared_prelude` L1191, 1193-1194, 1196-1199;
    B1171c2F B1190c2T B1276c2F B1281c2F;
    `build_context_with_dynamic_wrapper_prelude` B1327c2F B1391c2F B1396c2F.
  - `initialize_native_wrappers` B1632F and `initialize_exact_wrappers`
    B1684F B1694F: the swet trap is built as `[battery [gun sample]]`, so
    its payload type always decodes.
  - Constant wrapper sources always parse, compile and evaluate against
    hoon.hoon: `parse_inline_wrapper` L943; `initialize_exact_wrappers`
    L1655; `compile_exact_wrapper_gates` L1745, 1758, 1763, 1775, 1791,
    1804, 1819, 1835, 1851, 1871, 1876, 1887, 1898;
    `extract_exact_wrapper_batteries` L1959, 1974; `compile_wrapper_gate`
    L2025; `compile_wrapper_expression` L2051; `compile_wrapper_gate_exact`
    L2082; `slam_wrapper_gate` L2869-2871, B2868T B2869T/F.
  - Cache: `read_cache_object` L2261, `reject_cache_payload` B2266F and
    `flush_build_cache` B2292c2T are only called or queued when a cache
    exists; `hydrate_pack_root` B4020c2T: the reachability walk stops at
    hydrated nodes.
  - `noun_eq` L4709, 4719-4720; B4705c2F B4708T B4713F B4713c2F: two
    distinct nouns with equal mugs; unreachable in practice.
- Defensive:
  - `decode_cached_vase` L2199-2200 and `decode_cached_product` L2231,
    2235-2236, 2239, 2241, B2230T: payload version and flag checks. Packs
    are content-addressed under a compiler-fingerprint namespace, corrupt
    packs fail pack verification first, and honk never writes an eval value
    or a product without a trap.
  - `current_dir()` failure: `hoon_log_path` L530, B523F; `build_entry_wer`
    L1075, B1070F; `entry_path_for_hoon` L4981-4982, B4976F B4977F.
  - `main` L606 (worker thread spawn failure); `dump_exact_wrapper_assets`
    L3161, 3172 and `dump_native_wrapper_assets` L3200 (write errors);
    `evaluate_honc_isolated` L3489 (evaluation failure after a successful
    mint); `lexical_absolute_path` L5003 (`Component::Prefix`, Windows only).
- Diagnostics-only:
  - Fields of `info!`/`debug!` events, evaluated only when that level is
    enabled: `TimedHoonPathLog::drop` L446, `report_native_timing_totals`
    L509-512, `run` L661-663, `slam_wrapper_gate` L2864-2865,
    `compile_entry_with_hoonc` L3582, `eval_formula_noun_in_context`
    L4359-4360.
  - Type-intern statistics under `HONK_IR_ROUNDTRIP`, which cannot fail or
    be empty for a real type: `build_context_with_shared_prelude` L1258,
    1261, B1252F B1255F; `build_context_with_dynamic_wrapper_prelude` L1369,
    1372-1373, B1361F B1363F B1366F.
  - Tracing of interpreter failures: `eval_formula_noun_in_context` L4378,
    B4377T; `trace_interpret_error` L4394, 4396, 4405, B4404F
    (non-deterministic and scry errors, and a `mook` failure).

### Parity-only gaps [P] (unit-covered)

- `main` L611, B610T: the `HONK_MEMO_VERIFY` report, printed only with the
  variable set, which the parity corpus never sets (the verify CLI test
  covers it).
- CLI plumbing. The parity replay runs only compile invocations with Bazel's
  workspace-relative paths: `from_flags` L107-109, B106T (mutually exclusive
  modes); `usage` L141-142, 152; `parse_args` L194-197, 200-203, 206-209,
  212-215, 218-221, 225, 234-237, 241-254, 256-268, 273-287, 289-301,
  306-317, 319-331, 336; B224T B233T B233c2T/F B240T B241T/F B241c2T/F
  B247T/F B252T/F B272T B273T/F B273c2T/F B279T/F B285T/F B305T B306T/F
  B309T/F B315T/F B335T (`--sut-jam`, `--cache-dir`, `--batch-manifest`,
  the dump flags, and argument errors); `main` L542-545, 547-549, 554-557,
  559-561, 567-569, 571-574; B541T B553T B567T/F (`nockasm` and `cache`
  subcommands, `--help`, argument errors); `run` B681F (`--no-dbug`, which
  hoonc does not have); `run` L743, B738F and `compile_entry_with_hoonc`
  L3602, B3597F (an output path with no directory).
- Batch mode. The `c6_batch_*` genrule pairings are verified by Bazel but
  not replayed for coverage (the replay runs the dynock pairings and single
  entries only): `CompileMode::parse` L122-128, 130;
  `parse_batch_manifest` L356-374, 376, 378-382, 385-389; B361T/F B365T/F
  B373T/F B385T/F; `run` L627-628, 630, 632-635, 637-638, 649-654, 708-715;
  B626T B632T/F B648T B707T (including a whole batch delegated to hoonc);
  `check_dependency_tree` L793-795, 835, 911; B792T B793T/F B910T (manifest
  directory lists and the memo shared by batch entries); `run` L655 (a batch
  entry whose dependency tree fails `check_dependency_tree`);
  `compile_batch_with_shared_prelude` L1430-1439, 1444-1451, 1453-1457,
  1459-1461, 1463-1465; B1454T/F; and the manifest directory lists in
  `directory_mug_with_files` L4670-4672, B4669T B4670T/F,
  `hoonc_directory_allowed_paths` L4908-4914, 4916-4917, B4911T/F,
  `hoonc_manifest_relative_path` L4919-4927, 4929-4940, 4944-4949; B4920T/F
  B4921T/F B4925T/F B4935T/F B4937T/F, and `normal_path_components`
  L4951-4958.
- Persistent cache. Only the batch genrule passes `--cache-dir`:
  `dependency_merkle_for_path` L2089-2094, 2096-2116, 2118-2127, 2130-2132,
  2134-2135, 2137-2141, 2143-2147; B2091T/F B2094F; `cache_object_key`
  L2149-2159, 2161-2163; `decode_cache_noun` L2165-2179; `cached_vase_noun`
  L2181-2186, 2188; `decode_cached_vase` L2190-2198, 2202-2207, B2194T/F;
  `cached_product_noun` L2209-2222, 2224; `decode_cached_product`
  L2226-2230, 2232-2234, 2238, 2240, 2243-2252, B2230F; `read_cache_object`
  L2254-2260, 2263; `reject_cache_payload` L2265-2269, 2271, 2273, B2266T;
  `queue_cache_object` L2275-2289; `flush_build_cache` L2294-2295,
  2298-2302, 2304-2329; B2292F B2292c2F; `compile_entry` L2343-2345,
  2350-2351, 2353-2355, 2357, 2370-2371; B2342T B2349T B2350T/F B2369T;
  `compile_path` L2412-2415, 2420-2421, 2423-2428, 2434, 2436, 2460-2466;
  B2411T B2419T B2420T/F B2424T B2459T; `NativeBuildContext::drop`
  L3108-3112, B3107T; `hydrate_pack_root` L3999-4011, 4013-4017, 4019-4023,
  4025-4026; B4005T/F B4010T/F B4013T/F B4013c2T/F B4020T/F B4020c2F;
  `push_pack_children` L4028, 4030-4047, 4049; `build_pack_node` (every gap
  in L4056-4130; packs are written in noun mode, so its op arms only guard
  untrusted packs); `nasm_atom_to_slab` L4132-4139, B4133T/F B4134T/F;
  `cache_tuple_fields` L4495-4506, 4508-4510, B4496T/F; `atom_u64`
  L4491-4493 (also used by trace decoding).
- Wrapper asset dumps: maintenance tooling that regenerates the wrapper and
  cold-state assets; `test-assets/wrapper_asset_parity_test.sh` and
  `c6_cli_wrapper_asset_dumps_agree` compare the dynamic and native
  batteries. `run` L643, 666, 686-694, 698-702, 704; B639F B645F B645c2F
  B685T B697T; `parse_inline_wrapper` L939-942, 944; `hoonc_wrapper_source`
  L971-978, B974T/F; `hoonc_standard_output_source` (every gap in
  L980-1010); `hoonc_dir_hash_source` (every gap in L1012-1048);
  `hoonc_wrapper_wer` L1057-1059;
  `build_context_with_dynamic_wrapper_prelude` (every P gap in L1315-1428);
  `initialize_exact_wrappers` L1638-1644, 1646-1654, 1656-1658, 1660-1662,
  1664-1665, 1670-1672, 1674, 1677-1679, 1681-1696, 1698, 1700-1701;
  B1671T/F B1671c2T/F B1684T B1694T (L1674 and the F outcomes keep the
  minted prelude formula, for a non-canonical prelude or under
  `HONK_NATIVE_PARITY`); `compile_wrapper_gate_maybe_exact` L1703-1711,
  1713; `compile_exact_wrapper_gates` (every P gap in L1715-1900);
  `extract_exact_wrapper_batteries` (every P gap in L1902-2004);
  `compile_wrapper_gate` L2006-2009, 2011, 2013-2024, 2026-2030; B2007T/F
  B2007c2T/F B2008T/F; `compile_wrapper_expression` L2032-2035, 2037,
  2039-2050, 2052-2056, B2034T/F; `compile_wrapper_gate_exact` L2058-2066,
  2068, 2070-2081, 2083-2087; B2064T/F B2064c2T/F B2065T/F (L2037 and B2007F
  B2034F B2064F are `--dump-wrapper-assets --no-dbug`; hoonc has no
  `--no-dbug`); `mint_with_subject_type` L2602-2605; `exact_mint_gun`
  L2777-2787; `slam_wrapper_gate` L2843-2849, 2851-2861, 2868, 2872-2874,
  2877-2882, B2868F; `dump_exact_wrapper_assets` (every P gap in
  L3122-3174); `dump_native_wrapper_assets` L3176-3199, 3202-3203;
  `jam_noun_in_fresh_slab` L3205-3209; `evaluate_honc_isolated` L3460-3466,
  3468-3473, 3475-3487, 3490-3493, 3495; `trap_battery` L3685-3691;
  `jam_slab_noun` L3983-3986; `slam_gate_formula` L4584-4590.
- Non-canonical preludes and `--sut-jam`. hoonc has only its own hoon.hoon,
  so a non-canonical prelude has no reference, and `--sut-jam` with the
  canonical subject type gives the default build
  (`c6_cli_subject_type_override_and_relative_output`): `run` L732 and, for
  `--dump-native-wrapper-assets`, L703 (a prelude that fails to build);
  `parse_prelude_hoon` L928 (a prelude that fails to parse);
  `build_context_with_shared_prelude` L1181, 1288; B1171F B1186T B1186c2F
  B1190F B1201F B1276T B1281F; `data_vase` L2664-2668, 2670-2671, B2655F;
  `local_octs_type` L3639-3645; `ty_atom_local` B3967T;
  `seed_honc_type_with_ut` L3236, B3232F; `peel_prelude_wrappers`
  L3257-3259, 3261, B3256F (a `Dbug`, `Note` or multi-element `=~` wrapper
  on the prelude's compose chain; native parity parses hoon-138 with dbug
  off); `chunk_prelude` L3285, B3284F (a prelude that is not `=<`);
  `restore_spot_hints` L3314 (spots peeled from a dbug-on prelude);
  `mint_honc_formula_with_ut` L3447-3457, B3444F B3445F (the whole-prelude
  route for a non-`=<` or noted prelude, or `NATIVE_HOON_NO_CHUNK`, which
  gives the same artifact as the chunked route);
  `extract_subject_type_root` L4194, 4200-4202; B4193T B4196F B4198F;
  `prelude_type_from_subject_type` L4207-4208; `type_cell_parts` L4219,
  B4218T; `looks_like_type_noun` L4230, 4233, 4242-4250; B4229T B4232F;
  `type_tag_atom_text` L4261, 4267; B4260T B4263T B4265F B4265c2T/F.
- Debug environment variables and tracing (diagnostics-only):
  `build_context_with_shared_prelude` L1218, 1228-1232, 1249-1253,
  1255-1256, 1260, 1262; B1217T B1227T B1230T/F B1248T B1250T/F B1252T
  B1255T (`NATIVE_HOON_SKIP_BURP`, `HONK_IR_ROUNDTRIP`, `NATIVE_HOON_TRACE`);
  `mint_with_sut` L2635-2636, B2634T; `mint_honc_prelude_chunked` L3411,
  B3410T; `prelude_variant_name` L3321-3336 (names printed in a log line);
  `eval_formula_noun_in_context` L4339-4342, 4347-4350, 4366-4369; B4338T
  B4346T B4365T; `trace_interpret_error` L4391-4393, 4395, 4398-4404,
  4407-4413; B4399T/F B4404T; spot-trace decoding, every gap in L4415-4489
  (`normalize_interpret_trace`, `decode_spot_trace`, `decode_spot_noun`,
  `decode_path_noun`, `decode_pint_noun`, `atom_text`); `trace_native`
  L4618, B4617T; `trace_timed` L4625, 4627-4634, 4636, B4623F;
  `hoon_log_path` L526, 529, 532; B524F B525T (log paths equal to or
  outside the working directory).
- Rejections: `check_dependency_tree` L826, B825T: an empty file in the
  tree, which hangs hoonc, so there is no hoonc verdict.
  `compile_batch_with_shared_prelude` L1452: a standard-mode entry that is
  not a gate, built in a batch (the single-entry form is the replayed
  `standard-not-gate` tree; batch builds are not replayed).
  `eval_formula_noun_in_context` L4376-4377, 4379-4380, B4377F: an
  evaluation that crashes (no parity probe has a crashing `/dat` node).
- Defensive checks: `catch_nock_panic` L4596-4611 (a panic inside Nock
  evaluation); `native_value_override` L3519-3524, B3517F (an unpinned
  softed-constraints module that reached native compilation; `run` delegates
  it to hoonc first); `softed_constraints_pins_match` L3544-3545, B3544T/F
  (a pinned file that is missing or unreadable).
- Path fallbacks: `build_entry_wer` B1062T (the binary passes
  `absolute_entry_wer=true` for every entry); `build_entry_wer` L1074, 1076,
  B1072F, `path_components_for_dbug` L1120 and `entry_path_for_hoon` L4980,
  4984, B4978F (an entry outside both the dependency root and the working
  directory); `build_import_wer` L1100, 1102-1109; B1098F B1106T/F and
  `entry_path_for_hoon` L4970, B4969T (a root reached through a symlink; the
  replay's inputs sit lexically under their roots); `hoon_path_from_relative`
  L4989, B4988T (an entry equal to the root, which is not buildable);
  `lexical_absolute_path` L5005-5008 (`.` and `..` components).
- Mug ties: `dor` (every gap in L4772-4819), `gor_mug` L4826 and `mor_mug`
  L4839 (distinct keys with equal mugs, or equal mugs of mugs), and
  `map_put_mug` L4868-4870, B4866F (one path with two contents). A real
  directory walk produces neither.

## pipeline.rs

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
- Import rejections, which hoonc also rejects, that the replayed `reject/`
  trees do not reach:
  - Missing imports: `resolve_import` L263, B262T (a `/*` path with fewer
    than two knots) and L295, 297-298 (the not-found message for a missing
    `/-`, `/#` or `/*` import); `hoonc_tree_file` L314, B310T B321c2T (`.`
    or `..` knots, or a suffix hoonc does not load); `path_knots` L422,
    B421T and `stap` B581c2F (the root path `/`).
  - `reject_leftover_import` L721-723, B720T: a `/%` clause, which honk
    reports as unsupported.

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

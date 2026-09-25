# Known hoonc/honk divergences

Each entry is a place where honk (or its parser, hatch) disagrees with hoonc
on some source program. Every one is pinned by at least one probe that
compiles with both compilers; those probes are tagged manual (lab/, and
`coverage/<pkg>/divergent/`) so the parity suites stay green until the
divergence is fixed. Fixing an entry means changing honk or hatch until its
probes pass, then moving them into a curated or coverage package.

Verdicts: MISMATCH (both build, bytes differ), HOONC-ONLY (honk rejects or
crashes), HONK-ONLY (honk accepts what hoonc rejects).

## Type checker (`crates/honk/src/native/ut`)

| # | Divergence | Verdict | Probes | Where |
|---|---|---|---|---|
| 1 | `?!` is minted natively instead of opened to `?:`, so dead branches are not eliminated and flow typing differs | MISMATCH, HONK-ONLY, HOONC-ONLY | `lab/wtzp_*` | `mint_wtzp`, play `WutZap` arm (`mod.rs` ~4175, ~4506) |
| 2 | Bare axis hoons are read with `Way::Free`, skipping `++peel` for iron/lead/zinc cores (reached through `^=` cell skins) | MISMATCH | `lab/axis_grip_*` | `Hoon::Axis` in mint and play (`mod.rs` ~4325, ~4719) |
| 3 | Wet gate whose forked sample misses every case: `redo_dear` errors on an empty `wec` and `redo_fork_fallback_candidate` keeps the face | MISMATCH, HOONC-ONLY, HONK-ONLY | `lab/wet_dear_*` | `wet.rs` ~37, ~142 |
| 4 | Wet-arm recursion guard keyed on the redone core instead of the call-site subject | HOONC-ONLY | `lab/wet_rib_subject_key_{trap,arm}` | `mull_check_wet` (`wet.rs` ~102) |
| 5 | mull builds cores with the same blocked seminoun as play, so nest takes the equal-coil shortcut | HONK-ONLY | `lab/mile_mull_ktdt_core_meet` | `mull_mile` (`mod.rs` ~11600) |
| 6 | `?#` resolves its wing without hoon-138's `[%& 1]` prefix, so arms fail with `fend-fragment` | HOONC-ONLY | `lab/skin_wthx_arm`, `coverage/c4/divergent/c4_wthx_arm_fend` | `mint_wthx` (`mod.rs` ~5048), mull `WutHax` (~11389) |
| 7 | `%over` skins in `?#` test formulas derive `ref` from the wrong subject | MISMATCH, HOONC-ONLY | `lab/skin_over_*` | `skin_test_formula` (`mod.rs` ~5289, ~5326) |
| 8 | Cell-skin test formulas use `cond`/`and` where hoon-138 uses `flan`, leaving redundant cell tests | MISMATCH | `lab/skin_cell_*`, `lab/skin_wash_in_cell`, `coverage/c3/divergent/c3_wthx_cell_static_tail` | `skin_test_formula` `Skin::Cell` (`mod.rs` ~5314) |
| 9 | `gain`/`lose` on a cell skin over a `%core` ref test for a `%noun` base skin where hoon-138 tests the term `%noun` | MISMATCH, HONK-ONLY | `coverage/c3/divergent/c3_{gain,lose}_cell_core_*` | `gain_cell_skin` (~9883), `lose_cell_skin` (~10176) |
| 10 | `lose` on a cell skin re-wraps a `%face` that hoon-138 strips | MISMATCH | `coverage/c3/divergent/c3_lose_cell_face` | `lose_cell_skin` (~10184) |
| 11 | A wash skin (`,`) as a `?:` condition: hoon-138 `gain` recurses forever; honk compiles it | hoonc hangs | none (a probe would hang the suite) | `gain_skin` `Skin::Wash` (~9759) |

## Parser and desugarer (`crates/hatch`)

| # | Divergence | Verdict | Probes | Where (`utils.rs` unless noted) |
|---|---|---|---|---|
| 12 | Positive float literals compile negative: `binaryfloat_mul` repeats the zero test where hoon-138 compares signs | MISMATCH | `coverage/p2/divergent/p2_float_positive_integer_sign`, `coverage/p4/divergent/p4_float_sign`, `p4_float_rs_overflow` | ~4152 |
| 13 | Float literals that overflow u128 intermediates panic honk | HOONC-ONLY | `p2_float_wide_product_panic`, `p2_float_deep_underflow_panic`, `p4_float_rd_overflow`, `p4_float_rh_overflow` | `lug` ~3806, ~3828 |
| 14 | Float path segments render differently (large and subnormal values) | MISMATCH | `p2_float_path_large_render`, `p2_float_path_subnormal_render` | `drg` ~3563, `drg_fl` ~3653 |
| 15 | `@uv`/`@uw` literals over 128 bits panic | HOONC-ONLY | `p1_big_base32`, `p1_big_base64`, `p2_wide_base32_base64_panic` | ~179-213 |
| 16 | IP literals: octets over 255 rejected, uppercase `@is` hex accepted | HOONC-ONLY, HONK-ONLY | `p1_ipv4_wide_octet`, `p3_if_octet_over_255`, `p3_is_uppercase_hex` | ~229, ~6151, ~6174 |
| 17 | `@da`/`@dr` literal field ranges, overflow, and fraction rules | all three | `coverage/p3/divergent/p3_da_*`, `p3_dr_*` | `absolute_date`, `relative_date`, `yule`, `year` (~9372-9660) |
| 18 | Path segment rendering for `@uw`, `@uc` zero, and BC dates | MISMATCH | `p3_path_uw`, `p3_path_uc_zero`, `p3_path_bc_date` | `w_ne` ~9085, ~8843, `yore` ~9579 |
| 19 | Path rendering of `/-0x10` and `/.100`: honk follows the hoon-138 source; hoonc's output likely comes from runtime jets | MISMATCH | `p3_path_signed_radix`, `p3_path_float_int` | ~8860, `r_co` ~8913 (needs a decision on which side is right) |
| 20 | A negative `@uc` keeps aura `%uc` instead of `%sc` | MISMATCH | `p2_signed_bitcoin_aura`, `p3_sc_signed_uc` | `number` ~9321 |
| 21 | Non-ASCII text: tapes split by code point instead of byte, UTF-32 knots substitute U+FFFD, sail attributes merge multi-byte characters | MISMATCH, HOONC-ONLY | `p2_tape_*`, `p2_knot_utf32_*`, `p4_sail_attr_utf8` | ~5071, ~5988-6038, `runes/sail.rs` ~42 |
| 22 | `'''` block cords and zero-headed digit groups accept or reject the wrong inputs | HONK-ONLY, HOONC-ONLY | `p2_cord_*`, `p2_zero_head_digit_group` | ~5335-5381, ~6084-6214 |
| 23 | `autoname`: a bare `@` sample crashes honk; `=a=@` names the sample `a` instead of `a-atom` | HOONC-ONLY, MISMATCH | `p1_autoname_bare_atom`, `p2_autoname_bare_atom`, `p1_bucts_autoname_prefix`, `p4_buctis_prefixed_autoname` | ~2999, `runes/buc.rs` ~619 |
| 24 | `!?`: version stored as `[%atom 138]` instead of `138`; the range pair is read as `[min max]`; the tall pair needs a gap | MISMATCH, HOONC-ONLY, HONK-ONLY | `c1_zpwt_*`, `p1_zpwt_*`, `p4_zpwt_*` | `zpwt_arg_to_noun` ~13716, `open` ~2695, `runes/zap.rs` ~189 |
| 25 | Missing syntax: `~$`, sail `;%`, namespaced sail attributes, wide `$&`, wide `~!`, `;=` | HOONC-ONLY | `*_sigbuc_parse`, `p4_sgbc_rune`, `*_sail_call`, `p1_sail_attr_namespace`, `*_bucpam_wide`, `c3_sgzp_wide`, `p4_mcts_rune` | `runes/sig.rs`, `runes/sail.rs`, `runes/buc.rs` ~354 |
| 26 | Duplicate arm or chapter names are merged instead of becoming `%eror` arms | MISMATCH, HONK-ONLY | `coverage/c2/divergent/c2_dup_*`, `coverage/p4/divergent/p4_duplicate_*` | `chapters` ~4961-4977 |
| 27 | Under `!:`, a `::  +name:` doc that does not name the following `=/` anchors the spot differently | MISMATCH | `p3_spot_doc_link_before_tisfas` | ~11861-11872 |

## Import pipeline (`crates/honk/src/pipeline.rs`, CLI)

| # | Divergence | Verdict | Probes (`coverage/c6/divergent/`) | Where |
|---|---|---|---|---|
| 28 | A file whose imports start with `/-`, `/+`, or `/?` gets its body `%spot` starting at the import line | MISMATCH | `c6_sur_lib_spot`, `c6_ford_pin_spot` | hatch `should_skip_outer` (`utils.rs` ~11545) |
| 29 | `/#` imports are not evaluated eagerly | MISMATCH | `c6_dat_eager` | `bin/honk.rs` ~2357-2380 |
| 30 | Import-header edge cases: comment after a comma, blank or tab continuation lines, double hyphens, trailing commas, `/*` face and mark forms | HONK-ONLY, HOONC-ONLY | the matching `divergent/*` trees | `pipeline.rs` ~239-645 |
| 31 | hoonc parses every file in the dependency tree; honk only what the entry imports | HONK-ONLY | `c6_unreachable_broken_file` | by design? |
| 32 | Entries under an `open/hoon` root are keyed by the root marker instead of the absolute path | MISMATCH | `c6_hoon_root_marker` | `bin/honk.rs` ~4720 (looks deliberate) |

## Other honk bugs (no hoonc verdict)

- A damaged cache pack is never rewritten, so every later build misses it
  (`build_cache.rs` ~193). Pinned by the ignored test
  `cov_t1::cache_damaged_pack_is_repaired_across_sessions`.
- A batch manifest listing one entry twice fails with `--cache-dir`
  ("duplicate root name", `bin/honk.rs` ~2157, ~2212). Pinned by an ignored
  test in `tests/cov_t1_cache_parity.rs`.
- An empty `/*` data file panics the honk worker (hoonc also fails).
- A truncated `--sut-jam` panics in `cue` instead of returning an error.
- `NATIVE_HOON_NO_CHUNK=1` produces a different prelude artifact than the
  default chunked mint (debug path only).

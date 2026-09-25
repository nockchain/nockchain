# p3 coverage ledger: `crates/hatch/src/utils.rs` 8280-12029

What is still uncovered after the p3 unit tests
(`crates/hatch/src/cov/p3_atoms_specs.rs`) and the parity probes in this
directory. Tags: `U` = missed by unit tests, `P` = missed by the parity
corpus plus these probes. Line and branch labels follow the gap report
(`B<line>T/F`, `c2`/`c3` for later operands of `&&`/`||`).

Totals for the range: unit 180 lines / 39 branch outcomes left (from 1507 / 392);
parity 525 lines / 220 branch outcomes left (from 1662 / 461).

## Blocked by a divergence (P; unit-tested)

The honk side of these is exercised by the probes in `divergent/`, which CI
keeps manual until the divergence is fixed.

- `rend_with_rep` 'd' 'a' BC arm L8601-8603, B8600T; `yore` L9579, B9576F:
  `divergent/p3_path_bc_date` (hatch `yore` drops the `+1` of hoon `++yore`).
- `rend_with_rep` 'u' 'c' zero arm L8844, B8843T:
  `divergent/p3_path_uc_zero` (hatch prints `0` where hoonc prints the checksum).
- `w_ne` L9085-9100, B9087-B9095: `divergent/p3_path_uw` (digit order
  A-Z a-z 0-9 in hatch, 0-9 a-z A-Z in hoon `++ne:w`). `/0w0` is in
  `p3_path_unsigned`, so only the digit map is blocked.
- `number` negative bitcoin arm L9322-9327: `divergent/p3_sc_signed_uc` (aura
  `%uc` instead of `%sc`).
- `absolute_date` range errors L9373 (B9372T), L9391 (B9388F), L9404
  (B9401F), L9415 (B9412F), L9426 (B9423F): hoonc accepts year 0, day > 31,
  hour > 23, minute > 59, second > 59 (`divergent/p3_da_year_zero`,
  `p3_da_day_over_31`, `p3_da_hour_over_23`, `p3_da_minute_over_59`,
  `p3_da_second_over_59`).
- `lsh_big` zero shift B9920T (P): only reached rendering an integral float
  with binary exponent 0 (`/.8388608`), which hits the same MISMATCH as
  `divergent/p3_path_float_int`.
- `skip_plain_doc_before_equals_slash_start` L11871-11874, L11877-11878,
  B11864F, B11869F, B11877T, and the scan-continuation outcomes after such a
  line (B11843F, B11846T, L11847-11848, B11856F, B11860F) (P): reached
  only when a smol `+link` doc that does not name the binder sits above a
  `=/`. hoonc anchors the spot at the doc and hatch at the `=/`
  (`divergent/p3_spot_doc_link_before_tisfas`).

## Rejected by both compilers (P; unit-tested)

Checked with `probe-diag.sh`: hoonc and honk both reject these inputs, so they
cannot be parity probes.

- `zust` invalid ipv6 L8310 (`.1.2.3.4.5.6.7.fffff`).
- `path` posh failures L8451 (`/=/=/...` past the file path), L8461
  (`%/=/=/...`).
- `number` bad base58check L9303 (`0c...DivfNb`).
- `absolute_date` month out of range L9383, B9380F (`~2020.13.1`).
- `tiq` unknown suffix L10096, `ind` miss L10070 (`~zzz`); `phonemic_name`
  `doz` star prefix L10116, B10115T (`~dozzod`); zero leading word L10129,
  B10128T (`~dozzod-dozzod`); `.~doz`.
- `cue_inner`/`rub_*`/`get_size` malformed-jam errors L10777, L10780, L10783,
  L10804, L10838, L10849, L10897, L10909 and B10776T, B10779T, B10782T,
  B10803T, B10833F, B10837T, B10848T, B10896T, B10908T (`~00`, `~07`, `~0g0`).
- `unanchor_gap_glued_children` B11421F, B11427F (UP): `?-`/`?+` with no
  clauses is a syntax error in both.

## Unreachable (UP unless noted)

- Dead code, no callers: `atom_to_char` L9051-9063, `dvr_u64` L9160-9162,
  `w_co` L9203-9214, `is_leap` L9646-9648, `lsh_u128` L9900-9907, `rsh_big`
  L9927-9934, `mix`/`mix_big`/`mix_atoms` L10256-10273, `rol32` L10277-10279,
  generic `fe` L10398-10470, `noun_hash` L10620-10624. `is_leap_year`
  L9650-9653 (P; unit-tested).
- Unused builders, unit-tested only (P): `two_specs_closed_tall` L11134-11140,
  `name_spec_closed_tall` L11202-11210, `one_hoon_closed_tall` L11227-11234,
  `hoon_with_span`/`apply_hoon_trace` L11251-11261, L11388-11396, B11253T/F.
- `ParsedAtom::Big` arms where the value is always small: `trip` L8338,
  `rend_with_rep` prefix bytes L8540, L8545, L8881, `yell` digit/seconds
  L9542, L9551, `fein` after `feis` L10538, `offset_to_atom` L10674
  (B10671F).
- `zust` invalid ipv4 L8317: `ipv4_address` already filters octets to
  0-255 without leading zeros, so `Ipv4Addr` parsing cannot fail.
- `path` `%%...` posh failure L8470: `poon` with an empty tyke never fails for
  a non-empty file path (unit-tested with an empty path).
- `rend_with_rep` fallback arms L8670, L8676, L8691, L8829-8831, L8907, and
  `z_co` L9237-9241 (P; unit-tested): `nuck` never produces `%d?`, `%f` > 1,
  `%i?`, `%r?` or unknown auras, so only direct `rend_co` calls reach them.
- `w_ne` `unreachable!()` L9098, B9095F: digits are < 64.
- `relative_date` `_` unit L9506: `one_of("dhms")`.
- Bit helpers with fixed arguments: `bloq_bits` panic L9670 (B9669T, P);
  `met` on `Big(0)` L9689 (B9688T, P); `rep` zero step and >=128-bit chunks
  L9727-9731, B9713c2T, B9722F, B9727T/F (P); `rap` full-width and
  oversized-chunk panics L9761, L9766 (P); `cut` guards L9795, L9800, L9805,
  L9810 (P), L9825, L9831, B9824T, B9830T (bit_len and bit_start are bounded
  by the source width), Small >=128-bit mask L9848, B9847T (P), and Big
  wide masks L9856-9878, B9856F, B9867T (P); `lsh`/`rsh`/`end` overflow
  L9887, L9895, L9939 (P for L9887, L9895); `rsh_u128`/`end_u128` >=128-bit
  L9912, L9959, L9962, B9911T, B9961T (only called with `(0, 1)`);
  `end_big` L9947.
- `ins`/`ind` length checks L10042, L10059, B10041T, B10058T (P): `tip`/`tiq`
  always pass three bytes.
- `muk` blocks and 1/3-byte tails L10307-10358, B10310T/F, B10318T: `eff`
  always hashes two bytes. `fen` odd round count L10376, B10373F and `fe_u64`
  odd arm L10494, B10493T: `r` is always 4.
- `feis` re-encrypt L10481, B10480T and `feen` second pass L10520, B10517F:
  with r = 4, `fe`/`fen` outputs are at most 0xfffe.ffff, below k = 0xffff.0000.
- `fynd_big` below 0x1.0000 B10570F, B10575F: parsed planets and larger
  names are always >= 0x1.0000. `fynd_u64` 64-bit arm L10593-10595, B10588c2F,
  B10592T (P; unit-tested): `fynd_big` only passes 32-bit words.
- `fein` Small-planet and Small-moon arms L10532, L10543, L10549 (P;
  unit-tested): parsed planets and moons are `ParsedAtom::Big`.
- jam/cue helpers on wide atoms: `usize_bit_len(0)` L10710 (B10709T),
  `atom_bit_len`/`atom_get_bit` Big arms L10720, L10727-10735 (B10726F,
  B10731T/F), `bits_to_atom` empty L10743 (B10742T), `rub_atom` >128-bit
  payload L10819-10826 (B10808F, B10821T/F), `atom_to_bits` Big
  L10875-10885 (P): `~0` blobs are parsed into a u128, so cue never sees a
  wide atom. The `bits_to_atom` wide arm is covered by `p3_path_misc`.
- `stack_block_docs_clad::walk` defensive returns L11347-11360, B11346F,
  B11349F, B11352F, B11356F, B11357F: `map_to_noun` always builds a
  well-formed treap.
- `attach_help_to_hoon` same-help short cut L11376, B11375T (P;
  unit-tested): every parser call site checks `hoon_tail_has_help` first.
- `apply_hoon_docs` B11278F (P; unit-tested by
  `postfix_docs_attach_to_tall_micsig_args`): the same shape in
  `p3_docs_micsig` does not reach it through honk's pipeline. Not investigated.
- `unanchor_gap_glued_children` `.^` with non-`:*` or empty arguments L11447,
  B11443F, B11444F: the `.^` parser always builds a non-empty `%cltr`.
  `unanchor_hoon_spot`/`unanchor_spec_spot` fallthrough L11473, L11481: in
  traced mode every gap-glued child is `%dbug`- or `%note`-wrapped.
- `unanchor_spot_start` L11491, B11490F (spot line outside the source),
  B11497F, B11501F (a doc block running to EOF with no code after it: the
  child always has code).
- `wrap_hoon_with_trace` nested spots L11552, L11555, B11528F, B11530F: all
  spots in one parse share `wer`. B11538F, B11543F, B11543c3T: the nested
  `%dbug` case on a last line with no newline, a blank line or tab indentation.
  Only fas-rune import lines produce it, and those need deps, which the
  `--arbitrary` probes don't have.
- `wrap_spec_with_trace` nested spot L11595, B11592F: no spec parser re-wraps
  a traced spec with a different span.
- `arm_body_start_from_header` L11606-11678 and all its branches (P;
  unit-tested; L11675 is also U), plus `chumsky_spot_to_hoon_spot` L11888:
  no parser span starts on a `++`/`+$` header (arm bodies start after the
  gap), so only direct span calls reach it. L11675 needs a span past the end
  of the source.
- `non_doc_start_after_leading_doc_span` L11689-11774 and its branches (P;
  unit-tested): parser spans always start at a token, never at a `::` doc
  line or leading whitespace (B11703c2F is never false in any corpus).
  B11720c2F (U too): a blank line cannot start with `:`.
- `skip_plain_doc_before_equals_slash_start` EOF and first-line edges
  B11789F, B11793F, B11800F, B11804F, B11812F, B11816T, L11817, B11821F,
  B11837F (P; unit-tested): these need the `=/` at EOF, or the anchoring doc on
  the first two lines of the file, where the probe header comment sits.
  B11861F (P; unit-tested): a non-name `=/` skin is a syntax error in both
  compilers. L11880, B11877F (UP): the anchoring doc line always has
  content, so the scan either returns or sets `saw_plain_doc`.
- `print_noun`, `skip_dbug`, `diff_noun`, `print_context` L11905-12028 (P;
  unit-tested): diagnostics-only test helpers, never called by the compiler.

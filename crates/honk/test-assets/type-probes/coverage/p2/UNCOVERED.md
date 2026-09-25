# p2 (hatch `utils.rs` 3230-8279): branches left uncovered

Line numbers match `crates/hatch/src/utils.rs` at the scaffold commit.
`U` = not covered by unit tests (`crates/hatch/src/cov/p2_lexing.rs`),
`P` = not covered by parity probes (this directory). Branch labels follow
`covdiff.py`: `<line>T/F`, `c2` = second operand of `&&`/`||`.

## Two structural limits on parity coverage

- **Doc anchoring is parity-unreachable.** honk parses build-leaf (entry) files
  with docs off (`crates/honk/src/bin/honk.rs` `parse_build_leaf` calls
  `parse_native_hoon_source_without_docs`), so no probe can reach the `LineMap`
  help machinery. Only the fixed prelude drives it. That covers every `P` gap in
  `chapters` doc handling (4738, 4801, 4826, 4847, 4861, 4866; 4722T 4725T
  4726T 4758c2F 4800T 4813 4825T 4846T 4859F 4860T) and in the `LineMap` doc
  helpers 6464-7930 (`help_before_*`, `help_after_*`, `build_*doc_help*`,
  `parse_doc_link`, the named-arm, frag and coltar-opener helpers,
  `postfix_doc_summary_parts`, `arm_scye_help_after_name`, `doc_comment`,
  `line_starts_like_*`, `doc_summary_before_line`, `strip_doc_spaces` 6367c2F).
  These are unit-tested instead (the `helps(...)` tests), apart from the `U`
  items listed below.
- **Instrumentation artifact.** `LineMap::new`, `new_with_docs` and `pint`
  (6376-6436, 6457-6462) are `#[inline]` and report zero counts in the release
  parity profile, even though every compile runs them. They are unit-covered.

## Blocked by a divergence (see `divergent/`)

- 5073c6T, 5115c5T: tape chars 0x80-0xff (`p2_tape_latin1_char`,
  `p2_tape_wide_char_rejected`).
- 5988F/5991, 6020c2T, 6038T/6039: `@c` lead bytes f5+, surrogates, and code
  points above U+10FFFF (`p2_knot_utf32_above_max`, `p2_knot_utf32_surrogate`).
  Unit tests do not pin the U+FFFD behavior for these.
- 3508F, 3515-3520 (P): `sea` subnormal, and 3576F (P): the drg halfway
  tightening at `e == v` (`p2_float_path_subnormal_render`). drg_fl's wrong `v`
  makes `e == v` unreachable.
- 5575T/5576 (P): a `Big` zero magnitude comes only from `-0x0.0000...`
  (`p2_zero_head_digit_group`).

## Dead or unreachable code

- 3361: winglist lark `_` arm. The lark parser only yields `+ - < >`.
- 3487: `sig` `unreachable!()`. A 1-bit cut is always 0 or 1.
- 3498, 3502: `sea` >128-bit field panics. Width is at most 15 bits and precision at most 112.
- 3857-3869, 3878, 3880-3893, 3914-3932 (3858 3880 3881 3886 3914 3916 3922
  3924 3925 3929): `LugMode::{Smaller, Larger, NearestAway, NearestTowards}`.
  `rau` never constructs them.
- 3947T/3948-3952: `lug` mantissa zero after rounding. Reachable modes never
  round a nonzero mantissa down to 0.
- 3977-3988, 3981T/F: the finite arm of the `d == 'f'` flush. No caller passes
  `%f`. The condition is also inverted relative to hoon-138 `++lug`.
- 4107F 4110 4121F 4124 4134F 4137 4139F 4142 4192F 4195 4197F 4200: `if let
  Finite` else arms that run after NaN and infinity have already returned.
- 4153T 4153c2T 4154: a duplicate zero test after the first one returns. This
  is the `p2_float_positive_integer_sign` bug site.
- 4262T/4263: `fil` shift >= 128. The guard bounds `bloq*b <= 128`.
- 4295T/4296: `bif` NaN shift >= 128. Precision is at most 112.
- 4813F, 4813c2T, 4813c2F: every `chapters` caller passes
  `attach_single_named_prefix_docs = true`.
- 4859F, 4866: `prefix_help_has_link` implies `Some`.
- 5158F/5160-5161, 5393F/5395-5396: `line_col` is 1-based, so a column is never 0.
- 5530T/5531, 5538T/5539, 5546T/5547: `yawn` breaks for years below 100. The
  year is offset by 292,277,024,400.
- 5613F: `build_yek` index >= 256. The alphabet is ASCII.
- 5665T (P): `shay` with length 0. `tok` always hashes at least 21 bytes.
- 5737T/5738: `met_big(0)`. Every caller passes a nonzero mantissa.
- 5745-5752: `pad_fa_big`, which has no callers.
- 5802c4T, 5802c5F, 5805: `wick` on `_` or an invalid char. `urt` only admits `[0-9a-z.~-]`.
- 5828: `tuft` never returns `Big`.
- 5889T/5890, 5897F/5905-5906: `atom_mask_low_bits` with masks of 128 bits or
  more. Only reachable through the private `end`, whose callers in this range
  use widths of 64 bits or less.
- 5967: `atom_to_u8` `Big` arm. `end(3,1)` is always small.
- 6044: `decode_one_utf8` `_` arm. `teff` returns 1-4.
- 6159T: empty ipv4 octet (`at_least(1)`). 6185T/6186: empty ipv6 tail (`exactly(7)`).
- 6313-6315: `snag`, which has no callers.
- 6518T/6519-6520, 6521T/6522-6523: section-marker drain. After draining
  through the last blank line, no blank line is left at either end.
- 6559T/6560, 6598T/6599, 6622T/6623, 6656T, 6683c2F, 6726T/6727,
  6832T/6833, 6843T/6844-6845, 7052T/7053, 7153T/7154, 7239T/7240, 7373c2T,
  7563c2T, 7738c2T: empty doc text after stripping. `doc_comment` trims
  trailing whitespace and blank lines are handled first.
- 6639F: cuff 0 with an empty summary. `parse_doc_link` returns a nonempty
  summary unchanged whenever the cuff is 0.
- 6855T/6856, 6862F/6863, 6989F/6990, 6995F/6996, 7004F/7005, 7016T/7017,
  7831F/7832, 7860F/7861, 7908F/7909, 7783F: a first-line
  target that is never looked up, `line_bounds`/`line_indent` on valid
  indices, or a frag walk reaching line 0 (the `:*` opener line is code).
- 6961F, 7355F, 7720F, 7800F: `trimmed_end > cursor` is false only if the line
  lacks the `::` already found at `cursor`.
- 7435F, 7554F, 7585F/7588T/7589, 7710c2T: postfix scans whose preconditions
  rule these arms out: `::` at the expression start, a comment line (already
  returned), or `:x` after an arm name, which is not valid Hoon.
- 7913F/7927: hoon-target test on a blank line. Callers pass token starts.
- 7969F, 7991F, 7994c2T/7995, 8048F: `expand_gap_start` at EOF or starting
  inside a comment lead. Spans never start there.
- 8023F, 8031, 8035F, 8042: `if let Some(boundary)` fall-through regions after
  the `None` early return.
- 8216-8217, 8205F (P): `posh` with no pre tyke. Every caller passes `Some`.

## Tabs (not legal Hoon whitespace; hoonc and honk reject any tab)

6962c2T, 7303c3T, 7307c3F, 7313c3T, 7317c3T, 7347c3T, 7356c2T, 7707c3T,
7721c2T, 7783c3T, 7793c3T, 7801c2T, 7836c3T, 7865c3T, 7913c3T.

## Defensive or malformed input only (unit-tested where reachable)

- 3751T/3752 `sub_or_panic_big`, 3776T/3777 `lug` zero mantissa, 4323T/4324 and
  4341T/4342 `bif` mantissa above 128 bits: panics that callers never trigger.
- P-only, both compilers reject (unit tests assert honk's rejection): 3384
  (`=/  =*` cannot autoname); 5185T/5186-5187, 5201c2F/c3F/5202-5203,
  5435T/5436-5437, 5454c2F/c3F/5455-5458 (tall tape and `'''` indentation
  errors); 5073F/c2F/c3F/c5F, 5115c2F/c4F, 5361F, 5362F (control chars, DEL,
  bad escapes in tapes and cords); 5288T (`+0` wing: honk "peg: a and b must be
  non-zero", hoonc fails too); 5622T/5623, 5626T/5627, 5704F/5707 (bad `0c`
  base58 or checksum); 5798, 5802c3F (bad knot escape); 6116c2T/6117 (`0x0abc`);
  8231 (`posh` fail); 8245 (nusk re-parse failure); 4696-4702 (`?^  ^=  b=a`
  and `?^  ^=  b  a`).
- 6002T..6032 (P): malformed UTF-8 in `taft`. `urx` builds valid UTF-8
  via `tuft`, so source cannot produce it. Unit tests assert the U+FFFD
  fallback.

## Float paths that literal parsing never takes (P only; unit-covered)

Float literals always round to nearest with `d = %d`, and `grd` only
multiplies or divides finite operands by a nonzero power of 5, so these are
unreachable from source: 3671-3672 (`swr` d/u), 3686 (`fli` infinity), 3691-3697 (`zer`,
no callers), 3701-3704 (`rau` non-nearest), 3723T/3724 (`cmp_si` negative vs
positive), 3794T/3795 and 3955T/3956-3960, 4072T/4073 (`d == %i`),
3817-3826 (zero branch floor/ceiling), 3877-3878, 3896-3898, 3971F/3973-3990,
4099-4131 and 4145c2T (`mul` NaN/infinity/zero operands), 4172-4189 and
4204T/4205, 4214T/4215 (`div` special operands), 4246T/4247, 4251T/4252,
4258F/c2F, 4269-4275 (`fil` beyond bif's `fil 0 (w+1) 1`). Also 4099
(`mul` NaN) and 5275-5283 (`concatanate`, no caller in the parser).
Unit-only: 3896c2T/c2F (ceiling on an exact quotient with no dropped bits).

## Other unit gaps

- 4722T, 4725T, 4726T, 4758c2F, 4825T/4826, 4860T/4861: `chapters` doc
  combinations where the arm body already carries the identical help on the
  first child of `%-`/`=+`/`?:`, or a linked prefix help. Doc-only, and I found
  no source form that produces them.
- 6448T/6449 (P): `line_col` clamping a column inside a tall tape's indent.
  Spots never start or end there in real source. Unit-covered.

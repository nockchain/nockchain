# p2 uncovered ledger: `crates/hatch/src/utils.rs` lines 3247-8381

This package covers the wing, tiki, float, tape, cord, atom-literal and path
lexers, and the `LineMap` span and doc-anchoring code.

Uncovered on the final code: 1 line and 1 branch outcome missed only by the
unit tests (`U`), 251 lines and 158 branch outcomes missed only by the parity
corpus (`P`), and 131 lines and 123 branch outcomes missed by both (`UP`), for
383 lines and 282 branch outcomes in all. Numbers are current line numbers.
`B<n>T` and `B<n>F` are branch outcomes at line n, and `c2` to `c5` name the
later conditions on that line. "Unit-tested" means
`crates/hatch/src/cov/p2_lexing.rs` covers the entry.

## Structural limits on parity coverage (P; unit-tested)

- Doc anchoring. honk parses build-leaf (entry) files with docs off
  (`crates/honk/src/bin/honk.rs` `parse_build_leaf` calls
  `parse_native_hoon_source_without_docs`), so in parity only the fixed
  prelude and build wrappers drive the doc-anchoring code, and no probe can
  reach a doc shape they lack. The `helps(...)` unit tests cover these
  instead:
  - `hoon_tail_has_help`: B4731T, B4734T, B4735T (an arm doc that a
    child of `%-`, `%+` or `?:` already carries as a postfix doc).
  - `chapters`: B4809T, 4810, B4834T, 4835, B4855T, 4856, B4869T, 4870,
    and the `attach_help_to_bartis_tail` fallback at 4747.
  - `strip_doc_spaces`: B6375c2F.
  - `help_before_with_target_options`: B6585T, 6586, B6600T, 6601, B6615F,
    B6618F, 6626.
  - `build_doc_help_from_lines`: B6713T, B6713c2T, B6713c2F, 6714, B6718T,
    6719.
  - `parse_doc_link`: B6774F, B6777c2T, B6785F, 6788, B6790F, 6793,
    B6794c2F.
  - `help_before_named_arm_summary`: B6851T, 6852, B6862T, 6863, B6875T,
    6876, B6889F.
  - `doc_block_is_single_named_summary_for`: B6899T, 6900, B6909T, 6910.
  - `doc_block_follows_barcab_opener`: B6974T, 6975.
  - `help_before_named_arm_list_entry`: B6990T, 6991, B7003T, 7004, B7024F,
    B7031T, 7032.
  - `coltar_opener_inline_doc_comment`: B7056T, 7057, B7059T, 7060, B7064T,
    7065-7067, B7068F, B7069T, B7070T, B7070F, 7070, 7072.
  - `frag_block_doc_entries`: B7094T, 7095, B7111F, 7116.
  - `frag_doc_entries_from_docs`: B7136T, 7137, B7140T, 7141, B7148F, 7149,
    B7151F, 7152.
  - `help_before_plan_tail`: B7187T, 7188, B7199T, 7200, B7216T, 7217,
    B7222T, 7223, B7230c2F, B7241T, 7242, 7262.
  - `help_before_arm_tail`: B7271T, 7272, B7283T, 7284, B7344c2F, B7348c2T.
  - `postfix_doc_summary_parts`: B7458T, 7459-7461, B7463T, B7464T, B7464F,
    7464, 7466, B7480T, 7482.
  - `help_after_current_line_expr`: B7542T, 7543.
  - `help_after_choice_spec_item`: 7657-7658.
  - `help_before_choice_spec_item`: B7682T, 7683.
  - `help_after_line_expr_ending_at`: B7742T, 7743, B7749F.
  - `arm_scye_help_after_name`: B7817T, 7818, B7823T, 7824-7826, B7828T,
    B7829T, B7829F, 7829, 7831, 7844.
  - `line_bounds`: B7875F, B7875c2F.
  - `doc_comment`: B7903T, 7904-7906.
  - `line_starts_like_spec_doc_target`: B7938F, 7946, 7957.
  - `line_starts_like_body_spec_doc_target`: B7967F.
  - `doc_summary_before_line`: B7991F, B7999T, 8000-8001, B8004F.
- Instrumentation artifact. `LineMap::new` (6384-6386), `new_with_docs`
  (6389-6454, with all 30 branch outcomes at 6394, 6407, 6413, 6418, 6419,
  6425, 6426, 6429, 6430 and 6438) and `pint` (6559-6564) are `#[inline]` or
  `#[inline(always)]` and report zero counts in the release parity profile,
  even though every compile runs them.

## Dead or unreachable code (UP)

- 3378 `winglist` lark `_` arm: the lark parser only yields `+ - < >`.
- 3504 `sig` `unreachable!()`: a 1-bit cut is always 0 or 1.
- 3515, 3519 `sea` >128-bit field panics: the exponent width is at most 15
  bits and the precision at most 112.
- `lug` arms for `LugMode::{Smaller, Larger, NearestAway, NearestTowards}`,
  which `rau` never constructs: 3866-3878, B3867T, B3867F (`NearestAway` on a
  mantissa shifted out entirely); 3887 (`Larger`); 3889-3900, 3902, B3889T,
  B3889F, B3889c2T, B3889c2F, B3890T, B3890F, B3890c2T, B3890c2F, B3895T,
  B3895F (`Smaller`); 3923-3928, B3923T, B3923F, B3925T, B3925F
  (`NearestAway`); 3931-3940, B3931T, B3931F, B3933T, B3933F, B3934T, B3934F,
  B3937T, B3937F (`NearestTowards`).
- B3952T, 3953-3957 `lug` mantissa zero after rounding: the reachable modes
  never round a nonzero mantissa down to 0.
- B4116F, 4119, B4130F, 4133, B4143F, 4146, B4148F, 4151 (`binaryfloat_mul`)
  and B4201F, 4204, B4206F, 4209 (`binaryfloat_div`): `if let Finite` else
  arms that run only after NaN and infinity have already returned.
- B4271T, 4272 `fil` shift >= 128: the guard above bounds `bloq * b <= 128`.
- B4304T, 4305 `bif` NaN shift >= 128: the precision is at most 112.
- B4767c2F `doc_help_has_nonzero_cuff` on a nonzero atom cuff: a cuff is `~`
  or a list cell, never another atom.
- B4822F, B4822c2T, B4822c2F `chapters`: every caller passes
  `attach_single_named_prefix_docs = true`.
- B4868F, 4875 `chapters`: `prefix_help_has_link` implies `Some`.
- B5185F, 5187-5188 (tall tape) and B5419F, 5421-5422 (`'''` cord):
  `line_col` is 1-based, so a column is never 0.
- B5563T, 5564, B5571T, 5572, B5579T, 5580 `yawn` breaks: each guard
  contradicts the divisibility test just before it (a year not divisible by
  4 is nonzero, a nonzero multiple of 4 is at least 4, and a multiple of 100
  that is not a multiple of 400 is at least 100).
- B5646F `build_yek` index >= 256: the alphabet is ASCII.
- B5770T, 5771 `met_big(0)`: every caller passes a nonzero mantissa.
- 5778-5785 `pad_fa_big`: no callers.
- B5840c4T, B5840c5F, 5843 `wick` on `_` or an invalid char: `urt` only
  admits `[0-9a-z.~-]`.
- B5927T, 5928, B5935F, 5943-5944 `atom_mask_low_bits` with masks of 128 bits
  or more: only reachable through the private `end`, whose callers use widths
  of 64 bits or less.
- 6002 `atom_to_u8` `Big` arm: `end(3, 1)` is always small.
- B6147T `ipv4_address` empty octet (`at_least(1)`); B6181T, 6182
  `ipv6_address` empty tail (`exactly(7)`).
- 6313-6315 `snag`: no callers.
- B6476F, 6482 `set_column` and B6488F, 6489 `drift`: the `drifts` lock is
  poisoned only by a panic while it is held, and nothing that holds it can
  panic.
- B6620T, 6621-6622, B6623T, 6624-6625 section-marker drain: after draining
  through the last blank line, no blank line is left at either end.
- Empty doc text after stripping, which cannot happen because `doc_comment`
  trims trailing whitespace and blank lines are handled first: B6661T, 6662,
  B6700T, 6701, B6724T, 6725, B6758T, B6785c2F, B6828T, 6829, B6934T, 6935,
  B6945T, 6946-6947, B7154T, 7155, B7255T, 7256, B7341T, 7342, B7475c2T,
  B7665c2T, B7840c2T.
- B6741F cuff 0 with an empty summary: `parse_doc_link` returns a nonempty
  summary unchanged whenever the cuff is 0.
- Lookups that cannot fail: B6957T, 6958 (a first-line target that is never
  looked up); B6964F, 6965, B7097F, 7098, B7106F, 7107, B7933F, 7934, B7962F,
  7963, B8010F, 8011 (`line_bounds` and `line_indent` on valid indices);
  B7091F, 7092 (a line index already wrapped in `Some`); B7118T, 7119 (a frag
  walk reaching line 0, but the `:*` opener line is code); B7885F
  (`line_indent` reaching the end of its line, but it is only asked about
  non-blank lines).
- B7063F, B7457F, B7822F, B7902F: `trimmed_end > cursor` is false only if the
  line lacks the `::` already found at `cursor`.
- B7537F, B7656F, B7687F, B7690T, 7691, B7812c2T: postfix scans whose
  preconditions rule these arms out: `::` at the expression start, a comment
  line (already returned), or `:x` after an arm name, which is not valid Hoon.
- B8015F, 8029 `line_starts_like_hoon_target` on a blank line: callers pass
  token starts.
- B8093F, B8150F `expand_gap_start` scans that cannot run off the end of the
  source: a non-space byte precedes `start` in the first, and every gap line
  before `line_start` ends in a newline in the second.
- B8125F, 8133, B8137F, 8144 `expand_gap_start`: `if let Some(boundary)`
  fall-through regions after the `None` early return.
- 8319 `posh`: both arms of the `if let` produce `Some`, so the trailing `?`
  never returns early.

## Tabs (UP; not legal Hoon whitespace, and hoonc and honk reject any tab)

B7064c2T, B7405c3T, B7409c3F, B7415c3T, B7419c3T, B7449c3T, B7458c2T,
B7809c3T, B7823c2T, B7885c3T, B7895c3T, B7903c2T, B7938c3T, B7967c3T,
B8015c3T.

## Defensive only (UP)

- B3762T, 3763 `sub_or_panic_big`: no caller subtracts a larger value
  (`drg` guards with `two_s < mp ||`, and the `Smaller` arm is dead).
- B3787T, 3788 `lug` zero mantissa: `binaryfloat_mul` and `binaryfloat_div`
  return zero operands before rounding, so the mantissa is never 0.
- B4332T, 4333 and B4350T, 4351 `bif` mantissa above 128 bits: rounding
  keeps the mantissa within the precision, at most 113 bits.

## Rejected by both compilers (P; unit-tested)

- 3401 `variable_name_and_type` "cannot autoname" (`=/  =*`).
- 4705-4707, 4709-4711 `tiki_tall` named forms (`?^  ^=  b=a`).
- Wide tapes: B5116F (a control character), B5116c3F (a `{` that opens no
  interpolation). Tall tapes: B5150c4F (a bad escape), B5212T, 5213-5214
  (closing delimiter indentation), B5228c2F, B5228c3F, 5229-5230
  (inconsistent indentation).
- Cords: B5393F (a control character), B5394F (DEL), B5468T, 5469-5470,
  B5487c2F, B5487c3F, 5488-5489, 5491 (`'''` indentation errors).
- B5315T `wing` on `+0`: honk fails with "peg axis: peg: a and b must be
  non-zero", and hoonc fails too.
- B5659T, 5660 `cha_fa` (a character outside the base58 alphabet) and
  B5737F, 5740 `den_fa` (a bad checksum): malformed `0c` literals.
- 8333 `posh` failure; 8347 `nusk` re-parse failure.

## Unreachable from source (P; unit-tested)

Float literals all go through `grd_fl`, which rounds to nearest with
`d = %d` and multiplies or divides a finite mantissa by a nonzero power of 5,
so these float paths never run from source:

- 3682-3683 `swr` `d`/`u`; 3712-3713, 3715 `rau` non-nearest modes; in
  `lug`, 3826-3835 (floor and ceiling on a mantissa shifted out entirely),
  3886 (`Floor`), B3905T, B3905F, B3905c2T, B3905c2F and 3905-3907
  (`Ceiling`).
- `d == %i`: B3805T, 3806, B3960T, 3961-3965 (`lug`); B4081T, 4082 (`xpd`).
- `d == %f`: B3976F, 3978, 3980, 3982-3984, 3986-3993, B3986T, B3986F,
  3995 (`lug`).
- 3697 `fli` NaN: `fli` only flips `lug` results, which are never NaN.
- B4062T, 4063 `bex(0)`: every caller passes a width or precision of at
  least 5.
- B4085F `xpd` exponent below the minimum: `lug` raises the exponent to at
  least `v` first, and `drg` gets exponents from `sea`, which never go below
  `v`.
- `binaryfloat_mul` NaN, infinite and zero operands: 4108, B4111T, B4112T,
  B4112F, 4112-4114, B4116T, 4116-4117, B4121T, B4121F, 4121-4126, B4129T,
  B4130T, 4130-4131, B4135T, B4135F, 4135-4140, B4154c2T.
- `binaryfloat_div` special operands: 4181, B4184T, B4185T, B4185F,
  4185-4190, B4193T, 4194-4198, B4213T, 4214, B4223T, 4224.
- `fil` beyond `bif`'s `fil 0 w 1` and `fil 0 (w+1) 1`: B4255T, 4256,
  B4260T, 4261, B4267F, B4267c2F, 4278-4284.

Other paths:

- 3702-3708 `zer` and 5302-5310 `concatanate`: no callers.
- B5911T, 5912 `atom_shr` shifting a `Small` atom by 128 bits or more: its
  only caller is `rsh`, and every `rsh` call shifts by at most 64 bits
  (`rip`, `trip` and the `@p` renderer by one byte, `taft`, `tuft`,
  `den_fa` and the `++wood` helpers by up to 4 bytes, `yell` by 64). The
  generic `fe`, which shifts by an atom's width, has no callers.
- B5608T, 5609 `apply_sign` on a `Big` zero magnitude: every literal
  magnitude is either `Small` or a `Big` above `u128::MAX`
  (`hexadecimal_number` requires a nonzero lead digit, and the other literal
  parsers normalize values that fit).
- B5655T, 5656 `cha_fa` on a code point above 255: `alphanumeric` admits
  ASCII only.
- B5698T `shay` with length 0: `tok` always hashes at least 21 bytes.
- B6525T, 6526 `line_col` clamping a column inside a tall tape's indent:
  spots never start or end there in real source.
- B8307F, 8318 `posh` with no pre tyke: `poor` always passes `Some`.
- B8071F, 8073, B8096c2T, 8097 `expand_gap_start` at the end of the source,
  on whitespace, or after a `::` lead on the token's own line: spans never
  start there.

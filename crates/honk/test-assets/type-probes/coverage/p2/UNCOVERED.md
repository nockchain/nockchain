# p2 uncovered ledger: `crates/hatch/src/utils.rs` lines 3247-8371

This package covers the wing, tiki, float, tape, cord, atom-literal and path
lexers, and the `LineMap` span and doc-anchoring code.

Uncovered on the final code: 0 lines and 0 branch outcomes missed only by the
unit tests (`U`), 262 lines and 161 branch outcomes missed only by the parity
corpus (`P`), and 118 lines and 113 branch outcomes missed by both (`UP`), for
380 lines and 274 branch outcomes in all. Numbers are current line numbers.
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
  - `strip_doc_spaces`: B6365c2F.
  - `help_before_with_target_options`: B6575T, 6576, B6590T, 6591, B6605F,
    B6608F, 6616.
  - `build_doc_help_from_lines`: B6703T, B6703c2T, B6703c2F, 6704, B6708T,
    6709.
  - `parse_doc_link`: B6764F, B6767c2T, B6775F, 6778, B6780F, 6783,
    B6784c2F.
  - `help_before_named_arm_summary`: B6841T, 6842, B6852T, 6853, B6865T,
    6866, B6879F.
  - `doc_block_is_single_named_summary_for`: B6889T, 6890, B6899T, 6900.
  - `doc_block_follows_barcab_opener`: B6964T, 6965.
  - `help_before_named_arm_list_entry`: B6980T, 6981, B6993T, 6994, B7014F,
    B7021T, 7022.
  - `coltar_opener_inline_doc_comment`: B7046T, 7047, B7049T, 7050, B7054T,
    7055-7057, B7058F, B7059T, B7060T, B7060F, 7060, 7062.
  - `frag_block_doc_entries`: B7084T, 7085, B7101F, 7106.
  - `frag_doc_entries_from_docs`: B7126T, 7127, B7130T, 7131, B7138F, 7139,
    B7141F, 7142.
  - `help_before_plan_tail`: B7177T, 7178, B7189T, 7190, B7206T, 7207,
    B7212T, 7213, B7220c2F, B7231T, 7232, 7252.
  - `help_before_arm_tail`: B7261T, 7262, B7273T, 7274, B7334c2F, B7338c2T.
  - `postfix_doc_summary_parts`: B7448T, 7449-7451, B7453T, B7454T, B7454F,
    7454, 7456, B7470T, 7472.
  - `help_after_current_line_expr`: B7532T, 7533.
  - `help_after_choice_spec_item`: 7647-7648.
  - `help_before_choice_spec_item`: B7672T, 7673.
  - `help_after_line_expr_ending_at`: B7732T, 7733, B7739F.
  - `arm_scye_help_after_name`: B7807T, 7808, B7813T, 7814-7816, B7818T,
    B7819T, B7819F, 7819, 7821, 7834.
  - `line_bounds`: B7865F, B7865c2F.
  - `doc_comment`: B7893T, 7894-7896.
  - `line_starts_like_spec_doc_target`: B7928F, 7936, 7947.
  - `line_starts_like_body_spec_doc_target`: B7957F.
  - `doc_summary_before_line`: B7981F, B7989T, 7990-7991, B7994F.
- Instrumentation artifact. `LineMap::new` (6374-6376), `new_with_docs`
  (6379-6444, with all 30 branch outcomes at 6384, 6397, 6403, 6408, 6409,
  6415, 6416, 6419, 6420 and 6428) and `pint` (6549-6554) are `#[inline]` or
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
- B5635F `build_yek` index >= 256: the alphabet is ASCII.
- B5759T, 5760 `met_big(0)`: every caller passes a nonzero mantissa.
- 5767-5774 `pad_fa_big`: no callers.
- B5829c4T, B5829c5F, 5832 `wick` on `_` or an invalid char: `urt` only
  admits `[0-9a-z.~-]`.
- B5916T, 5917, B5924F, 5932-5933 `atom_mask_low_bits` with masks of 128 bits
  or more: only reachable through the private `end`, whose callers use widths
  of 64 bits or less.
- 5991 `atom_to_u8` `Big` arm: `end(3, 1)` is always small.
- B6136T `ipv4_address` empty octet (`at_least(1)`); B6170T, 6171
  `ipv6_address` empty tail (`exactly(7)`).
- 6303-6305 `snag`: no callers.
- B6466F, 6472 `set_column` and B6478F, 6479 `drift`: the `drifts` lock is
  poisoned only by a panic while it is held, and nothing that holds it can
  panic.
- B6610T, 6611-6612, B6613T, 6614-6615 section-marker drain: after draining
  through the last blank line, no blank line is left at either end.
- Empty doc text after stripping, which cannot happen because `doc_comment`
  trims trailing whitespace and blank lines are handled first: B6651T, 6652,
  B6690T, 6691, B6714T, 6715, B6748T, B6775c2F, B6818T, 6819, B6924T, 6925,
  B6935T, 6936-6937, B7144T, 7145, B7245T, 7246, B7331T, 7332, B7465c2T,
  B7655c2T, B7830c2T.
- B6731F cuff 0 with an empty summary: `parse_doc_link` returns a nonempty
  summary unchanged whenever the cuff is 0.
- Lookups that cannot fail: B6947T, 6948 (a first-line target that is never
  looked up); B6954F, 6955, B7087F, 7088, B7096F, 7097, B7923F, 7924, B7952F,
  7953, B8000F, 8001 (`line_bounds` and `line_indent` on valid indices);
  B7081F, 7082 (a line index already wrapped in `Some`); B7108T, 7109 (a frag
  walk reaching line 0, but the `:*` opener line is code); B7875F
  (`line_indent` reaching the end of its line, but it is only asked about
  non-blank lines).
- B7053F, B7447F, B7812F, B7892F: `trimmed_end > cursor` is false only if the
  line lacks the `::` already found at `cursor`.
- B7527F, B7646F, B7677F, B7680T, 7681, B7802c2T: postfix scans whose
  preconditions rule these arms out: `::` at the expression start, a comment
  line (already returned), or `:x` after an arm name, which is not valid Hoon.
- B8005F, 8019 `line_starts_like_hoon_target` on a blank line: callers pass
  token starts.
- B8083F, B8140F `expand_gap_start` scans that cannot run off the end of the
  source: a non-space byte precedes `start` in the first, and every gap line
  before `line_start` ends in a newline in the second.
- B8115F, 8123, B8127F, 8134 `expand_gap_start`: `if let Some(boundary)`
  fall-through regions after the `None` early return.
- 8309 `posh`: both arms of the `if let` produce `Some`, so the trailing `?`
  never returns early.

## Tabs (UP; not legal Hoon whitespace, and hoonc and honk reject any tab)

B7054c2T, B7395c3T, B7399c3F, B7405c3T, B7409c3T, B7439c3T, B7448c2T,
B7799c3T, B7813c2T, B7875c3T, B7885c3T, B7893c2T, B7928c3T, B7957c3T,
B8005c3T.

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
- B5648T, 5649 `cha_fa` (a character outside the base58 alphabet) and
  B5726F, 5729 `den_fa` (a bad checksum): malformed `0c` literals.
- 8323 `posh` failure; 8337 `nusk` re-parse failure.

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
- B5900T, 5901 `atom_shr` shifting a `Small` atom by 128 bits or more: its
  only caller is `rsh`, and every `rsh` call shifts by at most 64 bits
  (`rip`, `trip` and the `@p` renderer by one byte, `taft`, `tuft`,
  `den_fa` and the `++wood` helpers by up to 4 bytes, `yell` by 64). The
  generic `fe`, which shifts by an atom's width, has no callers.
- B5644T, 5645 `cha_fa` on a code point above 255: `alphanumeric` admits
  ASCII only.
- B5687T `shay` with length 0: `tok` always hashes at least 21 bytes.
- B6515T, 6516 `line_col` clamping a column inside a tall tape's indent:
  spots never start or end there in real source.
- B8297F, 8308 `posh` with no pre tyke: `poor` always passes `Some`.
- B8061F, 8063, B8086c2T, 8087 `expand_gap_start` at the end of the source,
  on whitespace, or after a `::` lead on the token's own line: spans never
  start there.

# p1 uncovered ledger: `crates/hatch/src/utils.rs` lines 1-3246

This package covers the atom-literal helpers at the top of `utils.rs` and the
desugarer (`+ax`, `open`, `flay`, `feck`, `grip`, `half`, `reek`, `name_ax`,
`autoname`, `peg`).

Uncovered on the final code: 0 lines and 0 branch outcomes missed only by the
unit tests (`U`), 286 lines and 20 branch outcomes missed only by the parity
corpus (`P`), and 6 lines and 7 branch outcomes missed by both (`UP`), for 292
lines and 27 branch outcomes in all. Numbers are current line numbers. `B<n>T`
and `B<n>F` are branch outcomes at line n, and `c2`, `c3` name the second and
third conditions on that line. "Unit-tested" means
`crates/hatch/src/cov/p1_desugar.rs` covers the entry.

## Dead code (UP)

- 1621 `unreel` `None => Hoon::Wing(one)`: `res` is known non-empty, so
  `res.first()` is always `Some`.
- 2325 `loop_yex` `_ => panic!("miccol error")`: the `[]`, `[h]` and
  `[h, t @ ..]` arms above it already cover every slice.
- 2999, B2992F `name_ax` final `else { None }`; 3039, B3032F `autoname`
  `%like` final `else { None }`: the wing was just checked non-empty, so
  `first()` cannot be `None`.

## Latent mis-port, left unasserted (UP)

- 2975 `reek` `Hoon::Pair(Hoon::Axis(a), _) => Some(..)`: hoon-138 `+reek`'s
  `[~ *]` case is a bare `[%$ p]` hoon (`Hoon::Axis`), not a pair headed by an
  axis. The parser never builds either shape in a `reek` position, so the
  difference is unobservable. `reek_reads_a_bare_axis_as_a_wing` (ignored)
  pins the hoon-138 behavior. The unit tests do not assert this arm, because
  that would lock in the wrong behavior.

## Defensive checks on parser output

- 134, B132F `hex_to_atom` (P; unit-tested): `hexadecimal_number` passes only
  hex digits, and up to 32 of them always fit in `u128`.
- 185 `base64_to_atom` and 203 `base32_to_atom` digit panics (P;
  unit-tested): the lexers pass only valid digits.
- 229, B228T, 234, B233c2T `ipv4_to_atom` (P; unit-tested); B233T, B233c3T
  (UP): `ipv4_address` always yields four groups of one to three digits, so
  the group-count, empty, length and digit checks never fail on parsed source.
- 245, B244T `ipv6_to_atom` (P; unit-tested); 250, B249T, B249c2T, B249c3T
  (UP): `ipv6_address` always yields eight groups of one to four hex digits.
- 2713 `open` `%zpwt` pair `_ => false` (P; unit-tested): `zpwt_arg` reads
  both versions with `dem`, so they always parse as numbers.
- 3215, B3214T, B3214c2T `peg` with axis 0 (P; unit-tested): no caller
  passes 0.

## No source form reaches it (P; unit-tested)

- 344-375 `interface`; 493 `spore` core specs; 776-822 `example`
  `%bcdt/%bcfs/%bczp/%bctc`; 842-871 `vair_case`; 1527-1592 `relative` core
  specs; 3051, 3057, 3061, 3065 `autoname` core specs: neither parser has a
  rule for `$.`, `$/`, `$!` or `` $` ``, so these specs never exist.
- 428-432 `spore` `%bcbc`, 1188-1202 `relative` `%bcbc`, 3046 `autoname`
  `%bcbc`: neither compiler has a `$$` spec rune (a hoon-level `$$` is a
  `%leaf`).
- 448 `spore` `%made`, 645-657 `example` `%made`, 1179-1185 `relative`
  `%made`, 3043 `autoname` `%made`: no parser rule produces `%made`.
- 454 `spore` `%over`, 1187 `relative` `%over`, 3045 `autoname` `%over`: the
  parser never builds `%over` specs. hatch builds them only inside `?=` tests
  (for `$<` and `$>`) and in `grip` casts, and `+example`'s own `%over` arm
  unwraps those before `spore`, `relative` or `autoname` could see them.
- 669, 673-674 `example` `%name`; 3044 `autoname` `%name`: `%name` specs only
  appear as the direct operand of a `+$` arm's `%ktcl`. That goes through
  `+factory`, never through `+example`, and it never needs an autoname.
- 2797 `flay` `%limb`; 2978 `reek` `%limb`; 3002 `name_ax` `%limb`: no
  hatch parser rule builds a hoon-level `%limb`; hatch writes names as
  `%wing`.
- 2742-2751 `chum_to_nounexpr` `VenProVerKel`: hoon-138 `+bonk` parses both
  `%a:b.1` and `%a:b..1` as `[ven pro kel]`. hatch does the same, so the
  four-field chum is never built.
- 2115, 2847-2849, B2846T `feck` `[%sand %tas @]`: no source form puts a bare
  `%sand %tas` in a `~|` trace. hoon-138 builds one only inside paths. The
  probes do cover the `%rock`, cell and cord (`%sand %t`) traces.
- 2895 `grip` `%dbug` skin: `flay` never produces `%dbug` skins.
- 3026 `autoname` `%gist`: `=spec` naming is wide-only, and wide specs carry
  no doccords.
- 2991, B2990T `name_ax` empty wing; 3031, B3030T `autoname` `%like` with an
  empty wing: the parser never builds an empty wing.
- 2981, B2981T, B2981F `reek` `%cncb`: a hatch-only arm. `%_(wing)` with no
  changes parses in neither compiler (`reject/reek_cncb_empty_alias.hoon`).
- B3017T `autoname` `aura == "$"`: the parser spells a bare `@` as `""`
  (the `is_empty()` test the probes do cover). Only the desugarer spells the
  empty aura `"$"`, in the `?=` specs it builds for `?@` and `?^`, and it
  never passes those to `autoname`, whose only callers are parser rules.

## Not yet covered by a parity probe (P; unit-tested)

- 440-442 `spore` `%loop`, 644 `example` `%loop`, 1164-1170 `relative`
  `%loop`, 1671-1672 `factory` `%loop`, 3028 `autoname` `%loop`: both
  compilers parse `/foo` as a `%loop` spec (hoon-138 `++scad`), and with no
  `$$` rule every such loop is free. Uncovered: no parity probe uses a `/foo`
  spec yet. In `spore` a free loop crashes both compilers (hoon-138
  `~(got by cox)`, hatch's `expect`).
- 3059 `autoname` `%bcpm`: uncovered: no parity probe names a sample with a
  wide `=$&(...)` spec yet.

## Rejection-only or malformed input (P; unit-tested)

- `flay` failures, where hatch reports a parse error and hoonc also rejects
  the program: 2762 a cell with a non-skin side; 2773-2776, 2778, B2774T,
  B2774F a `%cnts` skin, such as an axis-headed wing (`+3=5`) or a wing with
  changes (`a(b 1)=5`), which hoon-138 `+flay` accepts only as
  `[%cnts [@ ~] ~]` and hatch never builds in that shape; 2787 a `%tsgr`
  whose body is not a skin; 2806 a wing with a limb other than `,`; 2826,
  2828 a `%ktts` whose name is not over `%noun` or whose body is not a skin;
  2835, B2834T `open` leaves the hoon unchanged. `!>([b +(1)]=5)` fails in
  both under `probe-diag.sh`, and neither compiler writes an artifact.
- 2770 `flay` cell `%rock`: no source form puts a cell rock in a skin
  position.
- 2994 `name_ax` axis head; 3034 `autoname` `%like` axis head: `=$_(.)` gives
  "cannot name spec". hoonc reports a syntax error at the same place.
- 2949, B2948T `half` empty `%clsg`; 2958, B2957T `half` empty `%cltr`: these
  are reached only when a cell skin meets `~` or an empty `:*`, which neither
  compiler accepts.
- 1855, B1854T `open` `%brbc` with an empty sample; 2357 `open` `%mcsg` with
  an empty list: the parsers require at least one element. hatch panics.

## No callers (P; unit-tested)

- 98-102, 104-105, B99T, B99F `ta_to_atom`: nothing in hatch or honk calls
  it.

## Honk-native lowerings (P; unit-tested)

- 1745 `open` `%eror`: hatch builds `%eror` arms for duplicate arm and
  chapter names, but honk mints, plays and mulls `%eror` natively (failing
  with its tape), so it never asks hatch to open one.
- 1841 `open` `%note`: honk mints, plays and mulls `%note` natively, and
  `chip` only opens `?:` conditions, where the parser never attaches doc
  notes.

## Instrumentation artifact (P)

- 2973 `reek` `match gen` head region: the other `reek` arms (pair, wing,
  `%cnts`) run in parity through honk's `fund` and through `flay`. Unit tests
  cover this line.

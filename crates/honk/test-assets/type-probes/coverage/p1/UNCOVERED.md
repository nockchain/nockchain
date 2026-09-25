# p1 uncovered ledger: `crates/hatch/src/utils.rs` lines 1-3229

Scope: the desugarer (`+ax`, `open`, `flay`, `feck`, `grip`, `half`, `reek`,
`name_ax`, `autoname`, `+ah` tiki helpers) plus the atom-literal helpers at
the top of the file.

Measures after this package:

- Unit (`cargo test -p hatch cov::p1`): 5 lines, 2 branch outcomes left.
- Parity (probes in this directory): 327 lines, 23 branch outcomes left.

Tags: `U` = unit, `P` = parity, `UP` = both. Line numbers are from this
worktree. "Unit-tested" means `crates/hatch/src/cov/p1_desugar.rs` covers the
branch.

## Dead code (UP)

- 1607 `unreel` `None => Hoon::Wing(one)`: `res` is known non-empty, so
  `res.first()` is always `Some`.
- 2311 `loop_yex` `_ => panic!("miccol error")`: the `[]`, `[h]` and
  `[h, t @ ..]` arms above it already cover every slice.
- 2983, B2976F `name_ax` final `else { None }`; 3022, B3015F `autoname`
  `%like` final `else { None }`: the wing was just checked non-empty, so
  `first()` cannot be `None`.

## Latent mis-port, left unasserted (UP)

- 2959 `reek` `Hoon::Pair(Hoon::Axis(a), _) => Some(..)`: hoon-138 `+reek`'s
  `[~ *]` case is a bare `[%$ p]` hoon (`Hoon::Axis`), not a pair headed by an
  axis. The parser never builds either shape in a `reek` position, so the
  divergence is unobservable. `reek_reads_a_bare_axis_as_a_wing` (ignored)
  pins the hoon-138 behavior. The unit tests do not assert this arm, because
  that would lock in the wrong behavior.

## No hoon-138 source syntax (P; unit-tested)

- 330-361 `interface`; 762-808 `example` `%bcdt/%bcfs/%bczp/%bctc`; 828-857
  `vair_case`; 1513-1578 `relative` core specs; 479 `spore` core specs; 3034,
  3040, 3044, 3048 `autoname` core specs: hoon-138 has no parser rule for
  `$.`, `$/`, `$!` or `` $` ``, so hoonc cannot build these specs.
- 414-418 `spore` `%bcbc`, 426-428 `spore` `%loop`, 630 `example` `%loop`,
  1150-1156 `relative` `%loop`, 1174-1188 `relative` `%bcbc`, 1657-1658
  `factory` `%loop`, 3011 `autoname` `%loop`, 3029 `autoname` `%bcbc`:
  `$$` has no parser rule, and `%loop` only means something under `$$`.
  hatch's `/foo` loop spec is hatch-only syntax, and hoonc rejects it.
- 434 `spore` `%made`, 631-643 `example` `%made`, 1165-1171 `relative`
  `%made`, 3026 `autoname` `%made`: no hoon-138 parser rule produces `%made`.
- 440 `spore` `%over`, 1173 `relative` `%over`, 3028 `autoname` `%over`:
  `%over` specs only come from `+teal` (a hoon tiki under `?=`). `+example`'s
  own `%over` arm removes them before spore, relative or autoname could see
  them.
- 655, 659-660 `example` `%name`; 3027 `autoname` `%name`: `%name` specs only
  appear as the direct operand of a `+$` arm's `%ktcl`. That goes through
  `+factory`, never through `+example`, and it never needs an autoname.
- 1731 `open` `%eror`; 1816-1823 `open` `%leaf` (hoon-level); 2781 `flay`
  `%limb`; 2962 `reek` `%limb`; 2986 `name_ax` `%limb`: neither parser builds
  these hoons. hatch writes names as `%wing`.
- 2726-2735 `chum_to_nounexpr` `VenProVerKel`: hoon-138 `+bonk` parses both
  `%a:b.1` and `%a:b..1` as `[ven pro kel]`. hatch does the same, so the
  four-field chum is never built.
- 2101, 2831-2833, B2830T `feck` `[%sand %tas @]`: no source form puts a bare
  `%sand %tas` in a `~|` trace. hoon-138 builds one only inside paths. The
  probes do cover the `%rock`, cell and cord (`%sand %t`) traces.
- 2879 `grip` `%dbug` skin: `flay` never produces `%dbug` skins.
- 3009 `autoname` `%gist`: `=spec` naming is wide-only, and wide specs carry
  no doccords.
- 2975, B2974T `name_ax` empty wing; 3014, B3013T `autoname` `%like` with an
  empty wing: the parser never builds an empty wing.
- 2757-2760, 2762, B2758T/F `flay` `%cnts`: hatch does not parse `%=(a)` with
  no changes in a skin position, so `flay` never sees `%cnts`.
- 2965, B2965T/F `reek` `%cncb`: a hatch-only arm. `%_(wing)` with no changes
  parses in neither compiler (see `lab/reek_cncb_empty_alias.hoon`).

## Rejection-only or malformed input (P; unit-tested)

- 2746 `flay` cell with a non-skin side; 2771 `%tsgr` whose body is not a
  skin; 2790 wing with a limb other than `,`; 2810, 2812 `%ktts` whose name is
  not over `%noun` or whose body is not a skin; 2819, B2818T `open` leaves the
  hoon unchanged: `flay` fails, and hatch reports a parse error. hoonc also
  rejects these; `!>([b +(1)]=5)` was checked with `probe-diag.sh`, and
  neither compiler writes an artifact.
- 2754 `flay` cell `%rock`: no source form puts a cell rock in a skin
  position.
- 2978 `name_ax` axis head; 3017 `autoname` `%like` axis head: `=$_(.)`
  gives "cannot name spec". hoonc reports a syntax error at the same place
  (checked with `probe-diag.sh`).
- 2933, B2932T `half` empty `%clsg`; 2942, B2941T `half` empty `%cltr`: these
  are reached only when a cell skin meets `~` or an empty `:*`, which neither
  compiler accepts.
- 1841, B1840T `open` `%brbc` with an empty sample; 2343 `open` `%mcsg` with
  an empty list: the parsers require at least one element. hatch panics.
- 134, B132F `hex_to_atom`; 185 and 205 (the base64 and base32 digit
  panics): defensive only. The lexers pass only valid digits.
- 3198, B3197T/c2T `peg` with axis 0: defensive only. No caller passes 0.

## No callers (P; unit-tested)

- 98-102, 104-105, B99T/F `ta_to_atom`: nothing in hatch or honk calls it.

## Blocked by a divergence (P; unit-tested)

- 2154-2163 `open` `%sgbc`: hatch has no `~$` parser.
  `divergent/p1_sigbuc_parse.hoon` is HOONC-ONLY.
- 2233-2234 `open` `%mcts` `TunaTail::Call`: hatch's sail parser has no `;%`.
  `divergent/p1_sail_call.hoon` is HOONC-ONLY.
- 2614-2623 `open` `%xray` attribute `TagSpace`: hatch's sail attribute parser
  rejects `a_b` names. `divergent/p1_sail_attr_namespace.hoon` is HOONC-ONLY.
- 2690, 2693-2697, 2701-2702, 2704, B2696T/F, B2701T/F `open` `%zpwt`: any
  `!?` inside an arm diverges, because `zpwt_arg_to_noun` (utils.rs ~13717)
  encodes the version as `[%atom '138']`
  (`divergent/p1_zpwt_hoon_noun.hoon`, MISMATCH). The pair test also reads
  `[p q]` as `[min max]`, but hoon-138 treats p as the upper bound
  (`divergent/p1_zpwt_pair_order.hoon` HOONC-ONLY,
  `divergent/p1_zpwt_pair_reversed.hoon` HONK-ONLY).
- 3001, B2999T `autoname` aura `"$"`: the parser spells `@` as `""`, so this
  branch is never taken, and `=@` names the sample `%$`, which makes honk
  segfault (`divergent/p1_autoname_bare_atom.hoon`, HOONC-ONLY).
- 3042 `autoname` `%bcpm`: `=spec` must be wide, and hatch cannot parse wide
  `$&` (`divergent/p1_bucpam_wide.hoon`, HOONC-ONLY).

## Honk-native lowerings (P; unit-tested)

- 1827 `open` `%note`: honk mints, plays and mulls `%note` natively, and
  `chip` only opens `?:` conditions, where the parser never attaches doc
  notes. A `?&` with trailing doc comments was tried and did not reach this
  arm.

## Instrumentation artifact (P)

- 2957 `reek` `match gen` head region: the other `reek` arms (pair, wing,
  `%cnts`) run in parity through honk's `fund` and through `flay`. Unit tests
  cover this line.

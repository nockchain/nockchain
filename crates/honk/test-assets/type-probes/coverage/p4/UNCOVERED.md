# p4 (AST/noun conversion, rune parsers): branches left uncovered

Scope: `crates/hatch/src/utils.rs` 12030-end, `crates/hatch/src/main.rs`,
`crates/hatch/src/runes/*.rs`, `crates/hatch/src/ast/hoon.rs`.
Unit tests: `crates/hatch/src/cov/p4_nouns_runes.rs`. Probes: this directory.
Line numbers are for the worktree this ledger was written in.

Tags: **U** = no unit test reaches it, **P** = no parity probe reaches it.

## Two facts that explain most parity (P) gaps

1. **honk parses entry files with docs off** (`parse_build_leaf` in
   `crates/honk/src/bin/honk.rs`: "Build-leaf docs remain off for byte parity").
   Postfix `::` doc attachment in the rune parsers only runs on the fixed
   hoon-138 prelude, so no probe can reach it. The unit tests parse with docs on
   and cover these branches.
2. **`noun_to_hoon` only decodes prelude arm bodies.** Hoon nouns are decoded
   only when honk fires an arm whose body comes from the embedded hoon-138
   subject type. Decoder arms for forms that do not occur in any prelude arm body
   (or occur only in deep door arms a probe cannot fire on its own) cannot be
   reached from a probe. Every decoder arm is covered by round-trip unit tests.

## utils.rs

| Lines / branches | Measure | Reason |
|---|---|---|
| 12030-12035 `diff_and_report`, 12032T/F | P | Debug tool used only by the hatch CLI `--test` path; not on the compile path. Unit-covered. |
| 12037-12075 `atom_to_tas_string` | UP | Dead code: no callers. |
| 12142, 12135T (materialization cache hit) | UP | Unreachable: the cache is keyed by AST node address, and a Hoon tree owns each node once, so one materialization never revisits an address. |
| 12181-12188 `%eror`/`%hand` encode | P | Internal-only. The parser never produces `%hand`. hoonc produces `%eror` only for a duplicate arm or chapter, and hatch does not (see divergences `p4_duplicate_arm`, `p4_duplicate_chapter`). |
| 12205-12208 `%leaf`, 12214-12216 `%lost`, 12233-12235 `%tune` encode | P | Internal-only. These come from desugaring (`+open`), and desugared hoon is never stored as an arm body. |
| 12277-12279, 12328-12330 `|%`/`|@` with `Some` prefix | P | Unreachable from source: hoon-138 and hatch both always build `[%brcn ~ ...]`. |
| 12557-12560 `%sgbc` encode | P | Blocked: hatch cannot parse `~$` (`divergent/p4_sgbc_rune`). |
| 12834-12837 `%zpwt` encode, 13716-13730 `zpwt_arg_to_noun` | P | Blocked: the encoding differs from hoonc (`divergent/p4_zpwt_encoding`). |
| 12860, 12865-12878, 12859T, 12875T/F (`dor_in` cell and equal cases) | UP | Unreachable: every map key is an atom (a term), and equal keys return early in `map_put_mug`, so `dor` only ever compares two distinct atoms (covered by `p4_mug_collision`). |
| 12924T, 12925-12929 (repeated map key) | P | Needs two distinct Rust keys that encode to the same atom (`"$"` and `""`). The parser never emits an empty arm or chapter name. Unit-covered. |
| 12975F (`map_put_mug` returns None) | UP | Defensive: only for a malformed treap, and the function only ever passes in treaps it built itself. |
| 13002T (trimmed atom bytes empty) | UP | Unreachable: this branch requires `n > DIRECT_MAX`, so `n` has nonzero bytes. |
| 13018-13020 `biguint_to_ubig` | UP | Dead code: no callers. |
| 13022-13213 `opt_to_noun`, `type_to_noun`, `coil/garb/poly/vair/stencil/block/gate_to_noun` | P | Reached only through `Hoon::Hand`, which is internal (honk lowers its own types natively). Unit-covered through `%hand` round trips. |
| 13244-13254 `Loop`/`Made`, 13277-13314 `BucBuc`/`BucDot`, 13336-13391 `BucFas`/`BucTic`/`BucZap` spec encode | P | Not parseable in hoon-138: there is no `$$`, `$.`, `$/`, `` $` `` or `$!` rune. `%loop` and `%made` are only built internally. Unit-covered. |
| 13409-13412 skin `%dbug` | P | hatch `flay` strips dbug before building a skin, so no source produces it. Unit-covered. |
| 13500-13507 `%made` note with wings | P | Internal-only: made notes come from `+ax` synthesis. Unit-covered. |
| 13581-13586 chum `VenProVerKel` | P | The parser maps `%k:v..n` to `VenProKel`, which has the same noun shape as hoonc's result (probe `p4_misc_forms` j3 passes). The four-field form is never built. Unit-covered. |
| 13591-13703 `nock_to_noun`, `nock_hint_to_noun`, `term_or_tune_to_noun`, `tune_to_noun` | P | Reached only through internal `%hand` and `%tune` hoons. Unit-covered. |
| 13809-13811 `TunaTail::Call` encode | P | Blocked: hatch cannot parse sail `;%` (`divergent/p4_sail_call`). |
| 13865-15574 `noun_to_*` decoders (except `%cntr`, `%cnkt`, `%dtkt`, which `p4_prelude_decode` covers) | P | See fact 2: decode only runs on prelude arm bodies. Unit-covered by round trips and malformed-noun tests. |
| 13946-13975 fork-set decode, 13948T (node budget) | P | Decoded only inside `%hand` types. The budget branch is defensive. Unit-covered, including the budget. |
| 14572-14573, 14570F (unknown `tuna_tail` tag) | UP | Unreachable: `noun_to_tuna` calls `noun_to_tuna_tail` only after matching one of the four tags. |
| 15576-15610 `collect_inputs` | P | hatch CLI only. Unit-covered except the `std::process::exit` paths. |
| 15590-15591, 15596-15601, 15607-15608, 15588F (read errors and bad path) | UP | These call `std::process::exit`, which a test cannot survive. CLI diagnostics only. |

## main.rs

| Lines | Measure | Reason |
|---|---|---|
| 109 (`invalid spec constant`), 279 (`invalid p in p=q`) | P | Rejection branches. hoonc's grammar has the same guards (`+scad` only accepts a `%$` coin, and `p=q` needs a skin), and a probe cannot hold a build failure. Unit tests assert that honk's parser rejects them. |
| 578-926 (`Drop`, temp paths, hoonc boot, `run_test`, `run_parser`, `main`), 579T/F | UP | hatch binary CLI. These are private to `main.rs` and not on honk's compile path. |

## runes/*.rs

| Location | Measure | Reason |
|---|---|---|
| bar.rs 144/143T, 244-247/243T/244T/F; buc.rs 190T, 194; cen.rs 168, 167T/c2; col.rs 80, 110, 140, 182, 186 (79T 109T 139T 181T 185T); ket.rs 219/218T, 260/259T; tis.rs 180/179T, 341/340T; wut.rs 303, 311-314, 362, 433, 444-455 (302T 310T 311T/F 361T 432T 443T 444T/F 452T) | P | Postfix-doc attachment, which runs only with docs enabled (fact 1). Unit-covered. |
| bar.rs 210-216 (`barbuc_wide` doc before body) | P | Same reason. 211/210T is also unreachable in the unit measure: a wide `|$(...)` body never starts a line, and `help_before_body_spec` requires one that does. |
| buc.rs 191-192, 191T/F | UP | This `matches!` guard is reported with no counts even though its only successor, line 194, runs (`buclus_tall_attaches_postfix_doc_to_spec`). The pre-existing identical Gist case is unreachable because the inner spec never carries the same postfix doc. |
| buc.rs 459-470 `bucmic`/`bucmic_wide` | P | Dead in the grammar: `bucmic_wide` is never wired in, and `bucmic` is only called by it. Unit-covered. |
| buc.rs 619-622 (`=name=spec` irregular) | P | Blocked: a divergence (`divergent/p4_buctis_prefixed_autoname`). Unit-covered. |
| cen.rs 145c2F; col.rs 47c2F | UP | Unreachable: every caller passes `allow_four_space = false`. |
| fas.rs 46-80 `fastis`/`fastar`/`fashax` | P | Dead in the grammar: never wired into `fas_runes_tall`. Unit-covered. |
| ket.rs 140 (`Hoon::Limb` face) | UP | Unreachable: the parser produces `Hoon::Wing` for names and never `Hoon::Limb`. |
| ket.rs 143 (multi-limb wing face), 179 (`kettis_wide` reject) | P | Rejection: honk and hoonc both reject `^=  a.b  5` (BOTH-FAILED, checked with probe-run). Unit tests assert the errors. |
| ket.rs 148F | UP | Docs-only (fact 1), and in the unit measure the face is already noted by the time `kettis` sees the doc. |
| sail.rs 45-49, 46T/F (`Big` char code) | UP | Unreachable: a tape woof for one character always fits in `u128`. |
| wut.rs 184-187, 183T/184T/F (`wutcol_wide` doc) | UP | Unreachable: the closing `)` always follows `r`, so no `::` can come directly after `r`'s span. |
| wut.rs 242, 256 (`?#` invalid pattern) | P | Rejection branches that hoonc also rejects. Unit tests assert the errors. |

## ast/hoon.rs

| Lines | Measure | Reason |
|---|---|---|
| 22-27, 300-308 (serde for `BigUint` and `Axis`) | P | Used only for hatch CLI JSON output. Unit-covered. |
| 36-44, 51, 57, 63, 89-91, 112-114, 118-145, 149-178 (`From` impls, `to_u8`/`to_u32`/`to_u128` Big arms, `zero`, `to_u16_lossy`, `cmp`/`lt`/`le`/`gt`/`ge`/`eq` Big arms, `From<&str>`, `BitOr`), 157T/F | P | Not reached by any compile path: these are helpers used by parsing and desugar code that no probe drives with big atoms. Unit-covered. `p4_atom_helpers` covers the reachable ones (`to_u8` via @q, `to_u32` via @ta and @c knots, `to_u64_lossy` via @p). |
| 212-218 `BinaryFloat::sign` | P | Blocked by float divergences: `.1`, `.~1`, `.~~1` mismatch (`divergent/p4_float_sign`), and overflowing literals mismatch or panic (`p4_float_rs_overflow`, `p4_float_rd_overflow`, `p4_float_rh_overflow`). Unit-covered. |
| 252-254 `Axis::to_u64`, 288-290 `Display` | P | Used by honk diagnostics and the hatch CLI only. Unit-covered. |

## Divergences (in `divergent/`)

| Probe | Verdict | Cause |
|---|---|---|
| p4_zpwt_pair | HOONC-ONLY | runes/zap.rs `zapwut` (~189) needs `gap()` between the pair numbers, where hoonc's `+hinh` uses `ace`. |
| p4_zpwt_encoding | MISMATCH | utils.rs `zpwt_arg_to_noun` (~13716) emits `[%atom '138']`, where hoonc stores the atom `138`. |
| p4_sgbc_rune | HOONC-ONLY | runes/sig.rs has no `~$` parser. |
| p4_mcts_rune | HOONC-ONLY | hatch has no `;=` parser. |
| p4_sail_call | HOONC-ONLY | runes/sail.rs `tuna_tail` lacks the `;%` (`%call`) mode. |
| p4_bucpam_wide | HOONC-ONLY | runes/buc.rs `bucpam_wide`/`bucpam_spec_wide` (~354/805) do not take the surrounding parentheses. |
| p4_buctis_prefixed_autoname | MISMATCH | runes/buc.rs `buctis_irregular` (~619) names `=a=@` `%a`, where hoonc names it `%a-atom`. |
| p4_duplicate_arm | HONK-ONLY | utils.rs `chapters` (~4975): when an arm name repeats, the later arm wins; hoonc emits `%eror`. |
| p4_duplicate_chapter | HONK-ONLY | utils.rs `chapters` (~4967) merges repeated chapters; hoonc emits `%eror`. |
| p4_float_sign | MISMATCH | utils.rs `binaryfloat_mul` (~4152) tests `ma == 0 \|\| mb == 0` twice where hoon has `=(s.a s.b)`, so positive literals come out negative. |
| p4_float_rs_overflow | MISMATCH | `.1e39` encodes differently (float rounding path, utils.rs ~3780-3870); possibly the same sign flip as `p4_float_sign`. |
| p4_float_rd_overflow, p4_float_rh_overflow | HOONC-ONLY | honk panics with "value too large for u128" at utils.rs:3808. |
| p4_sail_attr_utf8 | MISMATCH | runes/sail.rs `parsed_atom_to_cord` (~42-54) merges a multi-byte UTF-8 char into one scalar, where hoonc keeps one beer per byte. |

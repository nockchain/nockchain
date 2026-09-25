# c3 ledger: uncovered branches in `crates/honk/src/native/ut/mod.rs` 9642-end

Unit tests: `crates/honk/src/native/ut/cov/c3_ut_c.rs`. Parity probes: this directory.
Tags: **U** = left uncovered by unit tests, **P** = left uncovered by the parity probes.
Rejection cases marked "both reject" were checked with `probe-diag.sh` (hoonc fails with
the same error name) and are asserted in the unit tests.

## Blocked by a divergence (see `divergent/`)

| Lines | Tag | Reason |
|---|---|---|
| 9759, 10074 (`Skin::Wash` gain/lose) | P | Known divergence (Wash in gain/lose); ledgered as blocked. Unit-tested. |
| 9884 (`gain_cell_skin` %core, `Base(NounExpr)` tail) | P | `divergent/c3_gain_cell_core_base_tail`, `c3_gain_cell_core_noun_term`: hoon-138 keeps the core only for the term tail `%noun`. |
| 10177, 10142F (`lose_cell_skin` %core, `Base(NounExpr)` tail) | UP | `divergent/c3_lose_cell_core_base_tail` (HONK-ONLY). Not pinned in a unit test, which would lock in the bug. |
| `mull_mile` blocked seminoun | - | Known divergence; ledgered as blocked. |

## Unreachable from source

| Lines | Tag | Reason |
|---|---|---|
| 9701-9702, 10040-10041 (`%base %null` skin) | P | `flay` turns `~` into a `%rock %n` leaf; no source skin is `[%base %null]`. Unit-tested. |
| 9704, 10043 (`%base %void` skin) | P | `!!` is `%zpzp`, which `flay` rejects. Unit-tested. |
| 9726-9730, 10058 (`Skin::Dbug`) | P | `flay` never builds `%dbug` skins (hoon-138 and hatch). Unit-tested. |
| 9733-9737, 10059 (`Skin::Help`) | P | Needs a `%note %help` hoon in `?#` skin position; hatch attaches docs only to `^=` names, which `flay` then rejects. Unit-tested. |
| 9707T, 9708, 9771, 9846, 9933, 10086, 10138, 10228, 10378, 10560 (void refs / void fuse subject) | P | Void cannot reach these: face, hint, cell and fork constructors collapse void, and a void leg makes the subject void (mint-vain first). Unit-tested. |
| 9850 (`gain_cell_skin` noun ref, void head) | P | A source head skin gains `%void` from `%noun` only via `%base %void`, which `flay` never builds. Unit-tested. |
| 9948T, 9950T, 9951F, 9957 (leaf skin with empty or `@` aura) | P | Leaf skins come only from `%rock`s, which always carry an aura. Unit-tested. |
| 10704 (`crop_sint` fallback) | P | `crop_inner` handles `%void`/`%noun` refs before reaching `crop_sint`, and atoms and cells never reach it. |
| 10754, 10753T, 10771, 10770T (`fitz` empty `end`/`rsh`) | UP | Dead: `$` is 0, so an empty atom returns from the `q == $` check first, and `rsh` returns `$` before reaching `rest.is_empty()`. |
| 10778c3F | U | Redundant `p_w != 0` inside a clause reached only when `p_w != 0`. |
| 10978c5T, 10978c6T/F, 10989T, 10990 (peek blocked context) | UP | `peel` returns `con = true` only together with `sam = true`, so the first clause always matches first. |
| 11214-11217, 13499-14094 (`%hand` and its `type_to_noun`/`spec_to_noun`/`nock_to_noun` lowerings) | P | `Hoon::Hand` comes only from `noun_to_hoon`, never from parsing. Unit-tested (every AST form). |
| 11474-11475 (`mull` `Hoon::Eror`) | UP | `%eror` comes only from `noun_to_hoon`. hoon-138 would crash in `open`; honk returns `%void`. Not pinned. |
| 11417T (`mull-skip`) | P | Any `%lost` with a non-void sut side fails `mint` first (mint-lost). |
| 11417F, 11421, 11521T, 11522, 11723F, 11725 (`vet` off inside mull) | P | hoon-138 runs `mull` only under `vet` (`fire` checks `!vet` first). Unit-tested with `vet = false`. |
| 11515-11516, 11513F (`mull-open`) | P | Every parsed gene has a mull arm or opens. Unit-tested. |
| 11716 (mixed synthetic/natural ports) | P | sut and dox share the definition context, so a wing resolves synthetically on both sides or on neither. Unit-tested (existing test). |
| 11808-11813 (`atom_is_zero`) | P | Only `play_sand` with aura `n` calls it, and no source form builds `%sand %n`. Unit-tested. |
| 11818-11823, 11820T (`atom_is_flag` Big) | P / U(11820T, 11821) | The parser emits `ParsedAtom::Small` for word-size values. `BigUint::to_bytes_le()` of zero is `[0]`, never empty, so 11820T and 11821 are dead. |
| 11863-11870, 13596-13612 (axis/parent limbs in note wings) | P | `%made` note wings come from `|$` argument names (terms only). Unit-tested. |
| 12124-12128, 12123T (`map_put_mug` existing key) | P | Map inputs come from `HashMap`s (unique keys). Unit-tested. |
| 12238T, 12239, 12254F, 12257-12258, 12808, 12837 (void payload/inner, unknown tag) | P | `burp` and `ty_core` get types already collapsed by the native constructors; unknown tags don't exist. Unit-tested. |
| 12409-12410, 12394-12395 (fork-set node missing branches) | UP (12409) | The pop side re-reads a node already validated on the push side. |
| 12621T, 12622-12624, 12651T, 12652-12654 | UP | `axis > 1` implies `bit_len >= 2`. |
| 12615T/12616, 12647T/12648, 12687T/12688-12690 | P | Defensive `axis <= 1` / `b == 0` checks; unit-tested. |
| 13473 (`tune_to_noun` `None` value) | P | Parsed tunes always carry a hoon (`=*`, busk). |

## Rejection paths (both compilers reject; unit-tested)

| Lines | Tag | Error |
|---|---|---|
| 9754T/9755, 10069T/10070 | P | `gain spec` / `lose spec`: `ar +fish` already requires the ref to nest in the spec, so after a successful mint this is unreachable. Unit-tested directly. |
| 9782c2T, 9783-9785, 9951c2T, 9952-9954 | P | `atom-mismatch` (`?#(@ud t)`, `?#(%5 t)` on `@t`); both reject. |
| 10548T, 10549-10551 | P | `fuse-loop` (`?=(@ v)`, `v=$@(@ r)`); both reject. |
| 10666T, 10667-10669 | P | `crop-loop`: every `?=` computes the gain first, which hits fuse-loop; unit-tested directly. |
| 11084 | P | `mull-none` (`=>(!! a)` in a wet arm); both reject. |
| 11328 | P | `mull-bonk-b`; both reject. |
| 11346 | P | `mull-bonk-c`; both reject. |
| 11379c2T, 11380 | P | `mull-bonk-a` (`?=(_a a)`); both reject. |
| 11397T, 11398 | P | `mull-bonk-x` (non-nesting `?#` wing); both reject. |
| 11379T, 11393T, 11394 | UP | Axis mismatch between sut and dox wings: wet samples keep the declared faces, so a wing resolves to the same axis on both sides. |
| 11456T, 11457 | P | `mull-bonk-f` (`!@(p.a ...)`); both reject. |
| 11708T, 11709 | P | Edits on a synthetic port (`tack` rejects in mint first). Unit-tested. |
| 11753T/11754-11756, 11774T/11775-11777, 11787T/11788-11790, 11801-11803 | P | `mull_endo` axis/opal mismatches: sut and dox cores share arm layout and sample faces. Unit-tested. |

## Performance-cache, hash-collision or test-only code

| Lines | Tag | Reason |
|---|---|---|
| 10909F, 10909c2F, 10911, 10928F, 10931F, 10932F/c2F, 10935-10936, 10938F, 10941 | UP | `peek` seen-hold buckets: reached only on a mug collision between different (hold, axis) pairs. |
| 10325c2F, 10329F | P | `set_miss_memo_persistence` toggles (prelude mint only). Unit-tested. |
| 10414c4T | P | Reversed-pair `miss` hold guard; needs mutually recursive molds through `redo`. Unit-tested. |
| 10414c4F | U | Needs a three-hold cycle where the new ref equals an earlier sut but the sut differs. Termination guard only. |
| 11075-11076, 11096 | UP | `mull` cache: the signature fallback (every AST gets a signature) and the store's error arm. |
| 11897-11946, 11953, 11966 (`dor`, `gor_mug`/`mor_mug` equal-mug tie-break) | P | Treap ordering reaches `dor` only on a mug collision. Unit-tested. |
| 11937T, 11938 | U | `dor` tail step after structurally-equal heads with equal mugs, inside the collision path. |
| 12140-12141, 12165-12166, 12384T/12385-12387, 12502-12504 | UP / P(12502) | Decode-error arms on treaps built in place, the 1M-node budget, and an unknown foot tag (unit-tested). |
| 12913-12919 (`NativeForkOptionIter::size_hint`) | P | Iterator size hint (allocation sizing only). Unit-tested. |
| 13300F, 13301, 13309F, 13310 | U | Assertion-failure arms inside the in-file `#[cfg(test)]` module. |
| 14107T, 14108-14114 (`cell_type` under `HONK_NATIVE_TYPES`) | U | Test helper branch behind an env flag cached process-wide; setting it would race other tests. |

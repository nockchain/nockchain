# hoonc divergences found by the coverage work

Each entry was a source program where honk (or its parser, hatch) disagreed
with hoonc. All of them are fixed. Their probes now run in CI with the type
probe parity suite: byte-identical builds in `coverage/regressions/` (or the
`coverage/c6` import pairings), programs both compilers reject in `reject/`
(`REJECTION_PROBES`), and two native-only rejections (`divergent_bunt`,
`wash_gain_loop`) where hoonc never terminates.

A new divergence goes in `coverage/<pkg>/divergent/` (tagged manual) with a
row here until it is fixed.

## Open

None.

## Type checker (`crates/honk/src/native/ut`)

| # | Divergence | Fixed in |
|---|---|---|
| 1 | `?!` was minted natively instead of opened to `?:`, skipping dead-branch elimination and flow typing | honk: open ?! through ?: and resolve bare axes with read permission |
| 2 | Bare axis hoons were read with `Way::Free`, skipping `++peel` for iron/lead/zinc cores | same |
| 3 | A wet gate whose forked sample missed every case kept its face or failed with redo-match | honk: match hoon-138 redo, wet-arm rib keys, and mulled core seminouns |
| 4 | The wet-arm recursion guard was keyed on the redone core instead of the call-site subject | same |
| 5 | Mulled cores shared play's blocked seminoun, so nest took the equal-coil shortcut | same |
| 6 | `?#` resolved its wing without the `[%& 1]` prefix | honk: port ++fish for ?# test formulas and fix skin gain/lose |
| 7 | `%over` skins in `?#` test formulas derived `ref` from the wrong subject | same |
| 8 | Cell-skin test formulas used `cond`/`and` where hoon-138 uses `flan` | same |
| 9 | Cell-skin gain/lose on a core tested a `%noun` base skin instead of the term `%noun` | same |
| 10 | Cell-skin lose re-wrapped a face that hoon-138 strips | same |
| 11 | A wash skin as a `?:` condition compiled in honk; hoonc recurses forever. honk now fails with `gain-wash` | same |

## Parser and desugarer (`crates/hatch`)

| # | Divergence | Fixed in |
|---|---|---|
| 12 | Positive float literals compiled negative | hatch: fix float literal sign, precision, and rendering, and wide atom literals |
| 13 | Float literals overflowing u128 intermediates panicked | same |
| 14 | Float path segments rendered differently | same |
| 15 | `@uv`/`@uw` literals over 128 bits panicked | same |
| 16 | IP literals: octets over 255 rejected, uppercase `@is` accepted | hatch: parse and render dates, IP literals, and @uw/@uc/@ux like hoonc |
| 17 | `@da`/`@dr` field ranges, overflow, and fraction rules | same |
| 18 | Path rendering of `@uw`, `@uc` zero, and BC dates | same |
| 19 | Signed path segments lost their radix (`/-0x10` rendered `-16`) | hatch: fix float literal sign, precision, and rendering, and wide atom literals |
| 20 | A negative `@uc` kept aura `%uc` instead of `%sc` | same |
| 21 | Non-ASCII text: tapes split by code point, UTF-32 knots substituted U+FFFD, sail attributes merged bytes | hatch: parse tapes, knots, and cords byte for byte like hoonc; hatch: parse ~$, wide ~! and $&, sail ;% and ;= forms, and !? like hoon-138 |
| 22 | `'''` block cords and zero-headed digit groups accepted or rejected the wrong inputs | hatch: parse tapes, knots, and cords byte for byte like hoonc; hatch: fix float literal sign, precision, and rendering, and wide atom literals |
| 23 | `autoname`: a bare `@` sample crashed honk; `=a=@` named the sample `a` | hatch, honk: autoname bare atoms, turn duplicate arms into %eror, anchor doc spots like hoonc |
| 24 | `!?`: version encoding, range order, and pair syntax | hatch: parse ~$, wide ~! and $&, sail ;% and ;= forms, and !? like hoon-138 |
| 25 | Missing syntax: `~$`, sail `;%` and `;=`, namespaced sail attributes, wide `$&`, wide `~!` | same |
| 26 | Duplicate arm or chapter names were merged instead of becoming `%eror` arms | hatch, honk: autoname bare atoms, turn duplicate arms into %eror, anchor doc spots like hoonc |
| 27 | Under `!:`, a plain doc above `=/` anchored the spot differently | same |
| 34 | hatch did not parse the sail `;p: text` form | hatch: port hoon-138's ++sail grammar |
| 35 | hatch did not parse the sail `/"url"` and `@"url"` tag shorthands (and a dozen other sail forms) | same |
| 36 | hatch did not implement sail markdown (`++cram`) | hatch: port hoon-138's ++cram sail markdown |
| 37 | In a tall `;"""` block, a deeper-indented `"""` line ended the block; hoonc reads it as text | same |

## Import pipeline (`crates/honk/src/pipeline.rs`, CLI)

| # | Divergence | Fixed in |
|---|---|---|
| 28 | Body `%spot` started at `/-`, `/+`, `/?` import lines | hatch, honk: parse import headers and resolve dependency trees like hoonc |
| 29 | `/#` imports were not evaluated eagerly | same |
| 30 | Import-header edge cases (comments, gaps, tabs, commas, `/*` faces and marks) | same |
| 31 | honk ignored broken files the entry does not import; hoonc checks the whole tree | same |
| 32 | Entries under an `open/hoon` root were keyed by the root marker instead of the path | same |
| 33 | A tall arm body starting with a `/-` path segment was read as an import header | same |

## Open honk bugs (no hoonc verdict)

- A damaged cache pack is never rewritten, so every later build misses it
  (`build_cache.rs` ~193). Pinned by the ignored test
  `cov_t1::cache_damaged_pack_is_repaired_across_sessions`.
- A batch manifest listing one entry twice fails with `--cache-dir`
  ("duplicate root name", `bin/honk.rs`). Pinned by an ignored test in
  `tests/cov_t1_cache_parity.rs`.
- An empty `/*` data file panics the honk worker (hoonc also fails).
- A truncated `--sut-jam` panics in `cue` instead of returning an error.
- `NATIVE_HOON_NO_CHUNK=1` produces a different prelude artifact than the
  default chunked mint (debug path only).

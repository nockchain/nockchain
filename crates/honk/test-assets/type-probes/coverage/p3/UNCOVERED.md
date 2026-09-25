# p3 coverage ledger: `crates/hatch/src/utils.rs` 8382-12125

This package covers the atom, knot, path, date, phonetic-name, jam/cue, spec-builder and spot-anchoring code in `crates/hatch/src/utils.rs` lines 8382-12125.

Gaps in the range: lines missed only by unit tests 2, only by the parity corpus 316, by both 180 (498 lines); branch outcomes missed only by unit tests 0, only by parity 160, by both 42 (202 outcomes).

Tags: `U` = unit tests miss it, `P` = the parity corpus misses it, `UP` = both. `B<line>T`/`F` are branch outcomes; `c2`/`c3` mark later operands of `&&`/`||`. Unit tests are in `crates/hatch/src/cov/p3_atoms_specs.rs`; probes are in this directory.

## Not yet covered

- `wood_crashes` malformed UTF-8 L9117, B9116T (P; unit-tested): uncovered: no probe uses a `~~` path knot whose escape encodes to bytes `++taft` rejects, such as `/~~~200000.` (above U+10FFFF).
- `w_ne` digits 36-63 L9230, L9232, L9234, B9229T, B9231T, B9233T (P; unit-tested): uncovered: no probe renders a `@uw` path segment with a digit above `z` yet (`p3_path_unsigned` and `regressions/p3_path_uw` use `0` and `1`).
- Wide `~0` blobs (P; unit-tested): `atom_to_bits` Big arm L11024, L11026-11031, L11034, `rub_atom` payloads over 128 bits L10968-10972, L10974-10975, B10957F, B10970T/F, `atom_bit_len` Big arm L10869, `atom_get_bit` Big arm L10876-10882 and B10880T. uncovered: no test exercises it yet. `base32_to_atom` returns a bignum, so any `~0` blob wider than 128 bits reaches `atom_to_bits`, one holding an atom wider than 128 bits reaches `rub_atom`, and such a blob in a path segment is re-jammed through `atom_bit_len` and `atom_get_bit`.
- `skip_plain_doc_before_equals_slash_start` scan body L11951-11952, L11967-11970, L11973-11974, L11976, B11941F, B11947F, B11950T, B11959F, B11962F, B11965F, B11973T/F (UP): uncovered: no test exercises it yet. The walked-back `start` always sits on an anchoring doccord (`expand_gap_start` only walks back to one) or on a code line with a trailing doccord, so the scan returns on its first line (a code line, or a doccord that anchors) before it can see a blank or plain doc line.
- `rend_crashes` `%blob` arm L9099 (U): uncovered: no unit test checks a `~0` blob path knot yet (the parity corpus reaches it).
- `unanchor_hoon_spot` fallthrough L11625 (U): uncovered: no unit test exercises it yet (the parity corpus reaches it).

## Rejected by both compilers (P; unit-tested unless noted)

Reject probes in `type-probes/reject/` count toward parity, so these stay uncovered until a reject probe uses the input.

- `path` posh failures L8560 (`/=/=/...` past the file path) and L8570 (`%/=/=/...`).
- `number` bad base58check L9441 (`0c...`) and L9463 (`-0c...`).
- `year_big` BC year before the pivot B9647c2T: hoon-138 `++year` underflows in `sub` and hatch returns a parse error.
- `ind` miss L10219 and `tiq` unknown suffix L10245 (`~zzz`).
- `phonemic_name` `doz` star prefix L10265, B10264T (`~dozzod`), and zero leading word L10278, B10277T (`~dozzod-dozzod`).
- Malformed jams in `~0` blobs (`~00`, `~07`, `~0g0`): `rub_backref` L10926, L10929, L10932, B10925T, B10928T, B10931T; `rub_atom` L10953, B10952T; `get_size` L10987, L10998, B10982F, B10986T, B10997T; `cue_inner` L11046, L11058, B11045T, B11057T.
- `unanchor_gap_glued_children` B11573F, B11579F (UP): `?-` or `?+` with no clauses is a syntax error in both.

## Unreachable from source

- Dead code, no callers (UP): `atom_to_char` L9193-9198, L9200, L9204-9205; `dvr_u64` L9298-9300; `w_co` L9341-9350, L9352; `is_leap` L9784-9786; `lsh_u128` L10049-10052, L10054, L10056; `mix` L10405-10407, `mix_big` L10409-10411, `mix_atoms` L10413-10419, L10422; `rol32` L10426-10428; generic `fe` L10547-10619; `noun_hash` L10769-10773.
- No caller outside tests (P; unit-tested): `is_leap_year` L9788, L9790-9791, B9790T/F, B9790c2T/F; `yule` L9793-9797.
- Unused builders (P; unit-tested): `two_specs_closed_tall` L11286-11292, `name_spec_closed_tall` L11354-11362, `one_hoon_closed_tall` L11379-11386, `hoon_with_span` L11540-11548 and the `apply_hoon_trace` it calls, L11403-11406, L11408-11409, L11411, L11413, B11405T/F.
- `ParsedAtom::Big` arms where the value is always small (UP): `trip` L8440 (one byte), `rend_with_rep` aura bytes L8649, L8654, L8990, `yell` 16-bit digit L9678, `fein` after `feis` L10687 (`feis` returns `Small`), `offset_to_atom` L10823, B10820F (a `usize` always fits in `u128`).
- `wood_go` raw-bit fallback L9134 (UP): `path` rejects any knot `rend_crashes` flags before rendering it, so `wood` only sees characters `++wood` accepts.
- `zust` invalid ipv6 L8412 and invalid ipv4 L8419 (UP): `ipv6_address` and `ipv4_address` already produce exactly the group count and digit widths that `ipv6_to_atom` and `ipv4_to_atom` check, so the conversions cannot fail.
- `path` `%%...` posh failure L8579 (P; unit-tested with an empty path): it fails only when the file path is empty, and a build always has one.
- `rend_with_rep` fallback arms L8779, L8785, L8800, L8938-8940, L9016 and `z_co` L9375-9379 (P; unit-tested): `nuck` never produces `%d?` other than `%da`/`%dr`, `%f` above 1, `%i?` other than `%if`/`%is`, `%r?` other than `%rd`/`%rh`/`%rq`/`%rs`, or an unknown aura, so only direct `rend_co` calls reach them.
- `relative_date` `_` unit L9603 (UP): the unit comes from `one_of("dhms")`.
- `year_big` day 0 or month out of range L9653, B9652T, B9652c2T (UP): `absolute_date` only parses days without a leading zero and months 1-12, and `year` passes a fixed valid date.
- Bit helpers with fixed arguments: `bloq_bits` panic L9819, B9818T (P); `met` on `Big(0)` L9838, B9837T (P); `rep` zero step and chunks of 128 bits or more L9876-9877, L9879-9880, B9862c2T, B9871F, B9876T/F (P; callers use bloq 3 or 4 with step 1); `rap` full-width mask and oversized-chunk panic L9910, L9915, B9909T, B9914T (P); `cut` guards L9944, L9949, L9954, L9959, B9943T (P), `bit_len == 0` and Small `bit_start >= 128` L9974, L9980, B9973T, B9979T (UP; both are bounded by the source width), Small mask of 128 bits or more L9997, B9996T (P), and Big masks of 128 bits or more L10017, L10024, L10026-10027, B10005F, B10016T (P); `lsh`/`rsh` overflow L10036, L10044 (P); `end` overflow L10088 (UP); `end_big` overflow L10096 (UP); `rsh_u128`/`end_u128` shifts of 128 bits or more L10061, L10108, L10111, B10060T, B10110T (UP; only called with `(0, 1)`).
- `ins`/`ind` length checks L10191, L10208, B10190T, B10207T (P): `tip` and `tiq` always pass three bytes.
- `muk` blocks and 1- or 3-byte tails L10456-10461, L10463, L10468-10477, L10483-10491, L10500-10507, B10459T/F, B10467T (UP): `eff` always hashes two bytes. `fen` odd round count L10525, B10522F and `fe_u64` odd arm L10643, B10642T (UP): `r` is always 4.
- `feis` re-encrypt L10630, B10629T and `feen` second pass L10669, B10666F (UP): with r = 4, the outputs are at most 0xfffe.ffff, below k = 0xffff.0000.
- `fein` Small-planet and Small-moon arms L10681, L10692, L10698, and the `dis`/`con` helpers that only they and the `fynd_u64` 64-bit arm use, L10386-10388, L10390-10392 (P; unit-tested): parsed planets and moons are `ParsedAtom::Big`.
- `fynd_big` values below 0x1.0000 B10719F, B10724F (UP): parsed planets and larger names are at least 0x1.0000. `fynd_u64` 64-bit arm L10742-10744, B10737c2F, B10741T (P; unit-tested): `fynd_big` only passes 32-bit words.
- jam helpers (UP): `usize_bit_len(0)` L10859, B10858T (`mat_bits` handles a zero atom first); `atom_get_bit` past the atom's width L10884, B10875F, B10880F (`mat_bits` only asks for bits below the width); `bits_to_atom` on no bits L10892, B10891T (jam always emits bits).
- `stack_block_docs_clad::walk` defensive returns L11499, L11502, L11505, L11512, B11498F, B11501F, B11504F, B11508F, B11509F (UP): `map_to_noun` always builds a well-formed treap.
- `attach_help_to_hoon` same-help short cut L11528, B11527T (P; unit-tested): every parser call site checks `hoon_tail_has_help` first.
- `apply_hoon_docs` B11430F (P; unit-tested): docs-only. honk parses entry files with docs off (`parse_build_leaf` in `crates/honk/src/bin/honk.rs`), so no probe reaches postfix doc attachment on a tall `;~`.
- `unanchor_gap_glued_children` `.^` with non-`:*` or empty arguments L11599, B11595F, B11596F (UP): the `.^` parser always builds a non-empty `%cltr`.
- `unanchor_spec_spot` fallthrough L11633 (UP): in traced mode the `?-`/`?+` clause specs come from the traced spec parser, which always wraps them in `%dbug`.
- `unanchor_spot_start` L11643, B11642F, B11648F, B11652F (UP): L11643 and B11642F need a spot line outside the source; B11648F and B11652F need a doc block that runs to EOF with no code after it, but the child always has code.
- `wrap_spec_with_trace` nested spot L11711, B11708F (UP): no spec parser re-wraps a traced spec with a different span.
- `arm_body_start_from_header` L11722, L11740, L11742-11755, L11758-11760, L11763-11783, L11786-11789, L11794 and its branch outcomes B11721T, B11721c2T, B11730F, B11730c3T, B11737F, and every outcome from B11743 through B11786 (P; unit-tested), plus `chumsky_spot_to_hoon_spot` L11984 (P): no parser span starts on a `++`/`+$` header (arm bodies start after the gap), is empty, starts at EOF, or follows tab indentation (hoon gaps have no tabs), so only direct span calls reach it. L11791 (UP) needs a span that ends past the source.
- `non_doc_start_after_leading_doc_span` L11805-11806, L11821, L11823-11834, L11836-11857, L11859-11871, L11873-11881, L11883-11885, L11887, L11890 and every branch outcome from B11804F through B11884F except B11836c2F (P; unit-tested): parser spans always start at a token, never at a `::` doc line or leading whitespace. B11836c2F (UP): the block is entered only for a blank line or a line starting `::`, so the second `:` check cannot fail.
- `skip_plain_doc_before_equals_slash_start` first-line and EOF edges L11921, B11905F, B11909F, B11916F, B11920T, B11925F (P; unit-tested): they need the `=/`, its doc, or the line above the doc on the first line of the file, where every probe keeps its header comment, or a span starting at EOF.

## Diagnostics only

- `print_noun` L12001-12004, L12006-12007, L12009-12011, L12013-12014, L12016-12017, L12019-12020, L12023-12024, L12026, L12029, L12031, L12037, B12002T/F, B12016T/F, B12016c2T/F; `skip_dbug` L12063, L12065-12067, L12070-12072, L12075-12077, L12079-12081, L12084, L12086, B12075T/F; `diff_noun` L12087-12089, L12091-12093, L12095-12103, L12105-12111, L12113, L12116, L12118, B12091T/F, B12097T/F, B12098T/F, B12105T/F, B12106T/F; `print_context` L12120-12124 (P; unit-tested): noun-diff helpers for the hatch CLI `--test` path and integration tests, never called by the compiler.

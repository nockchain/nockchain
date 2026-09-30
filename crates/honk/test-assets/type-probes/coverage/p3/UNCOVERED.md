# p3 coverage ledger: `crates/hatch/src/utils.rs` 8372-12108

This package covers the atom, knot, path, date, phonetic-name, jam/cue, spec-builder and spot-anchoring code in `crates/hatch/src/utils.rs` lines 8372-12108.

Gaps in the range: lines missed only by unit tests 0, only by the parity corpus 290, by both 178 (468 lines); branch outcomes missed only by unit tests 0, only by parity 153, by both 40 (193 outcomes).

Tags: `U` = unit tests miss it, `P` = the parity corpus misses it, `UP` = both. `B<line>T`/`F` are branch outcomes; `c2`/`c3` mark later operands of `&&`/`||`. Unit tests are in `crates/hatch/src/cov/p3_atoms_specs.rs`; probes are in this directory.

## Rejected by both compilers (P; unit-tested unless noted)

Reject probes in `type-probes/reject/` count toward parity, so these stay uncovered until a reject probe uses the input.

- `path` posh failures L8550 (`/=/=/...` past the file path) and L8560 (`%/=/=/...`).
- `number` bad base58check L9432 (`0c...`) and L9454 (`-0c...`).
- `year_big` BC year before the pivot B9638c2T: hoon-138 `++year` underflows in `sub` and hatch returns a parse error.
- `ind` miss L10210 and `tiq` unknown suffix L10236 (`~zzz`).
- `phonemic_name` `doz` star prefix L10256, B10255T (`~dozzod`), and zero leading word L10269, B10268T (`~dozzod-dozzod`).
- Malformed jams in `~0` blobs (`~00`, `~07`, `~0g0`): `rub_backref` L10917, L10920, L10923, B10916T, B10919T, B10922T; `get_size` L10978, L10989, B10973F, B10977T, B10988T; `cue_inner` L11029, L11041, B11028T, B11040T.
- `unanchor_gap_glued_children` B11556F, B11562F (UP): `?-` or `?+` with no clauses is a syntax error in both.

## Unreachable from source

- Dead code, no callers (UP): `atom_to_char` L9184-9189, L9191, L9195-9196; `dvr_u64` L9289-9291; `w_co` L9332-9341, L9343; `is_leap` L9775-9777; `lsh_u128` L10040-10043, L10045, L10047; `mix` L10396-10398, `mix_big` L10400-10402, `mix_atoms` L10404-10410, L10413; `rol32` L10417-10419; generic `fe` L10538-10610; `noun_hash` L10760-10764.
- No caller outside tests (P; unit-tested): `is_leap_year` L9779, L9781-9782, B9781T/F, B9781c2T/F; `yule` L9784-9788.
- Unused builders (P; unit-tested): `two_specs_closed_tall` L11269-11275, `name_spec_closed_tall` L11337-11345, `one_hoon_closed_tall` L11362-11369, `hoon_with_span` L11523-11531 and the `apply_hoon_trace` it calls, L11386-11389, L11391-11392, L11394, L11396, B11388T/F.
- `ParsedAtom::Big` arms where the value is always small (UP): `trip` L8430 (one byte), `rend_with_rep` aura bytes L8639, L8644, L8980, `yell` 16-bit digit L9669, `fein` after `feis` L10678 (`feis` returns `Small`), `offset_to_atom` L10814, B10811F (a `usize` always fits in `u128`).
- `wood_go` raw-bit fallback L9124 (UP): `path` rejects any knot `rend_crashes` flags before rendering it, so `wood` only sees characters `++wood` accepts.
- `zust` invalid ipv6 L8402 and invalid ipv4 L8409 (UP): `ipv6_address` and `ipv4_address` already produce exactly the group count and digit widths that `ipv6_to_atom` and `ipv4_to_atom` check, so the conversions cannot fail.
- `path` `%%...` posh failure L8569 (P; unit-tested with an empty path): it fails only when the file path is empty, and a build always has one.
- `rend_with_rep` fallback arms L8769, L8775, L8790, L8928-8930, L9006 and `z_co` L9366-9370 (P; unit-tested): `nuck` never produces `%d?` other than `%da`/`%dr`, `%f` above 1, `%i?` other than `%if`/`%is`, `%r?` other than `%rd`/`%rh`/`%rq`/`%rs`, or an unknown aura, so only direct `rend_co` calls reach them.
- `relative_date` `_` unit L9594 (UP): the unit comes from `one_of("dhms")`.
- `year_big` day 0 or month out of range L9644, B9643T, B9643c2T (UP): `absolute_date` only parses days without a leading zero and months 1-12, and `year` passes a fixed valid date.
- Bit helpers with fixed arguments: `bloq_bits` panic L9810, B9809T (P); `met` on `Big(0)` L9829, B9828T (P); `rep` zero step and chunks of 128 bits or more L9867-9868, L9870-9871, B9853c2T, B9862F, B9867T/F (P; callers use bloq 3 or 4 with step 1); `rap` full-width mask and oversized-chunk panic L9901, L9906, B9900T, B9905T (P); `cut` guards L9935, L9940, L9945, L9950, B9934T (P), `bit_len == 0` and Small `bit_start >= 128` L9965, L9971, B9964T, B9970T (UP; both are bounded by the source width), Small mask of 128 bits or more L9988, B9987T (P), and Big masks of 128 bits or more L10008, L10015, L10017-10018, B9996F, B10007T (P); `lsh`/`rsh` overflow L10027, L10035 (P); `end` overflow L10079 (UP); `end_big` overflow L10087 (UP); `rsh_u128`/`end_u128` shifts of 128 bits or more L10052, L10099, L10102, B10051T, B10101T (UP; only called with `(0, 1)`).
- `ins`/`ind` length checks L10182, L10199, B10181T, B10198T (P): `tip` and `tiq` always pass three bytes.
- `muk` blocks and 1- or 3-byte tails L10447-10452, L10454, L10459-10468, L10474-10482, L10491-10498, B10450T/F, B10458T (UP): `eff` always hashes two bytes. `fen` odd round count L10516, B10513F and `fe_u64` odd arm L10634, B10633T (UP): `r` is always 4.
- `feis` re-encrypt L10621, B10620T and `feen` second pass L10660, B10657F (UP): with r = 4, the outputs are at most 0xfffe.ffff, below k = 0xffff.0000.
- `fein` Small-planet and Small-moon arms L10672, L10683, L10689, and the `dis`/`con` helpers that only they and the `fynd_u64` 64-bit arm use, L10377-10379, L10381-10383 (P; unit-tested): parsed planets and moons are `ParsedAtom::Big`.
- `fynd_big` values below 0x1.0000 B10710F, B10715F (UP): parsed planets and larger names are at least 0x1.0000. `fynd_u64` 64-bit arm L10733-10735, B10728c2F, B10732T (P; unit-tested): `fynd_big` only passes 32-bit words.
- jam helpers (UP): `usize_bit_len(0)` L10850, B10849T (`mat_bits` handles a zero atom first); `atom_get_bit` past the atom's width L10875, B10866F, B10871F (`mat_bits` only asks for bits below the width); `bits_to_atom` on no bits L10883, B10882T (jam always emits bits).
- `stack_block_docs_clad::walk` defensive returns L11482, L11485, L11488, L11495, B11481F, B11484F, B11487F, B11491F, B11492F (UP): `map_to_noun` always builds a well-formed treap.
- `attach_help_to_hoon` same-help short cut L11511, B11510T (P; unit-tested): every parser call site checks `hoon_tail_has_help` first.
- `apply_hoon_docs` B11413F (P; unit-tested): docs-only. honk parses entry files with docs off (`parse_build_leaf` in `crates/honk/src/bin/honk.rs`), so no probe reaches postfix doc attachment on a tall `;~`.
- `unanchor_gap_glued_children` `.^` with non-`:*` or empty arguments L11582, B11578F, B11579F (UP): the `.^` parser always builds a non-empty `%cltr`.
- `unanchor_spec_spot` fallthrough L11616 (UP): in traced mode the `?-`/`?+` clause specs come from the traced spec parser, which always wraps them in `%dbug`.
- `unanchor_spot_start` L11626, B11625F, B11631F, B11635F (UP): L11626 and B11625F need a spot line outside the source; B11631F and B11635F need a doc block that runs to EOF with no code after it, but the child always has code.
- `wrap_spec_with_trace` nested spot L11694, B11691F (UP): no spec parser re-wraps a traced spec with a different span.
- `arm_body_start_from_header` L11705, L11723, L11725-11738, L11741-11743, L11746-11766, L11769-11772, L11777 and its branch outcomes B11704T, B11704c2T, B11713F, B11713c3T, B11720F, and every outcome from B11726 through B11769 (P; unit-tested), plus `chumsky_spot_to_hoon_spot` L11967 (P): no parser span starts on a `++`/`+$` header (arm bodies start after the gap), is empty, starts at EOF, or follows tab indentation (hoon gaps have no tabs), so only direct span calls reach it. L11774 (UP) needs a span that ends past the source.
- `non_doc_start_after_leading_doc_span` L11788-11789, L11804, L11806-11817, L11819-11840, L11842-11854, L11856-11864, L11866-11868, L11870, L11873 and every branch outcome from B11787F through B11867F except B11819c2F (P; unit-tested): parser spans always start at a token, never at a `::` doc line or leading whitespace. B11819c2F (UP): the block is entered only for a blank line or a line starting `::`, so the second `:` check cannot fail.
- `skip_plain_doc_before_equals_slash_start` scan body L11934-11935, L11950-11953, L11957, B11930F, B11933T, B11942F, B11945F, B11948F, B11956T (UP): the scan starts on the line of the walked-back `start`, and `expand_gap_start` only walks back to a doccord that anchors, either a comment line or the trailing comment of the code line above the gap. So the first scanned line is the binder's own line (the loop never runs), a code line, or a line that opens with an anchoring doccord, and the loop returns on it before it can see a blank or plain doc line.
- `skip_plain_doc_before_equals_slash_start` empty scan L11956, L11959, B11924F, B11956F (P; unit-tested): `start` lands on the binder's own line only when the span starts at the end of a `=/` line whose trailing larg doc anchors the doc block below it. Parser spans start at tokens, never at a line end.
- `skip_plain_doc_before_equals_slash_start` first-line and EOF edges L11904, B11888F, B11892F, B11899F, B11903T, B11908F (P; unit-tested): they need the `=/`, its doc, or the line above the doc on the first line of the file, where every probe keeps its header comment, or a span starting at EOF.

## Diagnostics only

- `print_noun` L11984-11987, L11989-11990, L11992-11994, L11996-11997, L11999-12000, L12002-12003, L12006-12007, L12009, L12012, L12014, L12020, B11985T/F, B11999T/F, B11999c2T/F; `skip_dbug` L12046, L12048-12050, L12053-12055, L12058-12060, L12062-12064, L12067, L12069, B12058T/F; `diff_noun` L12070-12072, L12074-12076, L12078-12086, L12088-12094, L12096, L12099, L12101, B12074T/F, B12080T/F, B12081T/F, B12088T/F, B12089T/F; `print_context` L12103-12107 (P; unit-tested): noun-diff helpers for the hatch CLI `--test` path and integration tests, never called by the compiler.

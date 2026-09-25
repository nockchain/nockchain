# p4 coverage ledger: noun conversion, the hatch CLI, rune parsers and AST helpers

This package covers `crates/hatch/src/utils.rs` from line 12126 to the end (noun encoding and decoding), `crates/hatch/src/main.rs`, `crates/hatch/src/lib.rs`, `crates/hatch/src/runes/*.rs` (including `runes/cram.rs`) and `crates/hatch/src/ast/hoon.rs`.

Gaps in the range: lines missed only by unit tests 51, only by the parity corpus 902, by both 361 (1314 lines); branch outcomes missed only by unit tests 15, only by parity 176, by both 69 (260 outcomes).

Tags: `U` = unit tests miss it, `P` = the parity corpus misses it, `UP` = both. `B<line>T`/`F` are branch outcomes; `c2`/`c3` mark later operands of `&&`/`||`. Unit tests are in `crates/hatch/src/cov/p4_nouns_runes.rs`; probes are in this directory.

## Docs-only (P; unit-tested)

honk parses entry files with docs off (`parse_build_leaf` in `crates/honk/src/bin/honk.rs`), so postfix `::` doc attachment in the rune parsers never runs for a probe; the unit tests parse with docs on. The gaps: `bar.rs` `bartis` L144, B143T and `barhep` L244-245, L247, B243T, B244T/F; `buc.rs` `buclus` L194, B190T; `cen.rs` `attach_rune_help` L168, B167T, B167c2T/F; `col.rs` `collus` L80, B79T, `colhep` L110, B109T, `colcab` L140, B139T, `colket` L182, L186, B181T, B185T; `ket.rs` `kethep` L219, B218T and `ketlus` L260, B259T; `tis.rs` `tisfas` L180, B179T and `tisdot` L341, B340T; `wut.rs` `wutdot` L303, L311-312, L314, B302T, B310T, B311T/F, `wuthep` L362, B361T, `wutlus` L433, L444-445, L447, L455, B432T, B443T, B444T/F, B452T.

## Rejected by both compilers (P; unit tests assert the errors)

- `main.rs` `spec_wide_parser` L109 (`invalid spec constant`) and `hoon_wide_parser` L197 (`invalid leaf constant`) and L296 (`invalid p in p=q`): hoonc's grammar has the same guards (`+scad` and the `$` leaf forms in `+scat` only accept a `%$` coin, and `p=q` needs a skin).
- `ket.rs` `kettis` multi-limb face L143 and `kettis_wide` L179: honk and hoonc both reject `^=  a.b  5`.
- `wut.rs` `wuthax` L242 and `wuthax_wide` L256: an invalid `?#` pattern.
- `sail.rs` `dedent_block` L301, B300T: a first line indented less than the `"""` (hoon-138 `++inde`).

## Unreachable from source

- `utils.rs` `atom_to_tas_string` L12133-12137, L12139-12142, L12144-12155, L12158-12161, L12164-12166, L12169, L12171 and `biguint_to_ubig` L13116-13118 (UP): dead code, no callers.
- `utils.rs` `hoon_to_noun` cache hit L12238, B12231T (UP): the cache is keyed by AST node address, and a Hoon tree owns each node once, so one materialization never revisits an address.
- `utils.rs` `hoon_to_noun_uncached` `%hand` L12283-12286 (P): the parser never produces `%hand`. `%lost` L12312-12314, `%tune` L12331-12333 (P): these come from desugaring (`+open`), and desugared hoon is never stored as an arm body. `|%` and `|@` with a `Some` prefix L12375-12377, L12426-12428 (P): hoon-138 and hatch both always build `[%brcn ~ ...]`. All unit-covered.
- `utils.rs` `dor_in` L12958, L12963-12971, L12973-12974, L12976, B12957T, B12973T/F (UP): `dor` runs only on a mug tie between two distinct keys, so its equal case never fires, and a tie between two cell keys needs a mug collision no corpus has (`p4_mug_collision` covers the atom case).
- `utils.rs` `map_put_mug` repeated key L13023-13027, B13022T, B13023T/F (P; unit-covered): arm and chapter maps would need two distinct Rust keys that encode to the same atom (`"$"` and `""`), and the parser never emits an empty arm or chapter name; a `:*` doc block that repeats a cuff also reaches it, but only with docs on, and honk parses entry files with docs off.
- `utils.rs` `map_to_noun` B13073F (UP): defensive; `map_put_mug` returns `None` only for a malformed treap, and `map_to_noun` only passes treaps it built.
- `utils.rs` `atom_to_noun` B13100T (UP): the branch requires `n > DIRECT_MAX`, so `n` has nonzero bytes.
- `utils.rs` encoders reached only through the internal `%hand` (P; unit-covered through `%hand` round trips): `opt_to_noun` L13120-13122, L13124-13128, L13131; `type_to_noun` L13158, L13160-13166, L13168-13171, L13173-13176, L13178-13181, L13183-13186, L13188-13193, L13195-13198, L13201; `face_type_to_noun` L13203-13208, L13211; `coil_to_noun` L13213-13216, L13218-13236, L13238-13240; `garb_to_noun` L13242-13247, L13250-13253; `poly_to_noun` L13255-13258, L13260; `vair_to_noun` L13262-13267, L13269; `semi_noun_expr_to_noun` L13271-13275; `stencil_to_noun` L13277-13282, L13284-13287, L13289-13292, L13295; `block_to_noun` L13297-13300; `gate_to_noun` L13307-13311; `nock_to_noun` L13689-13759 (every listed line); `nock_hint_to_noun` L13761-13766, L13769. `term_or_tune_to_noun` L13771-13774, L13776 and `tune_to_noun` L13778-13787, L13790-13792, L13794, L13796, L13798, L13800-13801 are reached only through the internal `%tune`.
- `utils.rs` `spec_to_noun` `Made` L13346-13352, `BucBuc` L13375-13382, `BucDot` L13405-13412, `BucFas` L13434-13441, `BucTic` L13457-13464, `BucZap` L13482-13489 (P; unit-covered): hoon-138 has no `$$`, `$.`, `$/`, `` $` `` or `$!` rune, and `%made` is only built internally.
- `utils.rs` `skin_to_noun` `%dbug` L13507-13510 (P; unit-covered): `flay` strips `%dbug` before building a skin.
- `utils.rs` `note_to_noun` `%made` with wings L13598, L13600-13601, L13605 (P; unit-covered): made notes come from `+ax` synthesis.
- `utils.rs` `chum_to_noun` `VenProVerKel` L13679-13684 (P; unit-covered): the parser maps `%k:v..n` to `VenProKel`, which has the same noun shape as hoonc's result; the four-field form is never built.
- `utils.rs` decoders (P; unit-covered by round trips and malformed-noun tests): honk decodes a hoon noun (`noun_to_hoon` and the decoders under it) only for a `%hold` whose AST it has not cached, in practice arm bodies of the embedded hoon-138 prelude, so decoder arms for forms those bodies do not use cannot be reached from a probe. The gaps: `noun_atom_to_text` L13963, B13960F; `noun_to_list` L14019-14021, B14016F, B14017F; `noun_to_opt` L14083-14084, B14080F, B14081F; `noun_to_basetype` L14097-14098, L14112-14116, B14094F, B14110F, B14113T/F; `noun_to_note` L14147-14148, L14159-14160, L14164, B14146T, B14157T, B14161F; `noun_to_limb` L14194, B14184F; `noun_to_skin` L14219, L14221-14222, L14233, L14235-14236, L14247, L14249-14250, L14259-14264, B14207F, B14218T, B14232T, B14246T, B14253F, B14260T/F; `noun_to_spec` L14296, L14299-14303, L14320, L14322-14323, L14330, L14338, L14340-14341, L14345-14350, L14377-14382, L14385, L14387-14388, L14413-14418, L14421, L14424, L14426-14427, L14438-14443, L14465-14474, B14295T, B14298T, B14319T, B14329T, B14337T, B14344T, B14376T, B14384T, B14412T, B14420T, B14423T, B14437T, B14459F, B14466T/F; `treap_walk` L14495-14496, B14492F, B14493F; `noun_to_chum` L14518-14531, L14533-14536, B14516F, B14522T/F, B14528T/F; `noun_to_term_or_pair` L14542, B14541T; `noun_to_tome` L14568, L14571, B14564F, B14565F; `noun_to_alas` L14583-14585; `noun_to_zpwt_arg` L14595 and `version` L14596-14604, B14599T/F; `noun_to_mane` L14606-14607, L14609-14611, L14613-14614, L14616, B14607T/F; `noun_to_beer` L14618-14619, L14622-14627, L14629-14630, B14619T/F, B14622T/F; `noun_to_mart` L14632-14639; `noun_to_marx` L14641-14642, L14644-14645, L14647; `noun_to_manx` L14649-14650, L14652-14653, L14655; `noun_to_tuna_tail` L14657-14670, L14673, B14660T/F, B14663T/F, B14666T/F, B14669T; `noun_to_tuna` L14675, L14677-14682, L14684-14689, B14677T/F, B14678T/F, B14679T/F, B14680T/F, B14681T/F, B14682T/F; `noun_to_marl` L14691-14693; `noun_to_nock` L14695-14783 (every listed line), B14696T/F; `noun_to_nock_hint` L14785-14789, L14791-14792, L14794, B14786T/F; `noun_to_type` L14796-14849 (every listed line) and B14797T/F through B14842T/F; `noun_to_hoon` tag arms L14893-15666 (every listed line) and B14892T through B15650T/F; `noun_to_term_or_tune` L15669-15672, L15674-15682, B15670T/F.
- `utils.rs` fork-set decode `noun_is_zero_handle` L14030-14035 and `noun_to_fork_set_options` L14037-14041, L14044-14057, L14060-14061, L14063-14069, L14072-14073, B14044T/F, B14046T/F, B14060T/F (P; unit-covered, including the node budget): decoded only inside `%hand` types, and the budget branch is defensive.
- `utils.rs` `noun_to_tuna_tail` unknown tag L14671-14672, B14669F (UP): `noun_to_tuna` calls it only after matching one of the four tags.
- `buc.rs` `bucmic` L460-465 and `bucmic_wide` L467-471 (P; unit-covered): dead in the grammar; `bucmic_wide` is never wired in, and `bucmic` is only called by it.
- `fas.rs` `fastis` L40-50, `fastar` L52-64, `fashax` L66-74 (P; unit-covered): dead in the grammar; never wired into `fas_runes_tall`.
- `bar.rs` `barbuc_wide` L211, B210T (UP): a wide `|$(...)` body never starts a line, and `help_before_body_spec` requires one that does.
- `cen.rs` B145c2F and `col.rs` B47c2F (UP): every caller passes `allow_four_space = false`.
- `ket.rs` `kettis` `Hoon::Limb` face L140 (UP): the parser produces `Hoon::Wing` for names, never `Hoon::Limb`. B148F (UP): docs-only (honk parses entry files with docs off), and in the unit measure the face is already noted by the time `kettis` sees the doc.
- `wut.rs` `wutcol_wide` L184-185, L187, B183T, B184T/F (UP): the closing `)` always follows `r`, so no `::` can come directly after `r`'s span.
- `sail.rs` `collapse_chars` newline L135 (UP): `dedent_block` turns every `Innard::Newline` into a byte before `collapse_chars` runs, and single-line quotes never produce one.
- `sail.rs` `apex` non-element `Top::One` L161 (UP): every `Top::One` from `tall_top` and `wide_top` holds a `Manx`; tuna tails reach the top as `Top::Many`.
- `sail.rs` `innard` B244c2F, B244c3F (UP): malformed input only; the text filter sees `\` or `{` only after an escape or embed fails to parse, and hoon-138 `++quote-innards` also refuses them as text.
- `sail.rs` `dedent_block` L298, B297T (UP): defensive; `block_innards` stops only at a closing `"""` indented exactly as far as the opening one, so the close always matches.
- `cram.rs` `down_item` `Graf::Text` L57 (UP): `down` merges text itself and never passes a `Graf::Text` to `down_item`.
- `cram.rs` `advance_to` L486, B485T (UP): defensive; the machine's offsets never pass the end of the input.
- `cram.rs` `cur_indent` root and heading L523-524 and verse L528 (UP): `back` runs only when no paragraph is open or one just closed, and headings and verse close with their paragraph while the root sits at the markdown's outer column (hoon-138 `++back` notes that poem blocks are handled elsewhere).
- `cram.rs` `close_item` L563, B562F (UP): defensive; every caller has a parent to close into.
- `cram.rs` `close_par` stanza break L614, B613T (UP): each verse stanza closes its own item, so the item has no children yet, as in hoon-138.
- `cram.rs` `entr` L670-671, B669F (UP): defensive; the column never passes `inr` when an item opens.
- `cram.rs` `open_item` `_` L695 (UP): `line` only passes the five opening kinds.
- `cram.rs` `line` L708-709 (UP): a blank line always reads to its newline. L772 (UP): a list is closed or given a new item before a paragraph can start under it (hoon-138 crashes here with `bad-leaf-container`). B789F (UP): a paragraph is continued only while one is open.
- `cram.rs` `collect` L106, B100F, B101F, B104F (UP): only text nodes are named `%$` (sail tag names are `mixed-case-symbol`, never empty), and `down` builds each with one `%$` attribute of char beers and no children, so a node named `%$` never fails the later tests.
- `cram.rs` `run` B811c2F (UP): `line` returns with `pos` still 0 only at an end line (`==`, an outdent, or the end of input) at the first byte, and `cram` always starts after a `gap`, which consumes all whitespace, so that line has no leading spaces and `loc.1 = col` leaves `loc` equal to `start`.

## Measurement artifacts

- `buc.rs` `buclus` L191-192, B191T/F (UP): the `matches!` guard is reported with no counts although its successor L194 runs in the unit tests, so the instrumentation does not attribute counts to it. The identical `%gist` case it checks for also needs an inner spec that already carries the same postfix doc, which the parser never builds.

## Hatch CLI, diagnostics and serialization

- `utils.rs` `diff_and_report` L12126-12131, B12128T/F (P; unit-covered): debug tool for the hatch CLI `--test` path, not on the compile path.
- `utils.rs` `collect_inputs` L15684-15689 and `collect_inputs_inner` L15691-15697, L15702-15703, L15712, L15718, B15692T/F, B15693T/F, B15696T (P; unit-covered): hatch CLI only. L15698-15699, L15704, L15706, L15709, L15715-15716, B15696F (UP): read errors and a bad path call `std::process::exit`, which a test cannot survive.
- `main.rs` L624-972 (every listed line; `Drop`, temp paths, hoonc boot, `run_test`, `run_parser`, `main`) with B625T/F, B658T/F, B661T/F, B682T/F, B715T/F, B775T/F, B869T/F, B874T/F, B902T/F, B933T/F, B953T/F (UP): the hatch binary CLI, private to `main.rs` and not on honk's compile path.
- `ast/hoon.rs` serde for `BigUint` and `Axis`: `serialize_biguint_decimal` L22-24, L26-27, `Axis::serialize` L300-302, L304-306, L308, and `Axis::to_u64` L252-254, whose only caller is that serializer (P; unit-covered): hatch CLI JSON output only. `Axis` `Display` L288-290 (P; unit-covered): used by diagnostics and the hatch CLI only.

## AST helpers off the compile path (`ast/hoon.rs`; P; unit-covered)

- No caller outside tests or dead code: `From<u32>` L36-38, `zero` L89-91, `to_u16_lossy` L112-114, `gt` L137-139, `eq` L143-145 (only the dead generic `fe` calls it), `From<&str>` L149, L151-158, L160, L162, B157T/F, and `BitOr` L168-170, L172, L174, L176, L178 (only the dead generic `fe` uses it).
- `lt` L131-133: only called from a `debug_assert!` in `feis`, which the release build skips.
- `cmp` with a `Big` right-hand side L119-121, L127: the only callers (`fein`'s `ge`/`le`, and `lt`) compare against a `Small` bound.
- `to_u8` and `to_u32` `Big` arms L51, L57: their callers pass bytes and single characters, which are always `Small`.
- `From<u64>` L42-44: used only by `/*` data imports (`pipeline.rs` `data_import_expr`), which no parity probe has.
- `BinaryFloat::sign` L212-216, L218: only the `Infinity` arms of `binaryfloat_mul` and `binaryfloat_div` call it, and `grd_fl` passes them finite operands.

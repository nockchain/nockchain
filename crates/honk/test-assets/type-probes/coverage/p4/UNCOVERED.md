# p4 coverage ledger: noun conversion, the hatch CLI, rune parsers and AST helpers

This package covers `crates/hatch/src/utils.rs` from line 12109 to the end (noun encoding and decoding), `crates/hatch/src/main.rs`, `crates/hatch/src/lib.rs`, `crates/hatch/src/runes/*.rs` (including `runes/cram.rs`) and `crates/hatch/src/ast/hoon.rs`.

Gaps in the range: lines missed only by unit tests 0, only by the parity corpus 890, by both 344 (1234 lines); branch outcomes missed only by unit tests 0, only by parity 174, by both 51 (225 outcomes).

Tags: `U` = unit tests miss it, `P` = the parity corpus misses it, `UP` = both. `B<line>T`/`F` are branch outcomes; `c2`/`c3` mark later operands of `&&`/`||`. Unit tests are in `crates/hatch/src/cov/p4_nouns_runes.rs`; probes are in this directory.

## Docs-only (P; unit-tested)

honk parses entry files with docs off (`parse_build_leaf` in `crates/honk/src/bin/honk.rs`), so postfix `::` doc attachment in the rune parsers never runs for a probe; the unit tests parse with docs on. The gaps: `bar.rs` `bartis` L144, B143T and `barhep` L244-245, L247, B243T, B244T/F; `buc.rs` `buclus` L194, B190T; `cen.rs` `attach_rune_help` L168, B167T, B167c2T/F; `col.rs` `collus` L80, B79T, `colhep` L110, B109T, `colcab` L140, B139T, `colket` L182, L186, B181T, B185T; `ket.rs` `kethep` L219, B218T and `ketlus` L260, B259T; `tis.rs` `tisfas` L180, B179T and `tisdot` L341, B340T; `wut.rs` `wutdot` L303, L311-312, L314, B302T, B310T, B311T/F, `wuthep` L362, B361T, `wutlus` L433, L444-445, L447, L455, B432T, B443T, B444T/F, B452T.

## Rejected by both compilers (P; unit tests assert the errors)

- `main.rs` `spec_wide_parser` L109 (`invalid spec constant`) and `hoon_wide_parser` L197 (`invalid leaf constant`) and L296 (`invalid p in p=q`): hoonc's grammar has the same guards (`+scad` and the `$` leaf forms in `+scat` only accept a `%$` coin, and `p=q` needs a skin).
- `ket.rs` `kettis` multi-limb face L143 and `kettis_wide` L179: honk and hoonc both reject `^=  a.b  5`.
- `wut.rs` `wuthax` L242 and `wuthax_wide` L256: an invalid `?#` pattern.
- `sail.rs` `dedent_block` L301, B300T: a first line indented less than the `"""` (hoon-138 `++inde`).

## Unreachable from source

- `utils.rs` `atom_to_tas_string` L12116-12120, L12122-12125, L12127-12138, L12141-12144, L12147-12149, L12152, L12154 and `biguint_to_ubig` L13099-13101 (UP): dead code, no callers.
- `utils.rs` `hoon_to_noun` cache hit L12221, B12214T (UP): the cache is keyed by AST node address, and a Hoon tree owns each node once, so one materialization never revisits an address.
- `utils.rs` `hoon_to_noun_uncached` `%hand` L12266-12269 (P): the parser never produces `%hand`. `%lost` L12295-12297, `%tune` L12314-12316 (P): these come from desugaring (`+open`), and desugared hoon is never stored as an arm body. `|%` and `|@` with a `Some` prefix L12358-12360, L12409-12411 (P): hoon-138 and hatch both always build `[%brcn ~ ...]`. All unit-covered.
- `utils.rs` `dor_in` L12941, L12946-12954, L12956-12957, L12959, B12940T, B12956T/F (UP): `dor` runs only on a mug tie between two distinct keys, so its equal case never fires, and a tie between two cell keys needs a mug collision no corpus has (`p4_mug_collision` covers the atom case).
- `utils.rs` `map_put_mug` repeated key L13006-13010, B13005T, B13006T/F (P; unit-covered): arm and chapter maps would need two distinct Rust keys that encode to the same atom (`"$"` and `""`), and the parser never emits an empty arm or chapter name; a `:*` doc block that repeats a cuff also reaches it, but only with docs on, and honk parses entry files with docs off.
- `utils.rs` `map_to_noun` B13056F (UP): defensive; `map_put_mug` returns `None` only for a malformed treap, and `map_to_noun` only passes treaps it built.
- `utils.rs` `atom_to_noun` B13083T (UP): the branch requires `n > DIRECT_MAX`, so `n` has nonzero bytes.
- `utils.rs` encoders reached only through the internal `%hand` (P; unit-covered through `%hand` round trips): `opt_to_noun` L13103-13105, L13107-13111, L13114; `type_to_noun` L13141, L13143-13149, L13151-13154, L13156-13159, L13161-13164, L13166-13169, L13171-13176, L13178-13181, L13184; `face_type_to_noun` L13186-13191, L13194; `coil_to_noun` L13196-13199, L13201-13219, L13221-13223; `garb_to_noun` L13225-13230, L13233-13236; `poly_to_noun` L13238-13241, L13243; `vair_to_noun` L13245-13250, L13252; `semi_noun_expr_to_noun` L13254-13258; `stencil_to_noun` L13260-13265, L13267-13270, L13272-13275, L13278; `block_to_noun` L13280-13283; `gate_to_noun` L13290-13294; `nock_to_noun` L13672-13742 (every listed line); `nock_hint_to_noun` L13744-13749, L13752. `term_or_tune_to_noun` L13754-13757, L13759 and `tune_to_noun` L13761-13770, L13773-13775, L13777, L13779, L13781, L13783-13784 are reached only through the internal `%tune`.
- `utils.rs` `spec_to_noun` `Made` L13329-13335, `BucBuc` L13358-13365, `BucDot` L13388-13395, `BucFas` L13417-13424, `BucTic` L13440-13447, `BucZap` L13465-13472 (P; unit-covered): hoon-138 has no `$$`, `$.`, `$/`, `` $` `` or `$!` rune, and `%made` is only built internally.
- `utils.rs` `skin_to_noun` `%dbug` L13490-13493 (P; unit-covered): `flay` strips `%dbug` before building a skin.
- `utils.rs` `note_to_noun` `%made` with wings L13581, L13583-13584, L13588 (P; unit-covered): made notes come from `+ax` synthesis.
- `utils.rs` `chum_to_noun` `VenProVerKel` L13662-13667 (P; unit-covered): the parser maps `%k:v..n` to `VenProKel`, which has the same noun shape as hoonc's result; the four-field form is never built.
- `utils.rs` decoders (P; unit-covered by round trips and malformed-noun tests): honk decodes a hoon noun (`noun_to_hoon` and the decoders under it) only for a `%hold` whose AST it has not cached, in practice arm bodies of the embedded hoon-138 prelude, so decoder arms for forms those bodies do not use cannot be reached from a probe. The gaps: `noun_atom_to_text` L13946, B13943F; `noun_to_list` L14002-14004, B13999F, B14000F; `noun_to_opt` L14066-14067, B14063F, B14064F; `noun_to_basetype` L14080-14081, L14095-14099, B14077F, B14093F, B14096T/F; `noun_to_note` L14130-14131, L14142-14143, L14147, B14129T, B14140T, B14144F; `noun_to_limb` L14177, B14167F; `noun_to_skin` L14202, L14204-14205, L14216, L14218-14219, L14230, L14232-14233, L14242-14247, B14190F, B14201T, B14215T, B14229T, B14236F, B14243T/F; `noun_to_spec` L14279, L14282-14286, L14303, L14305-14306, L14313, L14321, L14323-14324, L14328-14333, L14360-14365, L14368, L14370-14371, L14396-14401, L14404, L14407, L14409-14410, L14421-14426, L14448-14457, B14278T, B14281T, B14302T, B14312T, B14320T, B14327T, B14359T, B14367T, B14395T, B14403T, B14406T, B14420T, B14442F, B14449T/F; `treap_walk` L14478-14479, B14475F, B14476F; `noun_to_chum` L14501-14514, L14516-14519, B14499F, B14505T/F, B14511T/F; `noun_to_term_or_pair` L14525, B14524T; `noun_to_tome` L14551, L14554, B14547F, B14548F; `noun_to_alas` L14566-14568; `noun_to_zpwt_arg` L14578 and `version` L14579-14587, B14582T/F; `noun_to_mane` L14589-14590, L14592-14594, L14596-14597, L14599, B14590T/F; `noun_to_beer` L14601-14602, L14605-14610, L14612-14613, B14602T/F, B14605T/F; `noun_to_mart` L14615-14622; `noun_to_marx` L14624-14625, L14627-14628, L14630; `noun_to_manx` L14632-14633, L14635-14636, L14638; `noun_to_tuna_tail` L14640-14653, L14656, B14643T/F, B14646T/F, B14649T/F, B14652T; `noun_to_tuna` L14658, L14660-14665, L14667-14672, B14660T/F, B14661T/F, B14662T/F, B14663T/F, B14664T/F, B14665T/F; `noun_to_marl` L14674-14676; `noun_to_nock` L14678-14766 (every listed line), B14679T/F; `noun_to_nock_hint` L14768-14772, L14774-14775, L14777, B14769T/F; `noun_to_type` L14779-14832 (every listed line) and B14780T/F through B14825T/F; `noun_to_hoon` tag arms L14876-15649 (every listed line) and B14875T through B15633T/F; `noun_to_term_or_tune` L15652-15655, L15657-15665, B15653T/F.
- `utils.rs` fork-set decode `noun_is_zero_handle` L14013-14018 and `noun_to_fork_set_options` L14020-14024, L14027-14040, L14043-14044, L14046-14052, L14055-14056, B14027T/F, B14029T/F, B14043T/F (P; unit-covered, including the node budget): decoded only inside `%hand` types, and the budget branch is defensive.
- `utils.rs` `noun_to_tuna_tail` unknown tag L14654-14655, B14652F (UP): `noun_to_tuna` calls it only after matching one of the four tags.
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

- `utils.rs` `diff_and_report` L12109-12114, B12111T/F (P; unit-covered): debug tool for the hatch CLI `--test` path, not on the compile path.
- `utils.rs` `collect_inputs` L15667-15672 and `collect_inputs_inner` L15674-15680, L15685-15686, L15695, L15701, B15675T/F, B15676T/F, B15679T (P; unit-covered): hatch CLI only. L15681-15682, L15687, L15689, L15692, L15698-15699, B15679F (UP): read errors and a bad path call `std::process::exit`, which a test cannot survive.
- `main.rs` L624-972 (every listed line; `Drop`, temp paths, hoonc boot, `run_test`, `run_parser`, `main`) with B625T/F, B658T/F, B661T/F, B682T/F, B715T/F, B775T/F, B869T/F, B874T/F, B902T/F, B933T/F, B953T/F (UP): the hatch binary CLI, private to `main.rs` and not on honk's compile path.
- `ast/hoon.rs` serde for `BigUint` and `Axis`: `serialize_biguint_decimal` L22-24, L26-27, `Axis::serialize` L300-302, L304-306, L308, and `Axis::to_u64` L252-254, whose only caller is that serializer (P; unit-covered): hatch CLI JSON output only. `Axis` `Display` L288-290 (P; unit-covered): used by diagnostics and the hatch CLI only.

## AST helpers off the compile path (`ast/hoon.rs`; P; unit-covered)

- No caller outside tests or dead code: `From<u32>` L36-38, `zero` L89-91, `to_u16_lossy` L112-114, `gt` L137-139, `eq` L143-145 (only the dead generic `fe` calls it), `From<&str>` L149, L151-158, L160, L162, B157T/F, and `BitOr` L168-170, L172, L174, L176, L178 (only the dead generic `fe` uses it).
- `lt` L131-133: only called from a `debug_assert!` in `feis`, which the release build skips.
- `cmp` with a `Big` right-hand side L119-121, L127: the only callers (`fein`'s `ge`/`le`, and `lt`) compare against a `Small` bound.
- `to_u8` and `to_u32` `Big` arms L51, L57: their callers pass bytes and single characters, which are always `Small`.
- `From<u64>` L42-44: used only by `/*` data imports (`pipeline.rs` `data_import_expr`), which no parity probe has.
- `BinaryFloat::sign` L212-216, L218: only the `Infinity` arms of `binaryfloat_mul` and `binaryfloat_div` call it, and `grd_fl` passes them finite operands.

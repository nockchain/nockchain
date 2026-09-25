//! Coverage-driven tests for the honk CLI.
//!
//! Added to close branch-coverage gaps; see the coverage report in the PR.

use std::sync::atomic::AtomicU64;

use hatch::ast::hoon::{Note, Pint, Spot};

#[allow(unused_imports)]
use super::*;

/// A throwaway directory tree.
struct Tree {
    dir: tempfile::TempDir,
}

impl Tree {
    fn new() -> Self {
        Self {
            dir: tempfile::tempdir().expect("temp dir"),
        }
    }

    fn root(&self) -> &Path {
        self.dir.path()
    }

    fn path(&self, rel: &str) -> PathBuf {
        self.dir.path().join(rel)
    }

    fn write(&self, rel: &str, contents: impl AsRef<[u8]>) -> PathBuf {
        let path = self.path(rel);
        fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");
        fs::write(&path, contents).expect("write");
        path
    }
}

/// A scratch directory under the working directory, for cwd-relative paths.
struct CwdScratch {
    rel: PathBuf,
    abs: PathBuf,
}

impl CwdScratch {
    fn new(tag: &str) -> Self {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let rel = PathBuf::from(format!(".cov-c6-bin-{tag}-{}-{nanos}", process::id()));
        let abs = env::current_dir().expect("cwd").join(&rel);
        fs::create_dir_all(&abs).expect("scratch");
        Self { rel, abs }
    }
}

impl Drop for CwdScratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.abs);
    }
}

fn components(path: &Path) -> Vec<String> {
    normal_path_components(path)
}

fn parse(source: &str) -> Hoon {
    pipeline::parse_native_hoon_source(Path::new("cov.hoon"), source, Vec::new(), false)
        .expect("test source parses")
}

fn repo_hoon() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../hoon")
}

fn cli_for(dbug: bool, vet: bool) -> Cli {
    Cli {
        entry: None,
        directory: PathBuf::from("deps"),
        output: None,
        prelude: PathBuf::from("hoon.hoon"),
        sut_jam: None,
        mode: CompileMode::Standard,
        batch_manifest: None,
        wrapper_asset_dump: None,
        native_wrapper_asset_dump: None,
        cache_dir: None,
        fresh: false,
        dbug,
        vet,
    }
}

// ---------------------------------------------------------------------------
// Argument helpers

#[test]
fn compile_modes_from_flags_and_manifest_names() {
    assert_eq!(
        CompileMode::from_flags(false, false, false),
        Ok(CompileMode::Standard)
    );
    assert_eq!(
        CompileMode::from_flags(true, false, false),
        Ok(CompileMode::Arbitrary)
    );
    assert_eq!(
        CompileMode::from_flags(false, true, false),
        Ok(CompileMode::Dynock)
    );
    assert_eq!(
        CompileMode::from_flags(false, false, true),
        Ok(CompileMode::DynockTyped)
    );
    for flags in [(true, true, false), (false, true, true), (true, true, true)] {
        let err = CompileMode::from_flags(flags.0, flags.1, flags.2).expect_err("exclusive");
        assert!(err.contains("mutually exclusive"), "{err}");
    }
    for (name, mode) in [
        ("standard", CompileMode::Standard),
        ("arbitrary", CompileMode::Arbitrary),
        ("dynock", CompileMode::Dynock),
        ("dynock-typed", CompileMode::DynockTyped),
    ] {
        assert_eq!(CompileMode::parse(name), Ok(mode));
    }
    assert_eq!(
        CompileMode::parse("typed"),
        Err("unknown batch compile mode: typed".to_string())
    );
    let text = usage("honk-x");
    assert_eq!(text.matches("Usage: honk-x").count(), 6);
    assert!(text.contains("--batch-manifest <file>"));
}

#[test]
fn batch_manifests_parse_modes_and_directory_lists() {
    let tree = Tree::new();
    let manifest = tree.write(
        "batch.tsv",
        "\n  a.jam\ta.hoon\tarbitrary  \nb.jam\tb.hoon\tstandard\t/x/one.hoon\t/x/two.jam\n\nc.jam\tc.hoon\tdynock-typed\n",
    );
    let entries = parse_batch_manifest(&manifest).expect("manifest parses");
    assert_eq!(entries.len(), 3);
    assert_eq!(entries[0].output, PathBuf::from("a.jam"));
    assert_eq!(entries[0].entry, PathBuf::from("a.hoon"));
    assert_eq!(entries[0].mode, CompileMode::Arbitrary);
    assert!(entries[0].directory_files.is_none());
    assert_eq!(entries[1].mode, CompileMode::Standard);
    assert_eq!(
        entries[1].directory_files.as_deref(),
        Some(&[PathBuf::from("/x/one.hoon"), PathBuf::from("/x/two.jam")][..])
    );
    assert_eq!(entries[2].mode, CompileMode::DynockTyped);

    for (contents, expected) in [
        (
            "a.jam\ta.hoon\n", ":1: expected tab-separated output, entry, mode",
        ),
        (
            "\na.jam\ta.hoon\tbogus\n", "unknown batch compile mode: bogus",
        ),
        ("\n   \n", "batch manifest has no entries"),
    ] {
        let manifest = tree.write("bad.tsv", contents);
        let err = parse_batch_manifest(&manifest).expect_err("bad manifest");
        assert!(err.to_string().contains(expected), "{err}");
    }
    assert!(parse_batch_manifest(&tree.path("missing.tsv")).is_err());
}

// ---------------------------------------------------------------------------
// Wrapper sources and batteries

fn source_line(source: &str, line: u64) -> &str {
    source.lines().nth(line as usize - 1).expect("line exists")
}

/// The text a 1-based, end-exclusive single-line spot covers.
fn spot_text(source: &str, start: SourcePosition, end: SourcePosition) -> &str {
    assert_eq!(start.0, end.0);
    &source_line(source, start.0)[start.1 as usize - 1..end.1 as usize - 1]
}

/// The outermost `%spot` of a hoonc-shaped battery `[11 [%spot [1 spot]] f]`.
fn battery_spot(battery: Noun, space: &NounSpace) -> Spot_ {
    let cell = battery.in_space(space).as_cell().expect("battery cell");
    assert_eq!(atom_u64(cell.head().noun(), space), Some(11));
    let hint = cell
        .tail()
        .as_cell()
        .expect("hint")
        .head()
        .as_cell()
        .expect("spot hint");
    assert_eq!(
        type_tag_atom_text(hint.head().noun(), space).as_deref(),
        Some("spot")
    );
    let constant = hint.tail().as_cell().expect("constant spot");
    assert_eq!(atom_u64(constant.head().noun(), space), Some(1));
    decode_spot_noun(constant.tail().noun(), space).expect("spot decodes")
}

type Spot_ = super::Spot;

#[test]
fn wrapper_sources_align_with_native_battery_spots() {
    assert_eq!(hoonc_wrapper_source(3, "x"), "\n\nx\n");
    assert_eq!(hoonc_wrapper_source(1, "x\n"), "x\n");
    assert_eq!(hoonc_wrapper_source(0, "y"), "y\n");
    assert_eq!(hoonc_wrapper_wer(), ["apps", "hoonc", "hoonc.hoon"]);

    let standard = hoonc_standard_output_source();
    assert_eq!(source_line(&standard, 556), "=<");
    assert_eq!(source_line(&standard, 1070), "|%");
    assert_eq!(source_line(&standard, 1071), "++  shot");
    let dir_hash = hoonc_dir_hash_source();
    assert_eq!(source_line(&dir_hash, 535), "=<");
    assert_eq!(source_line(&dir_hash, 562), "|%");
    assert!(dir_hash.ends_with("--\n"));

    // The natively built batteries carry the spots hoonc gives these lines.
    let mut slab: NounSlab = NounSlab::new();
    let (batteries, empty_trap_vase) =
        construct_exact_wrapper_batteries(&mut slab).expect("batteries");
    let space = slab.noun_space();
    let spot = battery_spot(batteries.dir_hash_vase, &space);
    assert_eq!(spot.0, "apps/hoonc/hoonc.hoon");
    assert_eq!(spot_text(&standard, spot.1, spot.2), "d");
    let spot = battery_spot(batteries.value_trap_standard, &space);
    assert_eq!(spot_text(&standard, spot.1, spot.2), "+:^$");
    let spot = battery_spot(batteries.shot, &space);
    assert_eq!(
        spot_text(&standard, spot.1, spot.2),
        "[typ .*([q:$:gat q:$:sam] [%9 2 %10 [6 %0 3] %0 2])]"
    );
    // The empty trap vase is [battery [[%atom %n [~ 0]] 0]].
    let empty = empty_trap_vase.in_space(&space).as_cell().expect("trap");
    let vase = empty.tail().as_cell().expect("vase");
    assert_eq!(atom_u64(vase.tail().noun(), &space), Some(0));
    let ty = vase.head().as_cell().expect("type");
    assert_eq!(
        type_tag_atom_text(ty.head().noun(), &space).as_deref(),
        Some("atom")
    );
    // Batteries that only the dynamic wrappers produce stay empty.
    for unused in [
        batteries.dir_hash, batteries.mint_gun, batteries.swet_gate, batteries.shot_gun,
        batteries.standard_output,
    ] {
        assert!(noun_is_zero(unused));
    }
    let empty = ExactWrapperBatteries::empty();
    assert!(noun_is_zero(empty.slat) && noun_is_zero(empty.value_trap_arbitrary));
}

#[test]
fn jam_padding_rounds_up_to_words() {
    for (len, padded) in [(0, 0), (3, 8), (8, 8), (9, 16)] {
        let mut jam = vec![7u8; len];
        pad_hoonc_jam_atom_bytes(&mut jam);
        assert_eq!(jam.len(), padded);
        assert!(jam[len..].iter().all(|byte| *byte == 0));
    }
}

#[test]
fn local_types_and_trap_shapes() {
    let mut slab: NounSlab = NounSlab::new();
    let octs = local_octs_type(&mut slab);
    let empty_aura = ty_atom_local(&mut slab, "", None);
    let dollar = ty_atom_local(&mut slab, "$", None);
    let battery = hoonc_data_octs_trap_battery(&mut slab);
    let pair = T(&mut slab, &[D(5), D(6)]);
    let atom_payload = T(&mut slab, &[D(1), D(2)]);
    let atom_gun = T(&mut slab, &[D(1), atom_payload]);
    let good_gun = T(&mut slab, &[D(3), D(4)]);
    let good_payload = T(&mut slab, &[good_gun, D(0)]);
    let good_trap = T(&mut slab, &[D(1), good_payload]);
    let space = slab.noun_space();

    // [%cell [%face %p [%atom %ud ~]] [%face %q [%atom %$ ~]]]
    let (head, tail) = type_cell_parts(octs, &space).expect("cell type");
    // `%$` is the empty term, atom 0.
    for (face_ty, face, aura) in [(head, "p", Some("ud")), (tail, "q", None)] {
        let face_cell = face_ty.in_space(&space).as_cell().expect("face");
        assert_eq!(
            type_tag_atom_text(face_cell.head().noun(), &space).as_deref(),
            Some("face")
        );
        let rest = face_cell.tail().as_cell().expect("face payload");
        assert_eq!(atom_text(rest.head().noun(), &space).as_deref(), Some(face));
        let atom_ty = rest.tail().as_cell().expect("atom type");
        let aura_noun = atom_ty.tail().as_cell().expect("aura").head().noun();
        assert_eq!(atom_text(aura_noun, &space).as_deref(), aura);
    }
    assert!(noun_eq(empty_aura, dollar, &space).expect("eq"));

    // The canonical data battery is `[11 [%spot ..] [0 3]]` at hoonc.hoon 988.
    let spot = battery_spot(battery, &space);
    assert_eq!(
        spot,
        ("apps/hoonc/hoonc.hoon".to_string(), (988, 10), (988, 14))
    );

    assert!(trap_battery(D(3), &space).is_err());
    assert_eq!(
        atom_u64(trap_battery(pair, &space).expect("battery"), &space),
        Some(5)
    );
    for bad in [D(0), pair, atom_gun] {
        assert!(swet_trap_payload_gun(bad, &space).is_err());
    }
    let (ty, formula) = swet_trap_payload_gun(good_trap, &space).expect("gun");
    assert_eq!(
        (atom_u64(ty, &space), atom_u64(formula, &space)),
        (Some(3), Some(4))
    );
    assert_eq!(
        atom_u64(
            swet_trap_payload_type(good_trap, &space).expect("type"),
            &space
        ),
        Some(3)
    );
}

#[test]
fn dynock_output_wraps_formula_in_a_constant_trap() {
    let mut slab: NounSlab = NounSlab::new();
    let mut ut = Ut::new(&mut slab);
    let ty = ty_atom_local(&mut *ut.slab, "ud", None);
    let formula = T(&mut *ut.slab, &[D(1), D(42)]);
    let untyped = jam_dynock_output_native(&mut ut, ty, formula, false);
    let ty = ty_atom_local(&mut *ut.slab, "ud", None);
    let formula = T(&mut *ut.slab, &[D(1), D(42)]);
    let typed = jam_dynock_output_native(&mut ut, ty, formula, true);

    let expected = |typed: bool| {
        let mut slab: NounSlab = NounSlab::new();
        let header = if typed {
            ty_atom_local(&mut slab, "ud", None)
        } else {
            ty_noun(&mut slab)
        };
        let formula = T(&mut slab, &[D(1), D(42)]);
        let battery = T(&mut slab, &[D(1), formula]);
        let trap = T(&mut slab, &[battery, D(0)]);
        let root = T(&mut slab, &[header, trap]);
        jam_slab_noun(&mut slab, root)
    };
    assert_eq!(untyped, expected(false));
    assert_eq!(typed, expected(true));

    let mut other: NounSlab = NounSlab::new();
    let noun = T(&mut other, &[D(1), D(2)]);
    let space = other.noun_space();
    let fresh = jam_noun_in_fresh_slab(noun, &space);
    assert_eq!(fresh, jam_slab_noun(&mut other, noun));
}

// ---------------------------------------------------------------------------
// Prelude helpers

#[test]
fn prelude_peeling_and_variant_names() {
    let spot = Spot {
        p: vec!["a".to_string()],
        q: Pint {
            p: (1, 1),
            q: (1, 2),
        },
    };
    let leaf = Hoon::Axis(1u64.into());
    let boxed = || Box::new(Hoon::Axis(1u64.into()));
    let wrapped = Hoon::TisSig(vec![Hoon::Dbug(
        spot.clone(),
        Box::new(Hoon::Note(Note::Know("k".to_string()), boxed())),
    )]);
    assert_eq!(peel_transparent(&wrapped), &leaf);
    let two = Hoon::TisSig(vec![leaf.clone(), leaf.clone()]);
    assert_eq!(peel_transparent(&two), &two);

    let named = [
        (Hoon::TisGal(boxed(), boxed()), "TisGal(=<)"),
        (Hoon::TisGar(boxed(), boxed()), "TisGar(=>)"),
        (Hoon::Dbug(spot, boxed()), "Dbug"),
        (Hoon::Note(Note::Know("k".to_string()), boxed()), "Note"),
        (Hoon::CenDot(boxed(), boxed()), "CenDot"),
        (Hoon::CenHep(boxed(), boxed()), "CenHep"),
        (Hoon::CenCol(boxed(), vec![]), "CenCol"),
        (
            Hoon::CenSig(vec![Limb::Term("$".to_string())], boxed(), vec![]),
            "CenSig",
        ),
        (Hoon::KetSig(boxed()), "KetSig"),
        (Hoon::KetBar(boxed()), "KetBar"),
        (Hoon::KetDot(boxed(), boxed()), "KetDot"),
        (Hoon::BarCen(None, HashMap::new()), "BarCen(|%)"),
        (Hoon::TisCom(boxed(), boxed()), "TisCom"),
        (Hoon::TisHep(boxed(), boxed()), "TisHep"),
        (Hoon::TisLus(boxed(), boxed()), "TisLus"),
        (Hoon::Pair(boxed(), boxed()), "Pair"),
        (leaf, "other"),
    ];
    for (hoon, name) in named {
        assert_eq!(prelude_variant_name(&hoon), name);
    }
}

#[test]
fn small_preludes_seed_mint_and_evaluate() {
    let prelude = parse("[1 2]");
    let mut context = create_eval_context();

    // Seeding plays the prelude unless its type will be overwritten.
    let mut slab: NounSlab = NounSlab::new();
    let mut ut = Ut::new(&mut slab);
    let played = seed_honc_type_with_ut(&mut ut, &mut context, &prelude, false).expect("play");
    let skipped = seed_honc_type_with_ut(&mut ut, &mut context, &prelude, true).expect("skip");
    let (ty, formula) =
        mint_honc_formula_with_ut(&mut ut, &mut context, &prelude).expect("plain mint");
    let space = ut.slab.noun_space();
    let empty = {
        let mut other: NounSlab = NounSlab::new();
        let empty = empty_subject_type(&mut other);
        jam_slab_noun(&mut other, empty)
    };
    assert_eq!(jam_noun_in_fresh_slab(skipped.ty, &space), empty);
    assert!(played.eval_value.is_none() && noun_is_zero(played.trap));
    for ty in [played.ty, ty] {
        let cell = ty.in_space(&space).as_cell().expect("pair type");
        assert_eq!(
            type_tag_atom_text(cell.head().noun(), &space).as_deref(),
            Some("cell")
        );
    }
    assert!(formula.is_cell());

    // Only a `=<` prelude can be minted in chunks.
    let mut out: NounSlab = NounSlab::new();
    let err = mint_honc_prelude_chunked(&mut out, &prelude).expect_err("not =<");
    assert!(err.to_string().contains("root is not =<"), "{err}");

    // The isolated evaluation mints and runs the prelude against `~`.
    let mut formula_slab: NounSlab = NounSlab::new();
    let evaluated =
        evaluate_honc_isolated(&mut context, &prelude, &mut formula_slab).expect("evaluate");
    let stack_space = context.stack.noun_space();
    let value = evaluated
        .value
        .in_space(&stack_space)
        .as_cell()
        .expect("pair");
    assert_eq!(atom_u64(value.head().noun(), &stack_space), Some(1));
    assert_eq!(atom_u64(value.tail().noun(), &stack_space), Some(2));
    assert!(evaluated.formula.is_cell());
}

#[test]
fn formula_evaluation_reports_crashes() {
    let mut context = create_eval_context();
    let mut slab: NounSlab = NounSlab::new();
    let increment = {
        let slot = T(&mut slab, &[D(0), D(1)]);
        T(&mut slab, &[D(4), slot])
    };
    let crash = T(&mut slab, &[D(0), D(0)]);
    let space = slab.noun_space();
    let value = eval_formula_noun_in_context(&mut context, increment, &space, "inc", |_| D(41))
        .expect("increment");
    assert_eq!(atom_u64(value, &context.stack.noun_space()), Some(42));
    let err = eval_formula_noun_in_context(&mut context, crash, &space, "crash", |_| D(0))
        .expect_err("crash");
    assert!(err.to_string().contains("interpret formula"), "{err}");

    // A crash under a spot hint leaves a spot frame in the trace, which the
    // trace printer decodes.
    let mut decoded = None;
    unsafe {
        context.with_stack_frame(0, |context| {
            let spot_tag = term_to_noun(&mut context.stack, "spot");
            let path_head = term_to_noun(&mut context.stack, "cov");
            let path = T(&mut context.stack, &[path_head, D(0)]);
            let start = T(&mut context.stack, &[D(3), D(4)]);
            let end = T(&mut context.stack, &[D(5), D(6)]);
            let pint = T(&mut context.stack, &[start, end]);
            let spot = T(&mut context.stack, &[path, pint]);
            let clue = T(&mut context.stack, &[D(1), spot]);
            let hint = T(&mut context.stack, &[spot_tag, clue]);
            let body = T(&mut context.stack, &[D(0), D(0)]);
            let formula = T(&mut context.stack, &[D(11), hint, body]);
            let err = interpret(context, D(0), formula).expect_err("crash");
            trace_interpret_error(context, &err);
            let trace = match &err {
                NockError::Deterministic(_, trace)
                | NockError::NonDeterministic(_, trace)
                | NockError::ScryCrashed(trace) => *trace,
                NockError::ScryBlocked(path) => *path,
            };
            let space = context.stack.noun_space();
            decoded = decode_spot_trace(trace, &space);
        });
    }
    assert_eq!(decoded.as_deref(), Some("cov:3:4-5:6"));

    // A crash with no spot frame has nothing to decode.
    let mut decoded = Some(String::new());
    unsafe {
        context.with_stack_frame(0, |context| {
            let formula = T(&mut context.stack, &[D(0), D(0)]);
            let err = interpret(context, D(0), formula).expect_err("crash");
            trace_interpret_error(context, &err);
            if let NockError::Deterministic(_, trace) = err {
                let space = context.stack.noun_space();
                decoded = decode_spot_trace(trace, &space);
            }
        });
    }
    assert_eq!(decoded, None);
}

// ---------------------------------------------------------------------------
// Subject-type and trace decoding

#[test]
fn subject_type_jams_decode_or_are_rejected() {
    let mut slab: NounSlab = NounSlab::new();
    let subject = cue_subject_type_to_slab(&mut slab, EMBEDDED_HONC_TYPE_138_JAM).expect("cue");
    let space = slab.noun_space();
    let prelude_ty = prelude_type_from_subject_type(subject, &space).expect("prelude type");
    assert!(looks_like_type_noun(prelude_ty, &space));

    let jam = |build: &dyn Fn(&mut NounSlab) -> Noun| {
        let mut slab: NounSlab = NounSlab::new();
        let root = build(&mut slab);
        jam_slab_noun(&mut slab, root)
    };
    // A bare type tag is a type, and so is the head of a [type extra] pair.
    let bare = jam(&|slab| term_to_noun(slab, "void"));
    let wrapped = jam(&|slab| {
        let atom = term_to_noun(slab, "atom");
        let ud = term_to_noun(slab, "ud");
        let ty = T(slab, &[atom, ud, D(0)]);
        T(slab, &[ty, D(9)])
    });
    let mut slab: NounSlab = NounSlab::new();
    let ty = cue_subject_type_to_slab(&mut slab, &bare).expect("bare type");
    assert_eq!(
        type_tag_atom_text(ty, &slab.noun_space()).as_deref(),
        Some("void")
    );
    let ty = cue_subject_type_to_slab(&mut slab, &wrapped).expect("head type");
    let space = slab.noun_space();
    let head = ty.in_space(&space).as_cell().expect("type").head().noun();
    assert_eq!(type_tag_atom_text(head, &space).as_deref(), Some("atom"));
    for bad in [
        jam(&|_| D(1)),
        jam(&|slab| {
            let inner = T(slab, &[D(1), D(2)]);
            T(slab, &[inner, D(3)])
        }),
    ] {
        let err = cue_subject_type_to_slab(&mut slab, &bad).expect_err("not a type");
        assert!(err.to_string().contains("did not contain a Hoon type noun"));
    }
    // Well-formed jams cue onto a stack as well as into a slab.
    let mut stack = NockStack::new(1 << 16, 0);
    let cued = cue_bytes_to_stack(&mut stack, &bare).expect("cue");
    assert_eq!(
        type_tag_atom_text(cued, &stack.noun_space()).as_deref(),
        Some("void")
    );
    let cued = cue_noun_to_slab(&mut slab, &wrapped, "pair").expect("cue");
    assert!(cued.is_cell());

    // Type-shape predicates.
    let mut slab: NounSlab = NounSlab::new();
    let void = term_to_noun(&mut slab, "void");
    let foo = term_to_noun(&mut slab, "foo");
    let upper = term_to_noun(&mut slab, "ABC");
    let digits = term_to_noun(&mut slab, "a1-b");
    let core_tag = term_to_noun(&mut slab, "core");
    let core = T(&mut slab, &[core_tag, D(0), D(0)]);
    let odd = T(&mut slab, &[foo, D(0)]);
    let headless_inner = T(&mut slab, &[D(1), D(2)]);
    let headless = T(&mut slab, &[headless_inner, D(0)]);
    let face_tag = term_to_noun(&mut slab, "face");
    let faced = T(&mut slab, &[face_tag, D(0), D(0)]);
    let cell_tag = term_to_noun(&mut slab, "cell");
    let short_cell = T(&mut slab, &[cell_tag, D(0)]);
    let space = slab.noun_space();
    assert!(looks_like_type_noun(void, &space));
    assert!(!looks_like_type_noun(foo, &space));
    assert!(!looks_like_type_noun(D(0), &space));
    assert!(looks_like_type_noun(core, &space));
    assert!(!looks_like_type_noun(odd, &space));
    assert!(!looks_like_type_noun(headless, &space));
    assert_eq!(type_tag_atom_text(upper, &space), None);
    assert_eq!(type_tag_atom_text(digits, &space).as_deref(), Some("a1-b"));
    assert_eq!(type_tag_atom_text(D(0), &space), None);
    assert_eq!(type_tag_atom_text(headless, &space), None);
    assert!(type_cell_parts(D(0), &space).is_err());
    let err = type_cell_parts(faced, &space).expect_err("not a cell type");
    assert!(err.to_string().contains("expected %cell type, got %face"));
    assert!(type_cell_parts(short_cell, &space).is_err());
    assert!(prelude_type_from_subject_type(faced, &space).is_err());
}

#[test]
fn spot_traces_and_cache_tuples_decode() {
    let mut slab: NounSlab = NounSlab::new();
    let spot_tag = term_to_noun(&mut slab, "spot");
    let name = term_to_noun(&mut slab, "a.hoon");
    let dir = term_to_noun(&mut slab, "app");
    let tail = T(&mut slab, &[name, D(0)]);
    let path = T(&mut slab, &[dir, tail]);
    let start = T(&mut slab, &[D(1), D(2)]);
    let end = T(&mut slab, &[D(3), D(4)]);
    let pint = T(&mut slab, &[start, end]);
    let spot = T(&mut slab, &[path, pint]);
    let hunk = T(&mut slab, &[spot_tag, spot]);
    let list = T(&mut slab, &[hunk, D(0)]);
    let other_tag = term_to_noun(&mut slab, "mean");
    let other = T(&mut slab, &[other_tag, spot]);
    let bad_path = T(&mut slab, &[D(0), D(5)]);
    let bad_spot = T(&mut slab, &[bad_path, pint]);
    let bad_pint = T(&mut slab, &[D(1), D(2)]);
    let partial_spot = T(&mut slab, &[path, bad_pint]);
    let triple = T(&mut slab, &[D(1), D(2), D(3)]);
    let big = Atom::new(&mut slab, u64::MAX).as_noun();
    let wide = T(&mut slab, &[big, big]);
    let space = slab.noun_space();

    let expected = Some("app/a.hoon:1:2-3:4".to_string());
    assert_eq!(decode_spot_trace(list, &space), expected);
    assert_eq!(decode_spot_trace(hunk, &space), expected);
    assert_eq!(decode_spot_trace(other, &space), None);
    assert_eq!(decode_spot_trace(D(7), &space), None);
    assert_eq!(decode_spot_noun(bad_spot, &space), None);
    assert_eq!(decode_spot_noun(partial_spot, &space), None);
    assert_eq!(decode_spot_noun(D(0), &space), None);
    assert_eq!(decode_path_noun(D(3), &space), None);
    assert_eq!(atom_text(D(0), &space), None);
    assert_eq!(atom_text(spot, &space), None);
    assert_eq!(atom_u64(big, &space), Some(u64::MAX));
    assert_eq!(atom_u64(wide, &space), None);

    assert!(cache_tuple_fields(triple, 1, &space).is_err());
    let fields = cache_tuple_fields(triple, 3, &space).expect("three fields");
    let values: Vec<Option<u64>> = fields.iter().map(|f| atom_u64(*f, &space)).collect();
    assert_eq!(values, [Some(1), Some(2), Some(3)]);
    assert!(cache_tuple_fields(triple, 4, &space).is_err());

    let mut stack = NockStack::new(1 << 16, 0);
    let pair = T(&mut stack, &[D(1), D(2)]);
    let nested_head = T(&mut stack, &[D(1), D(2)]);
    let nested = T(&mut stack, &[nested_head, D(0)]);
    assert!(unsafe { normalize_interpret_trace(&mut stack, D(5)).raw_equals(&D(5)) });
    assert!(unsafe { normalize_interpret_trace(&mut stack, nested).raw_equals(&nested) });
    let normalized = normalize_interpret_trace(&mut stack, pair);
    let stack_space = stack.noun_space();
    let normalized = normalized.in_space(&stack_space).as_cell().expect("list");
    assert!(unsafe { normalized.head().noun().raw_equals(&pair) });
    assert_eq!(atom_u64(normalized.tail().noun(), &stack_space), Some(0));
}

#[test]
fn nock_panics_become_errors() {
    assert_eq!(catch_nock_panic("ok", || Ok(5)).expect("ok"), 5);
    let err = catch_nock_panic("owned", || -> Result<()> {
        panic!("{}", String::from("boom"))
    })
    .expect_err("panic");
    assert_eq!(err.to_string(), "owned: nock stack panic: boom");
    let err =
        catch_nock_panic("static", || -> Result<()> { panic!("static boom") }).expect_err("panic");
    assert_eq!(err.to_string(), "static: nock stack panic: static boom");
    let err = catch_nock_panic("opaque", || -> Result<()> { std::panic::panic_any(7u8) })
        .expect_err("panic");
    assert_eq!(err.to_string(), "opaque: nock stack panic");
    // Tracing is off, so the timed wrapper just runs its closure.
    assert_eq!(trace_timed("untraced", || Ok(3)).expect("ok"), 3);
    trace_native("untraced");
}

// ---------------------------------------------------------------------------
// Persistent-cache pack hydration

#[test]
fn packs_hydrate_every_node_kind() {
    let mut slab: NounSlab = NounSlab::new();
    let big = Atom::new(&mut slab, DIRECT_MAX + 1).as_noun();
    let huge = Atom::from_value(&mut slab, vec![0xabu8; 12])
        .expect("huge atom")
        .as_noun();
    let memo = term_to_noun(&mut slab, "memo");
    let spot = term_to_noun(&mut slab, "spot");
    let mut f = |cells: &[Noun]| T(&mut slab, cells);
    let s1 = f(&[D(0), D(1)]);
    let s2 = f(&[D(0), D(2)]);
    let s3 = f(&[D(0), D(3)]);
    let k0 = f(&[D(1), D(0)]);
    let k1 = f(&[D(1), D(1)]);
    let konst = f(&[D(42), D(43)]);
    let edit = f(&[D(6), s3]);
    let clue_body = f(&[D(1), D(0)]);
    let clue = f(&[spot, clue_body]);
    let formulas = vec![
        s1,
        f(&[D(0), big]),
        f(&[D(1), konst]),
        f(&[D(1), huge]),
        f(&[D(2), s1, k0]),
        f(&[D(3), s1]),
        f(&[D(4), s1]),
        f(&[D(5), s2, s3]),
        f(&[D(6), s1, k0, k1]),
        f(&[D(7), s1, s2]),
        f(&[D(8), s1, s2]),
        f(&[D(9), D(2), s1]),
        f(&[D(10), edit, s2]),
        f(&[D(11), memo, s1]),
        f(&[D(11), clue, s1]),
        f(&[D(12), s1, s2]),
        f(&[s1, s2]),
        f(&[D(99), D(1)]),
    ];
    let space = slab.noun_space();
    let mut bridge = SlabToNockasm::new();
    let converted: Vec<nockasm::Noun> = formulas
        .iter()
        .map(|formula| bridge.convert(*formula, &space).expect("convert"))
        .collect();
    let names: Vec<String> = (0..converted.len()).map(|i| format!("f{i}")).collect();
    let inputs: Vec<nockasm::DagInput<'_>> = converted
        .iter()
        .zip(&names)
        .map(|(noun, name)| nockasm::DagInput {
            name,
            noun,
            mode: nockasm::DagMode::Formula,
        })
        .collect();
    let bundle = nockasm::lift_bundle(&inputs).expect("lift");
    let mut children = Vec::new();
    for node in bundle.nodes() {
        push_pack_children(node, &mut children);
    }
    assert!(!children.is_empty());

    let mut target: NounSlab = NounSlab::new();
    let mut values = vec![None; bundle.nodes().len()];
    for (root, formula) in bundle.roots().iter().zip(&formulas) {
        hydrate_pack_root(&mut target, bundle.nodes(), root.id(), &mut values).expect("hydrate");
        // A second hydration of the same root is a no-op.
        hydrate_pack_root(&mut target, bundle.nodes(), root.id(), &mut values).expect("again");
        let hydrated = values[root.id().index()].expect("root value");
        assert_eq!(
            jam_noun_in_fresh_slab(hydrated, &target.noun_space()),
            jam_noun_in_fresh_slab(*formula, &space),
            "{}",
            root.name()
        );
    }
    for (value, direct) in [(DIRECT_MAX, true), (DIRECT_MAX + 1, false)] {
        let atom = nockasm::Atom::from(value);
        let noun = nasm_atom_to_slab(&mut target, &atom);
        assert_eq!(noun.is_direct(), direct);
        assert_eq!(atom_u64(noun, &target.noun_space()), Some(value));
    }
}

// ---------------------------------------------------------------------------
// Paths

#[test]
fn import_and_entry_wers() {
    let tree = Tree::new();
    let deps = tree.path("deps");
    let inside = tree.write("deps/app/a.hoon", "42\n");
    let outside = tree.write("elsewhere/b.hoon", "42\n");
    let outside_parts = components(&outside.canonicalize().unwrap());
    assert_eq!(build_import_wer(&inside, &deps), ["app", "a.hoon"]);
    assert_eq!(build_entry_wer(&inside, &deps, true), ["app", "a.hoon"]);
    assert_eq!(build_entry_wer(&outside, &deps, false), outside_parts);
    assert_eq!(build_entry_wer(&outside, &deps, true), outside_parts);

    // Relative dependency roots and entries resolve against the cwd; an entry
    // outside the root but under the cwd gets a cwd-relative wer.
    let scratch = CwdScratch::new("wer");
    fs::create_dir_all(scratch.abs.join("deps/lib")).expect("deps");
    fs::write(scratch.abs.join("deps/lib/c.hoon"), "42\n").expect("dep");
    fs::write(scratch.abs.join("d.hoon"), "42\n").expect("entry");
    let rel_deps = scratch.rel.join("deps");
    assert_eq!(
        build_import_wer(&rel_deps.join("./lib/../lib/c.hoon"), &rel_deps),
        ["lib", "c.hoon"]
    );
    let mut expected = components(&scratch.rel);
    expected.push("d.hoon".to_string());
    assert_eq!(
        build_entry_wer(&scratch.rel.join("d.hoon"), &rel_deps, true),
        expected
    );

    // A symlinked root matches only after canonicalization.
    #[cfg(unix)]
    {
        let link = tree.path("link");
        std::os::unix::fs::symlink(&deps, &link).expect("symlink");
        let via_real = deps.canonicalize().unwrap().join("app/a.hoon");
        assert_eq!(build_import_wer(&via_real, &link), ["app", "a.hoon"]);
        assert!(path_is_inside_dir(&via_real, &link));
        assert_eq!(
            entry_path_for_hoon(&via_real, &link).unwrap(),
            "/app/a.hoon"
        );
    }

    // Separate checkouts of the same open/hoon tree match by root marker.
    let entry = tree.write("one/open/hoon/app/k.hoon", "42\n");
    let other_root = tree.path("two/open/hoon");
    fs::create_dir_all(&other_root).expect("root");
    assert_eq!(
        matching_hoon_root_marker(&entry, &other_root).as_deref(),
        Some("open")
    );
    assert!(path_is_inside_dir(&entry, &other_root));
    assert_eq!(build_import_wer(&entry, &other_root), ["app", "k.hoon"]);
    assert_eq!(
        entry_path_for_hoon(&entry, &other_root).unwrap(),
        "/app/k.hoon"
    );
    let closed_root = tree.path("three/closed/hoon");
    fs::create_dir_all(&closed_root).expect("root");
    assert_eq!(matching_hoon_root_marker(&entry, &closed_root), None);
    assert!(!path_is_inside_dir(&entry, &closed_root));
    assert_eq!(hoon_root_marker(&tree.path("x/hoon/y")), None);
    assert_eq!(hoon_relative_components(&tree.path("x/hoon/y")), None);
    assert_eq!(hoon_relative_components(&tree.path("a/open/x/b")), None);
    assert_eq!(
        path_components_for_dbug(Path::new("/a/./b/../c")),
        ["a", "b", "c"]
    );
}

#[test]
fn entry_paths_for_directory_hashes() {
    let tree = Tree::new();
    let deps = tree.path("deps");
    let inside = tree.write("deps/app/a.hoon", "42\n");
    assert_eq!(entry_path_for_hoon(&inside, &deps).unwrap(), "/app/a.hoon");
    assert_eq!(entry_path_for_hoon(&deps, &deps).unwrap(), "/");
    assert_eq!(hoon_path_from_relative(Path::new("")), "/");

    // Outside the root and the cwd: the canonical absolute path.
    let outside = tree.write("elsewhere/b.hoon", "42\n");
    assert_eq!(
        entry_path_for_hoon(&outside, &deps).unwrap(),
        outside.canonicalize().unwrap().to_string_lossy()
    );
    assert!(entry_path_for_hoon(&tree.path("elsewhere/none.hoon"), &deps).is_err());

    // Outside the root but under the cwd: cwd-relative.
    let scratch = CwdScratch::new("entry");
    fs::write(scratch.abs.join("e.hoon"), "42\n").expect("entry");
    assert_eq!(
        entry_path_for_hoon(&scratch.abs.join("e.hoon"), &deps).unwrap(),
        format!("/{}/e.hoon", scratch.rel.display())
    );

    let cwd = env::current_dir().unwrap();
    assert_eq!(
        lexical_absolute_path(Path::new("a/./b/../c")).unwrap(),
        cwd.join("a/c")
    );
    assert_eq!(
        lexical_absolute_path(Path::new("/x/../y/.")).unwrap(),
        PathBuf::from("/y")
    );
    assert_eq!(
        lexical_absolute_path(Path::new("./a")).unwrap(),
        cwd.join("a")
    );
}

#[test]
fn log_paths_are_cwd_relative() {
    let cwd = env::current_dir().unwrap();
    assert_eq!(hoon_log_path(&cwd), ".");
    assert_eq!(hoon_log_path(&cwd.join("a/b.hoon")), "a/b.hoon");
    assert_eq!(hoon_log_path(Path::new("a/../b.hoon")), "b.hoon");
    let outside = PathBuf::from("/definitely-not-a-honk-cwd/x.hoon");
    assert_eq!(hoon_log_path(&outside), outside.to_string_lossy());
}

#[test]
fn timing_frames_nest_and_saturate() {
    // Child time recorded with no open compile frame is dropped.
    assert_eq!(pop_compile_child_timing_frame(), 0);
    add_compile_child_nanos(5);
    assert_eq!(pop_compile_child_timing_frame(), 0);

    push_compile_child_timing_frame();
    add_compile_child_timing(Duration::from_nanos(7));
    add_compile_child_nanos(u128::MAX);
    assert_eq!(pop_compile_child_timing_frame(), u128::MAX);

    let counter = AtomicU64::new(u64::MAX - 1);
    add_timing_nanos(&counter, 10);
    assert_eq!(counter.load(Ordering::Relaxed), u64::MAX);
    let counter = AtomicU64::new(0);
    add_timing_nanos(&counter, u128::MAX);
    assert_eq!(counter.load(Ordering::Relaxed), u64::MAX);
    add_timing(&counter, Duration::from_nanos(1));
    assert_eq!(timing_ms(&AtomicU64::new(2_500_000)), 2.5);
    assert_eq!(nanos_to_ms(1_500_000), 1.5);

    // A compile log opens a frame that the nested parse log charges.
    {
        let path = Path::new("compile.hoon");
        let _compile = TimedHoonPathLog::new(path, HoonLogOperation::Compile);
        {
            let _parse = TimedHoonPathLog::new(path, HoonLogOperation::Parse);
        }
        add_interpret_timing(Duration::from_nanos(3));
    }
    assert_eq!(pop_compile_child_timing_frame(), 0);
    report_native_timing_totals();
}

#[test]
fn cache_namespace_tracks_build_inputs() {
    let base = native_cache_namespace(&cli_for(true, true), "prelude", None);
    assert_eq!(
        base,
        native_cache_namespace(&cli_for(true, true), "prelude", None)
    );
    for other in [
        native_cache_namespace(&cli_for(false, true), "prelude", None),
        native_cache_namespace(&cli_for(true, false), "prelude", None),
        native_cache_namespace(&cli_for(true, true), "other prelude", None),
        native_cache_namespace(&cli_for(true, true), "prelude", Some(b"sut")),
    ] {
        assert_ne!(base, other);
    }
}

// ---------------------------------------------------------------------------
// Directory hash

#[test]
fn directory_hash_follows_the_file_set() {
    let tree = Tree::new();
    let deps = tree.path("deps");
    let entry = tree.write("deps/app/k.hoon", "|=(a=@ a)\n");
    let raw = tree.write("deps/common/raw.hoon", "42\n");
    let data = tree.write("deps/data/x.jam", b"\x01\x02");
    let all = directory_mug_with_files(&entry, &deps, None).expect("mug");
    let listed = directory_mug_with_files(
        &entry,
        &deps,
        Some(&[entry.clone(), raw.clone(), data.clone(), tree.path("deps/none.hoon")]),
    )
    .expect("mug");
    assert_eq!(all, listed);
    let partial =
        directory_mug_with_files(&entry, &deps, Some(&[entry.clone(), raw.clone()])).expect("mug");
    assert_ne!(all, partial);
    // Files hoonc does not read (by extension) do not change the hash.
    tree.write("deps/notes.md", "ignored");
    assert_eq!(
        directory_mug_with_files(&entry, &deps, None).expect("mug"),
        all
    );
    tree.write("deps/notes.txt", "read");
    assert_ne!(
        directory_mug_with_files(&entry, &deps, None).expect("mug"),
        all
    );
    assert!(directory_mug_with_files(&entry, &tree.path("missing"), None).is_err());

    let mut slab: NounSlab = NounSlab::new();
    let path = hoon_path_text_to_noun(&mut slab, "//a//b/").expect("path");
    let space = slab.noun_space();
    assert_eq!(decode_path_noun(path, &space).as_deref(), Some("a/b"));
}

#[test]
fn map_ordering_and_updates() {
    let mut slab: NounSlab = NounSlab::new();
    let a1 = T(&mut slab, &[D(1), D(2)]);
    let a2 = T(&mut slab, &[D(1), D(2)]);
    let b = T(&mut slab, &[D(1), D(3)]);
    let c = T(&mut slab, &[D(2), D(0)]);
    let h1 = T(&mut slab, &[a1, D(5)]);
    let h2 = T(&mut slab, &[a2, D(6)]);
    // dor: equal nouns, atom order, atoms before cells, and head/tail order.
    assert!(dor(&mut slab, a1, a1));
    assert!(dor(&mut slab, a1, a2));
    assert!(dor(&mut slab, D(1), D(2)));
    assert!(!dor(&mut slab, D(2), D(1)));
    assert!(dor(&mut slab, D(9), a1));
    assert!(!dor(&mut slab, a1, D(9)));
    assert!(dor(&mut slab, a1, b));
    assert!(!dor(&mut slab, b, a1));
    assert!(dor(&mut slab, a1, c));
    assert!(dor(&mut slab, h1, h2));
    assert!(!dor(&mut slab, h2, h1));
    // Equal mugs fall back to dor.
    assert!(gor_mug(&mut slab, a1, a2));
    assert!(mor_mug(&mut slab, a1, a2));

    let key = T(&mut slab, &[D(7), D(8)]);
    let key_copy = T(&mut slab, &[D(7), D(8)]);
    let one = map_put_mug(&mut slab, D(0), key, D(1)).expect("put");
    // Re-putting an equal key and value at the node returns that node's tree.
    let same = map_put_mug(&mut slab, one, key_copy, D(1)).expect("same");
    assert!(unsafe { same.raw_equals(&one) });
    let tree = map_put_mug(&mut slab, one, a1, D(2)).expect("put");
    let tree = map_put_mug(&mut slab, tree, c, D(3)).expect("put");
    let again = map_put_mug(&mut slab, tree, key_copy, D(1)).expect("again");
    let replaced = map_put_mug(&mut slab, tree, key_copy, D(9)).expect("replace");
    let space = slab.noun_space();
    assert!(noun_eq(again, tree, &space).expect("eq"));
    assert!(!noun_eq(replaced, tree, &space).expect("eq"));
    assert!(noun_eq(a1, a2, &space).expect("eq"));
    assert!(!noun_eq(a1, b, &space).expect("eq"));
    assert!(!noun_eq(a1, D(1), &space).expect("eq"));
}

#[test]
fn manifest_paths_relative_to_the_dependency_root() {
    let tree = Tree::new();
    let deps = tree.path("deps");
    let file = tree.write("deps/app/a.hoon", "42\n");
    let directory = deps.canonicalize().unwrap();
    assert_eq!(
        hoonc_manifest_relative_path(&directory, &file).unwrap(),
        "/app/a.hoon"
    );
    // A missing file still maps by its lexical prefix.
    assert_eq!(
        hoonc_manifest_relative_path(&directory, &directory.join("app/none.hoon")).unwrap(),
        "/app/none.hoon"
    );
    #[cfg(unix)]
    {
        // Through a symlinked root, the canonical file is outside the given
        // root but the lexical path is inside it.
        let link = tree.path("link");
        std::os::unix::fs::symlink(&deps, &link).expect("symlink");
        assert_eq!(
            hoonc_manifest_relative_path(&link, &link.join("app/a.hoon")).unwrap(),
            "/app/a.hoon"
        );
    }
    // A file from another copy of the tree maps by the shared root suffix.
    assert_eq!(
        hoonc_manifest_relative_path(Path::new("/x/y/deps"), Path::new("/other/deps/q/r.hoon"))
            .unwrap(),
        "/q/r.hoon"
    );
    assert!(
        hoonc_manifest_relative_path(Path::new("/x/y/deps"), Path::new("/other/deps")).is_err()
    );
    assert!(hoonc_manifest_relative_path(Path::new("/x/y/deps"), Path::new("/p/q.hoon")).is_err());

    let allowed =
        hoonc_directory_allowed_paths(&directory, &[file.clone(), directory.join("app/none.hoon")])
            .expect("allowed");
    assert_eq!(allowed.into_iter().collect::<Vec<_>>(), ["/app/a.hoon"]);
}

// ---------------------------------------------------------------------------
// Softed constraints

#[test]
fn softed_constraints_pins_and_delegation_checks() {
    let hoon = repo_hoon();
    let canonical = hoon.join("dat/softed-constraints.hoon");
    assert!(softed_constraints_pins_match(&canonical, &hoon).expect("pins"));
    let mut context = create_eval_context();
    let value = native_value_override(&mut context, &canonical, &hoon)
        .expect("override")
        .expect("pinned value");
    assert!(value.is_cell());

    let tree = Tree::new();
    let forked = tree.write("dat/softed-constraints.hoon", "42\n");
    assert!(!softed_constraints_pins_match(&forked, tree.root()).expect("missing jams"));
    let err = native_value_override(&mut context, &forked, tree.root()).expect_err("unpinned");
    assert!(err.to_string().contains("unpinned"), "{err}");
    let plain = tree.write("common/plain.hoon", "42\n");
    assert!(native_value_override(&mut context, &plain, tree.root())
        .expect("plain")
        .is_none());
    // The pinned source without its jams is a miss, not an error.
    let copied = tree.write(
        "copy/dat/softed-constraints.hoon",
        fs::read(&canonical).expect("canonical source"),
    );
    assert!(!softed_constraints_pins_match(&copied, &tree.path("copy")).expect("no jams"));
    // A directory where a pinned file should be is an I/O error, not a miss.
    assert!(softed_constraints_pins_match(&tree.path("dat"), tree.root()).is_err());

    // Data imports are not followed when looking for softed constraints.
    tree.write("data/blob.jam", b"\x01");
    let entry = tree.write(
        "app/data.hoon",
        "/=  plain  /common/plain\n/=  again  /common/plain\n/*  blob  %jam  /data/blob/jam\n42\n",
    );
    assert!(!entry_uses_unpinned_softed_constraints(&entry, tree.root()).expect("scan"));
}

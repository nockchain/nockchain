//! In-process rejection/acceptance sweep of the type-probe corpus.
//!
//! The same .hoon files are pair-tested against hoonc at the artifact level
//! by //crates/honk/test-assets/type-probes (Bazel, strict cmp for accepts /
//! both-must-reject verdicts); this harness runs them through `ut.mint` with
//! the embedded canonical hoon-138 subject type — the same subject the honk
//! binary compiles files against — so the exercised type-checker branches
//! (mint-nice/vain/lost, find/find-fork, fish-core/loop, fire-dry, mull,
//! redo-match, payload-block, ...) count toward cargo-tarpaulin coverage.
//!
//! reject/rest_loop_alias.hoon is deliberately skipped: both compilers
//! currently reject it via native stack overflow, which would abort the
//! test process (see the probe's header comment).

use std::fs;
use std::path::PathBuf;
use std::sync::Arc;

use chumsky::Parser;
use hatch::ast::hoon::Hoon;
use hatch::native_parser;
use hatch::utils::LineMap;
use honk::native::ut::{ty_noun, Ut};
use nockapp::noun::slab::NounSlab;
use nockapp::utils::NOCK_STACK_SIZE;
use nockvm::ext::NounExt;
use nockvm::mem::NockStack;
use nockvm::noun::{Noun, NounSpace};

static HONC_TYPE_138_JAM: &[u8] = include_bytes!(env!("HONK_HONC_TYPE_138_JAM"));

/// %term atom value of a short tag (LSB-first cord encoding).
fn term_u64(tag: &str) -> u64 {
    tag.bytes()
        .enumerate()
        .fold(0u64, |acc, (i, b)| acc | ((b as u64) << (8 * i)))
}

fn is_type_tag(value: u64) -> bool {
    ["noun", "void", "atom", "cell", "core", "face", "fork", "hint", "hold"]
        .iter()
        .any(|tag| term_u64(tag) == value)
}

fn is_type_noun(noun: Noun, space: &NounSpace) -> bool {
    let noun = noun.in_space(space);
    if let Ok(atom) = noun.as_atom() {
        return matches!(atom.as_u64(), Ok(v) if v == term_u64("noun") || v == term_u64("void"));
    }
    let Ok(cell) = noun.as_cell() else {
        return false;
    };
    let Ok(tag) = cell.head().noun().in_space(space).as_atom() else {
        return false;
    };
    matches!(tag.as_u64(), Ok(v) if is_type_tag(v))
}

/// Cue the embedded canonical hoon-138 subject type (the `--sut-jam` shape:
/// the root either is the type noun or carries it at its head) into `slab`.
fn cue_subject_type(slab: &mut NounSlab) -> Noun {
    let mut stack = NockStack::new(NOCK_STACK_SIZE, 0);
    let root = <Noun as NounExt>::cue_bytes_slice(&mut stack, HONC_TYPE_138_JAM)
        .expect("cue embedded honc type jam");
    let space = stack.noun_space();
    let ty = if is_type_noun(root, &space) {
        root
    } else {
        let cell = root
            .in_space(&space)
            .as_cell()
            .expect("subject type jam root should be a type or [type ...]");
        let head = cell.head().noun();
        assert!(
            is_type_noun(head, &space),
            "subject type jam did not contain a Hoon type noun"
        );
        head
    };
    slab.copy_into(ty, &space)
}

fn probes_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("test-assets/type-probes")
}

fn hoon_files(dir: &PathBuf) -> Vec<PathBuf> {
    let mut files: Vec<PathBuf> = fs::read_dir(dir)
        .unwrap_or_else(|err| panic!("read_dir {}: {err}", dir.display()))
        .filter_map(|entry| entry.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|ext| ext == "hoon"))
        .collect();
    files.sort();
    files
}

fn parse_probe(src: &str) -> Result<Hoon, String> {
    let linemap = Arc::new(LineMap::new(src));
    native_parser(vec!["test".to_string()], true, linemap)
        .parse(src)
        .into_result()
        .map_err(|errs| format!("{errs:?}"))
}

#[test]
fn rejection_probes_all_rejected() {
    let mut slab = NounSlab::new();
    let sut = cue_subject_type(&mut slab);
    let gol = ty_noun(&mut slab);
    let mut ut = Ut::new(&mut slab);
    let mut accepted = Vec::new();
    for path in hoon_files(&probes_dir().join("reject")) {
        let Some(stem) = path.file_stem() else {
            continue;
        };
        let name = stem.to_string_lossy().into_owned();
        if name == "rest_loop_alias" {
            continue;
        }
        let src = fs::read_to_string(&path).expect("read probe");
        let expr = match parse_probe(&src) {
            // a parse-level rejection (e.g. tissig_empty) is a rejection
            Err(_) => continue,
            Ok(expr) => expr,
        };
        if ut.mint_noun(sut, gol, &expr).is_ok() {
            accepted.push(name);
        }
    }
    assert!(
        accepted.is_empty(),
        "ill-typed rejection probes were ACCEPTED by honk: {accepted:?}"
    );
}

#[test]
fn acceptance_probes_all_compile() {
    let mut slab = NounSlab::new();
    let sut = cue_subject_type(&mut slab);
    let gol = ty_noun(&mut slab);
    let mut ut = Ut::new(&mut slab);
    let mut rejected = Vec::new();
    for path in hoon_files(&probes_dir()) {
        let Some(stem) = path.file_stem() else {
            continue;
        };
        let name = stem.to_string_lossy().into_owned();
        let src = fs::read_to_string(&path).expect("read probe");
        let expr = match parse_probe(&src) {
            Err(err) => {
                rejected.push(format!("{name} (parse: {err})"));
                continue;
            }
            Ok(expr) => expr,
        };
        if let Err(err) = ut.mint_noun(sut, gol, &expr) {
            rejected.push(format!("{name} ({err})"));
        }
    }
    assert!(
        rejected.is_empty(),
        "well-typed acceptance probes were REJECTED by honk: {rejected:?}"
    );
}

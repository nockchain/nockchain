//! Coverage-driven tests for the pipeline, library entry points, and errors.
//!
//! Added to close branch-coverage gaps; see the coverage report in the PR.

#[allow(unused_imports)]
use super::*;

use std::fs;
use std::path::{Path, PathBuf};

use hatch::ast::hoon::{BaseType, Hoon, NounExpr, Skin};
use nockapp::noun::slab::NounSlab;
use nockvm::noun::{Noun, D, T};

use crate::errors::CompilerError;
use crate::native::noun::term_to_noun;
use crate::pipeline::{
    parse_native_hoon, parse_native_hoon_leaf, parse_native_hoon_leaf_with_mode,
    parse_native_hoon_source, parse_native_hoon_with_mode, resolve_native_imports, CompileRequest,
    NativeImportKind, ResolvedNativeImport, ScopeMode,
};
use crate::types::TypeNoun;

/// A throwaway source tree.
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

/// A scratch directory under the process working directory (the package
/// root under `cargo test`), for exercising relative and cwd-relative paths.
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
        let rel = PathBuf::from(format!(".cov-c6-{tag}-{}-{nanos}", std::process::id()));
        let abs = std::env::current_dir().expect("cwd").join(&rel);
        fs::create_dir_all(&abs).expect("scratch dir");
        Self { rel, abs }
    }

    fn write(&self, rel: &str, contents: &str) {
        let path = self.abs.join(rel);
        fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");
        fs::write(path, contents).expect("write");
    }
}

impl Drop for CwdScratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.abs);
    }
}

fn canon(path: &Path) -> PathBuf {
    path.canonicalize().expect("canonicalize")
}

fn resolved(entry: &Path, deps: &Path) -> Vec<ResolvedNativeImport> {
    resolve_native_imports(entry, deps, ScopeMode::Standard).expect("imports resolve")
}

fn resolve_err(source: &str) -> CompilerError {
    let tree = Tree::new();
    let entry = tree.write("entry.hoon", source);
    resolve_native_imports(&entry, tree.root(), ScopeMode::Standard)
        .expect_err("import block should be rejected")
}

/// The first `%spot` path in a parsed AST, read from its debug rendering.
fn first_spot_path(expr: &Hoon) -> Vec<String> {
    let rendered = format!("{expr:?}");
    let start = rendered
        .find("Spot { p: [")
        .expect("dbug parse should produce a spot")
        + "Spot { p: [".len();
    let end = start + rendered[start..].find(']').expect("spot path end");
    rendered[start..end]
        .split(", ")
        .filter(|part| !part.is_empty())
        .map(|part| part.trim_matches('"').to_string())
        .collect()
}

fn term_face(expr: &Hoon) -> Option<&str> {
    match expr {
        Hoon::KetTis(Skin::Term(face), _) => Some(face.as_str()),
        _ => None,
    }
}

/// Splits `=+(dep body)` into (dep, body).
fn tislus(expr: &Hoon) -> (&Hoon, &Hoon) {
    match expr {
        Hoon::TisLus(dep, body) => (dep, body),
        other => panic!("expected an import wrapper, got {other:?}"),
    }
}

fn jam_of(build: impl FnOnce(&mut NounSlab) -> Noun) -> Vec<u8> {
    let mut slab: NounSlab = NounSlab::new();
    let root = build(&mut slab);
    slab.set_root(root);
    slab.jam().to_vec()
}

// ---------------------------------------------------------------------------
// errors.rs

#[test]
fn error_kind_display_names_every_kind() {
    let rendered: Vec<String> = [
        CompilerErrorKind::Parse,
        CompilerErrorKind::UnsupportedExpr,
        CompilerErrorKind::Backend,
        CompilerErrorKind::Decode,
        CompilerErrorKind::Noun,
    ]
    .iter()
    .map(ToString::to_string)
    .collect();
    assert_eq!(
        rendered,
        [
            "parse error",
            "unsupported Hoon expression",
            "backend error",
            "decode error",
            "noun error"
        ]
    );
}

#[test]
fn error_location_display_covers_every_shape() {
    let at = |file: Option<&str>, start: Option<(u64, u64)>, end: Option<(u64, u64)>| {
        CompilerErrorLocation {
            file: file.map(str::to_string),
            start_byte: None,
            end_byte: None,
            start_line: start.map(|p| p.0),
            start_col: start.map(|p| p.1),
            end_line: end.map(|p| p.0),
            end_col: end.map(|p| p.1),
        }
        .to_string()
    };
    assert_eq!(
        at(Some("f.hoon"), Some((1, 2)), Some((3, 4))),
        "f.hoon:1:2-3:4"
    );
    // A zero-width span prints as a point.
    assert_eq!(at(Some("f.hoon"), Some((1, 2)), Some((1, 2))), "f.hoon:1:2");
    assert_eq!(at(Some("f.hoon"), Some((1, 2)), None), "f.hoon:1:2");
    assert_eq!(at(None, Some((1, 2)), Some((3, 4))), "line 1:2-3:4");
    assert_eq!(at(None, Some((5, 6)), Some((5, 6))), "line 5:6");
    assert_eq!(at(None, Some((5, 6)), None), "line 5:6");
    assert_eq!(at(Some("f.hoon"), None, None), "f.hoon");
    assert_eq!(at(None, None, None), "");
    assert_eq!(CompilerErrorMetadata::default().to_string(), "");
}

#[test]
fn error_accessors_cover_every_variant() {
    let io = || CompilerError::Io(std::io::Error::other("disk"));
    let plain = [
        (
            CompilerError::Parse("p".into()),
            CompilerErrorKind::Parse,
            "p",
        ),
        (
            CompilerError::UnsupportedExpr("u".into()),
            CompilerErrorKind::UnsupportedExpr,
            "u",
        ),
        (
            CompilerError::Backend("b".into()),
            CompilerErrorKind::Backend,
            "b",
        ),
        (
            CompilerError::Decode("d".into()),
            CompilerErrorKind::Decode,
            "d",
        ),
        (CompilerError::Noun("n".into()), CompilerErrorKind::Noun, "n"),
    ];
    let location = CompilerErrorLocation {
        file: Some("x.hoon".into()),
        start_line: Some(1),
        start_col: Some(1),
        end_line: Some(1),
        end_col: Some(4),
        ..CompilerErrorLocation::default()
    };
    let metadata = CompilerErrorMetadata::default().with_location(location);
    for (error, kind, message) in plain {
        assert_eq!(error.kind(), Some(kind));
        assert_eq!(error.message(), Some(message));
        assert!(error.metadata().is_none());
        assert_eq!(
            error.is_unsupported_expr(),
            kind == CompilerErrorKind::UnsupportedExpr
        );
        let detailed = error.with_metadata(metadata.clone());
        assert_eq!(detailed.kind(), Some(kind));
        assert_eq!(detailed.message(), Some(message));
        assert_eq!(detailed.metadata(), Some(&metadata));
        assert_eq!(
            detailed.to_string(),
            format!("{kind}: {message} at x.hoon:1:1-1:4")
        );
    }

    // Re-attaching metadata to a detailed error replaces it.
    let detailed = CompilerError::Backend("b".into()).with_metadata(metadata.clone());
    let replaced = detailed.with_metadata(CompilerErrorMetadata::default());
    assert_eq!(replaced.metadata(), Some(&CompilerErrorMetadata::default()));
    assert_eq!(replaced.to_string(), "backend error: b");

    // I/O errors carry neither a kind nor a message, and ignore metadata.
    assert_eq!(io().kind(), None);
    assert_eq!(io().message(), None);
    assert!(io().metadata().is_none());
    assert!(!io().is_unsupported_expr());
    let still_io = io().with_metadata(metadata);
    assert!(matches!(still_io, CompilerError::Io(_)));
}

// ---------------------------------------------------------------------------
// lib.rs

#[test]
fn compiler_outputs_formula_and_both_dynock_shapes() {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .build()
        .expect("runtime");
    let mut compiler = runtime.block_on(Compiler::new()).expect("compiler");
    let expr = parse_native_hoon_source(Path::new("lib-test.hoon"), "`@ud`42", Vec::new(), false)
        .expect("test source parses");

    let mut compiled = compiler.compile_expr(&expr).expect("compile");
    let space = compiled.noun_space();
    assert_eq!(compiled.ty().tag(&space).as_deref(), Some("atom"));
    assert_eq!(compiled.arm_map().iter().count(), 0);

    // The plain jam is the formula itself: a constant.
    let formula = |slab: &mut NounSlab| T(slab, &[D(1), D(42)]);
    assert_eq!(compiled.jam(), jam_of(formula));

    // Untyped dynock: [%noun [[1 formula] 0]].
    let expected = jam_of(|slab| {
        let noun = term_to_noun(slab, "noun");
        let formula = formula(slab);
        let trap = wrap_formula_as_dynock_trap(slab, formula);
        T(slab, &[noun, trap])
    });
    assert_eq!(compiled.jam_dynock(), expected);

    // Typed dynock keeps the inferred type header: [%atom %ud ~].
    let expected = jam_of(|slab| {
        let atom = term_to_noun(slab, "atom");
        let ud = term_to_noun(slab, "ud");
        let ty = T(slab, &[atom, ud, D(0)]);
        let formula = formula(slab);
        let trap = wrap_formula_as_dynock_trap(slab, formula);
        T(slab, &[ty, trap])
    });
    assert_eq!(compiled.jam_dynock_typed(), expected);

    // Vet-disabled compilation takes the same route for a closed expression.
    let mut unvetted = compiler
        .compile_expr_with_vet(&expr, false)
        .expect("compile without vet");
    assert_eq!(unvetted.jam(), compiled.jam());
}

// ---------------------------------------------------------------------------
// types.rs

#[test]
fn type_noun_slots_tags_and_core_coils() {
    let mut slab: NounSlab = NounSlab::new();
    let core_tag = term_to_noun(&mut slab, "core");
    let face_tag = term_to_noun(&mut slab, "face");
    let hint_tag = term_to_noun(&mut slab, "hint");
    let hold_tag = term_to_noun(&mut slab, "hold");
    let cell_tag = term_to_noun(&mut slab, "cell");
    let atom_tag = term_to_noun(&mut slab, "atom");
    let coil = T(&mut slab, &[D(7), D(8)]);
    // [%core payload coil]
    let core = T(&mut slab, &[core_tag, D(5), coil]);
    let faced = T(&mut slab, &[face_tag, D(6), core]);
    let hinted = T(&mut slab, &[hint_tag, D(9), faced]);
    // [%hold type gene]: the coil search follows the hold's type.
    let held = T(&mut slab, &[hold_tag, hinted, D(10)]);
    let cell = T(&mut slab, &[cell_tag, D(1), D(2)]);
    let numbered = T(&mut slab, &[D(1), D(2)]);
    let headless_pair = T(&mut slab, &[D(1), D(2)]);
    let headless = T(&mut slab, &[headless_pair, D(3)]);
    let space = slab.noun_space();
    let atom_value = |ty: Option<TypeNoun>| {
        ty.and_then(|ty| ty.noun().in_space(&space).as_atom().ok()?.as_u64().ok())
    };

    for ty in [core, faced, hinted, held] {
        let ty = TypeNoun::new(ty);
        assert_eq!(atom_value(ty.core_slot(2, &space)), Some(7));
        assert_eq!(atom_value(ty.core_slot_big(&3u64.into(), &space)), Some(8));
    }
    // Only cores (possibly under faces, hints, and holds) have coils.
    assert!(TypeNoun::new(cell).core_slot(2, &space).is_none());
    assert!(TypeNoun::new(D(3)).core_slot(2, &space).is_none());
    assert!(TypeNoun::new(numbered).core_slot(2, &space).is_none());
    assert!(TypeNoun::new(headless).core_slot(2, &space).is_none());

    let cell_ty = TypeNoun::new(cell);
    assert_eq!(cell_ty.tag(&space).as_deref(), Some("cell"));
    assert_eq!(TypeNoun::new(atom_tag).tag(&space).as_deref(), Some("atom"));
    assert!(TypeNoun::new(headless).tag(&space).is_none());
    assert_eq!(atom_value(cell_ty.slot(6, &space)), Some(1));
    assert_eq!(atom_value(cell_ty.slot(7, &space)), Some(2));
    assert_eq!(
        cell_ty.slot(1, &space).map(|ty| ty.noun().is_cell()),
        Some(true)
    );
    // Axis 0 names nothing, and an axis through an atom has no noun.
    assert!(cell_ty.slot(0, &space).is_none());
    assert!(cell_ty.slot_big(&0u64.into(), &space).is_none());
    assert!(cell_ty.slot(12, &space).is_none());
}

// ---------------------------------------------------------------------------
// pipeline.rs: request and import-block parsing

#[test]
fn compile_request_defaults_to_a_standard_build() {
    let request = CompileRequest::new("a.hoon".into(), "deps".into());
    assert_eq!(request.entry, PathBuf::from("a.hoon"));
    assert_eq!(request.deps_dir, PathBuf::from("deps"));
    assert!(request.out_dir.is_none());
    assert!(!request.arbitrary && !request.dynock && !request.dynock_typed && !request.new);
}

fn import_tree() -> Tree {
    let tree = Tree::new();
    tree.write("sur/shapes.hoon", "|%\n+$  point  [x=@ y=@]\n--\n");
    tree.write("sur/kinds.hoon", "|%\n+$  kind  ?(%a %b)\n--\n");
    tree.write("lib/util.hoon", "|%\n++  double  |=(a=@ (mul 2 a))\n--\n");
    tree.write(
        "lib/math/extra.hoon",
        "|%\n++  triple  |=(a=@ (mul 3 a))\n--\n",
    );
    tree.write("lib/star-lib.hoon", "|%\n++  flat  1\n--\n");
    tree.write("lib/star/lib.hoon", "|%\n++  nested  1\n--\n");
    tree.write("common/raw.hoon", "[1 2]\n");
    tree.write("common/star.hoon", "|%\n++  sr  7\n--\n");
    tree.write("data/blob.jam", b"hi\x01\x02\x00\x00");
    tree.write("dat/const.hoon", "[%const 42]\n");
    tree
}

#[test]
fn import_block_resolves_every_rune_kind_in_order() {
    let tree = import_tree();
    let entry = tree.write(
        "app/entry.hoon",
        concat!(
            "/?  310\n",
            "::  a comment line before the imports\n",
            "\n",
            "/-  *shapes, kinds\n",
            "/+  util,\n",
            "    ::  a comment line inside a continued clause\n",
            "    \n",
            "    alias=math-extra,,\n",
            "\t*star-lib\n",
            "/=  raw  /common/raw  ::  a trailing comment\n",
            "/=  *  /common/star\n",
            "/=\n",
            "    twice\n",
            "    //common//raw\n",
            "/*  blob  %jam  /data/blob/jam\n",
            "/*  *  jam  /data/blob/jam\n",
            "/#  const\n",
            "|%\n",
            "--\n",
        ),
    );
    let imports = resolved(&entry, tree.root());
    let summary: Vec<(NativeImportKind, Option<&str>, PathBuf)> = imports
        .iter()
        .map(|import| (import.kind, import.face.as_deref(), canon(&import.path)))
        .collect();
    let hoon = NativeImportKind::Hoon;
    let data = NativeImportKind::Data;
    let at = |rel: &str| canon(&tree.path(rel));
    assert_eq!(
        summary,
        vec![
            (hoon, None, at("sur/shapes.hoon")),
            (hoon, Some("kinds"), at("sur/kinds.hoon")),
            (hoon, Some("util"), at("lib/util.hoon")),
            // `math-extra` has no flat file, so the hyphen becomes a slash.
            (hoon, Some("alias"), at("lib/math/extra.hoon")),
            // Both spellings exist; the one with fewer slashes wins.
            (hoon, None, at("lib/star-lib.hoon")),
            (hoon, Some("raw"), at("common/raw.hoon")),
            (hoon, None, at("common/star.hoon")),
            (hoon, Some("twice"), at("common/raw.hoon")),
            (data, Some("blob"), at("data/blob.jam")),
            (data, None, at("data/blob.jam")),
            (hoon, Some("const"), at("dat/const.hoon")),
        ]
    );
}

#[test]
fn import_block_ends_at_the_first_non_import_line() {
    let tree = import_tree();
    for source in [
        // A bare `/` has no rune.
        "/\n/-  missing\n42\n",
        // `/~` is not an import rune.
        "/~  missing\n/-  missing\n42\n",
        // Code ends the block even if import-looking lines follow.
        "42\n/-  missing\n",
        "",
    ] {
        let entry = tree.write("app/stop.hoon", source);
        assert!(
            resolved(&entry, tree.root()).is_empty(),
            "no imports expected for {source:?}"
        );
    }

    // The import block may run to the end of the file, including inside a
    // continued clause.
    let entry = tree.write(
        "app/eof.hoon",
        "/=  raw  /common/raw\n/+  util,\n    star-lib",
    );
    let faces: Vec<Option<String>> = resolved(&entry, tree.root())
        .into_iter()
        .map(|import| import.face)
        .collect();
    assert_eq!(
        faces,
        [
            Some("raw".to_string()),
            Some("util".to_string()),
            Some("star-lib".to_string())
        ]
    );
}

#[test]
fn malformed_import_clauses_are_parse_errors() {
    for source in [
        "/=\n",
        "/=  ::  only a comment\n",
        "/=  onlyface\n",
        "/=  a  /b  extra\n",
        "/*\n",
        "/*  ::  only a comment\n",
        "/*  a  %jam\n",
        "/*  a  %jam  /b/jam  extra\n",
        "/+  *\n",
        "/+  =x\n",
        "/+  x=\n",
        "/-  util, *  \n",
    ] {
        match resolve_err(source) {
            CompilerError::Parse(message) => {
                assert!(message.contains("malformed"), "{source:?}: {message}")
            }
            other => panic!("{source:?}: expected a parse error, got {other:?}"),
        }
    }
    // `/%` is recognized and rejected rather than dropped.
    assert!(resolve_err("/%  thing  %hoon  /lib/thing\n").is_unsupported_expr());
}

#[test]
fn data_imports_accept_only_the_jam_mark() {
    let tree = import_tree();
    let entry = tree.write("app/txt.hoon", "/*  blob  %txt  /data/blob/jam\n42\n");
    let err = resolve_native_imports(&entry, tree.root(), ScopeMode::Standard)
        .expect_err("non-jam marks are rejected");
    assert!(err.is_unsupported_expr());
    assert!(err.to_string().contains("%txt"), "{err}");
}

#[test]
fn missing_imports_name_their_rune() {
    let tree = import_tree();
    for (source, rune) in [
        ("/+  nope\n", "`/+ nope`"),
        ("/-  nope\n", "`/- nope`"),
        ("/#  nope\n", "`/# nope`"),
        ("/=  x  /common/nope\n", "`/= /common/nope`"),
        // A raw import naming no file at all.
        ("/=  x  /\n", "`/= /`"),
        ("/*  x  %jam  /data/nope/jam\n", "`/* /data/nope/jam`"),
        // A data import needs at least a stem and an extension.
        ("/*  x  %jam  /blob\n", "`/* /blob`"),
        // Hyphen variants are tried and none exists.
        ("/+  no-such-lib\n", "`/+ no-such-lib`"),
        // A suffix of only hyphens has no path candidates.
        ("/+  -\n", "`/+ -`"),
    ] {
        let entry = tree.write("app/missing.hoon", source);
        let err = resolve_native_imports(&entry, tree.root(), ScopeMode::Standard)
            .expect_err("missing import");
        let message = err.to_string();
        assert!(
            message.contains("native import not found") && message.contains(rune),
            "{source:?}: {message}"
        );
    }
}

#[test]
fn missing_entries_are_io_errors() {
    let tree = Tree::new();
    let missing = tree.path("missing.hoon");
    for err in [
        parse_native_hoon(&missing, tree.root(), false).expect_err("parse"),
        parse_native_hoon_leaf(&missing, tree.root(), false).expect_err("leaf"),
        resolve_native_imports(&missing, tree.root(), ScopeMode::Standard).expect_err("resolve"),
    ] {
        assert!(matches!(err, CompilerError::Io(_)), "{err:?}");
    }
}

// ---------------------------------------------------------------------------
// pipeline.rs: whole-graph parsing

#[test]
fn native_parse_wraps_imports_outermost_first() {
    let tree = import_tree();
    let entry = tree.write(
        "app/entry.hoon",
        "/-  *shapes\n/+  util, again=util\n/*  blob  %jam  /data/blob/jam\n[1 2]\n",
    );
    let expr = parse_native_hoon(&entry, tree.root(), false).expect("graph parses");

    let (shapes, rest) = tislus(&expr);
    assert!(term_face(shapes).is_none(), "`*` imports carry no face");
    let (util, rest) = tislus(rest);
    assert_eq!(term_face(util), Some("util"));
    // The second import of the same library reuses the cached parse.
    let (again, rest) = tislus(rest);
    assert_eq!(term_face(again), Some("again"));
    match (util, again) {
        (Hoon::KetTis(_, first), Hoon::KetTis(_, second)) => assert_eq!(first, second),
        _ => unreachable!(),
    }

    // `/*` data is spliced in as `^=  blob  ^-  [p=@ud q=@]  [len atom]`.
    let (blob, _body) = tislus(rest);
    let Hoon::KetTis(Skin::Term(face), data) = blob else {
        panic!("expected a faced data import, got {blob:?}");
    };
    assert_eq!(face, "blob");
    let Hoon::KetTis(Skin::Cell(p, q), value) = data.as_ref() else {
        panic!("expected an octs cast, got {data:?}");
    };
    assert!(matches!(
        p.as_ref(),
        Skin::Name(name, base) if name == "p"
            && matches!(base.as_ref(), Skin::Base(BaseType::Atom(aura)) if aura == "ud")
    ));
    assert!(matches!(
        q.as_ref(),
        Skin::Name(name, base) if name == "q"
            && matches!(base.as_ref(), Skin::Base(BaseType::Atom(aura)) if aura == "$")
    ));
    let Hoon::Rock(aura, NounExpr::Cell(len, atom)) = value.as_ref() else {
        panic!("expected a rock, got {value:?}");
    };
    assert_eq!(aura, "$");
    let (NounExpr::ParsedAtom(len), NounExpr::ParsedAtom(atom)) = (len.as_ref(), atom.as_ref())
    else {
        panic!("expected atom pair");
    };
    // The parse-only path records the file length, trailing zeros included.
    assert_eq!(len.to_biguint(), 6u32.into());
    assert_eq!(
        atom.to_biguint(),
        num_bigint::BigUint::from_bytes_le(b"hi\x01\x02")
    );
}

#[test]
fn cyclic_and_broken_dependencies_are_rejected() {
    let tree = Tree::new();
    let entry = tree.write("a.hoon", "/+  b\n42\n");
    tree.write("lib/b.hoon", "/=  a  /a\n42\n");
    let err = parse_native_hoon(&entry, tree.root(), false).expect_err("cycle");
    assert!(err.to_string().contains("cyclic native import"), "{err}");

    let tree = Tree::new();
    let entry = tree.write("a.hoon", "/+  bad\n42\n");
    tree.write("lib/bad.hoon", "this is not hoon\n");
    let err = parse_native_hoon(&entry, tree.root(), true).expect_err("bad dep");
    assert_eq!(err.kind(), Some(CompilerErrorKind::Parse));
    let location = err
        .metadata()
        .and_then(|metadata| metadata.location.clone())
        .expect("parse errors carry a location");
    assert!(location.file.expect("file").ends_with("bad.hoon"));
}

#[test]
fn leaf_parse_ignores_imports() {
    let tree = Tree::new();
    let entry = tree.write("a.hoon", "/+  missing\n[1 2]\n");
    let expr = parse_native_hoon_leaf(&entry, tree.root(), false).expect("leaf parses");
    assert!(!matches!(expr, Hoon::TisLus(..)), "{expr:?}");
}

// ---------------------------------------------------------------------------
// pipeline.rs: spot paths (hoon_path_for_any)

#[test]
fn spot_paths_are_relative_to_the_dependency_root() {
    // Lexically inside the root.
    let tree = Tree::new();
    let entry = tree.write("app/a.hoon", "[1 2]\n");
    let expr = parse_native_hoon_leaf(&entry, tree.root(), true).expect("parse");
    assert_eq!(first_spot_path(&expr), ["app", "a.hoon"]);

    // Inside the root only after resolving a symlinked root.
    #[cfg(unix)]
    {
        let real = tree.path("real");
        fs::create_dir_all(&real).expect("real dir");
        let file = tree.write("real/lib/b.hoon", "[1 2]\n");
        let link = tree.path("link");
        std::os::unix::fs::symlink(&real, &link).expect("symlink");
        let expr = parse_native_hoon_leaf(&file, &link, true).expect("parse");
        assert_eq!(first_spot_path(&expr), ["lib", "b.hoon"]);
    }

    // Outside both the root and the working directory: the canonical path.
    let other = Tree::new();
    let outside = other.write("x/c.hoon", "[1 2]\n");
    let expr = parse_native_hoon_leaf(&outside, tree.root(), true).expect("parse");
    let expected: Vec<String> = canon(&outside)
        .components()
        .filter_map(|component| match component {
            std::path::Component::Normal(part) => Some(part.to_string_lossy().into_owned()),
            _ => None,
        })
        .collect();
    assert_eq!(first_spot_path(&expr), expected);
}

#[test]
fn spot_paths_handle_relative_and_cwd_relative_inputs() {
    let scratch = CwdScratch::new("spots");
    scratch.write("deps/a.hoon", "[1 2]\n");
    scratch.write("other/b.hoon", "[1 2]\n");
    let rel_deps = scratch.rel.join("deps");
    let abs_deps = scratch.abs.join("deps");

    // Relative entry, absolute root.
    let expr = parse_native_hoon_leaf(&rel_deps.join("a.hoon"), &abs_deps, true).expect("parse");
    assert_eq!(first_spot_path(&expr), ["a.hoon"]);
    // Absolute entry, relative root.
    let expr = parse_native_hoon_leaf(&abs_deps.join("a.hoon"), &rel_deps, true).expect("parse");
    assert_eq!(first_spot_path(&expr), ["a.hoon"]);
    // Outside the root but inside the working directory: cwd-relative.
    let expr =
        parse_native_hoon_leaf(&scratch.abs.join("other/b.hoon"), &abs_deps, true).expect("parse");
    let mut expected = vec![scratch.rel.to_string_lossy().into_owned()];
    expected.extend(["other".to_string(), "b.hoon".to_string()]);
    assert_eq!(first_spot_path(&expr), expected);
}

// ---------------------------------------------------------------------------
// pipeline.rs: Urbit scope mode

fn arvo_tree() -> (Tree, PathBuf) {
    let tree = Tree::new();
    let arvo = tree.path("pkg/arvo");
    tree.write("pkg/arvo/sys/hoon.hoon", "|%\n++  part  ~\n--\n");
    tree.write(
        "pkg/arvo/sys/lull.hoon",
        concat!(
            "!:\n",
            "=>  ..part\n",
            "~%  %lull  ..part  ~\n",
            "|%\n",
            "++  l  1\n",
            "++  m\n",
            "  ~%  %keep  ..part  ~\n",
            "  |=(a=@ a)\n",
            "--\n",
        ),
    );
    tree.write(
        "pkg/arvo/sys/zuse.hoon",
        "=>  ..lull\n|_  a=@\n++  z  a\n--\n",
    );
    tree.write("pkg/arvo/sys/arvo.hoon", "=>  +\n|*  a=@\na\n");
    tree.write("pkg/arvo/lib/helper.hoon", "|%\n++  h  1\n--\n");
    tree.write("pkg/arvo/app/ping.hoon", "/+  helper\n|=(a=@ a)\n");
    (tree, arvo)
}

#[test]
fn urbit_scope_chains_zuse_lull_and_hoon() {
    let (tree, arvo) = arvo_tree();
    let ping = tree.path("pkg/arvo/app/ping.hoon");

    // The root file gets zuse ahead of its own imports; the library does not.
    let imports = resolve_native_imports(&ping, &arvo, ScopeMode::Urbit).expect("resolve");
    let summary: Vec<(Option<&str>, PathBuf)> = imports
        .iter()
        .map(|import| (import.face.as_deref(), canon(&import.path)))
        .collect();
    assert_eq!(
        summary,
        [
            (None, canon(&arvo.join("sys/zuse.hoon"))),
            (Some("helper"), canon(&arvo.join("lib/helper.hoon"))),
        ]
    );

    let expr = parse_native_hoon_with_mode(&ping, &arvo, false, ScopeMode::Urbit).expect("parse");
    let (zuse, rest) = tislus(&expr);
    assert!(term_face(zuse).is_none());
    let (helper, _) = tislus(rest);
    assert_eq!(term_face(helper), Some("helper"));
    // zuse <- lull (as `lull`) <- hoon (as `part`), and /sys/hoon gets `ride`.
    let (lull, _) = tislus(zuse);
    assert_eq!(term_face(lull), Some("lull"));
    let Hoon::KetTis(_, lull) = lull else {
        unreachable!()
    };
    let (part, _) = tislus(lull);
    assert_eq!(term_face(part), Some("part"));
    let Hoon::KetTis(_, hoon) = part else {
        unreachable!()
    };
    let (ride, _) = tislus(hoon);
    assert_eq!(term_face(ride), Some("ride"));

    // The deps root need not be the arvo root: it is found from the entry.
    let elsewhere = Tree::new();
    let again = parse_native_hoon_with_mode(&ping, elsewhere.root(), false, ScopeMode::Urbit)
        .expect("parse from entry-derived root");
    assert_eq!(again, expr);
}

#[test]
fn urbit_leaf_parse_adds_ambient_faces_and_sanitizes_sys_headers() {
    let (tree, arvo) = arvo_tree();

    // /sys/arvo gets ride and zuse; its `=>  +` header line is kept because
    // it does not reach into a named core.
    let arvo_file = arvo.join("sys/arvo.hoon");
    let expr = parse_native_hoon_leaf_with_mode(&arvo_file, &arvo, false, ScopeMode::Urbit)
        .expect("arvo parses");
    let (ride, rest) = tislus(&expr);
    assert_eq!(term_face(ride), Some("ride"));
    let (zuse, _) = tislus(rest);
    assert_eq!(term_face(zuse), Some("zuse"));

    // lull's `=>  ..part` and `~%` registration lines are dropped only
    // before the first core; the `~%` inside an arm stays.
    let lull = arvo.join("sys/lull.hoon");
    let sanitized =
        parse_native_hoon_leaf_with_mode(&lull, &arvo, false, ScopeMode::Urbit).expect("lull");
    let verbatim =
        parse_native_hoon_leaf_with_mode(&lull, &arvo, false, ScopeMode::Standard).expect("raw");
    assert_ne!(sanitized, verbatim);
    let sanitized_debug = format!("{sanitized:?}");
    let verbatim_debug = format!("{verbatim:?}");
    // Dropping the two header runes leaves a strictly smaller tree.
    assert!(sanitized_debug.len() < verbatim_debug.len());

    // A header-only sys file sanitizes to nothing and falls back to the
    // original source, which does not parse.
    let header_only = tree.write(
        "pkg/arvo/sys/header.hoon",
        "=>  ..part\n~%  %x  ..part  ~\n",
    );
    assert!(
        parse_native_hoon_leaf_with_mode(&header_only, &arvo, false, ScopeMode::Urbit).is_err()
    );

    // Outside a pkg/arvo tree that has a sys directory, the deps dir is the
    // root and no ambient imports are added.
    let loose = Tree::new();
    let not_pkg = loose.write("notpkg/arvo/sys/zuse.hoon", "|%\n++  z  1\n--\n");
    let no_sys = loose.write("pkg/arvo/app/x.hoon", "42\n");
    for (entry, deps) in [
        (not_pkg, loose.path("notpkg/arvo")),
        (no_sys, loose.path("pkg/arvo")),
    ] {
        let imports = resolve_native_imports(&entry, &deps, ScopeMode::Urbit).expect("resolve");
        assert!(imports.is_empty(), "{entry:?}: {imports:?}");
    }
}

#[cfg(target_os = "linux")]
#[test]
fn urbit_scope_skips_files_without_a_utf8_stem() {
    use std::ffi::OsStr;
    use std::os::unix::ffi::OsStrExt;

    let (_tree, arvo) = arvo_tree();
    let name = OsStr::from_bytes(b"\xff\xfe.hoon");
    let path = arvo.join("app").join(name);
    fs::write(&path, "42\n").expect("write non-utf8 name");
    let imports = resolve_native_imports(&path, &arvo, ScopeMode::Urbit).expect("resolve");
    assert!(imports.is_empty());
    let expr =
        parse_native_hoon_leaf_with_mode(&path, &arvo, false, ScopeMode::Urbit).expect("parse");
    assert!(!matches!(expr, Hoon::TisLus(..)));
}

//! Coverage-driven end-to-end tests for the honk CLI: argument handling,
//! output modes, batch manifests, subject-type overrides, non-canonical
//! preludes, wrapper asset dumps, build failures, and the pinned and changed
//! softed constraints.
//!
//! Added to close branch-coverage gaps; see the coverage report in the PR.

// `args!` must yield a Vec for the call sites that extend it.
#![allow(clippy::useless_vec)]

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use nockapp::noun::slab::NounSlab;
use nockvm::noun::{Noun, NounAllocator};

type Arg<'a> = &'a dyn AsRef<std::ffi::OsStr>;

/// Borrows each argument as an `OsStr`-like trait object.
macro_rules! args {
    ($($arg:expr),* $(,)?) => {
        vec![$(&$arg as Arg<'_>),*]
    };
}

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("repo root")
        .to_path_buf()
}

fn prelude() -> PathBuf {
    repo_root().join("hoon/common/hoon.hoon")
}

fn honk(args: &[Arg<'_>], cwd: &Path) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_honk"));
    command
        .current_dir(cwd)
        .env("HOME", cwd)
        .env("XDG_DATA_HOME", cwd.join(".local/share"))
        .env("XDG_CONFIG_HOME", cwd.join(".config"))
        .env("TMPDIR", cwd)
        .env_remove("HONK_NATIVE_PARITY")
        .env_remove("NATIVE_HOON_TRACE")
        .env_remove("HONK_IR_ROUNDTRIP");
    for arg in args {
        command.arg(arg);
    }
    command.output().expect("run honk")
}

fn honk_env(args: &[Arg<'_>], cwd: &Path, env: &[(&str, &str)]) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_honk"));
    command
        .current_dir(cwd)
        .env("HOME", cwd)
        .env("TMPDIR", cwd)
        .env_remove("HONK_NATIVE_PARITY");
    for (key, value) in env {
        command.env(key, value);
    }
    for arg in args {
        command.arg(arg);
    }
    command.output().expect("run honk")
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

fn assert_ok(output: &Output) {
    assert!(
        output.status.success(),
        "honk failed ({:?}):\n{}",
        output.status.code(),
        stderr(output)
    );
}

fn write(root: &Path, rel: &str, contents: impl AsRef<[u8]>) -> PathBuf {
    let path = root.join(rel);
    fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");
    fs::write(&path, contents).expect("write");
    path
}

/// A small dependency tree: a raw import, a transitive import, and data.
fn deps_tree(root: &Path) -> PathBuf {
    let deps = root.join("deps");
    write(&deps, "common/raw.hoon", "[%raw (add 1 2)]\n");
    // Same bytes as raw.hoon: compiled once and reused by content.
    write(&deps, "common/raw-copy.hoon", "[%raw (add 1 2)]\n");
    write(
        &deps, "common/deep.hoon", "/=  inner  /common/raw\n[%deep inner]\n",
    );
    write(&deps, "data/blob.jam", b"hi\x01\x02\x00\x00");
    write(
        &deps,
        "app/arb.hoon",
        "/=  raw  /common/raw\n/=  copy  /common/raw-copy\n/=  deep  /common/deep\n/*  blob  %jam  /data/blob/jam\n[raw copy deep p.blob q.blob]\n",
    );
    write(
        &deps, "app/kernel.hoon", "/=  raw  /common/raw\n|=  hash=@uvI\n^-  *\n[hash raw]\n",
    );
    write(&deps, "app/gate.hoon", "|=  a=@ud\n[a (add a 1)]\n");
    deps
}

fn cue(bytes: &[u8]) -> (NounSlab, Noun) {
    let mut slab: NounSlab = NounSlab::new();
    let noun = slab.cue_into(bytes.to_vec().into()).expect("cue");
    (slab, noun)
}

fn rejam(slab: &NounSlab, noun: Noun) -> Vec<u8> {
    let mut out: NounSlab = NounSlab::new();
    let copied = out.copy_into(noun, &slab.noun_space());
    out.set_root(copied);
    out.jam().to_vec()
}

fn build(cwd: &Path, mode: Option<&str>, entry: &Path, deps: &Path, name: &str) -> Vec<u8> {
    let output = cwd.join(name);
    let prelude = prelude();
    let mut args: Vec<Arg<'_>> = args![];
    if let Some(mode) = mode.as_ref() {
        args.push(mode);
    }
    args.extend(args!["--output", output, "--prelude", prelude, entry, deps]);
    assert_ok(&honk(&args, cwd));
    fs::read(output).expect("read artifact")
}

#[test]
fn c6_cli_argument_errors_print_usage_and_exit_2() {
    let temp = tempfile::tempdir().expect("tempdir");
    let cwd = temp.path();
    let cases: &[(&[&str], &str)] = &[
        (&["--arbitrary", "--dynock"], "mutually exclusive"),
        (&["--dynock", "--dynock-typed"], "mutually exclusive"),
        (&["--output"], "missing value after --output"),
        (&["--prelude"], "missing value after --prelude"),
        (&["--sut-jam"], "missing value after --sut-jam"),
        (&["--cache-dir"], "missing value after --cache-dir"),
        (
            &["--batch-manifest"],
            "missing value after --batch-manifest",
        ),
        (
            &["--dump-wrapper-assets"],
            "missing value after --dump-wrapper-assets",
        ),
        (
            &["--dump-native-wrapper-assets"],
            "missing value after --dump-native-wrapper-assets",
        ),
        (&["--bogus"], "unknown argument: --bogus"),
        (
            &["--dump-wrapper-assets", "d", "--dump-native-wrapper-assets", "e"],
            "are mutually exclusive",
        ),
        (
            &["--dump-wrapper-assets", "d", "--output", "o", "deps"],
            "cannot be combined with --output",
        ),
        (
            &["--dump-wrapper-assets", "d", "--batch-manifest", "m", "deps"],
            "cannot be combined with --output",
        ),
        (
            &["--dump-wrapper-assets", "d", "--arbitrary", "deps"],
            "cannot be combined with compile mode flags",
        ),
        (
            &["--dump-wrapper-assets", "d"],
            "expected <deps_dir> with --dump-wrapper-assets",
        ),
        (
            &["--dump-wrapper-assets", "d", "deps"],
            "missing required --prelude",
        ),
        (
            &["--dump-native-wrapper-assets", "d", "--output", "o", "deps"],
            "cannot be combined with --output",
        ),
        (
            &["--dump-native-wrapper-assets", "d", "--batch-manifest", "m", "deps"],
            "cannot be combined with --output",
        ),
        (
            &["--dump-native-wrapper-assets", "d", "--dynock", "deps"],
            "cannot be combined with compile mode flags",
        ),
        (
            &["--dump-native-wrapper-assets", "d", "a", "b"],
            "expected <deps_dir> with --dump-native-wrapper-assets",
        ),
        (
            &["--dump-native-wrapper-assets", "d", "deps"],
            "missing required --prelude",
        ),
        (
            &["--batch-manifest", "m", "--output", "o", "deps"],
            "--output is not used with --batch-manifest",
        ),
        (
            &["--batch-manifest", "m", "--dynock-typed", "deps"],
            "use per-entry modes",
        ),
        (
            &["--batch-manifest", "m"],
            "expected <deps_dir> with --batch-manifest",
        ),
        (
            &["--batch-manifest", "m", "deps"],
            "missing required --prelude",
        ),
        (&["entry.hoon"], "expected <entry> and <deps_dir>"),
        (
            &["--prelude", "p", "entry.hoon", "deps"],
            "missing required --output",
        ),
        (
            &["--output", "o", "entry.hoon", "deps"],
            "missing required --prelude",
        ),
    ];
    for (args, expected) in cases {
        let argv: Vec<Arg<'_>> = args.iter().map(|arg| arg as Arg<'_>).collect();
        let output = honk(&argv, cwd);
        let err = stderr(&output);
        assert_eq!(output.status.code(), Some(2), "{args:?}: {err}");
        assert!(err.contains(expected), "expected {expected:?} in:\n{err}");
        assert!(err.contains("Usage:"), "usage missing from:\n{err}");
    }

    for help in ["--help", "-h"] {
        let output = honk(&args![help], cwd);
        assert_eq!(output.status.code(), Some(0));
        assert!(String::from_utf8_lossy(&output.stdout).contains("--batch-manifest <file>"));
    }
}

#[test]
fn c6_cli_subcommands_and_runtime_errors() {
    let temp = tempfile::tempdir().expect("tempdir");
    let cwd = temp.path();

    let output = honk(&args!["nockasm", "help"], cwd);
    assert_ok(&output);
    assert!(String::from_utf8_lossy(&output.stdout).contains("honk nockasm export"));
    let output = honk(&args!["nockasm"], cwd);
    assert_eq!(output.status.code(), Some(1));
    assert!(stderr(&output).contains("nockasm artifact operation failed"));

    let cache = cwd.join("cache");
    let output = honk(&args!["cache", "stats", "--cache-dir", cache], cwd);
    assert_ok(&output);
    assert!(String::from_utf8_lossy(&output.stdout).contains("objects=0"));
    let output = honk(&args!["cache"], cwd);
    assert_eq!(output.status.code(), Some(1));
    assert!(stderr(&output).contains("cache operation failed"));

    // Errors after argument parsing surface as compile failures.
    let deps = deps_tree(cwd);
    let entry = deps.join("app/gate.hoon");
    let out = cwd.join("out.jam");
    let missing = cwd.join("missing");
    let prelude = prelude();
    for args in [
        args!["--output", out, "--prelude", missing, entry, deps],
        args!["--output", out, "--prelude", prelude, "--sut-jam", missing, entry, deps],
        args!["--output", out, "--prelude", prelude, missing, deps],
        args!["--batch-manifest", missing, "--prelude", prelude, deps],
    ] {
        let output = honk(&args, cwd);
        assert_eq!(output.status.code(), Some(1), "{}", stderr(&output));
        assert!(stderr(&output).contains("native hoon compile failed"));
    }
    assert!(!out.exists());

    // Source-level failures: a syntax error, a missing import, and an import
    // cycle. hoonc parses the whole dependency tree first, so each gets its
    // own tree, and a broken file fails the build even when nothing imports
    // it.
    let cases = [
        (
            "bad", "app/bad.hoon", "|=  a=@\n(\n", "native parser failed",
        ),
        (
            "missing", "app/missing.hoon", "/=  x  /common/nope\n42\n", "native import not found",
        ),
        (
            "cycle", "cycle/a.hoon", "/=  b  /cycle/b\n42\n", "import cycle",
        ),
    ];
    for (name, rel, contents, expected) in cases {
        let deps = deps_tree(&cwd.join(name));
        let broken = write(&deps, rel, contents);
        if name == "cycle" {
            write(&deps, "cycle/b.hoon", "/=  a  /cycle/a\n42\n");
        }
        let cache = cwd.join(format!("{name}-cache"));
        for entry in [broken, deps.join("app/gate.hoon")] {
            for args in [
                args!["--arbitrary", "--output", out, "--prelude", prelude, entry, deps],
                args![
                    "--arbitrary", "--cache-dir", cache, "--output", out, "--prelude", prelude,
                    entry, deps
                ],
            ] {
                let output = honk(&args, cwd);
                assert_eq!(output.status.code(), Some(1), "{}", stderr(&output));
                assert!(
                    stderr(&output).contains(expected),
                    "{name}: {}",
                    stderr(&output)
                );
            }
        }
    }
    assert!(!out.exists());
}

#[test]
fn c6_cli_dynock_outputs_wrap_the_formula_in_a_trap() {
    let temp = tempfile::tempdir().expect("tempdir");
    let cwd = temp.path();
    let deps = deps_tree(cwd);
    let entry = deps.join("app/gate.hoon");
    let untyped = build(cwd, Some("--dynock"), &entry, &deps, "dyn.jam");
    let typed = build(cwd, Some("--dynock-typed"), &entry, &deps, "typed.jam");

    let (untyped_slab, untyped) = cue(&untyped);
    let (typed_slab, typed) = cue(&typed);
    let untyped_space = untyped_slab.noun_space();
    let typed_space = typed_slab.noun_space();
    let untyped = untyped.in_space(&untyped_space).as_cell().expect("cell");
    let typed = typed.in_space(&typed_space).as_cell().expect("cell");
    // [%noun trap] versus [inferred-type trap], with the same trap.
    assert!(untyped
        .head()
        .as_atom()
        .expect("noun tag")
        .eq_bytes(b"noun"));
    let typed_head = typed.head().as_cell().expect("typed header is a type");
    assert!(typed_head
        .head()
        .as_atom()
        .expect("type tag")
        .eq_bytes(b"core"));
    assert_eq!(
        rejam(&untyped_slab, untyped.tail().noun()),
        rejam(&typed_slab, typed.tail().noun())
    );
    // The trap is [[1 formula] 0].
    let trap = untyped.tail().as_cell().expect("trap");
    assert_eq!(
        trap.tail().as_atom().expect("payload").as_u64().ok(),
        Some(0)
    );
    let battery = trap.head().as_cell().expect("battery");
    assert_eq!(battery.head().as_atom().expect("op").as_u64().ok(), Some(1));
}

#[test]
fn c6_cli_subject_type_override_and_relative_output() {
    let temp = tempfile::tempdir().expect("tempdir");
    let cwd = temp.path();
    let deps = deps_tree(cwd);
    let entry = deps.join("app/arb.hoon");
    let reference = build(cwd, Some("--arbitrary"), &entry, &deps, "ref.jam");

    // Passing the embedded subject type explicitly changes nothing, and a
    // bare output name is written to the working directory.
    let sut = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("assets/honc-type-138.jam");
    let prelude = prelude();
    let output = honk(
        &args![
            "--arbitrary", "--no-dbug", "--dbug", "--no-vet", "--strict", "--sut-jam", sut,
            "--output", "sut.jam", "--prelude", prelude, entry, deps
        ],
        cwd,
    );
    assert_ok(&output);
    assert_eq!(
        fs::read(cwd.join("sut.jam")).expect("sut artifact"),
        reference
    );

    // Without spots the artifact is smaller; the IR round trip checks the
    // subject type without changing the output.
    let output = honk_env(
        &args![
            "--arbitrary", "--no-dbug", "--output", "nodbug.jam", "--prelude", prelude, entry, deps
        ],
        cwd,
        &[("HONK_IR_ROUNDTRIP", "1")],
    );
    assert_ok(&output);
    let nodbug = fs::read(cwd.join("nodbug.jam")).expect("no-dbug artifact");
    assert!(nodbug.len() < reference.len());

    // A subject type that is not a [%cell deps prelude] type is rejected.
    let bad_sut = {
        let mut slab: NounSlab = NounSlab::new();
        slab.set_root(nockvm::noun::D(1));
        write(cwd, "bad-sut.jam", slab.jam())
    };
    let output = honk(
        &args![
            "--arbitrary", "--sut-jam", bad_sut, "--output", "bad.jam", "--prelude", prelude,
            entry, deps
        ],
        cwd,
    );
    assert_eq!(output.status.code(), Some(1));
    assert!(
        stderr(&output).contains("subject type jam did not contain a Hoon type noun"),
        "{}",
        stderr(&output)
    );
}

#[test]
fn c6_cli_batch_manifest_matches_single_builds_and_reuses_the_cache() {
    let temp = tempfile::tempdir().expect("tempdir");
    let cwd = temp.path();
    let deps = deps_tree(cwd);
    let arb = deps.join("app/arb.hoon");
    let kernel = deps.join("app/kernel.hoon");
    let gate = deps.join("app/gate.hoon");
    // Distinct entries: listing one entry twice in a --cache-dir batch fails
    // (two pending cache roots with the same key; see UNCOVERED.md).
    let gate2 = write(&deps, "app/gate2.hoon", "|=  a=@ud\n[a (add a 2)]\n");
    let singles = [
        build(cwd, Some("--arbitrary"), &arb, &deps, "s-arb.jam"),
        build(cwd, None, &kernel, &deps, "s-kernel.jam"),
        build(cwd, Some("--dynock"), &gate, &deps, "s-dyn.jam"),
        build(cwd, Some("--dynock-typed"), &gate2, &deps, "s-typed.jam"),
    ];

    // Every file of the dependency tree, listed explicitly for the kernel's
    // directory hash: the same set hoonc hashes.
    let mut files: Vec<String> = walkdir::WalkDir::new(&deps)
        .into_iter()
        .filter_map(Result::ok)
        .filter(|entry| entry.file_type().is_file())
        .map(|entry| entry.path().display().to_string())
        .collect();
    files.sort();
    let manifest = cwd.join("batch.tsv");
    fs::write(
        &manifest,
        format!(
            "\nb/arb.jam\t{}\tarbitrary\nb/nested/kernel.jam\t{}\tstandard\t{}\nb/dyn.jam\t{}\tdynock\nb/typed.jam\t{}\tdynock-typed\n",
            arb.display(),
            kernel.display(),
            files.join("\t"),
            gate.display(),
            gate2.display()
        ),
    )
    .expect("manifest");
    let prelude = prelude();
    let cache = cwd.join("cache");
    for pass in ["cold", "warm"] {
        let output = honk(
            &args!["--batch-manifest", manifest, "--cache-dir", cache, "--prelude", prelude, deps],
            cwd,
        );
        assert_ok(&output);
        if pass == "warm" {
            assert!(stderr(&output).contains("writes=0"), "{}", stderr(&output));
        }
        for (name, single) in ["b/arb.jam", "b/nested/kernel.jam", "b/dyn.jam", "b/typed.jam"]
            .iter()
            .zip(&singles)
        {
            assert_eq!(
                &fs::read(cwd.join(name)).expect("batch artifact"),
                single,
                "{pass} {name}"
            );
        }
    }

    // Changing the entry misses its product but reuses the cached
    // dependency vases; a bare output name lands in the working directory.
    fs::write(
        &arb,
        format!("{}::  changed\n", fs::read_to_string(&arb).expect("entry")),
    )
    .expect("edit entry");
    let fresh = build(cwd, Some("--arbitrary"), &arb, &deps, "fresh-arb.jam");
    fs::write(
        &manifest,
        format!("bare.jam\t{}\tarbitrary\n", arb.display()),
    )
    .expect("manifest");
    let output = honk(
        &args!["--batch-manifest", manifest, "--cache-dir", cache, "--prelude", prelude, deps],
        cwd,
    );
    assert_ok(&output);
    assert!(stderr(&output).contains("misses=1"), "{}", stderr(&output));
    assert_eq!(fs::read(cwd.join("bare.jam")).expect("bare"), fresh);

    // A kernel whose directory list omits files hashes a different tree.
    fs::write(
        &manifest,
        format!(
            "b/partial.jam\t{}\tstandard\t{}\t{}\n",
            kernel.display(),
            kernel.display(),
            cwd.join("not-a-file").display()
        ),
    )
    .expect("manifest");
    assert_ok(&honk(
        &args!["--batch-manifest", manifest, "--prelude", prelude, deps],
        cwd,
    ));
    assert_ne!(
        fs::read(cwd.join("b/partial.jam")).expect("partial"),
        singles[1]
    );

    for (contents, expected) in [
        ("only\ttwo\n", "expected tab-separated output, entry, mode"),
        (
            "o.jam\te.hoon\tweird\n", "unknown batch compile mode: weird",
        ),
        ("\n\n", "batch manifest has no entries"),
    ] {
        fs::write(&manifest, contents).expect("manifest");
        let output = honk(
            &args!["--batch-manifest", manifest, "--prelude", prelude, deps],
            cwd,
        );
        assert_eq!(output.status.code(), Some(1));
        assert!(stderr(&output).contains(expected), "{}", stderr(&output));
    }
}

#[test]
fn c6_cli_non_canonical_preludes_are_minted_natively() {
    let temp = tempfile::tempdir().expect("tempdir");
    let cwd = temp.path();
    let deps = deps_tree(cwd);
    let plain = write(
        cwd,
        "preludes/plain.hoon",
        "|%\n++  add  |=([a=@ b=@] ?:(=(0 a) b $(a (dec a), b +(b))))\n++  dec  |=(a=@ ~>(%mean.'dec' ?<(=(0 a) =+(b=0 |-(?:(=(a +(b)) b $(b +(b))))))))\n--\n",
    );
    let chained = write(
        cwd,
        "preludes/chained.hoon",
        "=<  ride\n=>  %138\n|%\n++  ride  |=(a=@ a)\n++  add  |=([a=@ b=@] ?:(=(0 a) b $(a (dec a), b +(b))))\n++  dec  |=(a=@ ~>(%mean.'dec' ?<(=(0 a) =+(b=0 |-(?:(=(a +(b)) b $(b +(b))))))))\n--\n",
    );
    let sut = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("assets/honc-type-138.jam");
    let arb = deps.join("app/arb.hoon");
    let kernel = deps.join("app/kernel.hoon");
    let canonical = build(cwd, Some("--arbitrary"), &arb, &deps, "canonical.jam");

    let run =
        |prelude: &Path, mode: Option<&str>, entry: &Path, env: &[(&str, &str)], name: &str| {
            let output_path = cwd.join(name);
            let mut args: Vec<Arg<'_>> = args![];
            if let Some(mode) = mode.as_ref() {
                args.push(mode);
            }
            args.extend(args![
                "--output", output_path, "--prelude", prelude, entry, deps
            ]);
            let output = honk_env(&args, cwd, env);
            assert_ok(&output);
            (fs::read(output_path).expect("artifact"), stderr(&output))
        };

    // The data import uses the local `[p=@ud q=@]` octs type here.
    let (plain_arb, log) = run(
        &plain,
        Some("--arbitrary"),
        &arb,
        &[("HONK_IR_ROUNDTRIP", "1"), ("NATIVE_HOON_TRACE", "1")],
        "plain-arb.jam",
    );
    // The prelude is not `=<`, so it is minted whole.
    assert!(
        log.contains("mint_honc_formula path: prelude root ="),
        "{log}"
    );
    assert!(!log.contains("chunked prelude"), "{log}");
    assert!(log.contains("[honk] start"), "{log}");
    assert_ne!(plain_arb, canonical);
    let (again, _) = run(
        &plain,
        Some("--arbitrary"),
        &arb,
        &[("NATIVE_HOON_SKIP_BURP", "1"), ("HONK_IR_ROUNDTRIP", "1")],
        "plain-arb-noburp.jam",
    );
    assert!(!again.is_empty());
    let (plain_kernel, _) = run(&plain, None, &kernel, &[], "plain-kernel.jam");
    assert!(!plain_kernel.is_empty());

    // A `=<` prelude is minted one layer at a time unless chunking is off.
    let (chunked, log) = run(
        &chained,
        Some("--arbitrary"),
        &arb,
        &[("NATIVE_HOON_TRACE", "1")],
        "chunked.jam",
    );
    assert!(log.contains("chunked prelude: 2 layers + ride"), "{log}");
    assert!(log.contains("chunked prelude: minted layer 3/3"), "{log}");
    let (untraced, log) = run(&chained, Some("--arbitrary"), &arb, &[], "untraced.jam");
    assert!(!log.contains("minted layer"), "{log}");
    assert_eq!(untraced, chunked);
    let (whole, log) = run(
        &chained,
        Some("--arbitrary"),
        &arb,
        &[("NATIVE_HOON_NO_CHUNK", "1")],
        "whole.jam",
    );
    assert!(!log.contains("chunked prelude"), "{log}");
    // The two routes are not byte-identical (the whole-prelude mint burps
    // only the final type); only the chunked route is parity-checked, via
    // the hoon-138 self-mint.
    assert!(!chunked.is_empty() && !whole.is_empty());

    // An explicit subject type replaces the minted prelude type.
    let sut_prelude = {
        let output_path = cwd.join("plain-sut.jam");
        let output = honk(
            &args![
                "--arbitrary",
                "--sut-jam",
                sut,
                "--output",
                output_path,
                "--prelude",
                plain,
                deps.join("app/gate.hoon"),
                deps
            ],
            cwd,
        );
        assert_ok(&output);
        fs::read(output_path).expect("artifact")
    };
    assert!(!sut_prelude.is_empty());
}

#[test]
fn c6_cli_trace_and_ir_roundtrip_diagnostics_do_not_change_output() {
    let temp = tempfile::tempdir().expect("tempdir");
    let cwd = temp.path();
    let deps = deps_tree(cwd);
    let entry = deps.join("app/gate.hoon");
    let reference = build(cwd, Some("--arbitrary"), &entry, &deps, "ref.jam");
    let output_path = cwd.join("traced.jam");
    let prelude = prelude();
    let output = honk_env(
        &args!["--arbitrary", "--output", output_path, "--prelude", prelude, entry, deps],
        cwd,
        &[("HONK_IR_ROUNDTRIP", "1"), ("NATIVE_HOON_TRACE", "1")],
    );
    assert_ok(&output);
    let log = stderr(&output);
    assert!(log.contains("[ir-roundtrip] prelude TYPE OK"), "{log}");
    assert!(log.contains("[honk] compiling"), "{log}");
    assert_eq!(fs::read(output_path).expect("artifact"), reference);
}

#[test]
fn c6_cli_wrapper_asset_dumps_agree() {
    let temp = tempfile::tempdir().expect("tempdir");
    let cwd = temp.path();
    let deps = cwd.join("deps");
    fs::create_dir_all(&deps).expect("deps");
    let prelude = prelude();
    let native = cwd.join("native");
    let dynamic = cwd.join("dynamic");
    assert_ok(&honk(
        &args!["--dump-native-wrapper-assets", native, "--prelude", prelude, deps],
        cwd,
    ));
    assert_ok(&honk(
        &args!["--new", "--dump-wrapper-assets", dynamic, "--prelude", prelude, deps],
        cwd,
    ));
    let mut names: Vec<String> = fs::read_dir(&native)
        .expect("native dir")
        .map(|entry| {
            entry
                .expect("entry")
                .file_name()
                .to_string_lossy()
                .into_owned()
        })
        .collect();
    names.sort();
    assert_eq!(names.len(), 11, "{names:?}");
    for name in &names {
        assert_eq!(
            fs::read(native.join(name)).expect("native asset"),
            fs::read(dynamic.join(name)).expect("dynamic asset"),
            "{name}"
        );
    }
    // The dynamic dump also serializes the cold jet state.
    assert!(
        fs::metadata(dynamic.join("honc-cold-138.jam"))
            .expect("cold")
            .len()
            > 0
    );

    // An explicit subject type and the diagnostics do not change the dump.
    let sut = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("assets/honc-type-138.jam");
    let traced = cwd.join("traced");
    let output = honk_env(
        &args!["--dump-wrapper-assets", traced, "--sut-jam", sut, "--prelude", prelude, deps],
        cwd,
        &[("HONK_IR_ROUNDTRIP", "1"), ("NATIVE_HOON_TRACE", "1")],
    );
    assert_ok(&output);
    assert!(
        stderr(&output).contains("[ir-intern] prelude TYPE"),
        "{}",
        stderr(&output)
    );
    for name in &names {
        assert_eq!(
            fs::read(native.join(name)).expect("native asset"),
            fs::read(traced.join(name)).expect("traced asset"),
            "{name}"
        );
    }

    // The dynamic wrappers need hoon.hoon's compiler; a bare prelude cannot
    // build them.
    let tiny = write(cwd, "tiny.hoon", "|%\n++  x  1\n--\n");
    let output = honk_env(
        &args!["--dump-wrapper-assets", cwd.join("tiny-out"), "--prelude", tiny, deps],
        cwd,
        &[("NATIVE_HOON_TRACE", "1")],
    );
    assert_eq!(output.status.code(), Some(1), "{}", stderr(&output));
    assert!(
        stderr(&output).contains("[honk] failed"),
        "{}",
        stderr(&output)
    );
}

#[test]
fn c6_cli_changed_softed_constraints_are_delegated_to_hoonc() {
    let temp = tempfile::tempdir().expect("tempdir");
    let cwd = temp.path();
    let deps = cwd.join("deps");
    let softed = write(
        &deps, "common/entry.hoon",
        "/#  softed-constraints\n|%\n++  value  softed-constraints\n--\n",
    );
    write(&deps, "dat/softed-constraints.hoon", "::  forked\n42\n");
    let plain = write(&deps, "common/plain.hoon", "|=  a=@\n[a a]\n");
    let prelude = prelude();

    let output = honk(
        &args!["--arbitrary", "--output", "single.jam", "--prelude", prelude, softed, deps],
        cwd,
    );
    assert_ok(&output);
    let single = fs::read(cwd.join("single.jam")).expect("delegated artifact");
    assert!(!single.is_empty());

    // One entry with changed constraints sends the whole batch to hoonc.
    let manifest = cwd.join("batch.tsv");
    fs::write(
        &manifest,
        format!(
            "b/softed.jam\t{}\tarbitrary\nb/plain.jam\t{}\tdynock\n",
            softed.display(),
            plain.display()
        ),
    )
    .expect("manifest");
    assert_ok(&honk(
        &args!["--batch-manifest", manifest, "--prelude", prelude, deps],
        cwd,
    ));
    assert_eq!(fs::read(cwd.join("b/softed.jam")).expect("softed"), single);
    let native_plain = build(cwd, Some("--dynock"), &plain, &deps, "plain.jam");
    assert_eq!(
        fs::read(cwd.join("b/plain.jam")).expect("plain"),
        native_plain
    );
}

#[test]
fn c6_cli_failures_before_and_during_the_build() {
    let temp = tempfile::tempdir().expect("tempdir");
    let cwd = temp.path();
    let prelude = prelude();
    let out = cwd.join("out.jam");

    // A batch entry whose dependency tree holds a file that does not parse.
    let deps = deps_tree(&cwd.join("broken"));
    write(&deps, "junk/bad.hoon", "|=  a=@\n(\n");
    let manifest = write(
        cwd,
        "broken.tsv",
        format!(
            "b/gate.jam\t{}\tarbitrary\n",
            deps.join("app/gate.hoon").display()
        ),
    );
    let output = honk(
        &args!["--batch-manifest", manifest, "--prelude", prelude, deps],
        cwd,
    );
    assert_eq!(output.status.code(), Some(1), "{}", stderr(&output));
    assert!(
        stderr(&output).contains("native parser failed"),
        "{}",
        stderr(&output)
    );
    assert!(!cwd.join("b/gate.jam").exists());

    // A prelude that does not parse, and one that parses but does not build.
    let deps = deps_tree(&cwd.join("ok"));
    let gate = deps.join("app/gate.hoon");
    let unparsable = write(cwd, "preludes/unparsable.hoon", "|%\n++  x\n");
    let unbuildable = write(
        cwd, "preludes/unbuildable.hoon", "|%\n++  x  (nope 1)\n--\n",
    );
    let native = cwd.join("native-dump");
    for args in [
        args!["--arbitrary", "--output", out, "--prelude", unparsable, gate, deps],
        args!["--dump-native-wrapper-assets", native, "--prelude", unbuildable, deps],
    ] {
        let output = honk(&args, cwd);
        assert_eq!(output.status.code(), Some(1), "{}", stderr(&output));
        assert!(
            stderr(&output).contains("native hoon compile failed"),
            "{}",
            stderr(&output)
        );
    }
    assert!(!out.exists());
    assert!(!native.exists());

    // A standard (kernel) build slams the product with the directory hash, so
    // an entry that is not a gate fails, alone or in a batch.
    let not_gate = write(&deps, "app/not-gate.hoon", "[%not %a %gate]\n");
    let manifest = write(
        cwd,
        "not-gate.tsv",
        format!("b/not-gate.jam\t{}\tstandard\n", not_gate.display()),
    );
    for args in [
        args!["--output", out, "--prelude", prelude, not_gate, deps],
        args!["--batch-manifest", manifest, "--prelude", prelude, deps],
    ] {
        let output = honk(&args, cwd);
        assert_eq!(output.status.code(), Some(1), "{}", stderr(&output));
        assert!(
            stderr(&output).contains("failed to build deferred trap"),
            "{}",
            stderr(&output)
        );
    }
    assert!(!out.exists());
    assert!(!cwd.join("b/not-gate.jam").exists());

    // A panic on the worker thread is reported as a failed compile. hatch
    // panics on a `!?` version miss, where hoon-138's `open` crashes.
    let entry = write(&deps, "app/version.hoon", "!?(100 42)\n");
    let output = honk(
        &args!["--arbitrary", "--output", out, "--prelude", prelude, entry, deps],
        cwd,
    );
    assert_eq!(output.status.code(), Some(1), "{}", stderr(&output));
    assert!(
        stderr(&output).contains("native compiler worker thread panicked"),
        "{}",
        stderr(&output)
    );
    assert!(!out.exists());
}

#[test]
fn c6_cli_pinned_softed_constraints_and_dat_nodes_build_natively() {
    let temp = tempfile::tempdir().expect("tempdir");
    let cwd = temp.path();
    let deps = cwd.join("deps");
    let repo = repo_root();
    // The pinned module and constraint jams, byte for byte, so the native
    // value override applies. Only those three files are pinned, so a stub
    // /common/zeke (a faceless import of the module) supplies its molds.
    for rel in [
        "dat/softed-constraints.hoon", "jams/constraints-0-1.jam", "jams/constraints-2.jam",
    ] {
        write(
            &deps,
            rel,
            fs::read(repo.join("hoon").join(rel)).expect("pinned file"),
        );
    }
    write(
        &deps,
        "common/zeke.hoon",
        "|%\n+$  preprocess  [preprocess-0-1 preprocess-2]\n+$  preprocess-0-1  *\n+$  preprocess-2  *\n--\n",
    );
    // An ordinary /dat node, which is kicked while it is built.
    write(&deps, "dat/const.hoon", "[%const 42]\n");
    let entry = write(
        &deps, "app/entry.hoon",
        "/#  softed-constraints\n/#  const\n[?=(^ softed-constraints) const]\n",
    );
    let prelude = prelude();
    let output = honk_env(
        &args!["--dynock", "--output", "out.jam", "--prelude", prelude, entry, deps],
        cwd,
        &[("NATIVE_HOON_TRACE", "1")],
    );
    assert_ok(&output);
    let log = stderr(&output);
    assert!(
        log.contains("[honk] cueing softed-constraints static jam pair"),
        "{log}"
    );
    assert!(log.contains("dat/const.hoon"), "{log}");
    assert!(!fs::read(cwd.join("out.jam")).expect("artifact").is_empty());
}

#[test]
fn c6_cli_dynamic_wrapper_dumps_with_a_minted_prelude_formula() {
    let temp = tempfile::tempdir().expect("tempdir");
    let cwd = temp.path();
    let deps = cwd.join("deps");
    fs::create_dir_all(&deps).expect("deps");
    let prelude = prelude();
    let sut = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("assets/honc-type-138.jam");
    let native = cwd.join("native");
    assert_ok(&honk(
        &args!["--dump-native-wrapper-assets", native, "--prelude", prelude, deps],
        cwd,
    ));

    // HONK_NATIVE_PARITY, or a prelude that is not byte-for-byte hoon-138,
    // keeps the prelude formula minted here instead of the embedded one. The
    // subject type is given, so the prelude is not played.
    let parity = cwd.join("parity");
    assert_ok(&honk_env(
        &args!["--dump-wrapper-assets", parity, "--sut-jam", sut, "--prelude", prelude, deps],
        cwd,
        &[("HONK_NATIVE_PARITY", "1")],
    ));
    let mut source = fs::read_to_string(&prelude).expect("prelude");
    source.push_str("::  not the canonical bytes\n");
    let changed = write(cwd, "changed-hoon.hoon", source);
    // With --no-dbug the wrappers are compiled without spots.
    let nodbug = cwd.join("nodbug");
    assert_ok(&honk(
        &args![
            "--no-dbug", "--dump-wrapper-assets", nodbug, "--sut-jam", sut, "--prelude", changed,
            deps
        ],
        cwd,
    ));

    let mut names: Vec<String> = fs::read_dir(&native)
        .expect("native dir")
        .map(|entry| {
            entry
                .expect("entry")
                .file_name()
                .to_string_lossy()
                .into_owned()
        })
        .collect();
    names.sort();
    assert_eq!(names.len(), 11, "{names:?}");
    for name in &names {
        let reference = fs::read(native.join(name)).expect("native asset");
        assert_eq!(
            fs::read(parity.join(name)).expect("parity asset"),
            reference,
            "{name}"
        );
        // Only the constant and data wrappers are always compiled without
        // spots.
        let spotless = name == "constant-vase-battery.jam" || name == "data-vase-battery.jam";
        assert_eq!(
            fs::read(nodbug.join(name)).expect("no-dbug asset") == reference,
            spotless,
            "{name}"
        );
    }
}

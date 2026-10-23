//! Smoke tests for the `kick` build-trap evaluator.
//!
//! Every fixture is a trap assembled here rather than a compiler artifact, so
//! the suite costs milliseconds and cannot rot against a checked-in jam: a
//! trap is just a core whose arm at axis 2 gets fired, and a constant formula
//! `[1 x]` is the smallest arm that produces a known product.

use std::path::Path;
use std::process::{Command, Output};

use nockapp::noun::slab::{NockJammer, NounSlab};
use nockvm::noun::{Noun, D, T};
use tempfile::TempDir;

/// Words of NockStack for a test process: the default reservation is 16GB of
/// virtual address space, and a suite running in parallel asks a
/// heuristic-overcommit kernel for more than it will grant.
const TEST_STACK_WORDS: &str = "33554432"; // 256MB

fn jam_of(build: impl FnOnce(&mut NounSlab<NockJammer>) -> Noun) -> Vec<u8> {
    let mut slab: NounSlab<NockJammer> = NounSlab::new();
    let root = build(&mut slab);
    slab.set_root(root);
    slab.jam().to_vec()
}

/// A core `[battery 0]`, the minimal thing `[9 2 0 1]` can fire.
fn trap_jam(battery: impl FnOnce(&mut NounSlab<NockJammer>) -> Noun) -> Vec<u8> {
    jam_of(|slab| {
        let battery = battery(slab);
        T(slab, &[battery, D(0)])
    })
}

/// A trap whose arm is the constant formula `[1 nouns]`.
fn constant_trap(nouns: &[u64]) -> Vec<u8> {
    trap_jam(|slab| {
        let mut formula = vec![D(1)];
        formula.extend(nouns.iter().map(|value| D(*value)));
        T(slab, &formula)
    })
}

fn run_kick(dir: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_kick"))
        .current_dir(dir)
        .env("HONK_EVAL_STACK_WORDS", TEST_STACK_WORDS)
        .args(args)
        .output()
        .expect("run kick")
}

fn stderr_of(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

/// The happy path: fire a trap, get the jam of its product.
#[test]
fn fires_a_trap_and_jams_the_product() {
    let dir = TempDir::new().unwrap();
    std::fs::write(dir.path().join("trap.jam"), constant_trap(&[42])).unwrap();

    let output = run_kick(dir.path(), &["trap.jam", "out.jam"]);
    assert!(output.status.success(), "{}", stderr_of(&output));
    assert_eq!(
        std::fs::read(dir.path().join("out.jam")).unwrap(),
        jam_of(|_| D(42))
    );
}

/// An axis argument selects a subnoun of the product.
#[test]
fn selects_a_subnoun_by_axis() {
    let dir = TempDir::new().unwrap();
    // Product `[42 43]`; axis 3 is its tail.
    std::fs::write(dir.path().join("trap.jam"), constant_trap(&[42, 43])).unwrap();

    let output = run_kick(dir.path(), &["trap.jam", "out.jam", "3"]);
    assert!(output.status.success(), "{}", stderr_of(&output));
    assert_eq!(
        std::fs::read(dir.path().join("out.jam")).unwrap(),
        jam_of(|_| D(43))
    );
}

/// Axis 0 names nothing. A bit-walking descent mistakes it for the root, so
/// it is the one axis worth a test of its own.
#[test]
fn rejects_axis_zero() {
    let dir = TempDir::new().unwrap();
    std::fs::write(dir.path().join("trap.jam"), constant_trap(&[42])).unwrap();

    let output = run_kick(dir.path(), &["trap.jam", "out.jam", "0"]);
    assert_eq!(output.status.code(), Some(2), "{}", stderr_of(&output));
    assert!(stderr_of(&output).contains("0 is not an axis"));
    assert!(!dir.path().join("out.jam").exists());
}

/// An axis is a path, not a machine word: one past 2^64 must be reported as
/// a miss, never truncated into a different axis and never a parse panic.
#[test]
fn accepts_an_axis_wider_than_a_machine_word() {
    let dir = TempDir::new().unwrap();
    std::fs::write(dir.path().join("trap.jam"), constant_trap(&[42])).unwrap();

    // 2^70, whose low 64 bits are 0 — a truncating parse would either panic
    // or silently ask for axis 0.
    let output = run_kick(
        dir.path(),
        &["trap.jam", "out.jam", "1180591620717411303424"],
    );
    assert_eq!(output.status.code(), Some(1), "{}", stderr_of(&output));
    let stderr = stderr_of(&output);
    assert!(stderr.contains("axis 1180591620717411303424"), "{stderr}");
    assert!(!stderr.contains("panicked"), "{stderr}");
}

/// Garbage in the input is a diagnostic, not an abort.
#[test]
fn reports_an_invalid_jam() {
    let dir = TempDir::new().unwrap();
    std::fs::write(dir.path().join("trap.jam"), b"not a jam at all").unwrap();

    let output = run_kick(dir.path(), &["trap.jam", "out.jam"]);
    assert_eq!(output.status.code(), Some(1), "{}", stderr_of(&output));
    let stderr = stderr_of(&output);
    assert!(!stderr.contains("panicked"), "{stderr}");
    assert!(!dir.path().join("out.jam").exists());
}

/// So is a missing input.
#[test]
fn reports_a_missing_input() {
    let dir = TempDir::new().unwrap();

    let output = run_kick(dir.path(), &["absent.jam", "out.jam"]);
    assert_eq!(output.status.code(), Some(1), "{}", stderr_of(&output));
    assert!(stderr_of(&output).contains("cannot read absent.jam"));
}

/// A trap that never returns is bounded by `--timeout`, which is the whole
/// point of the flag for a build system.
#[test]
fn times_out_on_a_trap_that_never_returns() {
    let dir = TempDir::new().unwrap();
    // Arm `[9 2 0 1]`: firing it fires it again, forever, in tail position.
    let spin = trap_jam(|slab| T(slab, &[D(9), D(2), D(0), D(1)]));
    std::fs::write(dir.path().join("spin.jam"), spin).unwrap();

    let output = run_kick(dir.path(), &["spin.jam", "out.jam", "--timeout", "1"]);
    assert_eq!(output.status.code(), Some(124), "{}", stderr_of(&output));
    assert!(stderr_of(&output).contains("timed out"));
}

/// Bad arguments are usage errors, distinct from failures.
#[test]
fn reports_bad_arguments() {
    let dir = TempDir::new().unwrap();

    for args in [
        vec!["trap.jam"],
        vec!["trap.jam", "out.jam", "--timeout"],
        vec!["trap.jam", "out.jam", "--timeout", "soon"],
        vec!["trap.jam", "out.jam", "--timeout", "0"],
        vec!["trap.jam", "out.jam", "--nonesuch"],
        vec!["trap.jam", "out.jam", "1", "2"],
    ] {
        let output = run_kick(dir.path(), &args);
        assert_eq!(output.status.code(), Some(2), "{args:?}");
    }
}

#[test]
fn prints_its_usage() {
    let dir = TempDir::new().unwrap();

    let output = run_kick(dir.path(), &["--help"]);
    assert!(output.status.success());
    assert!(String::from_utf8_lossy(&output.stdout).contains("usage: kick"));
}

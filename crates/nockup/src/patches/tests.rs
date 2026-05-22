use std::collections::BTreeMap;
use std::fs;

use tempfile::TempDir;

use super::*;
use crate::manifest::{PackagePatch, PatchOp};

/// Build a fresh empty project dir for a test.
fn scratch() -> TempDir {
    tempfile::tempdir().expect("tempdir")
}

fn patch_overwrite(file: &str, body: &str) -> PackagePatch {
    PackagePatch {
        file: file.into(),
        description: None,
        op: PatchOp::Overwrite {
            content: body.into(),
        },
    }
}

fn patch_replace(file: &str, pattern: &str, replacement: &str) -> PackagePatch {
    PackagePatch {
        file: file.into(),
        description: None,
        op: PatchOp::ReplacePattern {
            pattern: pattern.into(),
            replacement: replacement.into(),
            once: true,
        },
    }
}

fn patch_ensure_dep(file: &str, name: &str, value: toml::Value) -> PackagePatch {
    PackagePatch {
        file: file.into(),
        description: None,
        op: PatchOp::EnsureDependency {
            table: "dependencies".into(),
            name: name.into(),
            value,
        },
    }
}

fn patch_ensure_patch(
    file: &str,
    registry: &str,
    entries: BTreeMap<String, toml::Value>,
) -> PackagePatch {
    PackagePatch {
        file: file.into(),
        description: None,
        op: PatchOp::EnsurePatch {
            registry: registry.into(),
            entries,
        },
    }
}

fn opts() -> PatchOptions {
    PatchOptions { dry_run: false }
}

// --- overwrite -------------------------------------------------------------

#[test]
fn overwrite_applies_to_fresh_file() {
    let dir = scratch();
    let patches = vec![patch_overwrite("build.rs", "fn main() {}\n")];
    let report = apply_patches(dir.path(), &patches, &[], opts()).unwrap();
    let body = fs::read_to_string(dir.path().join("build.rs")).unwrap();
    assert_eq!(body, "fn main() {}\n");
    assert_eq!(report.applied.len(), 1);
    assert!(report.refused.is_empty());
}

#[test]
fn overwrite_is_idempotent() {
    let dir = scratch();
    let patches = vec![patch_overwrite("build.rs", "fn main() {}\n")];
    let first = apply_patches(dir.path(), &patches, &[], opts()).unwrap();
    // Re-running with the prior ledger should no-op — file already matches.
    let second = apply_patches(dir.path(), &patches, &first.applied, opts()).unwrap();
    assert_eq!(second.skipped_noop, 1);
    assert!(second.refused.is_empty());
}

#[test]
fn overwrite_refuses_user_edited_file() {
    let dir = scratch();
    let patches = vec![patch_overwrite("build.rs", "fn main() {}\n")];
    let first = apply_patches(dir.path(), &patches, &[], opts()).unwrap();
    // User edits the patched file out-of-band.
    fs::write(dir.path().join("build.rs"), "// user's own version\n").unwrap();
    let second = apply_patches(dir.path(), &patches, &first.applied, opts()).unwrap();
    assert_eq!(second.refused.len(), 1);
    assert!(second.refused[0].reason.contains("edited since last install"));
}

// --- replace_pattern -------------------------------------------------------

#[test]
fn replace_pattern_once_replaces_first_match_only() {
    let dir = scratch();
    fs::write(dir.path().join("main.rs"), "Some(cli) Some(cli)").unwrap();
    let patches = vec![patch_replace(
        "main.rs",
        r#"Some\(cli\)"#,
        "cli",
    )];
    apply_patches(dir.path(), &patches, &[], opts()).unwrap();
    let body = fs::read_to_string(dir.path().join("main.rs")).unwrap();
    assert_eq!(body, "cli Some(cli)");
}

#[test]
fn replace_pattern_idempotent_after_first_apply() {
    let dir = scratch();
    fs::write(dir.path().join("main.rs"), "Some(cli)").unwrap();
    let patches = vec![patch_replace(
        "main.rs",
        r#"Some\(cli\)"#,
        "cli",
    )];
    let first = apply_patches(dir.path(), &patches, &[], opts()).unwrap();
    let second = apply_patches(dir.path(), &patches, &first.applied, opts()).unwrap();
    assert_eq!(second.skipped_noop, 1);
    assert!(second.refused.is_empty());
}

#[test]
fn replace_pattern_refuses_on_missing_pattern_and_replacement() {
    let dir = scratch();
    // Neither pattern nor replacement present → consumer file is in an
    // unknown state; engine refuses rather than guessing.
    fs::write(dir.path().join("main.rs"), "something unrelated").unwrap();
    let patches = vec![patch_replace(
        "main.rs",
        r#"Some\(cli\)"#,
        "cli",
    )];
    let report = apply_patches(dir.path(), &patches, &[], opts()).unwrap();
    assert_eq!(report.refused.len(), 1);
}

// --- ensure_dependency ----------------------------------------------------

#[test]
fn ensure_dependency_inserts_into_fresh_file() {
    let dir = scratch();
    fs::write(
        dir.path().join("Cargo.toml"),
        "[package]\nname = \"x\"\n\n[dependencies]\n",
    )
    .unwrap();
    let patches = vec![patch_ensure_dep(
        "Cargo.toml",
        "vesl-core",
        toml::Value::String("1.0".into()),
    )];
    apply_patches(dir.path(), &patches, &[], opts()).unwrap();
    let body = fs::read_to_string(dir.path().join("Cargo.toml")).unwrap();
    assert!(body.contains("vesl-core = \"1.0\""), "body was: {}", body);
}

#[test]
fn ensure_dependency_no_op_when_identical() {
    let dir = scratch();
    fs::write(
        dir.path().join("Cargo.toml"),
        "[package]\nname = \"x\"\n\n[dependencies]\nvesl-core = \"1.0\"\n",
    )
    .unwrap();
    let patches = vec![patch_ensure_dep(
        "Cargo.toml",
        "vesl-core",
        toml::Value::String("1.0".into()),
    )];
    let report = apply_patches(dir.path(), &patches, &[], opts()).unwrap();
    assert_eq!(report.skipped_noop, 1);
    assert!(report.refused.is_empty());
}

#[test]
fn ensure_dependency_preserves_user_value_on_conflict() {
    let dir = scratch();
    let before = "[package]\nname = \"x\"\n\n[dependencies]\nvesl-core = \"9.9\"\n";
    fs::write(dir.path().join("Cargo.toml"), before).unwrap();
    let patches = vec![patch_ensure_dep(
        "Cargo.toml",
        "vesl-core",
        toml::Value::String("1.0".into()),
    )];
    let report = apply_patches(dir.path(), &patches, &[], opts()).unwrap();
    assert_eq!(report.refused.len(), 1);
    let body = fs::read_to_string(dir.path().join("Cargo.toml")).unwrap();
    assert_eq!(body, before, "user's version must be preserved");
}

// --- ensure_patch ---------------------------------------------------------

#[test]
fn ensure_dependency_inline_table_round_trips_values() {
    // Regression guard: an earlier draft of `toml_value_to_edit` emitted
    // empty RHS when handed an inline table, so `vesl-core = { path = .. }`
    // rendered as `vesl-core = ""` in the consumer project.
    let dir = scratch();
    fs::write(
        dir.path().join("Cargo.toml"),
        "[package]\nname = \"x\"\n\n[dependencies]\n",
    )
    .unwrap();
    let mut inline = toml::value::Table::new();
    inline.insert(
        "path".into(),
        toml::Value::String("../../vesl/crates/vesl-core".into()),
    );
    let patches = vec![patch_ensure_dep(
        "Cargo.toml",
        "vesl-core",
        toml::Value::Table(inline),
    )];
    apply_patches(dir.path(), &patches, &[], opts()).unwrap();
    let body = fs::read_to_string(dir.path().join("Cargo.toml")).unwrap();
    assert!(
        body.contains(r#"vesl-core = { path = "../../vesl/crates/vesl-core" }"#),
        "inline table should render with its contents, got:\n{}",
        body
    );
}

#[test]
fn ensure_patch_merges_into_registry_table() {
    let dir = scratch();
    fs::write(dir.path().join("Cargo.toml"), "[package]\nname = \"x\"\n").unwrap();
    let mut entries = BTreeMap::new();
    entries.insert("nockapp".into(), {
        let mut inline = toml::value::Table::new();
        inline.insert(
            "path".into(),
            toml::Value::String("../nockchain/crates/nockapp".into()),
        );
        toml::Value::Table(inline)
    });
    let patches = vec![patch_ensure_patch(
        "Cargo.toml",
        "https://github.com/nockchain/nockchain.git",
        entries,
    )];
    apply_patches(dir.path(), &patches, &[], opts()).unwrap();
    let body = fs::read_to_string(dir.path().join("Cargo.toml")).unwrap();
    assert!(body.contains("[patch.\"https://github.com/nockchain/nockchain.git\"]"));
    // Inline-table value must round-trip — not collapse to `nockapp = ""`.
    assert!(
        body.contains(r#"nockapp = { path = "../nockchain/crates/nockapp" }"#),
        "entry should render as a full inline table, got:\n{}",
        body
    );
    // And the parent `[patch]` header must stay implicit (no empty section).
    assert!(
        !body.contains("\n[patch]\n"),
        "parent `[patch]` table should be implicit, got:\n{}",
        body
    );
}

// --- dry run / summary ---------------------------------------------------

#[test]
fn dry_run_does_not_touch_filesystem() {
    let dir = scratch();
    let patches = vec![patch_overwrite("build.rs", "fn main() {}\n")];
    let report = apply_patches(
        dir.path(),
        &patches,
        &[],
        PatchOptions { dry_run: true },
    )
    .unwrap();
    assert_eq!(report.applied.len(), 1);
    assert!(!dir.path().join("build.rs").exists());
}

#[test]
fn summarize_lists_every_patch() {
    let dir = scratch();
    let patches = vec![
        patch_overwrite("build.rs", "x"),
        patch_replace("main.rs", "a", "b"),
    ];
    let text = summarize(dir.path(), "zkvesl/vesl-graft", &patches);
    assert!(text.contains("zkvesl/vesl-graft"));
    assert!(text.contains("build.rs"));
    assert!(text.contains("main.rs"));
}

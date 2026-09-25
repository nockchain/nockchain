//! Coverage-driven tests for build cache, Nockasm artifacts, the nasm bridge, and arm maps.
//!
//! Added to close branch-coverage gaps; see the coverage report in the PR.

use std::fs;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::{Duration, SystemTime};

use nockasm::{lift_bundle, DagInput, DagMode, DagNode, NasmBundle};
use nockvm::ext::AtomExt;
use nockvm::noun::{Atom, IndirectAtom};
use num_bigint::BigUint;
use serde_json::{json, Value};

#[allow(unused_imports)]
use super::*;
use crate::arm_map::{arm_map_from_noun_list, arm_map_from_type};
use crate::artifact;
use crate::build_cache::{self, BuildCache, CacheObjectKind, CacheWrite};
use crate::nasm_bridge::SlabToNockasm;
use crate::native::noun::term_to_noun;

// ---------------------------------------------------------------------------
// Shared helpers
// ---------------------------------------------------------------------------

type BoxResult<T> = std::result::Result<T, Box<dyn std::error::Error>>;

fn strings(args: &[&str]) -> Vec<String> {
    args.iter().map(|arg| arg.to_string()).collect()
}

fn path_str(path: &Path) -> &str {
    path.to_str().expect("test paths are UTF-8")
}

fn nasm_atom(value: u64) -> nockasm::Noun {
    nockasm::Noun::from(value)
}

fn nasm_cell(head: nockasm::Noun, tail: nockasm::Noun) -> nockasm::Noun {
    nockasm::Noun::cell(head, tail)
}

fn nasm_list(items: impl IntoIterator<Item = nockasm::Noun>) -> nockasm::Noun {
    let items: Vec<_> = items.into_iter().collect();
    items
        .into_iter()
        .rev()
        .fold(nasm_atom(0), |tail, head| nasm_cell(head, tail))
}

fn write_jam(path: &Path, noun: &nockasm::Noun) {
    fs::write(path, nockasm::jam(noun)).unwrap();
}

/// Names of entries directly under `dir` that start with `prefix`.
fn entries_with_prefix(dir: &Path, prefix: &str) -> Vec<String> {
    fs::read_dir(dir)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .filter(|name| name.starts_with(prefix))
        .collect()
}

fn assert_not_found(error: Box<dyn std::error::Error>) {
    assert_eq!(
        error
            .downcast_ref::<std::io::Error>()
            .map(std::io::Error::kind),
        Some(std::io::ErrorKind::NotFound),
        "{error}"
    );
}

// ---------------------------------------------------------------------------
// artifact.rs: export, verify, diff, and the `honk nockasm` CLI
// ---------------------------------------------------------------------------

/// A kernel-sized noun with sharing, a wide atom, and enough distinct nodes
/// that several of the 256 hash shards hold more than one node.
fn sample_kernel() -> nockasm::Noun {
    let shared = nockasm::noun![[1 2] [3 4]];
    let wide = nockasm::Noun::from(nockasm::Atom::from_le_bytes(&[0xab; 20]));
    let items = (0..200u64).map(|index| nasm_cell(nasm_atom(index), shared.clone()));
    nasm_cell(wide, nasm_list(items))
}

/// A formula whose formula-mode lift contains every `DagOp`, both `%nock`
/// fallbacks, autocons cells, and a wide slot axis.
fn every_op_formula() -> nockasm::Noun {
    use nockasm::noun;
    let wide_slot = nasm_cell(
        nasm_atom(0),
        nockasm::Noun::from(nockasm::Atom::from_le_bytes(&[0x5a; 12])),
    );
    let ops = noun![
        [0 1]
        [1 42]
        [2 [0 1] [1 [0 1]]]
        [3 [0 1]]
        [4 [0 1]]
        [5 [0 2] [0 3]]
        [6 [1 0] [1 1] [1 2]]
        [7 [0 1] [0 2]]
        [8 [1 7] [0 2]]
        [9 2 [0 1]]
        [10 [2 [1 5]] [0 1]]
        [11 3 [0 1]]
        [11 [4 [1 9]] [0 1]]
        [12 [0 1] [0 2]]
        [0 [1 2]]
        [99 1]
    ];
    nasm_cell(wide_slot, ops)
}

fn export_noun(dir: &Path, name: &str, noun: &nockasm::Noun) -> PathBuf {
    let jam_path = dir.join(format!("{name}.jam"));
    write_jam(&jam_path, noun);
    let output = dir.join(format!("{name}.nockasm"));
    artifact::export(&jam_path, &output, false).unwrap();
    output
}

fn read_manifest(dir: &Path) -> Value {
    serde_json::from_slice(&fs::read(dir.join("manifest.json")).unwrap()).unwrap()
}

fn write_manifest(dir: &Path, manifest: &Value) {
    fs::write(
        dir.join("manifest.json"),
        serde_json::to_vec_pretty(manifest).unwrap(),
    )
    .unwrap();
}

fn edit_manifest(dir: &Path, key: &str, value: Value) {
    let mut manifest = read_manifest(dir);
    manifest[key] = value;
    write_manifest(dir, &manifest);
}

fn copy_dir(source: &Path, destination: &Path) {
    fs::create_dir_all(destination).unwrap();
    for entry in fs::read_dir(source).unwrap() {
        let entry = entry.unwrap();
        let target = destination.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_dir(&entry.path(), &target);
        } else {
            fs::copy(entry.path(), target).unwrap();
        }
    }
}

/// Replace an artifact's graph with `graph` and keep the manifest's graph
/// hash and byte count consistent with it, so verification proceeds past
/// the envelope checks.
fn replace_graph(dir: &Path, graph: &[u8]) {
    fs::write(dir.join("graph.ndag"), graph).unwrap();
    let mut manifest = read_manifest(dir);
    manifest["graph_blake3"] = json!(blake3::hash(graph).to_hex().to_string());
    manifest["graph_bytes"] = json!(graph.len());
    write_manifest(dir, &manifest);
}

/// Re-encode a canonical bundle's version varint (1) in two bytes. The result
/// still decodes to the same bundle but is not its canonical encoding.
fn noncanonical(graph: &[u8]) -> Vec<u8> {
    assert_eq!(&graph[..8], b"NSDAGB01");
    assert_eq!(graph[8], 1);
    let mut padded = graph[..8].to_vec();
    padded.extend_from_slice(&[0x81, 0x00]);
    padded.extend_from_slice(&graph[9..]);
    assert_eq!(
        NasmBundle::from_bytes(&padded).unwrap(),
        NasmBundle::from_bytes(graph).unwrap()
    );
    padded
}

/// The node hash of an artifact's root, read from a structural diff summary
/// (`diff` reports the full root hash of each side).
fn root_hash_via_diff(artifact_dir: &Path, scratch: &Path, label: &str) -> String {
    let other = scratch.join(format!("{label}-zero.jam"));
    write_jam(&other, &nasm_atom(0));
    let output = scratch.join(format!("{label}-root-probe"));
    artifact::diff(artifact_dir, &other, &output, 0).unwrap();
    let summary = fs::read_to_string(output.join("summary.tsv")).unwrap();
    summary
        .lines()
        .find_map(|line| line.strip_prefix("left-root\t"))
        .expect("summary names the left root")
        .to_string()
}

/// Lay out an artifact directory for an arbitrary bundle: the graph, a
/// manifest that matches it in every checked field, and 256 empty shards.
/// Verification of such a directory reaches the shard comparison.
fn install_bundle_artifact(dir: &Path, bundle: &NasmBundle, scratch: &Path, label: &str) {
    let graph = bundle.to_bytes();
    fs::create_dir_all(dir.join("tree")).unwrap();
    for shard in 0..256 {
        fs::write(dir.join(format!("tree/{shard:02x}.nodes")), b"").unwrap();
    }
    fs::write(dir.join("graph.ndag"), &graph).unwrap();
    let lowered = bundle
        .lower_root("kernel")
        .expect("bundle has a kernel root");
    let canonical_jam = nockasm::jam(&lowered);
    let atoms = bundle
        .nodes()
        .iter()
        .filter(|node| matches!(node, DagNode::Atom(_)))
        .count();
    let cells = bundle
        .nodes()
        .iter()
        .filter(|node| matches!(node, DagNode::Cell(_, _)))
        .count();
    let mut manifest = json!({
        "artifact_version": 1,
        "nockasm_bundle_version": nockasm::NASM_BUNDLE_VERSION,
        "debug_hash_bits": 128,
        "root_name": "kernel",
        "root_blake3": "",
        "canonical_jam_blake3": blake3::hash(&canonical_jam).to_hex().to_string(),
        "canonical_jam_bytes": canonical_jam.len(),
        "source_jam_blake3": blake3::hash(&canonical_jam).to_hex().to_string(),
        "source_jam_bytes": canonical_jam.len(),
        "graph_blake3": blake3::hash(&graph).to_hex().to_string(),
        "graph_bytes": graph.len(),
        "unique_nodes": bundle.nodes().len(),
        "atom_nodes": atoms,
        "cell_nodes": cells,
    });
    write_manifest(dir, &manifest);
    manifest["root_blake3"] = json!(root_hash_via_diff(dir, scratch, label));
    write_manifest(dir, &manifest);
}

#[test]
fn artifact_export_writes_consistent_manifest_shards_and_formula_root() {
    let temp = tempfile::tempdir().unwrap();
    let noun = sample_kernel();
    let input = temp.path().join("kernel.jam");
    write_jam(&input, &noun);
    // The output's parent does not exist yet; export creates it.
    let output = temp.path().join("nested/dir/kernel.nockasm");

    let message = artifact::run_cli(&strings(&[
        "export",
        path_str(&input),
        "--output",
        path_str(&output),
        "--formula-root",
    ]))
    .unwrap();
    assert_eq!(
        message,
        format!("exported {} to {}", path_str(&input), output.display())
    );
    assert!(entries_with_prefix(output.parent().unwrap(), ".kernel.nockasm.tmp-").is_empty());

    let manifest = read_manifest(&output);
    let source = fs::read(&input).unwrap();
    let graph = fs::read(output.join("graph.ndag")).unwrap();
    let canonical_jam = nockasm::jam(&noun);
    assert_eq!(manifest["artifact_version"], 1);
    assert_eq!(
        manifest["nockasm_bundle_version"],
        nockasm::NASM_BUNDLE_VERSION
    );
    assert_eq!(manifest["debug_hash_bits"], 128);
    assert_eq!(manifest["root_name"], "kernel");
    assert_eq!(
        manifest["source_jam_blake3"],
        blake3::hash(&source).to_hex().as_str()
    );
    assert_eq!(manifest["source_jam_bytes"], source.len());
    assert_eq!(
        manifest["canonical_jam_blake3"],
        blake3::hash(&canonical_jam).to_hex().as_str()
    );
    assert_eq!(manifest["canonical_jam_bytes"], canonical_jam.len());
    assert_eq!(
        manifest["graph_blake3"],
        blake3::hash(&graph).to_hex().as_str()
    );
    assert_eq!(manifest["graph_bytes"], graph.len());

    let bundle = NasmBundle::from_bytes(&graph).unwrap();
    assert_eq!(bundle.lower_root("kernel"), Some(noun.clone()));
    assert_eq!(manifest["unique_nodes"], bundle.nodes().len());
    // A structural (noun-mode) export holds only atoms and cells.
    assert_eq!(
        manifest["atom_nodes"].as_u64().unwrap() + manifest["cell_nodes"].as_u64().unwrap(),
        bundle.nodes().len() as u64
    );

    let root_hash = manifest["root_blake3"].as_str().unwrap().to_string();
    assert_eq!(
        fs::read_to_string(output.join("tree/root.ref")).unwrap(),
        format!("kernel\t{root_hash}\n")
    );
    assert_eq!(
        fs::read_to_string(output.join("formula-root.nasm-dag")).unwrap(),
        nockasm::lift_dag(&noun).unwrap().render()
    );

    // Every node appears once, in the shard named by its hash's first byte,
    // and each shard is sorted by hash.
    let mut total_lines = 0usize;
    let mut crowded_shards = 0usize;
    for shard in 0..256 {
        let name = format!("{shard:02x}");
        let text = fs::read_to_string(output.join(format!("tree/{name}.nodes"))).unwrap();
        let lines: Vec<&str> = text.lines().collect();
        if lines.len() > 1 {
            crowded_shards += 1;
        }
        let mut sorted = lines.clone();
        sorted.sort_unstable();
        assert_eq!(lines, sorted, "shard {name} is not hash-sorted");
        for line in &lines {
            assert!(line.starts_with(&name), "{line} is in shard {name}");
            let kind = line.split('\t').nth(1).unwrap();
            assert!(
                kind == "atom" || kind == "cell",
                "unexpected node line {line}"
            );
        }
        total_lines += lines.len();
    }
    assert_eq!(total_lines, bundle.nodes().len());
    assert!(crowded_shards > 0, "the sample must exercise shard sorting");

    // The root hash `diff` reports is the manifest's root hash.
    assert_eq!(
        root_hash_via_diff(&output, temp.path(), "export"),
        root_hash
    );

    let report = artifact::run_cli(&strings(&["verify", path_str(&output)])).unwrap();
    assert_eq!(
        report,
        format!(
            "verified {}: {} unique nodes, {} graph bytes",
            output.display(),
            bundle.nodes().len(),
            graph.len()
        )
    );

    // Without --formula-root no formula rendering is written.
    let plain = export_noun(temp.path(), "plain", &noun);
    assert!(!plain.join("formula-root.nasm-dag").exists());
    artifact::verify(&plain).unwrap();
}

#[test]
fn artifact_cli_reports_usage_and_argument_errors() {
    let usage = artifact::run_cli(&[]).unwrap_err().to_string();
    assert!(usage.starts_with("Usage: honk nockasm export"));
    assert!(usage.contains("Usage: honk nockasm verify"));
    assert!(usage.contains("Usage: honk nockasm diff"));
    for help in ["help", "-h", "--help"] {
        assert_eq!(artifact::run_cli(&strings(&[help])).unwrap(), usage);
    }

    let error = |args: &[&str]| artifact::run_cli(&strings(args)).unwrap_err().to_string();
    let unknown = error(&["frobnicate"]);
    assert!(unknown.starts_with("unknown nockasm command \"frobnicate\"\nUsage:"));
    assert!(error(&["export"]).starts_with("missing input jam\n"));
    // A flag in the positional slot is not an input path.
    assert!(error(&["export", "--output", "out"]).starts_with("missing input jam\n"));
    assert!(error(&["export", "in.jam"]).starts_with("missing --output\n"));
    // `--output` with no value is as good as missing.
    assert!(error(&["export", "in.jam", "--output"]).starts_with("missing --output\n"));
    assert!(error(&["verify"]).starts_with("missing artifact directory\n"));
    assert!(error(&["diff"]).starts_with("missing left jam or artifact\n"));
    assert!(error(&["diff", "left.jam"]).starts_with("missing right jam or artifact\n"));
    assert!(error(&["diff", "left.jam", "right.jam"]).starts_with("missing --output\n"));
    assert!(
        error(&["diff", "left.jam", "right.jam", "--output", "out", "--limit", "many"])
            .contains("invalid digit")
    );
}

#[test]
fn artifact_verify_rejects_each_inconsistent_manifest_field() {
    let temp = tempfile::tempdir().unwrap();
    let base = export_noun(temp.path(), "base", &sample_kernel());
    artifact::verify(&base).unwrap();
    let base_manifest = read_manifest(&base);
    let bump = |key: &str| json!(base_manifest[key].as_u64().unwrap() + 1);
    let wrong_hash = json!(blake3::hash(b"not this").to_hex().to_string());

    let check = |name: &str, edit: &dyn Fn(&Path), expected: &str| {
        let dir = temp.path().join(name);
        copy_dir(&base, &dir);
        edit(&dir);
        let error = artifact::verify(&dir)
            .expect_err("a tampered artifact must not verify")
            .to_string();
        assert!(
            error.contains(expected),
            "{name}: expected {expected:?}, got {error:?}"
        );
    };

    check(
        "version",
        &|dir| edit_manifest(dir, "artifact_version", json!(2)),
        "unsupported artifact version 2",
    );
    check(
        "bundle-version",
        &|dir| edit_manifest(dir, "nockasm_bundle_version", json!(999)),
        "artifact encoding parameters do not match this Honk build",
    );
    check(
        "hash-bits",
        &|dir| edit_manifest(dir, "debug_hash_bits", json!(64)),
        "artifact encoding parameters do not match this Honk build",
    );
    check(
        "graph-hash",
        &|dir| edit_manifest(dir, "graph_blake3", wrong_hash.clone()),
        "graph hash does not match manifest",
    );
    check(
        "graph-bytes",
        &|dir| edit_manifest(dir, "graph_bytes", bump("graph_bytes")),
        "graph byte count does not match manifest",
    );
    check(
        "graph-garbage",
        &|dir| replace_graph(dir, b"definitely not a bundle"),
        "bad Nockasm bundle magic",
    );
    check(
        "graph-noncanonical",
        &|dir| {
            replace_graph(
                dir,
                &noncanonical(&fs::read(dir.join("graph.ndag")).unwrap()),
            )
        },
        "graph encoding is not canonical",
    );
    check(
        "root-name",
        &|dir| edit_manifest(dir, "root_name", json!("not-the-kernel")),
        "manifest root is missing from graph",
    );
    check(
        "unique-nodes",
        &|dir| edit_manifest(dir, "unique_nodes", bump("unique_nodes")),
        "node count does not match manifest",
    );
    check(
        "atom-nodes",
        &|dir| edit_manifest(dir, "atom_nodes", bump("atom_nodes")),
        "node-kind counts do not match manifest",
    );
    check(
        "cell-nodes",
        &|dir| edit_manifest(dir, "cell_nodes", bump("cell_nodes")),
        "node-kind counts do not match manifest",
    );
    check(
        "root-hash",
        &|dir| edit_manifest(dir, "root_blake3", wrong_hash.clone()),
        "root hash does not match manifest",
    );
    check(
        "canonical-jam-hash",
        &|dir| edit_manifest(dir, "canonical_jam_blake3", wrong_hash.clone()),
        "canonical jam hash does not match manifest",
    );
    check(
        "canonical-jam-bytes",
        &|dir| edit_manifest(dir, "canonical_jam_bytes", bump("canonical_jam_bytes")),
        "canonical jam byte count does not match manifest",
    );
    check(
        "manifest-garbage",
        &|dir| fs::write(dir.join("manifest.json"), b"{ not json").unwrap(),
        "key must be a string",
    );
    check(
        "manifest-missing",
        &|dir| fs::remove_file(dir.join("manifest.json")).unwrap(),
        "No such file",
    );
    check(
        "graph-missing",
        &|dir| fs::remove_file(dir.join("graph.ndag")).unwrap(),
        "No such file",
    );
}

#[test]
fn artifact_verify_rejects_damaged_shards_and_a_stale_scratch_directory() {
    let temp = tempfile::tempdir().unwrap();
    let base = export_noun(temp.path(), "base", &sample_kernel());

    let non_empty_shard = (0..256)
        .map(|shard| format!("{shard:02x}.nodes"))
        .find(|name| fs::metadata(base.join("tree").join(name)).unwrap().len() > 0)
        .unwrap();

    let tampered = temp.path().join("tampered");
    copy_dir(&base, &tampered);
    let shard_path = tampered.join("tree").join(&non_empty_shard);
    let mut text = fs::read_to_string(&shard_path).unwrap();
    text.push_str("extra\n");
    fs::write(&shard_path, text).unwrap();
    let error = artifact::verify(&tampered).unwrap_err().to_string();
    assert_eq!(
        error,
        format!("content-addressed shard {non_empty_shard} does not match graph")
    );
    // The scratch directory used for the comparison is removed on failure.
    assert!(entries_with_prefix(&tampered, ".verify-shards-").is_empty());

    let missing = temp.path().join("missing");
    copy_dir(&base, &missing);
    fs::remove_file(missing.join("tree").join(&non_empty_shard)).unwrap();
    assert_not_found(artifact::verify(&missing).unwrap_err());
    assert!(entries_with_prefix(&missing, ".verify-shards-").is_empty());

    let stale = temp.path().join("stale");
    copy_dir(&base, &stale);
    let scratch = stale.join(format!(".verify-shards-{}", std::process::id()));
    fs::create_dir(&scratch).unwrap();
    let error = artifact::verify(&stale).unwrap_err().to_string();
    assert_eq!(
        error,
        format!(
            "temporary verification directory exists: {}",
            scratch.display()
        )
    );
    // Verification never deletes a directory it did not create.
    assert!(scratch.is_dir());
    fs::remove_dir(&scratch).unwrap();
    artifact::verify(&stale).unwrap();
}

#[test]
fn artifact_formula_bundle_hashes_renders_and_diffs_every_op() {
    let temp = tempfile::tempdir().unwrap();
    let formula = every_op_formula();
    let bundle = lift_bundle(&[DagInput {
        name: "kernel",
        noun: &formula,
        mode: DagMode::Formula,
    }])
    .unwrap();
    let ops = bundle
        .nodes()
        .iter()
        .filter(|node| matches!(node, DagNode::Op(_)))
        .count();
    let nocks = bundle
        .nodes()
        .iter()
        .filter(|node| matches!(node, DagNode::Nock(_)))
        .count();
    assert!(
        ops >= 14,
        "every DagOp variant must be present, got {ops} ops"
    );
    assert_eq!(nocks, 2);

    let dir = temp.path().join("formula.nockasm");
    install_bundle_artifact(&dir, &bundle, temp.path(), "formula");
    // The manifest matches the graph in every field, so verification hashes
    // and renders every node (ops included) and fails only at the shards,
    // which the helper left empty.
    let error = artifact::verify(&dir).unwrap_err().to_string();
    assert!(
        error.starts_with("content-addressed shard ") && error.ends_with(" does not match graph"),
        "{error}"
    );

    // A directory artifact against itself: equal roots, nothing reported.
    let same = temp.path().join("same-diff");
    artifact::diff(&dir, &dir, &same, 8).unwrap();
    let summary = fs::read_to_string(same.join("summary.tsv")).unwrap();
    assert!(summary.contains("equal-subgraphs-skipped\t1\n"));
    assert!(summary.contains("reported-changes\t0\n"));
    assert_eq!(fs::read_dir(same.join("changes")).unwrap().count(), 0);

    // Against an atom the whole formula graph is one change, so its fragment
    // walk visits every op's children.
    let atom = temp.path().join("atom.jam");
    write_jam(&atom, &nasm_atom(0));
    let changed = temp.path().join("changed-diff");
    artifact::run_cli(&strings(&[
        "diff",
        path_str(&dir),
        path_str(&atom),
        "--output",
        path_str(&changed),
        "--limit",
        "4",
    ]))
    .unwrap();
    let summary = fs::read_to_string(changed.join("summary.tsv")).unwrap();
    assert!(summary.contains("reported-changes\t1\n"));
    assert!(summary.contains("change-0000\taxis=0b1\t"));
    assert_eq!(
        fs::read_to_string(changed.join("changes/0000-left.nasm-dag")).unwrap(),
        nockasm::lift_dag(&formula).unwrap().render()
    );
    assert_eq!(
        fs::read_to_string(changed.join("changes/0000-right.nasm-dag")).unwrap(),
        nockasm::lift_dag(&nasm_atom(0)).unwrap().render()
    );
}

#[test]
fn artifact_duplicate_nodes_share_one_content_hash() {
    let temp = tempfile::tempdir().unwrap();
    // A hand-encoded canonical bundle holding atom 5 twice: [5 5] = cell(@0 @1).
    let mut graph = b"NSDAGB01".to_vec();
    graph.extend_from_slice(&[1, 3]);
    graph.extend_from_slice(&[0, 1, 5]);
    graph.extend_from_slice(&[0, 1, 5]);
    graph.extend_from_slice(&[1, 0, 1]);
    graph.extend_from_slice(&[1, 6]);
    graph.extend_from_slice(b"kernel");
    graph.extend_from_slice(&[1, 2]);
    let bundle = NasmBundle::from_bytes(&graph).unwrap();
    assert_eq!(bundle.to_bytes(), graph);
    assert_eq!(bundle.nodes().len(), 3);

    let dir = temp.path().join("dup.nockasm");
    install_bundle_artifact(&dir, &bundle, temp.path(), "dup");
    // Shard generation orders the two equal-hash atoms by node id.
    let error = artifact::verify(&dir).unwrap_err().to_string();
    assert!(error.ends_with(" does not match graph"), "{error}");

    // Content hashes ignore node identity: the duplicated graph equals the
    // hash-consed export of the same noun.
    let consed = export_noun(temp.path(), "consed", &nockasm::noun![5 5]);
    assert_eq!(read_manifest(&consed)["unique_nodes"], 2);
    let output = temp.path().join("dup-diff");
    artifact::diff(&dir, &consed, &output, 8).unwrap();
    let summary = fs::read_to_string(output.join("summary.tsv")).unwrap();
    assert!(summary.contains("equal-subgraphs-skipped\t1\n"));
    assert!(summary.contains("reported-changes\t0\n"));
}

#[test]
fn artifact_diff_honors_limits_and_writes_bounded_fragments() {
    let temp = tempfile::tempdir().unwrap();
    let left = temp.path().join("left.jam");
    let right = temp.path().join("right.jam");
    write_jam(&left, &nasm_list((1..=5).map(nasm_atom)));
    write_jam(&right, &nasm_list((6..=10).map(nasm_atom)));

    // The walk stops as soon as the limit is reached.
    let limited = temp.path().join("limited");
    artifact::diff(&left, &right, &limited, 2).unwrap();
    let summary = fs::read_to_string(limited.join("summary.tsv")).unwrap();
    assert!(summary.contains("reported-changes\t2\n"));
    assert!(summary.contains("change-limit\t2\n"));
    assert!(summary.contains("change-0000\taxis=0b10\t"));
    assert!(summary.contains("change-0001\taxis=0b110\t"));
    assert!(!summary.contains("change-0002"));
    let mut fragments: Vec<_> = fs::read_dir(limited.join("changes"))
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    fragments.sort();
    assert_eq!(
        fragments,
        [
            "0000-left.nasm-dag", "0000-right.nasm-dag", "0001-left.nasm-dag",
            "0001-right.nasm-dag"
        ]
    );
    assert_eq!(
        fs::read_to_string(limited.join("changes/0000-left.nasm-dag")).unwrap(),
        nockasm::lift_dag(&nasm_atom(1)).unwrap().render()
    );

    // A zero limit records nothing.
    let zero = temp.path().join("zero");
    artifact::diff(&left, &right, &zero, 0).unwrap();
    let summary = fs::read_to_string(zero.join("summary.tsv")).unwrap();
    assert!(summary.contains("reported-changes\t0\n"));
    assert_eq!(fs::read_dir(zero.join("changes")).unwrap().count(), 0);

    // The CLI default limit is 64 and all five changes fit.
    let defaulted = temp.path().join("defaulted");
    let message = artifact::run_cli(&strings(&[
        "diff",
        path_str(&left),
        path_str(&right),
        "--output",
        path_str(&defaulted),
    ]))
    .unwrap();
    assert_eq!(
        message,
        format!("wrote structural diff to {}", defaulted.display())
    );
    let summary = fs::read_to_string(defaulted.join("summary.tsv")).unwrap();
    assert!(summary.contains("change-limit\t64\n"));
    assert!(summary.contains("reported-changes\t5\n"));

    // A shared subtree is walked once, and a change whose subtree exceeds the
    // fragment bound is written as a reference instead of a rendering.
    let shared = nockasm::noun![[1 2] [1 2]];
    let huge = nasm_list((1..=6000).map(nasm_atom));
    let small = temp.path().join("small.jam");
    let big = temp.path().join("big.jam");
    write_jam(&small, &nasm_cell(nasm_atom(0), shared.clone()));
    write_jam(&big, &nasm_cell(huge, nasm_atom(0)));
    let bounded = temp.path().join("bounded");
    artifact::diff(&small, &big, &bounded, 8).unwrap();
    let summary = fs::read_to_string(bounded.join("summary.tsv")).unwrap();
    assert!(summary.contains("reported-changes\t2\n"));
    let reference = fs::read_to_string(bounded.join("changes/0000-right.ref")).unwrap();
    assert!(reference.starts_with("axis\t0b10\nhash\t"), "{reference}");
    assert!(
        reference.ends_with("subtree-nodes\t>10000\n"),
        "{reference}"
    );
    assert!(!bounded.join("changes/0000-right.nasm-dag").exists());
    assert!(bounded.join("changes/0000-left.nasm-dag").exists());
    assert_eq!(
        fs::read_to_string(bounded.join("changes/0001-left.nasm-dag")).unwrap(),
        nockasm::lift_dag(&shared).unwrap().render()
    );
}

#[test]
fn artifact_export_and_diff_reject_bad_inputs() {
    let temp = tempfile::tempdir().unwrap();
    let good = temp.path().join("good.jam");
    write_jam(&good, &nockasm::noun![1 2]);
    let empty = temp.path().join("empty.jam");
    fs::write(&empty, b"").unwrap();
    let missing = temp.path().join("missing.jam");

    assert_not_found(artifact::export(&missing, &temp.path().join("a"), false).unwrap_err());
    let error = artifact::export(&empty, &temp.path().join("b"), false)
        .unwrap_err()
        .to_string();
    assert!(
        error.starts_with(&format!("cue {}: ", empty.display())),
        "{error}"
    );

    let existing = temp.path().join("existing");
    fs::create_dir(&existing).unwrap();
    let refusal = format!("refusing to replace existing {}", existing.display());
    assert_eq!(
        artifact::export(&good, &existing, false)
            .unwrap_err()
            .to_string(),
        refusal
    );
    assert_eq!(
        artifact::diff(&good, &good, &existing, 8)
            .unwrap_err()
            .to_string(),
        refusal
    );

    assert_not_found(artifact::diff(&missing, &good, &temp.path().join("c"), 8).unwrap_err());
    assert_not_found(artifact::diff(&good, &missing, &temp.path().join("d"), 8).unwrap_err());
    let error = artifact::diff(&good, &empty, &temp.path().join("e"), 8)
        .unwrap_err()
        .to_string();
    assert!(
        error.starts_with(&format!("cue {}: ", empty.display())),
        "{error}"
    );

    // A directory without a graph, and a graph without a `kernel` root.
    let hollow = temp.path().join("hollow");
    fs::create_dir(&hollow).unwrap();
    assert_not_found(artifact::diff(&hollow, &good, &temp.path().join("f"), 8).unwrap_err());
    let rootless = temp.path().join("rootless");
    fs::create_dir(&rootless).unwrap();
    let noun = nockasm::noun![1 2];
    let other_root = lift_bundle(&[DagInput {
        name: "not-kernel",
        noun: &noun,
        mode: DagMode::Noun,
    }])
    .unwrap();
    fs::write(rootless.join("graph.ndag"), other_root.to_bytes()).unwrap();
    assert_eq!(
        artifact::diff(&rootless, &good, &temp.path().join("g"), 8)
            .unwrap_err()
            .to_string(),
        "bundle has no \"kernel\" root"
    );
    let garbage = temp.path().join("garbage");
    fs::create_dir(&garbage).unwrap();
    fs::write(garbage.join("graph.ndag"), b"garbage, not a bundle").unwrap();
    assert!(artifact::diff(&garbage, &good, &temp.path().join("h"), 8)
        .unwrap_err()
        .to_string()
        .contains("bad Nockasm bundle magic"));
    // None of the failed runs created their outputs.
    for name in ["a", "b", "c", "d", "e", "f", "g", "h"] {
        assert!(!temp.path().join(name).exists(), "{name}");
    }
}

/// When the final rename fails, the pending directory is discarded. A dangling
/// symlink passes the "does not exist" check but cannot be replaced by a
/// directory rename.
#[cfg(unix)]
#[test]
fn artifact_failed_commit_removes_the_pending_directory() {
    let temp = tempfile::tempdir().unwrap();
    let input = temp.path().join("in.jam");
    write_jam(&input, &nockasm::noun![[1 2] 3]);
    let other = temp.path().join("other.jam");
    write_jam(&other, &nockasm::noun![[1 4] 3]);

    type Run<'a> = Box<dyn Fn(&Path) -> BoxResult<()> + 'a>;
    let runs: [(&str, Run<'_>); 2] = [
        (
            "export-link",
            Box::new(|output: &Path| artifact::export(&input, output, true)),
        ),
        (
            "diff-link",
            Box::new(|output: &Path| artifact::diff(&input, &other, output, 8)),
        ),
    ];
    for (label, run) in runs {
        let link = temp.path().join(label);
        std::os::unix::fs::symlink(temp.path().join("nowhere"), &link).unwrap();
        assert!(!link.exists());
        let error = run(&link).expect_err("renaming onto a symlink must fail");
        assert!(
            error.downcast_ref::<std::io::Error>().is_some(),
            "{label}: {error}"
        );
        assert!(
            entries_with_prefix(temp.path(), &format!(".{label}.tmp-")).is_empty(),
            "{label} left its pending directory behind"
        );
        assert!(fs::symlink_metadata(&link)
            .unwrap()
            .file_type()
            .is_symlink());
    }
}

// ---------------------------------------------------------------------------
// build_cache.rs: reads, writes, corruption handling, stats, and gc
// ---------------------------------------------------------------------------

fn pack_graph(roots: &[(&str, nockasm::Noun)]) -> Vec<u8> {
    let inputs: Vec<_> = roots
        .iter()
        .map(|(name, noun)| DagInput {
            name,
            noun,
            mode: DagMode::Noun,
        })
        .collect();
    lift_bundle(&inputs).unwrap().to_bytes()
}

fn cache_entry(key: blake3::Hash, kind: CacheObjectKind, root_name: &str) -> CacheWrite<'_> {
    CacheWrite {
        key,
        kind,
        logical_source: "cov/t1.hoon",
        dependency_keys: &[],
        root_name,
    }
}

fn metadata_file(root: &Path, key: blake3::Hash) -> PathBuf {
    let hex = key.to_hex().to_string();
    root.join("v1/objects")
        .join(&hex[..2])
        .join(&hex[2..])
        .with_extension("json")
}

fn pack_file(root: &Path, graph: &[u8]) -> PathBuf {
    let hex = blake3::hash(graph).to_hex().to_string();
    root.join("v1/packs")
        .join(&hex[..2])
        .join(&hex[2..])
        .with_extension("ndag")
}

fn read_json(path: &Path) -> Value {
    serde_json::from_slice(&fs::read(path).unwrap()).unwrap()
}

fn write_json(path: &Path, value: &Value) {
    fs::write(path, serde_json::to_vec_pretty(value).unwrap()).unwrap();
}

/// `(hits, misses, writes, corrupt)`.
fn stats(cache: &BuildCache) -> (u64, u64, u64, u64) {
    let stats = cache.stats();
    (stats.hits, stats.misses, stats.writes, stats.corrupt)
}

/// One cache directory holding a single `DependencyVase` object.
struct SingleObject {
    _temp: tempfile::TempDir,
    root: PathBuf,
    key: blake3::Hash,
    root_name: String,
    noun: nockasm::Noun,
    graph: Vec<u8>,
}

impl SingleObject {
    fn new(seed: &str) -> Self {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("cache");
        let key = blake3::hash(seed.as_bytes());
        let root_name = key.to_hex().to_string();
        let noun = nockasm::noun![[1 2] [1 2] 99];
        let graph = pack_graph(&[(&root_name, noun.clone())]);
        let mut cache = BuildCache::new(root.clone(), false);
        cache
            .write_pack(
                &[cache_entry(key, CacheObjectKind::DependencyVase, &root_name)],
                &graph,
            )
            .unwrap();
        assert_eq!(stats(&cache), (0, 0, 1, 0));
        Self {
            _temp: temp,
            root,
            key,
            root_name,
            noun,
            graph,
        }
    }

    fn metadata(&self) -> PathBuf {
        metadata_file(&self.root, self.key)
    }

    fn pack(&self) -> PathBuf {
        pack_file(&self.root, &self.graph)
    }

    fn edit_metadata(&self, key: &str, value: Value) {
        let mut metadata = read_json(&self.metadata());
        metadata[key] = value;
        write_json(&self.metadata(), &metadata);
    }

    fn rewrite(&self, cache: &mut BuildCache) {
        cache
            .write_pack(
                &[cache_entry(self.key, CacheObjectKind::DependencyVase, &self.root_name)],
                &self.graph,
            )
            .unwrap();
    }

    fn read(&self, cache: &mut BuildCache) -> Option<build_cache::CacheRead> {
        cache
            .read(self.key, CacheObjectKind::DependencyVase)
            .unwrap()
    }

    /// Read through a new session and assert the object is served intact.
    fn assert_hit(&self) {
        let mut cache = BuildCache::new(self.root.clone(), false);
        let cached = self.read(&mut cache).expect("expected a cache hit");
        assert_eq!(cached.root_name, self.root_name);
        assert_eq!(cached.bundle.to_bytes(), self.graph);
        assert_eq!(
            cached.bundle.lower_root(&cached.root_name),
            Some(self.noun.clone())
        );
        assert_eq!(stats(&cache), (1, 0, 0, 0));
    }
}

#[test]
fn cache_round_trip_records_metadata_and_shares_loaded_packs() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("cache");
    let vase_key = blake3::hash(b"vase");
    let product_key = blake3::hash(b"product");
    let dependency = blake3::hash(b"dependency");
    let vase_root = vase_key.to_hex().to_string();
    let product_root = product_key.to_hex().to_string();
    let vase_noun = nockasm::noun![1 [2 3] [2 3]];
    let product_noun = nockasm::noun![[2 3] 4];
    let graph =
        pack_graph(&[(&vase_root, vase_noun.clone()), (&product_root, product_noun.clone())]);

    let mut cache = BuildCache::new(root.clone(), false);
    let dependencies = [dependency];
    cache
        .write_pack(
            &[
                CacheWrite {
                    key: vase_key,
                    kind: CacheObjectKind::DependencyVase,
                    logical_source: "lib/vase.hoon",
                    dependency_keys: &dependencies,
                    root_name: &vase_root,
                },
                cache_entry(product_key, CacheObjectKind::EntryProduct, &product_root),
            ],
            &graph,
        )
        .unwrap();
    assert_eq!(stats(&cache), (0, 0, 2, 0));
    assert_eq!(fs::read(pack_file(&root, &graph)).unwrap(), graph);
    let pack_hex = blake3::hash(&graph).to_hex().to_string();
    assert_eq!(
        read_json(&metadata_file(&root, vase_key)),
        json!({
            "cache_version": 1,
            "key": vase_key.to_hex().to_string(),
            "kind": "dependency-vase",
            "logical_source": "lib/vase.hoon",
            "dependency_keys": [dependency.to_hex().to_string()],
            "pack_blake3": pack_hex,
            "pack_bytes": graph.len(),
            "root_name": vase_root,
        })
    );
    assert_eq!(
        read_json(&metadata_file(&root, product_key))["kind"],
        "entry-product"
    );
    // No temporary files survive a successful write.
    for dir in [
        metadata_file(&root, vase_key)
            .parent()
            .unwrap()
            .to_path_buf(),
        pack_file(&root, &graph).parent().unwrap().to_path_buf(),
    ] {
        assert!(entries_with_prefix(&dir, ".").is_empty());
    }

    // A new session decodes the pack once and serves both roots from it.
    let mut reader = BuildCache::new(root.clone(), false);
    let vase = reader
        .read(vase_key, CacheObjectKind::DependencyVase)
        .unwrap()
        .unwrap();
    let product = reader
        .read(product_key, CacheObjectKind::EntryProduct)
        .unwrap()
        .unwrap();
    assert!(Rc::ptr_eq(&vase.bundle, &product.bundle));
    assert_eq!(vase.bundle.lower_root(&vase.root_name), Some(vase_noun));
    assert_eq!(
        product.bundle.lower_root(&product.root_name),
        Some(product_noun)
    );
    assert_eq!(product.bundle.to_bytes(), graph);
    assert_eq!(stats(&reader), (2, 0, 0, 0));

    // Asking for an object under the wrong kind is a corrupt miss.
    assert!(reader
        .read(vase_key, CacheObjectKind::EntryProduct)
        .unwrap()
        .is_none());
    assert_eq!(stats(&reader), (2, 1, 0, 1));

    // Unknown keys and read-disabled (fresh) sessions are plain misses.
    assert!(reader
        .read(blake3::hash(b"unknown"), CacheObjectKind::EntryProduct)
        .unwrap()
        .is_none());
    assert_eq!(stats(&reader), (2, 2, 0, 1));
    let mut fresh = BuildCache::new(root, true);
    assert!(fresh
        .read(vase_key, CacheObjectKind::DependencyVase)
        .unwrap()
        .is_none());
    assert_eq!(stats(&fresh), (0, 1, 0, 0));
}

#[test]
fn cache_damaged_metadata_is_a_miss_and_is_repaired_by_a_rewrite() {
    type Damage = fn(&SingleObject);
    let cases: [(&str, Damage); 9] = [
        ("garbled json", |object| {
            fs::write(object.metadata(), b"{\"cache_version\": 1,").unwrap()
        }),
        ("truncated json", |object| {
            let bytes = fs::read(object.metadata()).unwrap();
            fs::write(object.metadata(), &bytes[..bytes.len() / 2]).unwrap();
        }),
        ("cache version", |object| {
            object.edit_metadata("cache_version", json!(2))
        }),
        ("key", |object| {
            object.edit_metadata("key", json!(blake3::hash(b"other").to_hex().to_string()))
        }),
        ("unknown kind", |object| {
            object.edit_metadata("kind", json!("mystery"))
        }),
        ("short pack hash", |object| {
            object.edit_metadata("pack_blake3", json!("abc123"))
        }),
        ("non-hex pack hash", |object| {
            object.edit_metadata("pack_blake3", json!("g".repeat(64)))
        }),
        ("absent pack", |object| {
            object.edit_metadata(
                "pack_blake3",
                json!(blake3::hash(b"no such pack").to_hex().to_string()),
            )
        }),
        ("root not in pack", |object| {
            object.edit_metadata("root_name", json!("not-a-root"))
        }),
    ];
    for (label, damage) in cases {
        let object = SingleObject::new(label);
        damage(&object);
        let mut cache = BuildCache::new(object.root.clone(), false);
        assert!(
            object.read(&mut cache).is_none(),
            "{label}: damaged metadata must not be served"
        );
        assert_eq!(stats(&cache), (0, 1, 0, 1), "{label}");
        // The damaged key is rewritten even though a metadata file exists.
        object.rewrite(&mut cache);
        assert_eq!(stats(&cache), (0, 1, 1, 1), "{label}");
        object.assert_hit();
    }
}

#[test]
fn cache_damaged_packs_are_misses() {
    type Damage = fn(&SingleObject);
    let cases: [(&str, Damage); 4] = [
        ("truncated pack", |object| {
            let bytes = fs::read(object.pack()).unwrap();
            fs::write(object.pack(), &bytes[..bytes.len() - 1]).unwrap();
        }),
        ("same-length garbage", |object| {
            let mut bytes = fs::read(object.pack()).unwrap();
            let last = bytes.len() - 1;
            bytes[last] ^= 0xff;
            fs::write(object.pack(), bytes).unwrap();
        }),
        ("wrong recorded length", |object| {
            object.edit_metadata("pack_bytes", json!(object.graph.len() + 1))
        }),
        ("missing pack", |object| {
            fs::remove_file(object.pack()).unwrap()
        }),
    ];
    for (label, damage) in cases {
        let object = SingleObject::new(label);
        damage(&object);
        let mut cache = BuildCache::new(object.root.clone(), false);
        assert!(
            object.read(&mut cache).is_none(),
            "{label}: a damaged pack must not be served"
        );
        assert_eq!(stats(&cache), (0, 1, 0, 1), "{label}");
    }

    // Bytes that hash-match their metadata but do not decode as a bundle.
    let object = SingleObject::new("undecodable");
    let garbage = b"not a nockasm bundle".to_vec();
    let garbage_path = pack_file(&object.root, &garbage);
    fs::create_dir_all(garbage_path.parent().unwrap()).unwrap();
    fs::write(&garbage_path, &garbage).unwrap();
    object.edit_metadata(
        "pack_blake3",
        json!(blake3::hash(&garbage).to_hex().to_string()),
    );
    object.edit_metadata("pack_bytes", json!(garbage.len()));
    let mut cache = BuildCache::new(object.root.clone(), false);
    assert!(object.read(&mut cache).is_none());
    assert_eq!(stats(&cache), (0, 1, 0, 1));
}

/// A damaged pack is repaired by a fresh (`--new`) session, and a normal
/// session that rewrites the same object serves it from memory. See
/// `cache_damaged_pack_is_repaired_across_sessions` for the cross-session case.
#[test]
fn cache_damaged_pack_is_repaired_by_a_fresh_session() {
    let object = SingleObject::new("repair");
    let mut bytes = fs::read(object.pack()).unwrap();
    bytes[0] ^= 0xff;
    fs::write(object.pack(), &bytes).unwrap();

    let mut cache = BuildCache::new(object.root.clone(), false);
    assert!(object.read(&mut cache).is_none());
    object.rewrite(&mut cache);
    // The session that rebuilt the object serves it from the bundle it wrote.
    let cached = object.read(&mut cache).unwrap();
    assert_eq!(cached.bundle.to_bytes(), object.graph);
    assert_eq!(stats(&cache), (1, 1, 1, 1));

    let mut fresh = BuildCache::new(object.root.clone(), true);
    object.rewrite(&mut fresh);
    assert_eq!(stats(&fresh), (0, 0, 1, 0));
    assert_eq!(fs::read(object.pack()).unwrap(), object.graph);
    object.assert_hit();
}

/// The README promises that a hash-mismatched object "is repaired by the
/// successful build". A normal session that rebuilds a byte-identical pack
/// skips the pack write because a file already exists at the pack path, so
/// the damaged bytes stay and every later session misses again.
#[test]
#[ignore = "known gap: a damaged pack whose rebuilt bytes are identical is never rewritten"]
fn cache_damaged_pack_is_repaired_across_sessions() {
    let object = SingleObject::new("cross-session");
    let mut bytes = fs::read(object.pack()).unwrap();
    bytes[0] ^= 0xff;
    fs::write(object.pack(), &bytes).unwrap();
    let mut cache = BuildCache::new(object.root.clone(), false);
    assert!(object.read(&mut cache).is_none());
    object.rewrite(&mut cache);
    object.assert_hit();
}

#[test]
fn cache_writes_skip_present_objects_unless_fresh_or_rejected() {
    let object = SingleObject::new("skip");
    let pack_modified = fs::metadata(object.pack()).unwrap().modified().unwrap();
    object.edit_metadata("logical_source", json!("marker.hoon"));

    // A normal session leaves an existing pack and metadata untouched.
    let mut cache = BuildCache::new(object.root.clone(), false);
    object.rewrite(&mut cache);
    assert_eq!(stats(&cache), (0, 0, 0, 0));
    assert_eq!(
        read_json(&object.metadata())["logical_source"],
        "marker.hoon"
    );
    assert_eq!(
        fs::metadata(object.pack()).unwrap().modified().unwrap(),
        pack_modified
    );
    // Its in-memory pack is reused for reads in the same session.
    assert!(object.read(&mut cache).is_some());
    assert_eq!(stats(&cache), (1, 0, 0, 0));

    // A payload the caller could not decode is reclassified as corrupt and
    // its metadata is rewritten on the next write.
    cache.reject_loaded_payload(object.key);
    assert_eq!(stats(&cache), (0, 1, 0, 1));
    object.rewrite(&mut cache);
    assert_eq!(stats(&cache), (0, 1, 1, 1));
    assert_eq!(
        read_json(&object.metadata())["logical_source"],
        "cov/t1.hoon"
    );
    // Rejecting with no recorded hit does not underflow.
    let mut idle = BuildCache::new(object.root.clone(), false);
    idle.reject_loaded_payload(object.key);
    assert_eq!(stats(&idle), (0, 1, 0, 1));

    // A fresh session replaces both the pack and the metadata.
    object.edit_metadata("logical_source", json!("marker.hoon"));
    let mut garbled = fs::read(object.pack()).unwrap();
    garbled[0] ^= 0xff;
    fs::write(object.pack(), &garbled).unwrap();
    let mut fresh = BuildCache::new(object.root.clone(), true);
    object.rewrite(&mut fresh);
    assert_eq!(stats(&fresh), (0, 0, 1, 0));
    assert_eq!(fs::read(object.pack()).unwrap(), object.graph);
    assert_eq!(
        read_json(&object.metadata())["logical_source"],
        "cov/t1.hoon"
    );
    object.assert_hit();
}

#[test]
fn cache_write_rejects_empty_undecodable_noncanonical_and_rootless_packs() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("cache");
    let key = blake3::hash(b"reject");
    let root_name = key.to_hex().to_string();
    let noun = nockasm::noun![7 8];
    let bundle = lift_bundle(&[DagInput {
        name: &root_name,
        noun: &noun,
        mode: DagMode::Noun,
    }])
    .unwrap();
    let graph = bundle.to_bytes();
    let mut cache = BuildCache::new(root.clone(), false);

    // Empty entry lists are no-ops, even for bytes that would not decode.
    cache.write_pack(&[], b"garbage, not a bundle").unwrap();
    cache
        .write_pack_prebuilt(&[], Rc::new(bundle.clone()), &graph)
        .unwrap();
    assert!(!root.exists());

    let entries = [cache_entry(key, CacheObjectKind::EntryProduct, &root_name)];
    assert!(cache
        .write_pack(&entries, b"garbage, not a bundle")
        .unwrap_err()
        .to_string()
        .contains("bad Nockasm bundle magic"));
    assert_eq!(
        cache
            .write_pack(&entries, &noncanonical(&graph))
            .unwrap_err()
            .to_string(),
        "refusing to write non-canonical cache pack"
    );

    let rootless = [cache_entry(key, CacheObjectKind::EntryProduct, "absent")];
    assert_eq!(
        cache.write_pack(&rootless, &graph).unwrap_err().to_string(),
        "cache pack is missing root \"absent\""
    );
    assert_eq!(
        cache
            .write_pack_prebuilt(&rootless, Rc::new(bundle.clone()), &graph)
            .unwrap_err()
            .to_string(),
        "cache pack is missing root \"absent\""
    );
    assert!(!root.exists());
    assert_eq!(stats(&cache), (0, 0, 0, 0));

    cache
        .write_pack_prebuilt(&entries, Rc::new(bundle), &graph)
        .unwrap();
    assert_eq!(stats(&cache), (0, 0, 1, 0));
}

#[test]
fn cache_failed_atomic_writes_leave_no_temporary_files() {
    let temp = tempfile::tempdir().unwrap();
    let key = blake3::hash(b"blocked");
    let root_name = key.to_hex().to_string();
    let graph = pack_graph(&[(&root_name, nockasm::noun![1 2])]);
    let entries = [cache_entry(key, CacheObjectKind::DependencyVase, &root_name)];

    // A directory squatting on the metadata path makes the rename fail after
    // the pack was written.
    let root = temp.path().join("metadata-blocked");
    let metadata = metadata_file(&root, key);
    fs::create_dir_all(&metadata).unwrap();
    let mut cache = BuildCache::new(root.clone(), false);
    assert!(cache.write_pack(&entries, &graph).is_err());
    assert_eq!(stats(&cache), (0, 0, 0, 0));
    assert_eq!(fs::read(pack_file(&root, &graph)).unwrap(), graph);
    let leftovers: Vec<_> = fs::read_dir(metadata.parent().unwrap())
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .collect();
    assert_eq!(leftovers, [metadata.clone()]);

    // The same for the pack path; no metadata is written without its pack.
    let root = temp.path().join("pack-blocked");
    let pack = pack_file(&root, &graph);
    fs::create_dir_all(&pack).unwrap();
    let mut cache = BuildCache::new(root.clone(), false);
    assert!(cache.write_pack(&entries, &graph).is_err());
    let leftovers: Vec<_> = fs::read_dir(pack.parent().unwrap())
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .collect();
    assert_eq!(leftovers, [pack.clone()]);
    assert!(!metadata_file(&root, key).exists());
}

fn cache_cli(args: &[&str]) -> std::result::Result<String, String> {
    build_cache::run_cli(&strings(args)).map_err(|error| error.to_string())
}

fn set_age(path: &Path, age: Duration) {
    fs::File::options()
        .write(true)
        .open(path)
        .unwrap()
        .set_modified(SystemTime::now() - age)
        .unwrap();
}

fn file_len(path: &Path) -> u64 {
    fs::metadata(path).unwrap().len()
}

#[test]
fn cache_cli_reports_usage_and_argument_errors() {
    let usage = cache_cli(&[]).unwrap_err();
    assert!(usage.starts_with("Usage: honk cache stats --cache-dir"));
    assert!(usage.contains("Usage: honk cache gc --cache-dir"));
    assert_eq!(cache_cli(&["stats"]).unwrap_err(), usage);
    assert_eq!(cache_cli(&["gc", "--cache-dir"]).unwrap_err(), usage);
    for help in ["help", "-h", "--help"] {
        assert_eq!(cache_cli(&[help, "--cache-dir", "unused"]).unwrap(), usage);
    }
    assert!(cache_cli(&["compact", "--cache-dir", "unused"])
        .unwrap_err()
        .starts_with("unknown cache command \"compact\"\nUsage:"));
    assert!(
        cache_cli(&["gc", "--cache-dir", "unused", "--max-age-days", "soon"])
            .unwrap_err()
            .contains("invalid digit")
    );
}

#[test]
fn cache_cli_stats_and_gc_handle_empty_and_partial_roots() {
    let temp = tempfile::tempdir().unwrap();
    let absent = temp.path().join("absent");
    let absent_str = path_str(&absent);
    assert_eq!(
        cache_cli(&["stats", "--cache-dir", absent_str]).unwrap(),
        format!(
            "cache={absent_str} objects=0 packs=0 pack-bytes=0 metadata-bytes=0 orphan-files=0"
        )
    );
    assert_eq!(
        cache_cli(&["gc", "--cache-dir", absent_str]).unwrap(),
        format!("cache={absent_str} removed-objects=0 removed-bytes=0")
    );
    assert!(!absent.exists());

    // Objects without packs: an expired object is removed, and its empty
    // shard directory with it.
    let object = SingleObject::new("objects-only");
    fs::remove_dir_all(object.root.join("v1/packs")).unwrap();
    set_age(&object.metadata(), Duration::from_secs(90 * 86_400));
    let metadata_bytes = file_len(&object.metadata());
    let root = path_str(&object.root).to_string();
    assert_eq!(
        cache_cli(&["gc", "--cache-dir", &root]).unwrap(),
        format!("cache={root} removed-objects=1 removed-bytes={metadata_bytes}")
    );
    assert!(!object.metadata().parent().unwrap().exists());

    // Packs without objects: every pack is unreferenced.
    let object = SingleObject::new("packs-only");
    fs::remove_dir_all(object.root.join("v1/objects")).unwrap();
    let root = path_str(&object.root).to_string();
    assert_eq!(
        cache_cli(&["stats", "--cache-dir", &root]).unwrap(),
        format!("cache={root} objects=0 packs=0 pack-bytes=0 metadata-bytes=0 orphan-files=1")
    );
    assert_eq!(
        cache_cli(&["gc", "--cache-dir", &root, "--max-age-days", "1"]).unwrap(),
        format!(
            "cache={root} removed-objects=0 removed-bytes={}",
            object.graph.len()
        )
    );
    assert!(!object.pack().parent().unwrap().exists());
}

#[test]
fn cache_cli_stats_and_gc_classify_objects_packs_and_orphans() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("cache");
    let root_str = path_str(&root).to_string();
    let old_key = blake3::hash(b"old object");
    let new_key = blake3::hash(b"new object");
    let old_root = old_key.to_hex().to_string();
    let new_root = new_key.to_hex().to_string();
    let old_graph = pack_graph(&[(&old_root, nockasm::noun![1 2])]);
    let new_graph = pack_graph(&[(&new_root, nockasm::noun![3 4])]);
    let mut cache = BuildCache::new(root.clone(), false);
    cache
        .write_pack(
            &[cache_entry(old_key, CacheObjectKind::DependencyVase, &old_root)],
            &old_graph,
        )
        .unwrap();
    cache
        .write_pack(
            &[cache_entry(new_key, CacheObjectKind::EntryProduct, &new_root)],
            &new_graph,
        )
        .unwrap();
    let old_metadata = metadata_file(&root, old_key);
    let new_metadata = metadata_file(&root, new_key);
    let old_pack = pack_file(&root, &old_graph);
    let new_pack = pack_file(&root, &new_graph);
    assert_ne!(old_metadata.parent(), new_metadata.parent());
    assert_ne!(old_pack.parent(), new_pack.parent());

    // A pack nothing references.
    let stray_graph = pack_graph(&[("stray", nockasm::noun![5 6])]);
    let stray_pack = pack_file(&root, &stray_graph);
    fs::create_dir_all(stray_pack.parent().unwrap()).unwrap();
    fs::write(&stray_pack, &stray_graph).unwrap();
    assert_ne!(stray_pack.parent(), new_pack.parent());

    // Clutter the gc and stats walks must tolerate.
    let object_shard = new_metadata.parent().unwrap();
    let pack_shard = new_pack.parent().unwrap();
    fs::write(root.join("v1/objects/README"), b"top-level file").unwrap();
    fs::write(root.join("v1/packs/README"), b"top-level file").unwrap();
    fs::write(object_shard.join("notes.txt"), b"not metadata").unwrap();
    fs::write(object_shard.join("broken.json"), b"{ nope").unwrap();
    fs::create_dir(object_shard.join("nested")).unwrap();
    fs::write(pack_shard.join("notes.txt"), b"not a pack").unwrap();
    fs::create_dir(pack_shard.join("nested")).unwrap();

    let metadata_bytes = file_len(&old_metadata) + file_len(&new_metadata);
    let pack_bytes = old_graph.len() + new_graph.len();
    // Orphans: the stray pack, both notes files, and the broken metadata.
    assert_eq!(
        cache_cli(&["stats", "--cache-dir", &root_str]).unwrap(),
        format!(
            "cache={root_str} objects=2 packs=2 pack-bytes={pack_bytes} \
             metadata-bytes={metadata_bytes} orphan-files=4"
        )
    );

    // Age out the old object only.
    set_age(&old_metadata, Duration::from_secs(60 * 86_400));
    set_age(&new_metadata, Duration::from_secs(86_400));
    let removed_bytes = file_len(&old_metadata) + file_len(&old_pack) + file_len(&stray_pack);
    assert_eq!(
        cache_cli(&["gc", "--cache-dir", &root_str, "--max-age-days", "30"]).unwrap(),
        format!("cache={root_str} removed-objects=1 removed-bytes={removed_bytes}")
    );
    assert!(!old_metadata.exists());
    assert!(!old_metadata.parent().unwrap().exists());
    assert!(!old_pack.exists());
    assert!(!old_pack.parent().unwrap().exists());
    assert!(!stray_pack.exists());
    assert!(new_metadata.exists());
    assert!(new_pack.exists());
    for survivor in ["notes.txt", "broken.json", "nested"] {
        assert!(object_shard.join(survivor).exists(), "{survivor}");
    }
    assert!(pack_shard.join("notes.txt").exists());
    assert!(root.join("v1/objects/README").exists());
    assert!(root.join("v1/packs/README").exists());

    // The surviving object still reads, and its pack is still counted.
    let mut reader = BuildCache::new(root.clone(), false);
    assert!(reader
        .read(new_key, CacheObjectKind::EntryProduct)
        .unwrap()
        .is_some());
    assert_eq!(
        cache_cli(&["stats", "--cache-dir", &root_str]).unwrap(),
        format!(
            "cache={root_str} objects=1 packs=1 pack-bytes={} metadata-bytes={} orphan-files=3",
            new_graph.len(),
            file_len(&new_metadata)
        )
    );
    // A second gc with the default age finds nothing more to remove.
    assert_eq!(
        cache_cli(&["gc", "--cache-dir", &root_str]).unwrap(),
        format!("cache={root_str} removed-objects=0 removed-bytes=0")
    );
}

// ---------------------------------------------------------------------------
// nasm_bridge.rs: slab noun to hash-consed Nockasm noun conversion
// ---------------------------------------------------------------------------

/// An indirect atom left unnormalized (high words zero) is not u64-representable
/// to nockvm, so the bridge reads its bytes and must trim the zero padding.
#[test]
fn bridge_trims_zero_padded_indirect_atoms() {
    let mut slab: NounSlab = NounSlab::new();
    // SAFETY: both buffers are zeroed and then written before use; they are
    // deliberately left unnormalized.
    let (padded_small, padded_wide) = unsafe {
        let (small, data) = IndirectAtom::new_raw_mut_zeroed(&mut slab, 2);
        *data = 5;
        let (wide, data) = IndirectAtom::new_raw_mut_zeroed(&mut slab, 3);
        *data = 1;
        *data.add(1) = 2;
        (small.as_noun(), wide.as_noun())
    };
    let mut wide_bytes = [0u8; 9];
    wide_bytes[0] = 1;
    wide_bytes[8] = 2;
    let normal_wide = Atom::from_bytes(&mut slab, &wide_bytes).as_noun();
    let root = T(&mut slab, &[padded_small, D(5), padded_wide, normal_wide]);
    slab.set_root(root);
    let space = slab.noun_space();
    assert!(padded_small
        .in_space(&space)
        .as_atom()
        .unwrap()
        .as_u64()
        .is_err());

    let mut bridge = SlabToNockasm::new();
    let converted = bridge.convert(root, &space).unwrap();
    let wide = nockasm::Noun::from(nockasm::Atom::from_le_bytes(&wide_bytes));
    let expected = nasm_cell(
        nasm_atom(5),
        nasm_cell(nasm_atom(5), nasm_cell(wide.clone(), wide)),
    );
    assert_eq!(converted, expected);
    // Padded and normalized spellings intern to one atom each.
    let bundle = lift_bundle(&[DagInput {
        name: "root",
        noun: &converted,
        mode: DagMode::Noun,
    }])
    .unwrap();
    assert_eq!(bundle.nodes().len(), 5);
    // A padded atom converted as its own root matches too.
    assert_eq!(bridge.convert(padded_small, &space).unwrap(), nasm_atom(5));
}

// ---------------------------------------------------------------------------
// arm_map.rs: arm name to axis extraction from core types
// ---------------------------------------------------------------------------

fn term(slab: &mut NounSlab, name: &str) -> Noun {
    term_to_noun(slab, name)
}

/// A `(map term hoon)` / `(map term tome)` treap node.
fn map_node(slab: &mut NounSlab, entry: Noun, left: Noun, right: Noun) -> Noun {
    T(slab, &[entry, left, right])
}

fn arm_entry(slab: &mut NounSlab, name: &str) -> Noun {
    let name = term(slab, name);
    T(slab, &[name, D(0)])
}

fn arm_leaf(slab: &mut NounSlab, name: &str) -> Noun {
    let entry = arm_entry(slab, name);
    map_node(slab, entry, D(0), D(0))
}

fn chapter_entry(slab: &mut NounSlab, name: &str, arms: Noun) -> Noun {
    let name = term(slab, name);
    let tome = T(slab, &[D(0), arms]);
    T(slab, &[name, tome])
}

/// `[%core payload coil]` with `coil = [garb type seminoun tomes]`.
fn core_type(slab: &mut NounSlab, tomes: Noun) -> Noun {
    let tag = term(slab, "core");
    let coil = T(slab, &[D(0), D(0), D(0), tomes]);
    T(slab, &[tag, D(0), coil])
}

fn arm_axes(map: &ArmMap) -> Vec<(String, u64)> {
    let mut axes: Vec<_> = map
        .iter()
        .map(|(name, axis)| (name.clone(), u64::try_from(axis).unwrap()))
        .collect();
    axes.sort();
    axes
}

fn pairs(expected: &[(&str, u64)]) -> Vec<(String, u64)> {
    expected
        .iter()
        .map(|(name, axis)| (name.to_string(), *axis))
        .collect()
}

fn arms_of(slab: &mut NounSlab, ty: Noun) -> crate::errors::Result<ArmMap> {
    slab.set_root(ty);
    let space = slab.noun_space();
    arm_map_from_type(&TypeNoun::new(ty), &space)
}

#[test]
fn arm_map_axes_follow_every_battery_tree_shape() {
    let mut slab: NounSlab = NounSlab::new();
    // Chapter `one`: arm `a` with a left child `b`.
    let a = arm_entry(&mut slab, "a");
    let b = arm_leaf(&mut slab, "b");
    let arms1 = map_node(&mut slab, a, b, D(0));
    // Chapter `two`: arm `c` with a right child `d`.
    let c = arm_entry(&mut slab, "c");
    let d = arm_leaf(&mut slab, "d");
    let arms2 = map_node(&mut slab, c, D(0), d);
    // Chapter `three`: arm `e` with children `f` and `g`.
    let e = arm_entry(&mut slab, "e");
    let f = arm_leaf(&mut slab, "f");
    let g = arm_leaf(&mut slab, "g");
    let arms3 = map_node(&mut slab, e, f, g);
    // Chapter `empty` has no arms at all.
    let one = chapter_entry(&mut slab, "one", arms1);
    let two = chapter_entry(&mut slab, "two", arms2);
    let three = chapter_entry(&mut slab, "three", arms3);
    let empty = chapter_entry(&mut slab, "empty", D(0));
    let two = map_node(&mut slab, two, D(0), D(0));
    let empty = map_node(&mut slab, empty, D(0), D(0));
    let three = map_node(&mut slab, three, empty, D(0));
    let tomes = map_node(&mut slab, one, two, three);
    let core = core_type(&mut slab, tomes);

    // Battery at +2. Chapter `one` is the node at +4 with `two` at +10 and
    // `three` at +11; `three` has a lone child (`empty`, at +23), so its own
    // arms hang off +22.
    let map = arms_of(&mut slab, core).unwrap();
    assert_eq!(
        arm_axes(&map),
        pairs(&[("a", 8), ("b", 9), ("c", 20), ("d", 21), ("e", 44), ("f", 90), ("g", 91),])
    );
    let space = slab.noun_space();
    let via_method = ArmMap::from_type(&TypeNoun::new(core), &space).unwrap();
    assert_eq!(arm_axes(&via_method), arm_axes(&map));
}

#[test]
fn arm_map_single_child_chapters_and_wrapped_cores() {
    let mut slab: NounSlab = NounSlab::new();
    let build = |slab: &mut NounSlab, right_child: bool| {
        let x = arm_leaf(slab, "x");
        let y = arm_leaf(slab, "y");
        let root = chapter_entry(slab, "root", x);
        let child = chapter_entry(slab, "child", y);
        let child = map_node(slab, child, D(0), D(0));
        let tomes = if right_child {
            map_node(slab, root, D(0), child)
        } else {
            map_node(slab, root, child, D(0))
        };
        core_type(slab, tomes)
    };
    // A lone child chapter sits at +3 of the battery either way.
    for right_child in [false, true] {
        let core = build(&mut slab, right_child);
        let map = arms_of(&mut slab, core).unwrap();
        assert_eq!(arm_axes(&map), pairs(&[("x", 4), ("y", 5)]));
    }

    // One chapter with one arm: the battery is the arm.
    let only = arm_leaf(&mut slab, "only");
    let chapter = chapter_entry(&mut slab, "", only);
    let tomes = map_node(&mut slab, chapter, D(0), D(0));
    let core = core_type(&mut slab, tomes);
    assert_eq!(
        arm_axes(&arms_of(&mut slab, core).unwrap()),
        pairs(&[("only", 2)])
    );

    // %face, %hint, and %hold wrappers are looked through.
    let face_tag = term(&mut slab, "face");
    let hint_tag = term(&mut slab, "hint");
    let hold_tag = term(&mut slab, "hold");
    let name = term(&mut slab, "cor");
    let held = T(&mut slab, &[hold_tag, core, D(0)]);
    let hinted = T(&mut slab, &[hint_tag, D(0), held]);
    let faced = T(&mut slab, &[face_tag, name, hinted]);
    assert_eq!(
        arm_axes(&arms_of(&mut slab, faced).unwrap()),
        pairs(&[("only", 2)])
    );

    // An arm named with the empty term is the buc arm.
    let buc = T(&mut slab, &[D(0), D(0)]);
    let buc = map_node(&mut slab, buc, D(0), D(0));
    let chapter = chapter_entry(&mut slab, "", buc);
    let tomes = map_node(&mut slab, chapter, D(0), D(0));
    let gate = core_type(&mut slab, tomes);
    let map = arms_of(&mut slab, gate).unwrap();
    assert_eq!(map.axis_for("$"), Some(BigUint::from(2u32)));

    // Types that are not cores, and cores without chapters, have no arms.
    let atom_tag = term(&mut slab, "atom");
    let atom_type = T(&mut slab, &[atom_tag, D(0), D(0)]);
    let inner = T(&mut slab, &[D(1), D(2)]);
    let cell_headed = T(&mut slab, &[inner, D(3)]);
    let armless = core_type(&mut slab, D(0));
    for ty in [D(0), atom_type, cell_headed, armless] {
        assert!(arms_of(&mut slab, ty).unwrap().iter().next().is_none());
    }
}

#[test]
fn arm_map_rejects_malformed_core_types() {
    let mut slab: NounSlab = NounSlab::new();
    let mut cases: Vec<(Noun, &str)> = Vec::new();

    cases.push((T(&mut slab, &[D(0xff), D(0)]), "type tag decode failed"));
    for (tag, message) in [
        ("core", "core type missing tail"),
        ("face", "face type missing tail"),
        ("hint", "hint type missing tail"),
        ("hold", "hold type missing tail"),
    ] {
        let tag = term(&mut slab, tag);
        cases.push((T(&mut slab, &[tag, D(5)]), message));
    }
    let core_tag = term(&mut slab, "core");
    cases.push((T(&mut slab, &[core_tag, D(0), D(7)]), "core coil not cell"));
    cases.push((
        T(&mut slab, &[core_tag, D(0), D(0), D(7)]),
        "core coil missing tail",
    ));
    cases.push((
        T(&mut slab, &[core_tag, D(0), D(0), D(0), D(7)]),
        "core coil missing tomes",
    ));

    let tomes = T(&mut slab, &[D(5), D(0), D(0)]);
    cases.push((core_type(&mut slab, tomes), "tome entry not cell"));
    let name = term(&mut slab, "chap");
    let entry = T(&mut slab, &[name, D(5)]);
    let tomes = map_node(&mut slab, entry, D(0), D(0));
    cases.push((core_type(&mut slab, tomes), "tome value not cell"));
    let chapter = chapter_entry(&mut slab, "chap", D(0));
    let tomes = T(&mut slab, &[chapter, D(5)]);
    cases.push((core_type(&mut slab, tomes), "map node missing branches"));

    let arms = T(&mut slab, &[D(5), D(0), D(0)]);
    let chapter = chapter_entry(&mut slab, "chap", arms);
    let tomes = map_node(&mut slab, chapter, D(0), D(0));
    cases.push((core_type(&mut slab, tomes), "arm entry not cell"));
    let cell_name = T(&mut slab, &[D(1), D(2)]);
    let entry = T(&mut slab, &[cell_name, D(0)]);
    let arms = map_node(&mut slab, entry, D(0), D(0));
    let chapter = chapter_entry(&mut slab, "chap", arms);
    let tomes = map_node(&mut slab, chapter, D(0), D(0));
    cases.push((core_type(&mut slab, tomes), "arm name not atom"));
    let entry = T(&mut slab, &[D(0xff), D(0)]);
    let arms = map_node(&mut slab, entry, D(0), D(0));
    let chapter = chapter_entry(&mut slab, "chap", arms);
    let tomes = map_node(&mut slab, chapter, D(0), D(0));
    cases.push((core_type(&mut slab, tomes), "arm name decode failed"));

    // A malformed arm or chapter below a well-formed node fails the whole
    // walk, whichever child slot it sits in: right only, left only, and
    // either side of a node with both children.
    for (left_bad, right_bad) in [(false, true), (true, false), (true, true), (false, false)] {
        let pick = |bad: Noun, fine: Noun| match (left_bad, right_bad) {
            (false, true) => (D(0), bad),
            (true, false) => (bad, D(0)),
            (true, true) => (bad, fine),
            (false, false) => (fine, bad),
        };
        let good = arm_entry(&mut slab, "ok");
        let bad = T(&mut slab, &[D(5), D(0), D(0)]);
        let fine = arm_leaf(&mut slab, "fine");
        let (left, right) = pick(bad, fine);
        let arms = map_node(&mut slab, good, left, right);
        let chapter = chapter_entry(&mut slab, "chap", arms);
        let tomes = map_node(&mut slab, chapter, D(0), D(0));
        cases.push((core_type(&mut slab, tomes), "arm entry not cell"));
        // The same shape one level up, in the chapter tree.
        let ok_arms = arm_leaf(&mut slab, "ok");
        let good = chapter_entry(&mut slab, "good", ok_arms);
        let bad = T(&mut slab, &[D(5), D(0), D(0)]);
        let fine_arms = arm_leaf(&mut slab, "fine");
        let fine = chapter_entry(&mut slab, "fine", fine_arms);
        let fine = map_node(&mut slab, fine, D(0), D(0));
        let (left, right) = pick(bad, fine);
        let tomes = map_node(&mut slab, good, left, right);
        cases.push((core_type(&mut slab, tomes), "tome entry not cell"));
    }

    for (ty, message) in cases {
        let error = arms_of(&mut slab, ty).unwrap_err().to_string();
        assert!(
            error.contains(message),
            "expected {message:?}, got {error:?}"
        );
    }
}

#[test]
fn arm_map_from_noun_list_decodes_pairs_and_rejects_malformed_lists() {
    let mut slab: NounSlab = NounSlab::new();
    let wide_axis_bytes = [1u8; 12];
    let wide_axis = Atom::from_bytes(&mut slab, &wide_axis_bytes).as_noun();
    let foo = term(&mut slab, "foo");
    let bar = term(&mut slab, "bar");
    let foo_pair = T(&mut slab, &[foo, D(2)]);
    let bar_pair = T(&mut slab, &[bar, wide_axis]);
    let buc_pair = T(&mut slab, &[D(0), D(7)]);
    let list = T(&mut slab, &[foo_pair, bar_pair, buc_pair, D(0)]);

    let wide_tail = Atom::from_bytes(&mut slab, &[3u8; 10]).as_noun();
    let cell_name = T(&mut slab, &[D(1), D(2)]);
    let cell_axis = T(&mut slab, &[D(1), D(2)]);
    let bad_name_pair = T(&mut slab, &[cell_name, D(2)]);
    let bad_axis_pair = T(&mut slab, &[foo, cell_axis]);
    let bad_utf8_pair = T(&mut slab, &[D(0xff), D(2)]);
    let cases = [
        (
            T(&mut slab, &[foo_pair, D(9)]),
            "unexpected atom in arm-map list: 9",
        ),
        (T(&mut slab, &[foo_pair, wide_tail]), "noun error"),
        (T(&mut slab, &[D(4), D(0)]), "arm-map entry not a cell"),
        (
            T(&mut slab, &[bad_name_pair, D(0)]),
            "arm-map name not atom",
        ),
        (
            T(&mut slab, &[bad_axis_pair, D(0)]),
            "arm-map axis not atom",
        ),
        (
            T(&mut slab, &[bad_utf8_pair, D(0)]),
            "arm-map name decode failed",
        ),
    ];
    let everything: Vec<Noun> = std::iter::once(list)
        .chain(cases.iter().map(|(noun, _)| *noun))
        .collect();
    let root = T(&mut slab, &everything);
    slab.set_root(root);
    let space = slab.noun_space();

    let map = arm_map_from_noun_list(list, &space).unwrap();
    assert_eq!(map.axis_for("foo"), Some(BigUint::from(2u32)));
    assert_eq!(map.axis_for("$"), Some(BigUint::from(7u32)));
    assert_eq!(
        map.axis_for("bar"),
        Some(BigUint::from_bytes_le(&wide_axis_bytes))
    );
    assert_eq!(map.iter().count(), 3);
    assert!(arm_map_from_noun_list(D(0), &space)
        .unwrap()
        .iter()
        .next()
        .is_none());

    for (noun, message) in cases {
        let error = arm_map_from_noun_list(noun, &space)
            .unwrap_err()
            .to_string();
        assert!(
            error.contains(message),
            "expected {message:?}, got {error:?}"
        );
    }
}

#[test]
fn arm_map_builders_insert_and_iterate() {
    let mut map = ArmMap::with_pairs(vec![("a".to_string(), 2u32), ("b".to_string(), 6u32)]);
    assert_eq!(map.axis_for("a"), Some(BigUint::from(2u32)));
    assert_eq!(map.axis_for("missing"), None);
    map.insert("c".to_string(), 7u64);
    map.insert("a".to_string(), BigUint::from(12u32));
    assert_eq!(arm_axes(&map), pairs(&[("a", 12), ("b", 6), ("c", 7)]));
    let copy = map.clone();
    assert_eq!(arm_axes(&copy), arm_axes(&map));
    assert!(format!("{copy:?}").contains("by_name"));
    assert!(ArmMap::new().iter().next().is_none());
    assert!(ArmMap::default().axis_for("a").is_none());
}

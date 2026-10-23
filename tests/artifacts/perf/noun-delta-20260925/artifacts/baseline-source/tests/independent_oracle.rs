//! Correctness checks use a separate logical value model and hand-built files.
//! A matching content hash alone is not considered a reconstruction proof.
mod common;

use common::{Fixture, Layout, Model, Scratch, Value};
use nockchain_noun_delta_bench::engine::{
    build_delta, build_index, verify_equal, BuildOptions, ContentLookup, Delta, DeltaView, Graph,
};
use nockchain_noun_delta_bench::pma::{Node, Snapshot};
use std::collections::HashSet;

fn significant_number(value: u64) -> Vec<u8> {
    let mut bytes = value.to_le_bytes().to_vec();
    while bytes.last() == Some(&0) {
        bytes.pop();
    }
    bytes
}

fn assert_snapshot_value<G: Graph>(snapshot: &G, raw_root: u64, model: &Model, model_root: usize) {
    let mut work = vec![(raw_root, model_root)];
    let mut compared = HashSet::new();
    while let Some((raw, expected)) = work.pop() {
        if !compared.insert((raw, expected)) {
            continue;
        }
        match (
            snapshot.node(raw).expect("read fixture noun"),
            &model.nodes[expected],
        ) {
            (Node::Direct(actual), Value::Atom(bytes)) => {
                assert_eq!(significant_number(actual), *bytes)
            }
            (Node::Atom(actual), Value::Atom(bytes)) => {
                assert_eq!(
                    actual.len(),
                    bytes.len(),
                    "atom length differs at model node {expected}"
                );
                assert!(
                    actual == bytes.as_slice(),
                    "atom contents differ at model node {expected}"
                );
            }
            (Node::Cell { head, tail }, Value::Cell(ehead, etail)) => {
                work.push((tail, *etail));
                work.push((head, *ehead));
            }
            (_, value) => {
                let kind = match value {
                    Value::Atom(_) => "atom",
                    Value::Cell(_, _) => "cell",
                };
                panic!("node shape differs at model node {expected}: expected {kind}")
            }
        }
    }
}

fn open(fixture: &Fixture) -> Snapshot {
    let snapshot =
        Snapshot::open(&fixture.pma, &fixture.manifest).expect("open hand-built fixture");
    assert_eq!(snapshot.root(), fixture.root);
    snapshot
        .verify_used_hash()
        .expect("independent used-prefix hash");
    snapshot
}

#[test]
fn reader_matches_independent_model_across_relocation_metadata_and_atom_encodings() {
    let scratch = Scratch::new();
    let mut model = Model::default();
    let zero = model.number(0);
    let seven = model.number(7);
    let high = model.number(1 << 63);
    let mut huge = vec![0x5a; 1024 * 1024 + 3];
    huge[0] = 0;
    let huge = model.atom(&huge);
    let shared = model.cell(seven, huge);
    let pair = model.cell(shared, shared);
    let end = model.cell(high, zero);
    let root = model.cell(pair, end);
    let baseline = Fixture::write(scratch.path(), "base", &model, root, Layout::default());
    let relocated = Fixture::write(
        scratch.path(),
        "relocated",
        &model,
        root,
        Layout {
            padding_words: 79,
            metadata: 0x7fff_ffff,
            force_indirect: true,
            atom_padding_words: 2,
        },
    );
    assert_ne!(baseline.root, relocated.root);
    assert_ne!(baseline.original_pma, relocated.original_pma);
    for fixture in [&baseline, &relocated] {
        let snapshot = open(fixture);
        assert_snapshot_value(&snapshot, snapshot.root(), &model, root);
        fixture.assert_unchanged();
    }
}

#[test]
fn reader_traverses_deep_state_without_recursive_test_oracle() {
    let scratch = Scratch::new();
    let mut model = Model::default();
    let mut root = model.number(0);
    let item = model.number(1);
    for _ in 0..50_000 {
        root = model.cell(item, root);
    }
    let fixture = Fixture::write(scratch.path(), "deep", &model, root, Layout::default());
    let snapshot = open(&fixture);
    assert_snapshot_value(&snapshot, snapshot.root(), &model, root);
    fixture.assert_unchanged();
}

#[test]
fn state_axis_six_selects_state_not_full_arvo_subject() {
    let scratch = Scratch::new();
    let mut model = Model::default();
    let battery = model.number(11);
    let context = model.number(22);
    let state_head = model.number(33);
    let state_tail = model.number(44);
    let state = model.cell(state_head, state_tail);
    let payload = model.cell(state, context);
    let core = model.cell(battery, payload);
    let fixture = Fixture::write(scratch.path(), "core", &model, core, Layout::default());
    let snapshot = open(&fixture);
    let selected = snapshot.slot(snapshot.root(), 6).expect("state axis");
    assert_eq!(selected, fixture.raw_nodes[state]);
    assert_snapshot_value(&snapshot, selected, &model, state);
    fixture.assert_unchanged();
}

// Independent protocol oracle: computes only from semantic values, in model
// topological order. It never calls the engine hashing functions or reads PMA.
fn model_hash(model: &Model, root: usize) -> [u8; 32] {
    let mut hashes: Vec<[u8; 32]> = Vec::with_capacity(model.nodes.len());
    for node in &model.nodes {
        let mut hasher = blake3::Hasher::new();
        match node {
            Value::Atom(bytes) => {
                hasher.update(b"nockchain-noun-atom-v1\0");
                hasher.update(&(bytes.len() as u64).to_le_bytes());
                hasher.update(bytes);
            }
            Value::Cell(head, tail) => {
                hasher.update(b"nockchain-noun-cell-v1\0");
                hasher.update(&hashes[*head]);
                hasher.update(&hashes[*tail]);
            }
        }
        hashes.push(*hasher.finalize().as_bytes());
    }
    hashes[root]
}

fn options() -> BuildOptions {
    BuildOptions {
        max_nodes: None,
        progress_every: 0,
    }
}

fn sample_model(changed: u64, huge: &[u8], duplicate_shared_branch: bool) -> (Model, usize) {
    let mut model = Model::default();
    let zero = model.number(0);
    let small = model.number(changed);
    let blob = model.atom(huge);
    let branch = model.cell(small, blob);
    let second = if duplicate_shared_branch {
        model.cell(small, blob)
    } else {
        branch
    };
    let pair = model.cell(branch, second);
    let root = model.cell(pair, zero);
    (model, root)
}

#[test]
fn relocated_and_differently_shared_values_have_same_content_root_and_reconstruct() {
    let scratch = Scratch::new();
    let (base_model, base_root) = sample_model(7, &[0x88; 29], false);
    let (target_model, target_root) = sample_model(7, &[0x88; 29], true);
    let base_fixture = Fixture::write(
        scratch.path(),
        "base",
        &base_model,
        base_root,
        Layout::default(),
    );
    let target_fixture = Fixture::write(
        scratch.path(),
        "target",
        &target_model,
        target_root,
        Layout {
            padding_words: 95,
            metadata: 0x7654321,
            force_indirect: true,
            atom_padding_words: 3,
        },
    );
    let base = open(&base_fixture);
    let target = open(&target_fixture);
    let base_index = build_index(&base, options()).unwrap();
    let target_index = build_index(&target, options()).unwrap();
    assert_eq!(base_index.root_hash(), model_hash(&base_model, base_root));
    assert_eq!(
        target_index.root_hash(),
        model_hash(&target_model, target_root)
    );
    assert_eq!(base_index.root_hash(), target_index.root_hash());
    verify_equal(&base, &target).expect("physical layouts preserve the same noun value");
    let lookup = ContentLookup::new(&base_index);
    let delta = build_delta(&target, &target_index, &base_index, &lookup).unwrap();
    let view = DeltaView::new(&base, &base_index, &delta).unwrap();
    assert_snapshot_value(&view, view.root(), &target_model, target_root);
    let mut encoded = Vec::new();
    delta.write_to(&mut encoded).unwrap();
    let decoded = Delta::read_from(&mut encoded.as_slice()).unwrap();
    let roundtrip = DeltaView::new(&base, &base_index, &decoded).unwrap();
    assert_snapshot_value(&roundtrip, roundtrip.root(), &target_model, target_root);
    base_fixture.assert_unchanged();
    target_fixture.assert_unchanged();
}

#[test]
fn changed_atom_and_large_atom_delta_chain_match_independent_values_and_hashes() {
    let scratch = Scratch::new();
    let large = vec![0x93; 1024 * 1024 + 11];
    let mut changed_large = large.clone();
    changed_large[131_073] ^= 0x55;
    let (a_model, a_root) = sample_model(7, &large, false);
    let (b_model, b_root) = sample_model(8, &large, false);
    let (c_model, c_root) = sample_model(8, &changed_large, true);
    let a_fixture = Fixture::write(scratch.path(), "a", &a_model, a_root, Layout::default());
    let b_fixture = Fixture::write(
        scratch.path(),
        "b",
        &b_model,
        b_root,
        Layout {
            padding_words: 113,
            ..Layout::default()
        },
    );
    let c_fixture = Fixture::write(
        scratch.path(),
        "c",
        &c_model,
        c_root,
        Layout {
            padding_words: 227,
            metadata: 1234,
            ..Layout::default()
        },
    );
    let a = open(&a_fixture);
    let b = open(&b_fixture);
    let c = open(&c_fixture);
    let a_index = build_index(&a, options()).unwrap();
    let b_index = build_index(&b, options()).unwrap();
    let c_index = build_index(&c, options()).unwrap();
    assert_eq!(a_index.root_hash(), model_hash(&a_model, a_root));
    assert_eq!(b_index.root_hash(), model_hash(&b_model, b_root));
    assert_eq!(c_index.root_hash(), model_hash(&c_model, c_root));
    assert_ne!(a_index.root_hash(), b_index.root_hash());
    assert_ne!(b_index.root_hash(), c_index.root_hash());
    assert!(
        verify_equal(&a, &b).is_err(),
        "changed small atom differs structurally"
    );
    assert!(
        verify_equal(&b, &c).is_err(),
        "changed large atom differs structurally"
    );
    let delta_ab = build_delta(&b, &b_index, &a_index, &ContentLookup::new(&a_index)).unwrap();
    let view_b = DeltaView::new(&a, &a_index, &delta_ab).unwrap();
    assert_snapshot_value(&view_b, view_b.root(), &b_model, b_root);
    let view_b_index = build_index(&view_b, options()).unwrap();
    assert_eq!(view_b_index.root_hash(), model_hash(&b_model, b_root));
    let delta_bc = build_delta(
        &c,
        &c_index,
        &view_b_index,
        &ContentLookup::new(&view_b_index),
    )
    .unwrap();
    let mut encoded = Vec::new();
    delta_bc.write_to(&mut encoded).unwrap();
    let decoded = Delta::read_from(&mut encoded.as_slice()).unwrap();
    let view_c = DeltaView::new(&view_b, &view_b_index, &decoded).unwrap();
    assert_snapshot_value(&view_c, view_c.root(), &c_model, c_root);
    verify_equal(&c, &view_c).expect("layered view preserves the target noun value");
    assert_eq!(
        build_index(&view_c, options()).unwrap().root_hash(),
        model_hash(&c_model, c_root)
    );
    assert!(
        DeltaView::new(&c, &c_index, &delta_ab).is_err(),
        "delta must reject a different base state"
    );
    for fixture in [&a_fixture, &b_fixture, &c_fixture] {
        fixture.assert_unchanged();
    }
}

#[test]
fn deep_chain_index_and_delta_verification_use_independent_iterative_oracle() {
    let scratch = Scratch::new();
    let mut base_model = Model::default();
    let mut base_root = base_model.number(0);
    let one = base_model.number(1);
    for _ in 0..50_000 {
        base_root = base_model.cell(one, base_root);
    }
    let mut target_model = base_model.clone();
    let two = target_model.number(2);
    let target_root = target_model.cell(two, base_root);
    let base_fixture = Fixture::write(
        scratch.path(),
        "deep-base",
        &base_model,
        base_root,
        Layout::default(),
    );
    let target_fixture = Fixture::write(
        scratch.path(),
        "deep-target",
        &target_model,
        target_root,
        Layout {
            padding_words: 127,
            ..Layout::default()
        },
    );
    let base = open(&base_fixture);
    let target = open(&target_fixture);
    let base_index = build_index(&base, options()).unwrap();
    let target_index = build_index(&target, options()).unwrap();
    assert_eq!(base_index.root_hash(), model_hash(&base_model, base_root));
    assert_eq!(
        target_index.root_hash(),
        model_hash(&target_model, target_root)
    );
    let delta = build_delta(
        &target,
        &target_index,
        &base_index,
        &ContentLookup::new(&base_index),
    )
    .unwrap();
    let view = DeltaView::new(&base, &base_index, &delta).unwrap();
    assert_snapshot_value(&view, view.root(), &target_model, target_root);
    base_fixture.assert_unchanged();
    target_fixture.assert_unchanged();
}

#[test]
fn scalar_roots_preserve_numeric_boundaries_across_indirect_representations() {
    let scratch = Scratch::new();
    let mut values: Vec<Vec<u8>> = [0, 1, 255, 256, (1 << 63) - 1, 1 << 63, u64::MAX]
        .into_iter()
        .map(significant_number)
        .collect();
    values.push(vec![0, 0, 0, 0, 0, 0, 0, 0, 1]);
    for (case, value) in values.iter().enumerate() {
        let mut model = Model::default();
        let root = model.atom(value);
        let base_fixture = Fixture::write(
            scratch.path(),
            &format!("scalar-base-{case}"),
            &model,
            root,
            Layout::default(),
        );
        let target_fixture = Fixture::write(
            scratch.path(),
            &format!("scalar-indirect-{case}"),
            &model,
            root,
            Layout {
                padding_words: 31,
                force_indirect: true,
                atom_padding_words: 2,
                ..Layout::default()
            },
        );
        let base = open(&base_fixture);
        let target = open(&target_fixture);
        let base_index = build_index(&base, options()).unwrap();
        let target_index = build_index(&target, options()).unwrap();
        assert_eq!(base_index.root_hash(), model_hash(&model, root));
        assert_eq!(target_index.root_hash(), model_hash(&model, root));
        let delta = build_delta(
            &target,
            &target_index,
            &base_index,
            &ContentLookup::new(&base_index),
        )
        .unwrap();
        let view = DeltaView::new(&base, &base_index, &delta).unwrap();
        assert_snapshot_value(&view, view.root(), &model, root);
        assert_eq!(
            build_index(&view, options()).unwrap().root_hash(),
            model_hash(&model, root)
        );
        base_fixture.assert_unchanged();
        target_fixture.assert_unchanged();
    }
}

#[test]
fn serialized_delta_rejects_changed_payload_and_false_target_hash() {
    let scratch = Scratch::new();
    let (base_model, base_root) = sample_model(7, &[0x87; 29], false);
    let marker = vec![0x93; 257];
    let (target_model, target_root) = sample_model(8, &marker, false);
    let base_fixture = Fixture::write(
        scratch.path(),
        "corruption-base",
        &base_model,
        base_root,
        Layout::default(),
    );
    let target_fixture = Fixture::write(
        scratch.path(),
        "corruption-target",
        &target_model,
        target_root,
        Layout::default(),
    );
    let base = open(&base_fixture);
    let target = open(&target_fixture);
    let base_index = build_index(&base, options()).unwrap();
    let target_index = build_index(&target, options()).unwrap();
    let delta = build_delta(
        &target,
        &target_index,
        &base_index,
        &ContentLookup::new(&base_index),
    )
    .unwrap();
    let mut encoded = Vec::new();
    assert_eq!(
        delta.write_to(&mut encoded).unwrap(),
        delta.stats().encoded_bytes
    );
    let decoded = Delta::read_from(&mut encoded.as_slice()).unwrap();
    let view = DeltaView::new(&base, &base_index, &decoded).unwrap();
    assert_snapshot_value(&view, view.root(), &target_model, target_root);

    let atom_start = encoded
        .windows(marker.len())
        .position(|bytes| bytes == marker)
        .unwrap();
    let mut changed_atom = encoded.clone();
    changed_atom[atom_start + 101] ^= 0x40;
    let changed_atom = Delta::read_from(&mut changed_atom.as_slice()).unwrap();
    assert!(
        DeltaView::new(&base, &base_index, &changed_atom).is_err(),
        "valid framing must not allow changed atom bytes to retain the original target fingerprint"
    );

    // Wire header: 8-byte magic, 32-byte base root, 32-byte base layout, target root.
    let mut false_target = encoded.clone();
    false_target[72] ^= 1;
    let false_target = Delta::read_from(&mut false_target.as_slice()).unwrap();
    assert!(
        DeltaView::new(&base, &base_index, &false_target).is_err(),
        "a claimed target fingerprint must agree with the reconstructed envelope root"
    );

    let mut trailing = encoded.clone();
    trailing.push(0);
    assert!(Delta::read_from(&mut trailing.as_slice()).is_err());
    assert!(Delta::read_from(&mut &encoded[..encoded.len() - 1]).is_err());
    base_fixture.assert_unchanged();
    target_fixture.assert_unchanged();
}

#[test]
fn cyclic_graph_and_explicit_index_budget_fail_instead_of_partially_succeeding() {
    struct CyclicGraph;
    impl Graph for CyclicGraph {
        fn root(&self) -> u64 {
            1 << 63
        }
        fn node(&self, raw: u64) -> anyhow::Result<Node<'_>> {
            if raw == 0 {
                return Ok(Node::Direct(0));
            }
            Ok(Node::Cell {
                head: 0,
                tail: self.root(),
            })
        }
    }
    assert!(build_index(&CyclicGraph, options()).is_err());
    let scratch = Scratch::new();
    let (model, root) = sample_model(7, &[0x91; 29], false);
    let fixture = Fixture::write(scratch.path(), "budget", &model, root, Layout::default());
    let snapshot = open(&fixture);
    assert!(build_index(
        &snapshot,
        BuildOptions {
            max_nodes: Some(2),
            progress_every: 0
        }
    )
    .is_err());
    assert_eq!(
        build_index(&snapshot, options()).unwrap().root_hash(),
        model_hash(&model, root)
    );
    fixture.assert_unchanged();
}

//! Independent value oracle for exact shared-dictionary snapshot deltas.
//! Reconstruction tests discard the target mapping before mounting a payload.
#[allow(dead_code)]
mod common;
#[path = "../src/hashcons.rs"]
mod hashcons;

use common::{Fixture, Layout, Model, Scratch, Value};
use hashcons::{Delta, Dictionary, Options, View};
use nockchain_noun_delta_bench::engine::{verify_equal, Graph};
use nockchain_noun_delta_bench::pma::{Node, Snapshot};
use std::collections::HashSet;

fn options() -> Options {
    Options {
        max_nodes: None,
        progress_every: 0,
    }
}

fn open(f: &Fixture) -> Snapshot {
    let s = Snapshot::open(&f.pma, &f.manifest).unwrap();
    assert_eq!(s.root(), f.root);
    s.verify_used_hash().unwrap();
    s
}

fn number_bytes(n: u64) -> Vec<u8> {
    let mut out = n.to_le_bytes().to_vec();
    while out.last() == Some(&0) {
        out.pop();
    }
    out
}

// No dictionary/hashcons code participates in this protocol oracle.
fn portable_hash(model: &Model, root: usize) -> [u8; 32] {
    let mut hashes: Vec<[u8; 32]> = Vec::new();
    for value in &model.nodes {
        let mut h = blake3::Hasher::new();
        match value {
            Value::Atom(bytes) => {
                h.update(b"nockchain-noun-atom-v1\0");
                h.update(&(bytes.len() as u64).to_le_bytes());
                h.update(bytes);
            }
            Value::Cell(head, tail) => {
                h.update(b"nockchain-noun-cell-v1\0");
                h.update(&hashes[*head]);
                h.update(&hashes[*tail]);
            }
        }
        hashes.push(*h.finalize().as_bytes());
    }
    hashes[root]
}

fn assert_value(graph: &impl Graph, model: &Model, root: usize) {
    let mut pending = vec![(graph.root(), root)];
    let mut seen = HashSet::new();
    while let Some((raw, expected)) = pending.pop() {
        if !seen.insert((raw, expected)) {
            continue;
        }
        match (graph.node(raw).unwrap(), &model.nodes[expected]) {
            (Node::Direct(value), Value::Atom(bytes)) => assert_eq!(number_bytes(value), *bytes),
            (Node::Atom(actual), Value::Atom(bytes)) => {
                assert_eq!(actual.len(), bytes.len());
                assert!(
                    actual == bytes.as_slice(),
                    "atom content differs at model node {expected}"
                );
            }
            (Node::Cell { head, tail }, Value::Cell(ehead, etail)) => {
                pending.push((tail, *etail));
                pending.push((head, *ehead));
            }
            _ => panic!("noun shape differs at model node {expected}"),
        }
    }
}

fn model(small: u64, atom: &[u8], duplicate: bool) -> (Model, usize) {
    let mut m = Model::default();
    let zero = m.number(0);
    let small = m.number(small);
    let large = m.atom(atom);
    let first = m.cell(small, large);
    let second = if duplicate {
        m.cell(small, large)
    } else {
        first
    };
    let pair = m.cell(first, second);
    let root = m.cell(pair, zero);
    (m, root)
}

#[test]
fn exact_dictionary_coalesces_relocation_sharing_and_padded_small_atoms() {
    let scratch = Scratch::new();
    let (a_model, a_root) = model(7, &[0x93; 257], false);
    let (b_model, b_root) = model(7, &[0x93; 257], true);
    let a_file = Fixture::write(
        scratch.path(),
        "same-a",
        &a_model,
        a_root,
        Layout::default(),
    );
    let b_file = Fixture::write(
        scratch.path(),
        "same-b",
        &b_model,
        b_root,
        Layout {
            padding_words: 127,
            metadata: 0x7654321,
            force_indirect: true,
            atom_padding_words: 2,
        },
    );
    let a = open(&a_file);
    let b = open(&b_file);
    let mut dictionary = Dictionary::new();
    let a_interned = dictionary.intern(&a, options()).unwrap();
    let base_count = dictionary.node_count();
    let b_interned = dictionary.intern(&b, options()).unwrap();
    assert_eq!(a_interned.stats.raw_memo_entries_retained, 0);
    assert!(a_interned.stats.raw_memo_peak_estimated_bytes > 0);
    assert_eq!(b_interned.stats.raw_memo_entries_retained, 0);
    assert_eq!(b_interned.stats.canonical_nodes_before, base_count);
    assert_eq!(b_interned.stats.canonical_nodes_after, base_count);
    assert_eq!(b_interned.stats.new_cells, 0);
    assert_eq!(b_interned.stats.new_atoms, 0);
    assert_eq!(a_interned.root, b_interned.root);
    assert_eq!(
        dictionary.node_count(),
        base_count,
        "allocation duplication adds no new noun values"
    );
    assert_eq!(
        dictionary.root_hash(a_interned.root).unwrap(),
        portable_hash(&a_model, a_root)
    );
    assert_eq!(
        dictionary.root_hash(b_interned.root).unwrap(),
        portable_hash(&b_model, b_root)
    );
    let mut encoded = Vec::new();
    let reported = dictionary
        .write_delta(base_count, a_interned.root, b_interned.root, &mut encoded)
        .unwrap();
    assert_eq!(reported, encoded.len() as u64);
    let decoded = Delta::read_from(&mut encoded.as_slice()).unwrap();
    let base = dictionary.into_base(base_count, a_interned.root).unwrap();
    let view = View::new(&base, &decoded).unwrap();
    assert_value(&view, &b_model, b_root);
    verify_equal(&b, &view).unwrap();
    a_file.assert_unchanged();
    b_file.assert_unchanged();
}

#[test]
fn changed_large_atom_payload_reconstructs_after_target_mapping_and_file_are_gone() {
    let scratch = Scratch::new();
    let atom = vec![0x93; 1024 * 1024 + 11];
    let mut changed_atom = atom.clone();
    changed_atom[131_073] ^= 0x40;
    let (a_model, a_root) = model(7, &atom, false);
    let (b_model, b_root) = model(8, &changed_atom, true);
    let a_file = Fixture::write(
        scratch.path(),
        "owned-a",
        &a_model,
        a_root,
        Layout::default(),
    );
    let b_file = Fixture::write(
        scratch.path(),
        "owned-b",
        &b_model,
        b_root,
        Layout {
            padding_words: 89,
            ..Layout::default()
        },
    );
    let a = open(&a_file);
    let (base_count, base_root, target_root, mut encoded) = {
        let b = open(&b_file);
        let mut dictionary = Dictionary::new();
        let first = dictionary.intern(&a, options()).unwrap();
        let count = dictionary.node_count();
        let second = dictionary.intern(&b, options()).unwrap();
        assert_eq!(second.stats.raw_memo_entries_retained, 0);
        assert_eq!(second.stats.new_atoms, 1);
        assert_eq!(
            second.stats.canonical_nodes_after - second.stats.canonical_nodes_before,
            second.stats.new_cells + second.stats.new_atoms
        );
        assert_ne!(first.root, second.root);
        assert!(dictionary.node_count() > count);
        assert_eq!(
            dictionary.root_hash(second.root).unwrap(),
            portable_hash(&b_model, b_root)
        );
        let mut bytes = Vec::new();
        assert_eq!(
            dictionary
                .write_delta(count, first.root, second.root, &mut bytes)
                .unwrap(),
            bytes.len() as u64
        );
        a_file.assert_unchanged();
        b_file.assert_unchanged();
        (count, first.root, second.root, bytes)
    }; // Both the combined dictionary and B mmap are dropped here.
    std::fs::remove_file(&b_file.pma).unwrap();
    std::fs::remove_file(&b_file.manifest).unwrap();
    let decoded = Delta::read_from(&mut encoded.as_slice()).unwrap();
    encoded.fill(0); // The parsed records must own their literal atom bytes.
    drop(encoded);
    let mut a_only = Dictionary::new();
    let a_again = a_only.intern(&a, options()).unwrap();
    assert_eq!(a_again.root, base_root);
    assert_eq!(a_only.node_count(), base_count);
    let base = a_only.into_base(base_count, base_root).unwrap();
    let view = View::new(&base, &decoded).unwrap();
    assert_eq!(view.root(), target_root);
    assert_value(&view, &b_model, b_root);
    // A second independent physical encoding supplies the production comparator.
    let check_file = Fixture::write(
        scratch.path(),
        "owned-check",
        &b_model,
        b_root,
        Layout {
            padding_words: 301,
            force_indirect: true,
            ..Layout::default()
        },
    );
    let check = open(&check_file);
    verify_equal(&check, &view).unwrap();
    assert!(!b_file.pma.exists());
    a_file.assert_unchanged();
    check_file.assert_unchanged();
}

#[test]
fn deep_exact_dictionary_and_portable_root_preserve_ordered_cells() {
    let scratch = Scratch::new();
    let mut m = Model::default();
    let zero = m.number(0);
    let one = m.number(1);
    let two = m.number(2);
    let mut root = zero;
    for i in 0..25_000 {
        root = m.cell(if i % 2 == 0 { one } else { two }, root);
    }
    let a_file = Fixture::write(scratch.path(), "deep-a", &m, root, Layout::default());
    let a = open(&a_file);
    let mut dictionary = Dictionary::new();
    let first = dictionary.intern(&a, options()).unwrap();
    let base_count = dictionary.node_count();
    assert_eq!(
        dictionary.root_hash(first.root).unwrap(),
        portable_hash(&m, root)
    );
    let mut target = m.clone();
    let target_root = target.cell(two, root);
    let b_file = Fixture::write(
        scratch.path(),
        "deep-b",
        &target,
        target_root,
        Layout {
            padding_words: 59,
            ..Layout::default()
        },
    );
    let b = open(&b_file);
    let second = dictionary.intern(&b, options()).unwrap();
    assert_eq!(dictionary.node_count(), base_count + 1);
    assert_eq!(
        dictionary.root_hash(second.root).unwrap(),
        portable_hash(&target, target_root)
    );
    let mut encoded = Vec::new();
    dictionary
        .write_delta(base_count, first.root, second.root, &mut encoded)
        .unwrap();
    let decoded = Delta::read_from(&mut encoded.as_slice()).unwrap();
    let base = dictionary.into_base(base_count, first.root).unwrap();
    let view = View::new(&base, &decoded).unwrap();
    assert_value(&view, &target, target_root);
    verify_equal(&b, &view).unwrap();
    a_file.assert_unchanged();
    b_file.assert_unchanged();
}

#[test]
fn delta_binds_retained_dictionary_namespace_beyond_current_root_value() {
    let scratch = Scratch::new();
    let pair = |head, tail| {
        let mut m = Model::default();
        let h = m.number(head);
        let t = m.number(tail);
        let root = m.cell(h, t);
        (m, root)
    };
    let (old_model, old_root) = pair(11, 12);
    let (other_model, other_root) = pair(13, 14);
    let (base_model, base_model_root) = pair(7, 8);
    let old_file = Fixture::write(
        scratch.path(),
        "namespace-old",
        &old_model,
        old_root,
        Layout::default(),
    );
    let other_file = Fixture::write(
        scratch.path(),
        "namespace-other",
        &other_model,
        other_root,
        Layout::default(),
    );
    let base_file = Fixture::write(
        scratch.path(),
        "namespace-base",
        &base_model,
        base_model_root,
        Layout::default(),
    );
    let old = open(&old_file);
    let other = open(&other_file);
    let base_graph = open(&base_file);
    let mut correct = Dictionary::new();
    let historical = correct.intern(&old, options()).unwrap();
    let current = correct.intern(&base_graph, options()).unwrap();
    let count = correct.node_count();
    let mut encoded = Vec::new();
    correct
        .write_delta(count, current.root, historical.root, &mut encoded)
        .unwrap();
    let decoded = Delta::read_from(&mut encoded.as_slice()).unwrap();
    let mut wrong = Dictionary::new();
    wrong.intern(&other, options()).unwrap();
    let wrong_current = wrong.intern(&base_graph, options()).unwrap();
    assert_eq!(wrong_current.root, current.root);
    assert_eq!(wrong.node_count(), count);
    assert_eq!(
        wrong.root_hash(wrong_current.root).unwrap(),
        correct.root_hash(current.root).unwrap()
    );
    let correct_base = correct.into_base(count, current.root).unwrap();
    let wrong_base = wrong.into_base(count, wrong_current.root).unwrap();
    let view = View::new(&correct_base, &decoded).unwrap();
    assert_value(&view, &old_model, old_root);
    assert!(
        View::new(&wrong_base, &decoded).is_err(),
        "equal current root/count cannot authorize different historical canonical IDs"
    );
    for fixture in [&old_file, &other_file, &base_file] {
        fixture.assert_unchanged();
    }
}

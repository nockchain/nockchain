//! Coverage-driven tests for the `ut` submodules, `noun.rs`, and `formula.rs`.
//!
//! Added to close branch-coverage gaps; see the coverage report in the PR.

use std::collections::HashMap as StdHashMap;
use std::path::Path;

use nockvm::mug::calc_cell_mug_u32;

#[allow(unused_imports)]
use super::super::*;
use crate::native::{formula as nf, noun as nn};

// ---------------------------------------------------------------------------
// Shared helpers
// ---------------------------------------------------------------------------

/// Mug of a direct atom (computed, never cached).
fn direct_mug(n: u64, space: &NounSpace) -> u32 {
    get_mug(D(n), space).expect("direct atoms always have a mug")
}

/// Two distinct direct atoms whose mugs collide (birthday search; deterministic).
fn colliding_atoms() -> (u64, u64) {
    let slab: NounSlab = NounSlab::new();
    let space = slab.noun_space();
    let mut seen: StdHashMap<u32, u64> = StdHashMap::new();
    for n in 1u64.. {
        let mug = direct_mug(n, &space);
        if let Some(prior) = seen.insert(mug, n) {
            return (prior, n);
        }
    }
    unreachable!()
}

/// Two distinct direct atoms `x`, `y` such that `[x 0]` and `[y 0]` have equal
/// cell mugs while `x` and `y` have different atom mugs.
fn colliding_cell_heads() -> (u64, u64) {
    let slab: NounSlab = NounSlab::new();
    let space = slab.noun_space();
    let zero = direct_mug(0, &space);
    let mut seen: StdHashMap<u32, u64> = StdHashMap::new();
    for n in 1u64.. {
        let head = direct_mug(n, &space);
        let mug = unsafe { calc_cell_mug_u32(head, zero, &space) };
        if let Some(prior) = seen.insert(mug, n) {
            if direct_mug(prior, &space) != head {
                return (prior, n);
            }
        }
    }
    unreachable!()
}

/// A direct atom `a` and a head `x` such that `a` and `[x 0]` share a mug.
fn colliding_atom_and_cell() -> (u64, u64) {
    let slab: NounSlab = NounSlab::new();
    let space = slab.noun_space();
    let zero = direct_mug(0, &space);
    let mut atoms: StdHashMap<u32, u64> = StdHashMap::new();
    for n in 1u64..=(1 << 17) {
        atoms.insert(direct_mug(n, &space), n);
    }
    for x in 1u64.. {
        let mug = unsafe { calc_cell_mug_u32(direct_mug(x, &space), zero, &space) };
        if let Some(atom) = atoms.get(&mug) {
            return (*atom, x);
        }
    }
    unreachable!()
}

/// A proper list of `len` direct atoms, freshly allocated.
fn fresh_list(slab: &mut NounSlab, len: u64) -> Noun {
    let items: Vec<Noun> = (0..len).map(D).collect();
    nn::vec_to_list(slab, items)
}

fn cell(slab: &mut NounSlab, items: &[Noun]) -> Noun {
    T(slab, items)
}

fn assert_noun(slab: &NounSlab, actual: Noun, expected: Noun, what: &str) {
    assert!(
        nn::noun_eq(actual, expected, &slab.noun_space()).expect("noun_eq"),
        "{what}: nouns differ"
    );
}

fn parse_src(src: &str) -> Hoon {
    crate::pipeline::parse_native_hoon_source_without_docs(
        Path::new("c4-cov.hoon"),
        src,
        Vec::new(),
        false,
    )
    .expect("parse c4 coverage source")
}

/// Mints `src` against a `%noun` subject without the prelude, returning the
/// slab with the type and formula nouns.
fn mint_src(src: &str) -> Result<(NounSlab, Noun, Noun)> {
    let gen = parse_src(src);
    let mut slab: NounSlab = NounSlab::new();
    let (ty, formula) = {
        let mut ut = Ut::new(&mut slab);
        let sut = ty_noun(&mut *ut.slab);
        let gol = ty_noun(&mut *ut.slab);
        ut.mint_noun(sut, gol, &gen)?
    };
    Ok((slab, ty, formula))
}

fn tag_of(slab: &NounSlab, noun: Noun) -> String {
    type_tag(noun, &slab.noun_space()).expect("type tag")
}

// ---------------------------------------------------------------------------
// noun.rs
// ---------------------------------------------------------------------------

#[test]
fn c4_noun_opt_from_noun_decodes_and_rejects() {
    let mut slab: NounSlab = NounSlab::new();
    let some = cell(&mut slab, &[D(0), D(42)]);
    let bad_head = cell(&mut slab, &[D(1), D(42)]);
    let space = slab.noun_space();
    assert!(nn::opt_from_noun(D(0), &space).expect("null").is_none());
    let value = nn::opt_from_noun(some, &space)
        .expect("some")
        .expect("value");
    assert!(nn::noun_eq_direct(value, 42, &space));
    let err = nn::opt_from_noun(D(7), &space).expect_err("nonzero atom is not a unit");
    assert!(format!("{err:?}").contains("unexpected opt atom value: 7"));
    let err = nn::opt_from_noun(bad_head, &space).expect_err("unit head must be 0");
    assert!(format!("{err:?}").contains("unexpected opt head value: 1"));
}

#[test]
fn c4_noun_parsed_atoms_round_trip() {
    let mut slab: NounSlab = NounSlab::new();
    let small = nn::parsed_atom_to_noun(&mut slab, &ParsedAtom::Small(7));
    let wide = nn::parsed_atom_to_noun(&mut slab, &ParsedAtom::Small(1u128 << 100));
    let big = nn::parsed_atom_to_noun(&mut slab, &ParsedAtom::Big(BigUint::from(0u32)));
    let big_wide =
        nn::parsed_atom_to_noun(&mut slab, &ParsedAtom::Big(BigUint::from(1u32) << 130u32));
    let space = slab.noun_space();
    assert!(nn::noun_eq_direct(small, 7, &space));
    assert!(nn::noun_eq_direct(big, 0, &space));
    let wide_atom = wide.in_space(&space).as_atom().expect("atom");
    assert_eq!(
        wide_atom.as_ne_bytes().iter().rposition(|b| *b != 0),
        Some(12)
    );
    let big_atom = big_wide.in_space(&space).as_atom().expect("atom");
    assert_eq!(
        big_atom.as_ne_bytes().iter().rposition(|b| *b != 0),
        Some(16)
    );
}

#[test]
fn c4_noun_list_to_vec_handles_proper_and_improper_lists() {
    let mut slab: NounSlab = NounSlab::new();
    let list = fresh_list(&mut slab, 3);
    let improper = cell(&mut slab, &[D(1), D(2), D(9)]);
    let space = slab.noun_space();
    let items = nn::list_to_vec(list, &space).expect("proper list");
    assert_eq!(items.len(), 3);
    assert!(nn::noun_eq_direct(items[2], 2, &space));
    assert!(nn::list_to_vec(D(0), &space).expect("null list").is_empty());
    let err = nn::list_to_vec(improper, &space).expect_err("improper list");
    assert!(format!("{err:?}").contains("improper list terminator: 9"));
}

#[test]
fn c4_noun_atom_to_string_wide_and_invalid() {
    let mut slab: NounSlab = NounSlab::new();
    let wide = Atom::from_bytes(&mut slab, b"a-long-term-name").as_noun();
    let invalid = Atom::from_bytes(&mut slab, &[0xff; 12]).as_noun();
    let space = slab.noun_space();
    assert_eq!(
        nn::atom_to_string(D(0).in_space(&space).as_atom().unwrap()).unwrap(),
        "$"
    );
    assert_eq!(
        nn::atom_to_string(wide.in_space(&space).as_atom().unwrap()).unwrap(),
        "a-long-term-name"
    );
    let err =
        nn::atom_to_string(invalid.in_space(&space).as_atom().unwrap()).expect_err("invalid utf-8");
    let msg = format!("{err:?}");
    assert!(msg.contains("atom decode failed at") && msg.contains("bytes=ff ff"));
}

#[test]
fn c4_noun_cell_head_and_tail() {
    let mut slab: NounSlab = NounSlab::new();
    let pair = cell(&mut slab, &[D(3), D(4)]);
    let space = slab.noun_space();
    assert!(nn::noun_eq_direct(
        nn::cell_head(pair, &space).unwrap(),
        3,
        &space
    ));
    assert!(nn::noun_eq_direct(
        nn::cell_tail(pair, &space).unwrap(),
        4,
        &space
    ));
    assert!(nn::cell_head(D(3), &space).is_err());
    assert!(nn::cell_tail(D(3), &space).is_err());
}

#[test]
fn c4_noun_eq_rejects_mug_collisions() {
    let (a1, a2) = colliding_atoms();
    let (h1, h2) = colliding_cell_heads();
    let (atom, head) = colliding_atom_and_cell();
    let mut slab: NounSlab = NounSlab::new();
    let c1 = cell(&mut slab, &[D(h1), D(0)]);
    let c2 = cell(&mut slab, &[D(h2), D(0)]);
    let ac = cell(&mut slab, &[D(head), D(0)]);
    let space = slab.noun_space();
    assert_eq!(direct_mug(a1, &space), direct_mug(a2, &space));
    // Equal mugs, different atoms: the byte comparison rejects.
    assert!(!nn::noun_eq(D(a1), D(a2), &space).unwrap());
    // Equal root mugs, different head mugs: the inner mug check rejects.
    assert_eq!(nn::slab_mug(c1, &space), nn::slab_mug(c2, &space));
    assert!(!nn::noun_eq(c1, c2, &space).unwrap());
    // An atom and a cell with the same mug are different nouns.
    assert_eq!(direct_mug(atom, &space), nn::slab_mug(ac, &space));
    assert!(!nn::noun_eq(D(atom), ac, &space).unwrap());
    assert!(!nn::noun_eq(ac, D(atom), &space).unwrap());
}

#[test]
fn c4_noun_eq_skips_repeated_shared_pairs() {
    // `[s s]` against `[t t]`, with `s` and `t` equal but separately allocated
    // long lists: the second visit reuses the pair set past 64 cell pairs.
    let mut slab: NounSlab = NounSlab::new();
    let s = fresh_list(&mut slab, 100);
    let t = fresh_list(&mut slab, 100);
    let a = cell(&mut slab, &[s, s]);
    let b = cell(&mut slab, &[t, t]);
    let u = fresh_list(&mut slab, 99);
    let c = cell(&mut slab, &[s, u]);
    let space = slab.noun_space();
    assert!(nn::noun_eq(a, b, &space).unwrap());
    assert!(!nn::noun_eq(a, c, &space).unwrap());
}

// ---------------------------------------------------------------------------
// formula.rs: the noun-level ++cons / ++comb / ++cond peepholes
// ---------------------------------------------------------------------------

#[test]
fn c4_formula_cons_folds_constants_only() {
    let mut slab: NounSlab = NounSlab::new();
    let k1 = cell(&mut slab, &[D(1), D(5)]);
    let k2 = cell(&mut slab, &[D(1), D(6)]);
    let axis = cell(&mut slab, &[D(0), D(2)]);
    let folded = nf::cons(&mut slab, k1, k2).unwrap();
    let expected = cell(&mut slab, &[D(1), D(5), D(6)]);
    assert_noun(&slab, folded, expected, "[1 5] [1 6] folds to [1 5 6]");
    let kept = nf::cons(&mut slab, k1, axis).unwrap();
    let expected = cell(&mut slab, &[k1, axis]);
    assert_noun(&slab, kept, expected, "constant/axis pair is kept");
    // An atom is not a formula cell, so it is never a constant.
    let atom_head = nf::cons(&mut slab, D(9), k2).unwrap();
    let expected = cell(&mut slab, &[D(9), k2]);
    assert_noun(&slab, atom_head, expected, "atom head is kept");
}

#[test]
fn c4_formula_comb_axis_rules() {
    let mut slab: NounSlab = NounSlab::new();
    let a2 = cell(&mut slab, &[D(0), D(2)]);
    let a3 = cell(&mut slab, &[D(0), D(3)]);
    let a0 = cell(&mut slab, &[D(0), D(0)]);
    let a1 = cell(&mut slab, &[D(0), D(1)]);

    // Rule 1a: [0 a] then [0 b] is [0 (peg a b)].
    let out = nf::comb(&mut slab, a2, a3).unwrap();
    let expected = cell(&mut slab, &[D(0), D(5)]);
    assert_noun(&slab, out, expected, "comb [0 2] [0 3]");

    // Rule 1b: [0 a] then [2 [0 x] [0 y]] pegs both axes.
    let two = cell(&mut slab, &[D(2), a2, a3]);
    let out = nf::comb(&mut slab, a3, two).unwrap();
    let e6 = cell(&mut slab, &[D(0), D(6)]);
    let e7 = cell(&mut slab, &[D(0), D(7)]);
    let expected = cell(&mut slab, &[D(2), e6, e7]);
    assert_noun(&slab, out, expected, "comb [0 3] [2 [0 2] [0 3]]");

    // Rule 1 fallthroughs all produce [7 mal buz].
    let konst = cell(&mut slab, &[D(1), D(4)]);
    let fallthroughs = [
        a0,                                  // b = 0
        D(5),                                // buz is an atom
        cell(&mut slab, &[D(2), D(5)]),      // [2 atom]
        cell(&mut slab, &[D(2), a2, konst]), // y is not an axis
        cell(&mut slab, &[D(2), a0, a3]),    // x = 0
        cell(&mut slab, &[D(2), a3, a0]),    // y = 0
        cell(&mut slab, &[D(4), a2]),        // opcode is not 2
    ];
    for buz in fallthroughs {
        let out = nf::comb(&mut slab, a2, buz).unwrap();
        let expected = cell(&mut slab, &[D(7), a2, buz]);
        assert_noun(&slab, out, expected, "comb [0 2] fallthrough");
    }

    // mal = [0 0] is not an axis formula; [0 1] as buz returns mal.
    let out = nf::comb(&mut slab, a0, a1).unwrap();
    assert_noun(&slab, out, a0, "comb [0 0] [0 1] is [0 0]");
    let out = nf::comb(&mut slab, a0, konst).unwrap();
    let expected = cell(&mut slab, &[D(7), a0, konst]);
    assert_noun(&slab, out, expected, "comb [0 0] [1 4]");

    // mal = [0 [1 2]] has a cell axis and is not an axis formula.
    let cell_axis = cell(&mut slab, &[D(0), D(1), D(2)]);
    let out = nf::comb(&mut slab, cell_axis, konst).unwrap();
    let expected = cell(&mut slab, &[D(7), cell_axis, konst]);
    assert_noun(&slab, out, expected, "comb [0 [1 2]] [1 4]");

    // Rule 2: [x [0 1]] then buz is [8 x buz].
    let push = cell(&mut slab, &[konst, a1]);
    let out = nf::comb(&mut slab, push, a3).unwrap();
    let expected = cell(&mut slab, &[D(8), konst, a3]);
    assert_noun(&slab, out, expected, "comb [[1 4] [0 1]] [0 3]");

    // Rule 3: an atom mal with buz = [0 1] returns mal unchanged.
    let out = nf::comb(&mut slab, D(9), a1).unwrap();
    assert!(nn::noun_eq_direct(out, 9, &slab.noun_space()));
    let out = nf::comb(&mut slab, D(9), konst).unwrap();
    let expected = cell(&mut slab, &[D(7), D(9), konst]);
    assert_noun(&slab, out, expected, "comb 9 [1 4]");

    // Big axes compose through BigUint.
    let big = Atom::from_bytes(&mut slab, &[0, 0, 0, 0, 0, 0, 0, 0, 1]).as_noun();
    let abig = cell(&mut slab, &[D(0), big]);
    let out = nf::comb(&mut slab, abig, a3).unwrap();
    let space = slab.noun_space();
    let (op, axis) = nn::noun_pair(out, &space).unwrap();
    assert!(nn::noun_eq_direct(op, 0, &space));
    let bytes = axis
        .in_space(&space)
        .as_atom()
        .unwrap()
        .as_ne_bytes()
        .to_vec();
    assert_eq!(bytes[0], 1);
    assert_eq!(bytes[8], 2);
}

#[test]
fn c4_formula_cond_rules() {
    let mut slab: NounSlab = NounSlab::new();
    let yes = cell(&mut slab, &[D(1), D(0)]);
    let no = cell(&mut slab, &[D(1), D(1)]);
    let a0 = cell(&mut slab, &[D(0), D(0)]);
    let a5 = cell(&mut slab, &[D(0), D(5)]);
    let y = cell(&mut slab, &[D(1), D(10)]);
    let n = cell(&mut slab, &[D(1), D(11)]);
    let out = nf::cond(&mut slab, yes, y, n).unwrap();
    assert_noun(&slab, out, y, "cond [1 0]");
    let out = nf::cond(&mut slab, no, y, n).unwrap();
    assert_noun(&slab, out, n, "cond [1 1]");
    let out = nf::cond(&mut slab, a0, y, n).unwrap();
    assert_noun(&slab, out, a0, "cond [0 0] crashes as [0 0]");
    let other_const = cell(&mut slab, &[D(1), D(7)]);
    for pex in [a5, D(3), other_const] {
        let out = nf::cond(&mut slab, pex, y, n).unwrap();
        let expected = cell(&mut slab, &[D(6), pex, y, n]);
        assert_noun(&slab, out, expected, "cond kept");
    }
}

// ---------------------------------------------------------------------------
// types.rs: memo maps, structural sets, interner, hasher
// ---------------------------------------------------------------------------

#[test]
fn c4_types_raw_memo_map_and_set_evict_in_order() {
    let mut map: RawMemoMap<u64, u64> = RawMemoMap::default();
    map.insert_with_limit(1, 10, 2);
    map.insert_with_limit(1, 11, 2); // existing key: value replaced, no new order slot
    assert_eq!(map.order.len(), 1);
    assert_eq!(map.get(&1), Some(11));
    map.insert_with_limit(2, 20, 2);
    map.insert_with_limit(3, 30, 2); // evicts 1
    assert_eq!(map.get(&1), None);
    assert_eq!(map.get(&3), Some(30));
    map.clear();
    assert!(map.values.is_empty() && map.order.is_empty());

    let mut set: RawMemoSet<u64> = RawMemoSet::default();
    set.insert_with_limit(1, 1);
    set.insert_with_limit(1, 1);
    assert!(set.contains(&1));
    set.insert_with_limit(2, 1); // evicts 1
    assert!(!set.contains(&1));
    assert!(set.contains(&2));
    assert_eq!(set.order.len(), 1);
}

#[test]
fn c4_types_bucket_memo_ensure_key_evicts_and_clears() {
    let mut memo: BucketMemo<u64, u64> = BucketMemo::default();
    memo.ensure_key(1, 1).push_back(10);
    memo.ensure_key(1, 1).push_back(11); // existing key
    assert_eq!(memo.get(&1).map(|b| b.len()), Some(2));
    memo.ensure_key(2, 1).push_back(20); // evicts 1
    assert!(memo.get(&1).is_none());
    assert_eq!(memo.get(&2).map(|b| b.len()), Some(1));
    memo.clear();
    assert!(memo.buckets.is_empty() && memo.order.is_empty());
}

#[test]
fn c4_types_fast_hasher_mixes_every_width() {
    let mut a = FastHasher::default();
    a.write_u16(7);
    a.write_i8(-1);
    a.write_i16(-2);
    a.write_i32(-3);
    a.write_i64(-4);
    a.write_isize(-5);
    let mut b = FastHasher::default();
    for v in [7u64, -1i8 as u64, -2i16 as u64, -3i32 as u64, -4i64 as u64, -5isize as u64] {
        b.mix_u64(v);
    }
    assert_eq!(a.finish(), b.finish());
    assert_ne!(a.finish(), FastHasher::default().finish());
}

#[test]
fn c4_types_struct_noun_set_structural_and_collisions() {
    let (a1, a2) = colliding_atoms();
    let mut slab: NounSlab = NounSlab::new();
    let x = cell(&mut slab, &[D(1), D(2)]);
    let x_copy = cell(&mut slab, &[D(1), D(2)]);
    let mut ut = Ut::new(&mut slab);
    let mut set = StructNounSet::new();
    assert!(!set.contains(&mut ut, x).unwrap());
    assert!(set.insert(&mut ut, x).unwrap());
    assert!(!set.insert(&mut ut, x).unwrap(), "raw duplicate");
    assert!(
        !set.insert(&mut ut, x_copy).unwrap(),
        "structural duplicate"
    );
    assert!(set.contains(&mut ut, x).unwrap());
    assert!(set.contains(&mut ut, x_copy).unwrap());

    assert!(set.insert(&mut ut, D(a1)).unwrap());
    assert!(
        !set.contains(&mut ut, D(a2)).unwrap(),
        "mug collision is not membership"
    );
    assert!(!set.remove(&mut ut, D(a2)).unwrap(), "colliding non-member");
    assert!(
        set.insert(&mut ut, D(a2)).unwrap(),
        "collision gets its own slot"
    );
    assert!(set.contains(&mut ut, D(a2)).unwrap());
    assert!(set.remove(&mut ut, D(a2)).unwrap(), "bucket keeps a1");
    assert!(set.contains(&mut ut, D(a1)).unwrap());
    assert!(set.remove(&mut ut, D(a1)).unwrap(), "bucket emptied");
    assert!(set.remove(&mut ut, x_copy).unwrap(), "structural removal");
    assert!(!set.remove(&mut ut, x).unwrap(), "missing mug bucket");
    assert!(set.buckets.is_empty());
}

#[test]
fn c4_types_struct_noun_pair_set_signature_and_membership() {
    let (a1, a2) = colliding_atoms();
    let mut slab: NounSlab = NounSlab::new();
    let x = cell(&mut slab, &[D(1), D(2)]);
    let x_copy = cell(&mut slab, &[D(1), D(2)]);
    let mut ut = Ut::new(&mut slab);
    let mut set = StructNounPairSet::new();
    assert!(set.is_empty());
    assert_eq!(set.cache_signature(), 0);
    assert!(set.insert(&mut ut, x, D(0)).unwrap());
    assert_ne!(set.cache_signature(), 0);
    assert!(!set.insert(&mut ut, x, D(0)).unwrap(), "raw duplicate pair");
    assert!(
        !set.insert(&mut ut, x_copy, D(0)).unwrap(),
        "structural duplicate pair"
    );
    // Same key, sut differs (collision) and ref differs (collision).
    assert!(set.insert(&mut ut, D(a1), D(0)).unwrap());
    assert!(set.insert(&mut ut, D(a2), D(0)).unwrap());
    assert!(set.insert(&mut ut, D(0), D(a1)).unwrap());
    assert!(set.insert(&mut ut, D(0), D(a2)).unwrap());
    assert_eq!(set.pair_count, 5);

    // Raw removal after skipping a same-key entry whose ref differs.
    assert!(set.remove(&mut ut, D(0), D(a2)).unwrap());
    // Raw removal after skipping a same-key entry whose sut differs.
    assert!(set.remove(&mut ut, D(a2), D(0)).unwrap());
    // Structural removal (raw pair differs).
    assert!(set.remove(&mut ut, x_copy, D(0)).unwrap());
    // Same key, not a member.
    assert!(!set.remove(&mut ut, D(a2), D(0)).unwrap());
    // Missing key.
    assert!(!set.remove(&mut ut, D(5), D(6)).unwrap());
    assert_eq!(set.pair_count, 2);
}

#[test]
fn c4_types_struct_noun_pair_set_structural_removal_scans_bucket() {
    let (a1, a2) = colliding_atoms();
    let mut slab: NounSlab = NounSlab::new();
    let x = cell(&mut slab, &[D(1), D(2)]);
    let x_copy = cell(&mut slab, &[D(1), D(2)]);
    let y = cell(&mut slab, &[D(a1), D(0)]);
    let y_copy = cell(&mut slab, &[D(a1), D(0)]);
    let mut ut = Ut::new(&mut slab);
    let mut set = StructNounPairSet::new();
    // Bucket (mug a, mug x): (a1, x) then (a2, x).
    assert!(set.insert(&mut ut, D(a1), x).unwrap());
    assert!(set.insert(&mut ut, D(a2), x).unwrap());
    // Structural removal of (a2, x_copy) skips (a1, x) on the sut test.
    assert!(set.remove(&mut ut, D(a2), x_copy).unwrap());
    // Bucket (mug y, 0): (y, a1) and (y, a2); removing (y_copy, a2) skips (y, a1)
    // on the ref test.
    assert!(set.insert(&mut ut, y, D(a1)).unwrap());
    assert!(set.insert(&mut ut, y, D(a2)).unwrap());
    assert!(set.remove(&mut ut, y_copy, D(a2)).unwrap());
    assert!(!set.remove(&mut ut, y_copy, D(a2)).unwrap());
    assert_eq!(set.pair_count, 2);
}

#[test]
fn c4_types_struct_noun_pair_set_snapshot_matching() {
    let (a1, a2) = colliding_atoms();
    let mut slab: NounSlab = NounSlab::new();
    let space = slab.noun_space();
    let mut ut = Ut::new(&mut slab);

    let mut base = StructNounPairSet::new();
    base.insert(&mut ut, D(a1), D(0)).unwrap();
    base.insert(&mut ut, D(a2), D(0)).unwrap();
    let snap = base.snapshot();
    assert!(base.matches_snapshot(&snap, &space).unwrap());

    // Reordered snapshot pairs: the scan skips a mismatched sut and a matched slot.
    let mut reordered = snap.clone();
    reordered.buckets[0].pairs.reverse();
    assert!(base.matches_snapshot(&reordered, &space).unwrap());

    // Bucket count differs.
    let mut more = StructNounPairSet::new();
    more.insert(&mut ut, D(a1), D(0)).unwrap();
    more.insert(&mut ut, D(a2), D(0)).unwrap();
    more.insert(&mut ut, D(7), D(8)).unwrap();
    assert!(!more.matches_snapshot(&snap, &space).unwrap());

    // Same buckets, different pair count.
    let mut fewer = StructNounPairSet::new();
    fewer.insert(&mut ut, D(a1), D(0)).unwrap();
    assert!(!fewer.matches_snapshot(&snap, &space).unwrap());

    // Same counts, different key.
    let mut other_key = StructNounPairSet::new();
    other_key.insert(&mut ut, D(7), D(8)).unwrap();
    other_key.insert(&mut ut, D(7), D(9)).unwrap();
    let mut two_keys = StructNounPairSet::new();
    two_keys.insert(&mut ut, D(a1), D(0)).unwrap();
    two_keys.insert(&mut ut, D(0), D(a1)).unwrap();
    let two_snap = two_keys.snapshot();
    assert!(!other_key.matches_snapshot(&two_snap, &space).unwrap());

    // Same keys and total, different per-bucket lengths.
    let mut left = StructNounPairSet::new();
    left.insert(&mut ut, D(a1), D(0)).unwrap();
    left.insert(&mut ut, D(a2), D(0)).unwrap();
    left.insert(&mut ut, D(0), D(a1)).unwrap();
    let mut right = StructNounPairSet::new();
    right.insert(&mut ut, D(a1), D(0)).unwrap();
    right.insert(&mut ut, D(0), D(a1)).unwrap();
    right.insert(&mut ut, D(0), D(a2)).unwrap();
    assert!(!left.matches_snapshot(&right.snapshot(), &space).unwrap());

    // Same bucket shape, a ref differs.
    let mut refs_a = StructNounPairSet::new();
    refs_a.insert(&mut ut, D(0), D(a1)).unwrap();
    let mut refs_b = StructNounPairSet::new();
    refs_b.insert(&mut ut, D(0), D(a2)).unwrap();
    assert!(!refs_a.matches_snapshot(&refs_b.snapshot(), &space).unwrap());
}

#[test]
fn c4_types_nest_interner_and_id_sets() {
    let (a1, a2) = colliding_atoms();
    let mut slab: NounSlab = NounSlab::new();
    let x = cell(&mut slab, &[D(1), D(2)]);
    let x_copy = cell(&mut slab, &[D(1), D(2)]);
    let mut ut = Ut::new(&mut slab);
    let mut interner = NestTypeInterner::new();
    let ix = interner.id_for(&mut ut, x).unwrap();
    assert_eq!(interner.id_for(&mut ut, x).unwrap(), ix, "raw hit");
    assert_eq!(
        interner.id_for(&mut ut, x_copy).unwrap(),
        ix,
        "structural hit"
    );
    let i1 = interner.id_for(&mut ut, D(a1)).unwrap();
    let i2 = interner.id_for(&mut ut, D(a2)).unwrap();
    assert_ne!(i1, i2, "mug collision gets a fresh id");

    let mut seen: NestSeenSet<NestNounId> = NestSeenSet::new();
    assert!(seen.insert(&mut ut, &mut interner, x).unwrap());
    assert!(!seen.insert(&mut ut, &mut interner, x_copy).unwrap());
    assert!(seen.contains(&mut ut, &mut interner, x_copy).unwrap());
    assert!(seen.contains_id(ix));
    assert!(seen.remove(&mut ut, &mut interner, x).unwrap());
    assert!(!seen.remove(&mut ut, &mut interner, x).unwrap());
    assert!(!seen.remove_id(ix));

    let mut pairs: NestPairSet<NestNounId> = NestPairSet::new();
    assert!(!pairs.contains(&mut ut, &mut interner, x, D(a1)).unwrap());
    assert!(pairs.insert(&mut ut, &mut interner, x, D(a1)).unwrap());
    assert!(!pairs.insert(&mut ut, &mut interner, x_copy, D(a1)).unwrap());
    assert!(pairs
        .contains(&mut ut, &mut interner, x_copy, D(a1))
        .unwrap());
    assert!(pairs.contains_id(ix, i1));
    assert!(!pairs.contains_id(ix, i2));
    assert!(pairs.remove(&mut ut, &mut interner, x, D(a1)).unwrap());
    assert!(!pairs.remove(&mut ut, &mut interner, x, D(a1)).unwrap());
    assert!(!pairs.remove_id(ix, i1));
    assert!(pairs.insert_id(ix, i2));
    assert!(pairs.remove_id(ix, i2));
}

// ---------------------------------------------------------------------------
// ut helpers for find/fire/repo/wet
// ---------------------------------------------------------------------------

/// Readable rendering of a noun: printable atoms as `%term`, others as numbers.
fn dump(slab: &NounSlab, noun: Noun) -> String {
    fn go(space: &NounSpace, n: Noun, out: &mut String) {
        if let Ok(c) = n.in_space(space).as_cell() {
            out.push('[');
            go(space, c.head().noun(), out);
            out.push(' ');
            go(space, c.tail().noun(), out);
            out.push(']');
            return;
        }
        let atom = n.in_space(space).as_atom().expect("atom");
        match atom.as_u64() {
            Ok(0) => out.push('0'),
            _ => match atom_to_string(atom) {
                Ok(s) if s.chars().all(|c| c.is_ascii_lowercase() || c == '-') => {
                    out.push('%');
                    out.push_str(&s);
                }
                _ => match atom.as_u64() {
                    Ok(v) => out.push_str(&v.to_string()),
                    Err(_) => out.push_str("BIG"),
                },
            },
        }
    }
    let space = slab.noun_space();
    let mut out = String::new();
    go(&space, noun, &mut out);
    out
}

fn nat(ut: &mut Ut<'_>, noun: Noun) -> NRc<NTy> {
    let space = ut.slab.noun_space();
    native_of(&mut ut.cx, noun, &space).expect("native_of")
}

fn low(ut: &mut Ut<'_>, ty: &NRc<NTy>) -> Noun {
    live_to_noun(&mut ut.cx, ty, ut.slab)
}

fn term(slab: &mut NounSlab, s: &str) -> Noun {
    term_to_noun(slab, s)
}

fn wing(names: &[&str]) -> WingType {
    names.iter().map(|n| Limb::Term((*n).to_string())).collect()
}

fn garb_noun(slab: &mut NounSlab, poly: &str, vair: &str) -> Noun {
    let name = term(slab, "test");
    let poly = term(slab, poly);
    let vair = term(slab, vair);
    T(slab, &[name, poly, vair])
}

/// A core type with one chapter holding `arms` (name, arm hoon).
fn core_with_arms(
    slab: &mut NounSlab,
    payload: Noun,
    context: Noun,
    poly: &str,
    arms: &[(&str, Hoon)],
) -> Noun {
    let garb = garb_noun(slab, poly, "gold");
    let mut pairs = Vec::new();
    for (name, hoon) in arms {
        let key = term(slab, name);
        let val = hoon_to_noun(slab, hoon);
        pairs.push((key, val));
    }
    let arms = map_to_noun(slab, pairs).expect("arms map");
    let tome = T(slab, &[D(0), arms]);
    let tome_key = term(slab, "core");
    let tomes = map_to_noun(slab, vec![(tome_key, tome)]).expect("tomes");
    let rest = T(slab, &[D(0), tomes]);
    let coil = coil_from_parts(slab, garb, context, rest);
    ty_core(slab, payload, coil)
}

/// A face whose tool is the tune `[aliases bridges]`.
fn tune_face(slab: &mut NounSlab, aliases: Vec<(&str, Noun)>, bridges: Noun, inner: Noun) -> Noun {
    let mut pairs = Vec::new();
    for (name, value) in aliases {
        let key = term(slab, name);
        pairs.push((key, value));
    }
    let map = map_to_noun(slab, pairs).expect("alias map");
    let tool = T(slab, &[map, bridges]);
    ty_face_tool(slab, tool, inner)
}

fn some_hoon(slab: &mut NounSlab, hoon: &Hoon) -> Noun {
    let noun = hoon_to_noun(slab, hoon);
    T(slab, &[D(0), noun])
}

fn wing_hoon(names: &[&str]) -> Hoon {
    Hoon::Wing(wing(names))
}

fn leg_of(port: Port) -> (Vec<Option<BigUint>>, NRc<NTy>) {
    match port {
        Port::Palo(Palo {
            vein,
            opal: Opal::Leg(ty),
        }) => (vein, ty),
        other => panic!("expected a leg port, got {other:?}"),
    }
}

fn err_text<T: std::fmt::Debug>(result: Result<T>) -> String {
    format!("{:?}", result.expect_err("expected an error"))
}

// ---------------------------------------------------------------------------
// find.rs: ++find / ++fond / ++fend / ++fund / ++twin, faces and tunes
// ---------------------------------------------------------------------------

#[test]
fn c4_find_face_named_buc_and_undecodable_face_names() {
    let mut slab: NounSlab = NounSlab::new();
    let ud = ty_atom(&mut slab, "ud", None);
    let buc = ty_face_tool(&mut slab, D(0), ud);
    let bad_name = Atom::from_bytes(&mut slab, &[0xff]).as_noun();
    let bad = ty_face_tool(&mut slab, bad_name, ud);
    let head_bad = ty_cell(&mut slab, bad, ud);
    let tail_bad = ty_cell(&mut slab, ud, bad);
    let mut ut = Ut::new(&mut slab);

    let (vein, ty) = leg_of(ut.find_noun(buc, Way::Read, &wing(&["$"])).unwrap());
    assert_eq!(tend_big(&vein).unwrap(), BigUint::from(1u32));
    assert!(matches!(&*ty, NTy::Atom { .. }));

    for sut in [bad, head_bad, tail_bad] {
        let msg = err_text(ut.find_noun(sut, Way::Read, &wing(&["x"])));
        assert!(msg.contains("face name in find"), "{msg}");
    }
}

#[test]
fn c4_find_tagged_tune_tools() {
    let mut slab: NounSlab = NounSlab::new();
    let ud = ty_atom(&mut slab, "ud", None);
    let inner = ty_face(&mut slab, "x", ud);
    let tune_tag = term(&mut slab, "tune");
    let empty_tune = T(&mut slab, &[D(0), D(0)]);
    let tagged = T(&mut slab, &[tune_tag, empty_tune]);
    let tagged_face = ty_face_tool(&mut slab, tagged, inner);
    let tagged_bad = T(&mut slab, &[tune_tag, D(5)]);
    let tagged_bad_face = ty_face_tool(&mut slab, tagged_bad, inner);
    let odd_head = Atom::from_bytes(&mut slab, &[0xff]).as_noun();
    let odd_tool = T(&mut slab, &[odd_head, D(0)]);
    let odd_face = ty_face_tool(&mut slab, odd_tool, inner);
    let mut ut = Ut::new(&mut slab);

    // `[%tune aliases bridges]` decodes like a bare tune: nothing aliased, no
    // bridges, so the search continues into the inner type.
    let (vein, ty) = leg_of(ut.find_noun(tagged_face, Way::Read, &wing(&["x"])).unwrap());
    assert_eq!(tend_big(&vein).unwrap(), BigUint::from(1u32));
    assert!(matches!(&*ty, NTy::Atom { .. }));

    let msg = err_text(ut.find_noun(tagged_bad_face, Way::Read, &wing(&["x"])));
    assert!(msg.contains("face tune payload not cell"), "{msg}");

    // A non-text head is read as the alias map itself, which is malformed.
    let msg = err_text(ut.find_noun(odd_face, Way::Read, &wing(&["x"])));
    assert!(msg.contains("map node not cell"), "{msg}");
}

#[test]
fn c4_find_tune_alias_without_hoon_hides_one_match() {
    // An alias `w` whose value is `~` bumps the skip count, so the first inner
    // `w` is passed over and the second one is found.
    let mut slab: NounSlab = NounSlab::new();
    let ud = ty_atom(&mut slab, "ud", None);
    let ux = ty_atom(&mut slab, "ux", None);
    let w_ud = ty_face(&mut slab, "w", ud);
    let w_ux = ty_face(&mut slab, "w", ux);
    let inner = ty_cell(&mut slab, w_ud, w_ux);
    let sut = tune_face(&mut slab, vec![("w", D(0))], D(0), inner);
    let mut ut = Ut::new(&mut slab);
    let (vein, ty) = leg_of(ut.find_noun(sut, Way::Read, &wing(&["w"])).unwrap());
    assert_eq!(tend_big(&vein).unwrap(), BigUint::from(3u32));
    let ty_noun = low(&mut ut, &ty);
    assert_eq!(dump(ut.slab, ty_noun), "[%atom [%ux 0]]");
}

#[test]
fn c4_find_tune_malformed_alias_values_and_bridges() {
    let mut slab: NounSlab = NounSlab::new();
    let ud = ty_atom(&mut slab, "ud", None);
    let inner = ty_face(&mut slab, "x", ud);
    let pair_tag = T(&mut slab, &[D(1), D(2)]);
    let big = Atom::from_bytes(&mut slab, &[1; 10]).as_noun();
    let v_cell_tag = T(&mut slab, &[pair_tag, D(0)]);
    let v_big_tag = T(&mut slab, &[big, D(0)]);
    let v_one_tag = T(&mut slab, &[D(1), D(0)]);
    let v_bad_hoon = T(&mut slab, &[D(0), D(12345)]);
    let cases = [
        (D(5), "face tune unit not cell"),
        (v_cell_tag, "face tune unit tag not atom"),
        (v_big_tag, "face tune unit tag decode"),
        (v_one_tag, "face tune unit tag mismatch"),
        (v_bad_hoon, "face tune bridge ast missing"),
    ];
    let mut suts = Vec::new();
    for (value, want) in cases {
        suts.push((tune_face(&mut slab, vec![("w", value)], D(0), inner), want));
    }
    let bad_list = tune_face(&mut slab, vec![], D(5), inner);
    suts.push((bad_list, "face tune bridge list not cell"));
    let bad_bridge_list = T(&mut slab, &[D(12345), D(0)]);
    let bad_bridge = tune_face(&mut slab, vec![], bad_bridge_list, inner);
    suts.push((bad_bridge, "face tune bridge expression ast missing"));
    // A bridge `.` whose type holds an undecodable face name fails inside the
    // bridge search.
    let bad_name = Atom::from_bytes(&mut slab, &[0xff]).as_noun();
    let bad_face = ty_face_tool(&mut slab, bad_name, ud);
    let dot = hoon_to_noun(&mut slab, &Hoon::Wing(vec![Limb::Axis(1u64.into())]));
    let dot_list = T(&mut slab, &[dot, D(0)]);
    let bad_inside = tune_face(&mut slab, vec![], dot_list, bad_face);
    suts.push((bad_inside, "face name in find"));
    let mut ut = Ut::new(&mut slab);
    for (sut, want) in suts {
        let msg = err_text(ut.find_noun(sut, Way::Read, &wing(&["w"])));
        assert!(msg.contains(want), "want {want}: {msg}");
    }
}

#[test]
fn c4_find_synthetic_alias_ports() {
    // `w` aliases the non-wing hoon `[y y]`, so find returns a synthetic port.
    let mut slab: NounSlab = NounSlab::new();
    let ud = ty_atom(&mut slab, "ud", None);
    let y = ty_face(&mut slab, "y", ud);
    let pair = Hoon::Pair(Box::new(wing_hoon(&["y"])), Box::new(wing_hoon(&["y"])));
    let alias = some_hoon(&mut slab, &pair);
    let sut = tune_face(&mut slab, vec![("w", alias)], D(0), y);
    let mut ut = Ut::new(&mut slab);

    let port = ut.find_noun(sut, Way::Read, &wing(&["w"])).unwrap();
    let (typ, formula) = match &port {
        Port::Synthetic { typ, formula } => (typ.clone(), *formula),
        other => panic!("expected synthetic, got {other:?}"),
    };
    let ty_noun = low(&mut ut, &typ);
    assert_eq!(
        dump(ut.slab, ty_noun),
        "[%cell [[%atom [%ud 0]] [%atom [%ud 0]]]]"
    );
    // `[y y]` is `[[0 1] 0 1]` inside the face; the face's axis-1 slot composes
    // with ++comb's plain `%7` (it only folds axis pairs other than 0).
    let f = ut.formula_materialize(formula);
    assert_eq!(dump(ut.slab, f), "[7 [[0 1] [[0 1] [0 1]]]]");

    // `fine` passes a synthetic port through unchanged.
    let (fine_ty, fine_formula) = ut.fine(&typ, &port).unwrap();
    assert!(NRc::ptr_eq(&fine_ty, &typ));
    assert_eq!(fine_formula, formula);

    // A limb after a synthetic port mints against the synthetic type.
    let sut_n = nat(&mut ut, sut);
    let head_wing = vec![Limb::Axis(2u64.into()), Limb::Term("w".to_string())];
    match ut.fond(sut_n.clone(), Way::Read, &head_wing).unwrap() {
        Pony::Synthetic { typ, .. } => {
            let ty_noun = low(&mut ut, &typ);
            assert_eq!(dump(ut.slab, ty_noun), "[%atom [%ud 0]]");
        }
        other => panic!("expected synthetic, got {other:?}"),
    }

    // fend refuses a synthetic port.
    let msg = err_text(ut.fend(sut_n.clone(), Way::Read, &wing(&["w"])));
    assert!(msg.contains("fend-fragment"), "{msg}");

    // fund takes the wing path for a wing and the mint path otherwise.
    assert!(matches!(
        ut.fund(sut_n.clone(), Way::Read, &wing_hoon(&["w"]))
            .unwrap(),
        Port::Synthetic { .. }
    ));
    let konst = Hoon::Sand("ud".to_string(), NounExpr::ParsedAtom(ParsedAtom::Small(4)));
    match ut.fund(sut_n, Way::Read, &konst).unwrap() {
        Port::Synthetic { formula, .. } => {
            let f = ut.formula_materialize(formula);
            assert_eq!(dump(ut.slab, f), "[1 4]");
        }
        other => panic!("expected synthetic, got {other:?}"),
    }
}

#[test]
fn c4_fond_void_and_unmatched_tails() {
    let mut slab: NounSlab = NounSlab::new();
    let void = ty_void(&mut slab);
    let payload = ty_noun(&mut slab);
    let context = ty_noun(&mut slab);
    let core = core_with_arms(
        &mut slab,
        payload,
        context,
        "dry",
        &[("b", Hoon::Axis(1u64.into()))],
    );
    let pair = ty_cell(&mut slab, payload, payload);
    let mut ut = Ut::new(&mut slab);
    let void_n = nat(&mut ut, void);
    // `b` is a void search, and hoon-138 `++fond` crashes on a void tail.
    assert!(matches!(
        ut.fond(void_n.clone(), Way::Read, &wing(&["b"])).unwrap(),
        Pony::Void
    ));
    let msg = err_text(ut.fond(void_n, Way::Read, &wing(&["a", "b"])));
    assert!(msg.contains("void wing tail"), "{msg}");
    // `p.b` looks for `p` in the core that holds arm `b`; it is not there.
    let core_n = nat(&mut ut, core);
    assert!(matches!(
        ut.fond(core_n.clone(), Way::Read, &wing(&["p", "b"]))
            .unwrap(),
        Pony::Unmatched(0)
    ));
    // `b.b` resolves the arm again from its own core.
    assert!(matches!(
        ut.fond(core_n.clone(), Way::Read, &wing(&["b", "b"]))
            .unwrap(),
        Pony::Palo(Palo {
            opal: Opal::Arm { .. },
            ..
        })
    ));
    // A nameless `^` limb (skip 1) is unmatched at the first stop.
    let pair_n = nat(&mut ut, pair);
    let skip = vec![Limb::Parent(1, None)];
    assert!(matches!(
        ut.fond(pair_n.clone(), Way::Read, &skip).unwrap(),
        Pony::Unmatched(0)
    ));
    assert!(ut.find(pair_n, Way::Read, &skip).is_err());
    // A nameless limb on a core stops at the core itself.
    let here = vec![Limb::Parent(0, None)];
    let (vein, ty) = leg_of(ut.find(core_n, Way::Read, &here).unwrap());
    assert_eq!(tend_big(&vein).unwrap(), BigUint::from(1u32));
    assert!(matches!(&*ty, NTy::Core { .. }));
}

#[test]
fn c4_fond_void_tail_crashes_like_hoon_138() {
    // `=+  loop` puts a hold that expands to itself at the subject head, so the
    // search for `zz` is void. With a one-limb wing `!@` answers no; with
    // `x.zz`, hoon-138 `++fond` crashes on the void tail.
    let msg = mint_err(include_str!(
        "../../../../test-assets/type-probes/reject/c4_fond_void_tail.hoon"
    ));
    assert!(msg.contains("void wing tail"), "{msg}");
    mint_dump(include_str!(
        "../../../../test-assets/type-probes/coverage/regressions/c4_feel_void_search.hoon"
    ));
}

#[test]
fn c4_find_empty_fork_is_void() {
    let mut slab: NounSlab = NounSlab::new();
    let fork_tag = term(&mut slab, "fork");
    let empty = T(&mut slab, &[fork_tag, D(0)]);
    let mut ut = Ut::new(&mut slab);
    let space = ut.slab.noun_space();
    // An empty `%fork` is not a canonical type; if it decodes, the walk treats
    // it as void.
    if let Ok(fork_n) = native_of(&mut ut.cx, empty, &space) {
        let result = ut.fond(fork_n, Way::Read, &wing(&["a"]));
        assert!(matches!(result, Ok(Pony::Void)) || result.is_err());
    }
}

#[test]
fn c4_find_resolve_wing_axis_and_look() {
    let mut slab: NounSlab = NounSlab::new();
    let ud = ty_atom(&mut slab, "ud", None);
    let a = ty_face(&mut slab, "a", ud);
    let b = ty_face(&mut slab, "b", ud);
    let sut = ty_cell(&mut slab, a, b);
    let mut ut = Ut::new(&mut slab);
    assert_eq!(
        ut.resolve_wing_axis_noun(sut, &Vec::new()).unwrap(),
        BigUint::from(1u32)
    );
    assert_eq!(
        ut.resolve_wing_axis_noun(sut, &wing(&["b"])).unwrap(),
        BigUint::from(3u32)
    );
    let cog = term(ut.slab, "a");
    assert!(ut.look(cog, D(0)).unwrap().is_none());
}

#[test]
fn c4_find_noun_seen_insert_structural() {
    let (a1, a2) = colliding_atoms();
    let mut slab: NounSlab = NounSlab::new();
    let x = cell(&mut slab, &[D(1), D(2)]);
    let x_copy = cell(&mut slab, &[D(1), D(2)]);
    let mut ut = Ut::new(&mut slab);
    let mut seen: HashMap<NounMug, Vec<Noun>> = HashMap::new();
    assert!(ut.noun_seen_insert_structural(&mut seen, x).unwrap());
    assert!(!ut.noun_seen_insert_structural(&mut seen, x_copy).unwrap());
    assert!(ut.noun_seen_insert_structural(&mut seen, D(a1)).unwrap());
    assert!(ut.noun_seen_insert_structural(&mut seen, D(a2)).unwrap());
    assert!(!ut.noun_seen_insert_structural(&mut seen, D(a2)).unwrap());
    assert_eq!(seen.values().map(Vec::len).sum::<usize>(), 3);
}

#[test]
fn c4_find_twin_merges_and_rejects() {
    let mut slab: NounSlab = NounSlab::new();
    let ud = ty_atom(&mut slab, "ud", None);
    let ux = ty_atom(&mut slab, "ux", None);
    let payload = ty_noun(&mut slab);
    let context = ty_noun(&mut slab);
    let core1 = core_with_arms(
        &mut slab,
        payload,
        context,
        "dry",
        &[("a", Hoon::Axis(1u64.into()))],
    );
    let core2 = core_with_arms(
        &mut slab,
        ud,
        context,
        "dry",
        &[("a", Hoon::Axis(1u64.into()))],
    );
    let arm_hoon = hoon_to_noun(&mut slab, &Hoon::Axis(1u64.into()));
    let arm_hoon_copy = hoon_to_noun(&mut slab, &Hoon::Axis(1u64.into()));
    let other_hoon = hoon_to_noun(&mut slab, &Hoon::Axis(2u64.into()));
    let foot = foot_from_poly(&mut slab, Poly::Dry, arm_hoon);
    let foot_copy = foot_from_poly(&mut slab, Poly::Dry, arm_hoon_copy);
    let foot_other = foot_from_poly(&mut slab, Poly::Dry, other_hoon);
    let mut ut = Ut::new(&mut slab);
    let ud_n = nat(&mut ut, ud);
    let ux_n = nat(&mut ut, ux);
    let c1 = nat(&mut ut, core1);
    let c2 = nat(&mut ut, core2);
    let leg = |ty: &NRc<NTy>, axis: u64| {
        Pony::Palo(Palo {
            vein: vec![Some(BigUint::from(axis))],
            opal: Opal::Leg(ty.clone()),
        })
    };
    let arm = |axis: u64, arms: Vec<(NRc<NTy>, Noun)>| {
        Pony::Palo(Palo {
            vein: vec![Some(BigUint::from(1u32))],
            opal: Opal::Arm {
                axis: BigUint::from(axis),
                arms,
            },
        })
    };

    // Void on either side yields the other.
    assert!(matches!(
        ut.twin(Pony::Void, leg(&ud_n, 2)).unwrap(),
        Pony::Palo(_)
    ));
    assert!(matches!(
        ut.twin(Pony::Unmatched(1), Pony::Void).unwrap(),
        Pony::Unmatched(1)
    ));
    // Unmatched counts must agree.
    assert!(matches!(
        ut.twin(Pony::Unmatched(1), Pony::Unmatched(1)).unwrap(),
        Pony::Unmatched(1)
    ));
    assert!(err_text(ut.twin(Pony::Unmatched(1), Pony::Unmatched(2))).contains("find-fork"));
    assert!(err_text(ut.twin(Pony::Unmatched(0), leg(&ud_n, 2))).contains("find-fork"));

    // Two synthetic ports with equal formulas fork their types.
    let f2 = ut.formula_slot(BigUint::from(2u32));
    let f3 = ut.formula_slot(BigUint::from(3u32));
    let synth = |typ: &NRc<NTy>, formula| Pony::Synthetic {
        typ: typ.clone(),
        formula,
    };
    match ut.twin(synth(&ud_n, f2), synth(&ux_n, f2)).unwrap() {
        Pony::Synthetic { typ, formula } => {
            assert_eq!(formula, f2);
            let ty_noun = low(&mut ut, &typ);
            assert_eq!(tag_of(ut.slab, ty_noun), "fork");
        }
        other => panic!("expected synthetic, got {other:?}"),
    }
    assert!(err_text(ut.twin(synth(&ud_n, f2), synth(&ux_n, f3))).contains("find-fork"));
    assert!(err_text(ut.twin(synth(&ud_n, f2), leg(&ud_n, 2))).contains("find-fork"));

    // Legs on the same vein fork; different veins do not merge.
    match ut.twin(leg(&ud_n, 2), leg(&ux_n, 2)).unwrap() {
        Pony::Palo(Palo {
            opal: Opal::Leg(typ),
            ..
        }) => {
            let ty_noun = low(&mut ut, &typ);
            assert_eq!(tag_of(ut.slab, ty_noun), "fork");
        }
        other => panic!("expected leg, got {other:?}"),
    }
    assert!(err_text(ut.twin(leg(&ud_n, 2), leg(&ud_n, 3))).contains("find-fork"));

    // Arms: axis mismatch, leg/arm mismatch, and the dedup cases.
    assert!(err_text(ut.twin(
        arm(2, vec![(c1.clone(), foot)]),
        arm(6, vec![(c1.clone(), foot)])
    ))
    .contains("find-fork"));
    let leg_one = Pony::Palo(Palo {
        vein: vec![Some(BigUint::from(1u32))],
        opal: Opal::Leg(ud_n.clone()),
    });
    assert!(err_text(ut.twin(leg_one, arm(2, vec![(c1.clone(), foot)]))).contains("find-fork"));
    let merged_len = |pony: Pony| match pony {
        Pony::Palo(Palo {
            opal: Opal::Arm { arms, .. },
            ..
        }) => arms.len(),
        other => panic!("expected arm, got {other:?}"),
    };
    // Same core, structurally equal foot: deduplicated.
    let out = ut
        .twin(
            arm(2, vec![(c1.clone(), foot)]),
            arm(2, vec![(c1.clone(), foot_copy)]),
        )
        .unwrap();
    assert_eq!(merged_len(out), 1);
    // Same core, different foot: both kept.
    let out = ut
        .twin(
            arm(2, vec![(c1.clone(), foot)]),
            arm(2, vec![(c1.clone(), foot_other)]),
        )
        .unwrap();
    assert_eq!(merged_len(out), 2);
    // Different cores: both kept.
    let out = ut
        .twin(arm(2, vec![(c1.clone(), foot)]), arm(2, vec![(c2, foot)]))
        .unwrap();
    assert_eq!(merged_len(out), 2);
}

/// Mints `src`, expecting success, and returns the dumped type and formula.
fn mint_dump(src: &str) -> (String, String) {
    let (slab, ty, formula) = mint_src(src).unwrap_or_else(|err| panic!("{src}: {err:?}"));
    (dump(&slab, ty), dump(&slab, formula))
}

fn mint_err(src: &str) -> String {
    match mint_src(src) {
        Ok(_) => panic!("{src}: expected a compile error"),
        Err(err) => format!("{err:?}"),
    }
}

#[test]
fn c4_find_source_tune_aliases() {
    // Non-wing alias: synthetic port, then a limb after it.
    let (ty, _) = mint_dump("=/  y  `@ud`1\n=*  w  [y y]\n[w -.w]");
    assert_eq!(
        ty,
        "[%cell [[%cell [[%atom [%ud 0]] [%atom [%ud 0]]]] [%atom [%ud 0]]]]"
    );
    // `^w` skips the alias and finds the outer `w`.
    let (ty, formula) = mint_dump("=/  w  0x5\n=*  w  [1 2]\n^w");
    assert_eq!(ty, "[%atom [%ux 0]]");
    assert_eq!(formula, "[8 [[1 5] [0 2]]]");
    // fend refuses the synthetic alias (hoonc: fend-fragment too).
    assert!(mint_err("=/  y  1\n=*  w  [y y]\n?#(^ w)").contains("fend-fragment"));
    // Without debug spots the two branches' aliases are the same hoon, so the
    // synthetic ports agree on the formula and their types fork.
    let (ty, _) = mint_dump(
        "=/  b  *?\n=/  s  ?:(b =/(y `@ud`1 =*(w [y y] .)) =/(y 0x1 =*(w [y y] .)))\nw.s",
    );
    assert!(ty.starts_with("[%fork"), "{ty}");
    // Aliases with different formulas cannot be merged (hoonc: find-fork too;
    // with debug spots any two source aliases differ).
    let msg =
        mint_err("=/  b  *?\n=/  s  ?:(b =/(y `@ud`1 =*(w [y y] .)) =/(y 0x1 =*(w [y 1] .)))\nw.s");
    assert!(msg.contains("find-fork"), "{msg}");
}

#[test]
fn c4_find_source_tune_bridges() {
    // Bridge resolves the name as a leg.
    let (ty, _) = mint_dump("=/  x  [p=`@ud`1 q=0x2]\n=,  x\n[q p]");
    assert_eq!(ty, "[%cell [[%atom [%ux 0]] [%atom [%ud 0]]]]");
    // First bridge misses, second hits.
    let (ty, _) = mint_dump("=/  x  [p=`@ud`1 q=2]\n=/  z  r=3\n=,  x\n=,  z\np");
    assert_eq!(ty, "[%atom [%ud 0]]");
    // Bridge that finds a synthetic alias.
    let (ty, _) = mint_dump("=/  y  0x1\n=,  =*(w [y y] .)\nw");
    assert_eq!(ty, "[%cell [[%atom [%ux 0]] [%atom [%ux 0]]]]");
    // A void bridge in one fork branch is dropped by twin.
    let (ty, _) = mint_dump("=/  b  *?\n=/  t  ?:(b =,(!! .) =,([p=`@ud`1 q=2] .))\np.t");
    assert_eq!(ty, "[%atom [%ud 0]]");
}

#[test]
fn c4_find_source_cores_and_holds() {
    // Empty battery: loot finds nothing and the payload is searched.
    let (ty, _) = mint_dump("=/  x  0x5\n=>  |%\n    --\nx");
    assert_eq!(ty, "[%atom [%ux 0]]");
    // `^b` skips arm `b` and finds the payload leg `b`.
    let (ty, _) = mint_dump("=/  b  0x5\n=>  |%  ++  b  1\n    --\n^b");
    assert_eq!(ty, "[%atom [%ux 0]]");
    // Zinc read and iron write search only the sample.
    let (ty, _) = mint_dump("=/  z  ^&  |=(q=@ux q)\nq.z");
    assert_eq!(ty, "[%atom [%ux 0]]");
    let (ty, _) = mint_dump("=/  i  ^|  |=(r=@ r)\ni(r 7)");
    assert!(ty.starts_with("[%core"), "{ty}");
    // Lead cores hide the sample from reads (hoonc rejects too).
    assert!(mint_err("=/  g  ^?  |=(q=@ q)\nq.g").contains("find failed"));
    // A head-position %hold cycle is cut to void and twin drops it.
    let (ty, _) =
        mint_dump("=>  |%\n    +$  rec  $@(~ [rec c=@ux])\n    --\n|=  s=rec\n?~(s 0 c.s)");
    assert!(ty.starts_with("[%core"), "{ty}");
}

#[test]
fn c4_find_source_fork_rejections() {
    // Different skip counts left over (hoonc: find-fork).
    let msg = mint_err("=/  b  *?\n=/  c  ?:(b [a=1 a=2] [a=1 z=2])\n^^a.c");
    assert!(msg.contains("find-fork"), "{msg}");
    // Arm `a` at different battery axes (hoonc: find-fork).
    let msg = mint_err(
        "=/  b  *?\n=/  c\n  ?:  b\n    |%  ++  a  1\n        ++  z  2\n    --\n  |%  ++  a  1\n  --\na.c",
    );
    assert!(msg.contains("find-fork"), "{msg}");
    // A leg in one branch and an arm in the other (hoonc: find-fork).
    let msg = mint_err("=/  b  *?\n=/  c\n  ?:  b\n    [a=1 z=2]\n  |%  ++  a  1\n  --\na.c");
    assert!(msg.contains("find-fork"), "{msg}");
}

#[test]
fn c4_find_source_arm_fork_fires_each_core() {
    // The same arm name at the same axis in two different cores merges both
    // arms; firing them forks the two products.
    let (ty, _) = mint_dump(
        "=/  b  *?\n=/  c\n  ?:  b\n    =>  x=`@ud`1\n    |%  ++  a  x\n    --\n  =>  x=0x2\n  |%  ++  a  x\n  --\na.c",
    );
    assert!(ty.starts_with("[%fork"), "{ty}");
}

// ---------------------------------------------------------------------------
// fire.rs
// ---------------------------------------------------------------------------

#[test]
fn c4_fire_edge_arms() {
    let mut slab: NounSlab = NounSlab::new();
    let payload = ty_noun(&mut slab);
    let context = ty_noun(&mut slab);
    let ud = ty_atom(&mut slab, "ud", None);
    let core_a = core_with_arms(
        &mut slab,
        payload,
        context,
        "wet",
        &[("a", Hoon::Axis(1u64.into()))],
    );
    let core_b = core_with_arms(
        &mut slab,
        ud,
        context,
        "dry",
        &[("a", Hoon::Axis(1u64.into()))],
    );
    let h1 = hoon_to_noun(&mut slab, &Hoon::Axis(1u64.into()));
    let h2 = hoon_to_noun(&mut slab, &Hoon::Axis(2u64.into()));
    let wet1 = foot_from_poly(&mut slab, Poly::Wet, h1);
    let wet2 = foot_from_poly(&mut slab, Poly::Wet, h2);
    let dry1 = foot_from_poly(&mut slab, Poly::Dry, h1);
    let mut ut = Ut::new(&mut slab);
    let ca = nat(&mut ut, core_a);
    let cb = nat(&mut ut, core_b);

    // No arms: void, as `(fork ~)` in hoon-138.
    assert!(matches!(&*ut.fire(&ca, &[]).unwrap(), NTy::Void));
    // One wet arm whose body is `[%$ 1]`: the core itself, no %hold.
    let out = ut.fire(&ca, &[(ca.clone(), wet1)]).unwrap();
    assert!(NRc::ptr_eq(&out, &ca));
    // A wet arm with any other body: a %hold on the redone core.
    ut.set_vet(false);
    let out = ut.fire(&ca, &[(ca.clone(), wet2)]).unwrap();
    assert!(matches!(&*out, NTy::Hold { .. }));
    // A malformed foot is rejected.
    assert!(ut.fire(&ca, &[(ca.clone(), D(5))]).is_err());
    // Two dry arms on different cores fork their holds.
    ut.set_vet(true);
    let out = ut.fire(&ca.clone(), &[(ca, dry1), (cb, dry1)]).unwrap();
    assert!(matches!(&*out, NTy::Fork { .. }));
}

#[test]
fn c4_fire_garb_with_each_vair() {
    let mut slab: NounSlab = NounSlab::new();
    let garb = garb_noun(&mut slab, "dry", "gold");
    let mut ut = Ut::new(&mut slab);
    for (vair, name) in [
        (Vair::Gold, "gold"),
        (Vair::Iron, "iron"),
        (Vair::Lead, "lead"),
        (Vair::Zinc, "zinc"),
    ] {
        let out = ut.garb_with_vair(garb, vair).unwrap();
        assert_eq!(dump(ut.slab, out), format!("[%test [%dry %{name}]]"));
    }
}

// ---------------------------------------------------------------------------
// repo.rs
// ---------------------------------------------------------------------------

#[test]
fn c4_repo_rest_scopes_and_caches() {
    let mut slab: NounSlab = NounSlab::new();
    let payload = ty_noun(&mut slab);
    let context = ty_noun(&mut slab);
    let inner = core_with_arms(
        &mut slab,
        payload,
        context,
        "dry",
        &[("a", Hoon::Axis(1u64.into()))],
    );
    let hoon = hoon_to_noun(&mut slab, &Hoon::Axis(1u64.into()));
    let hold = ty_hold(&mut slab, inner, hoon);
    let legs = [(inner, hoon)];
    let mut ut = Ut::new(&mut slab);
    assert_eq!(ut.with_rest_legs(&[], |_| Ok(7u32)).unwrap(), 7);
    let first = ut.rest(hold, &legs).unwrap();
    let second = ut.rest(hold, &legs).unwrap();
    assert!(nn::noun_eq(first, second, &ut.slab.noun_space()).unwrap());
    assert_eq!(tag_of(ut.slab, first), "core");
    // A hold gene that does not decode as hoon is reported with its tag.
    let inner_n = nat(&mut ut, inner);
    let msg = err_text(ut.rest_inner(&[(inner_n, D(12345))]));
    assert!(msg.contains("native rest: hold ast missing"), "{msg}");
}

#[test]
fn c4_repo_ty_hold_cached_raw_structural_and_bucket_limit() {
    let mut slab: NounSlab = NounSlab::new();
    let inner = cell(&mut slab, &[D(1), D(2)]);
    let inner_copy = cell(&mut slab, &[D(1), D(2)]);
    let inner2 = cell(&mut slab, &[D(3), D(4)]);
    let hoon = hoon_to_noun(&mut slab, &Hoon::Axis(1u64.into()));
    let hoon_copy = hoon_to_noun(&mut slab, &Hoon::Axis(1u64.into()));
    let other_hoon = hoon_to_noun(&mut slab, &Hoon::Axis(3u64.into()));
    let mut ut = Ut::new(&mut slab);

    let hold = ut.ty_hold_cached(inner, hoon).unwrap();
    assert!(dump(ut.slab, hold).starts_with("[%hold [[1 2] "));
    // Exact repeat: raw memo.
    let again = ut.ty_hold_cached(inner, hoon).unwrap();
    assert!(unsafe { again.raw_equals(&hold) });
    // Structurally equal inner with the same hoon noun, and the same inner
    // noun with a structurally equal hoon: both reuse the bucketed hold.
    let by_inner = ut.ty_hold_cached(inner_copy, hoon).unwrap();
    assert!(unsafe { by_inner.raw_equals(&hold) });
    let by_hoon = ut.ty_hold_cached(inner, hoon_copy).unwrap();
    assert!(unsafe { by_hoon.raw_equals(&hold) });

    // A full bucket of non-matching entries (other inners, and this inner with
    // another hoon) evicts its oldest entry when a new hold is added.
    let key = HoldKey {
        subject: ut.noun_mug_cached(inner2),
        gene: ut.noun_mug_cached(hoon),
    };
    let bucket = ut.hold_memo.hold_type.ensure_key(key, 65_536);
    for k in 0..7u64 {
        bucket.push_back(HoldTypeCacheEntry {
            inner: D(100 + k),
            hoon,
            hold: D(0),
        });
    }
    bucket.push_back(HoldTypeCacheEntry {
        inner: inner2,
        hoon: other_hoon,
        hold: D(0),
    });
    let fresh = ut.ty_hold_cached(inner2, hoon).unwrap();
    assert!(dump(ut.slab, fresh).starts_with("[%hold [[3 4] "));
    let bucket = ut.hold_memo.hold_type.get(&key).unwrap();
    assert_eq!(bucket.len(), 8);
    assert!(unsafe { bucket.back().unwrap().hold.raw_equals(&fresh) });
}

// ---------------------------------------------------------------------------
// wet.rs
// ---------------------------------------------------------------------------

#[test]
fn c4_wet_mull_check_wet_rejects_undecodable_arm() {
    let mut slab: NounSlab = NounSlab::new();
    let noun = ty_noun(&mut slab);
    let mut ut = Ut::new(&mut slab);
    let n = nat(&mut ut, noun);
    ut.set_vet(false);
    assert!(ut
        .mull_check_wet(&n, n.clone(), n.clone(), D(12345))
        .is_ok());
    ut.set_vet(true);
    let msg = err_text(ut.mull_check_wet(&n.clone(), n.clone(), n, D(12345)));
    assert!(msg.contains("fire-wet arm ast missing"), "{msg}");
    assert!(ut.fire_wet_rib.is_empty());
    assert!(ut.fire_wet_rib_raw.is_empty());
}

#[test]
fn c4_wet_redo_rejects_mixed_face_depths() {
    // The sample reference `a=?(b=@ @)` leaves two face stacks, `[a b]` and
    // `[a]`, for an atom argument; hoonc also fails with %dear-many/redo-match.
    let msg = mint_err("=/  gat  |*  a=?(b=@ @)  a\n(gat 5)");
    assert!(msg.contains("redo-match"), "{msg}");
}

#[test]
fn c4_wet_redo_empty_wec_drops_faces() {
    // Every case of the forked formal sample misses the actual sample, so
    // `++sint` leaves `wec` empty and hoon-138 `++dear` yields no faces: the
    // redone sample loses both the formal `a=` and the actual `q=`.
    let (ty, _) = mint_dump("=/  gat  |*  a=?(%foo %bar)  +6\n(gat q=%baz)");
    assert!(
        ty.starts_with("[%hold [[%core [[%cell [[%atom [%tas [0 %baz]]] %noun]] "),
        "{ty}"
    );
    // An atom against a fork of cells (formerly redo-match): faceless, accepted.
    let (ty, _) = mint_dump("=/  gat  |*  a=?([p=@ q=@] [r=@ s=@])  1\n(gat 5)");
    assert!(
        ty.starts_with("[%hold [[%core [[%cell [[%atom [%ud 0]] %noun]] "),
        "{ty}"
    );
    // The body can no longer name the dropped sample face (hoonc: find . a).
    let msg = mint_err("=/  gat  |*  a=?(%foo %bar)  a\n(gat %baz)");
    assert!(msg.contains("find"), "{msg}");
}

#[test]
fn c4_wet_rib_keyed_on_call_site_subject() {
    // `=>  foo  $` enters the wet arm from the formal gate itself, so the
    // recursive `$(x [x x])` fires from the same subject and hoon-138 ++fire
    // finds `[sut dox arm]` in `rib`: the `x=[@ @]` re-mull is skipped.
    mint_dump("=/  foo  |*(x=@ [.+(x) $(x [x x])])\n=>  foo\n$");
    // Entered with `$(x 0)`, the redone core differs from the entry subject,
    // so the inner call is re-mulled with `x=[@ud @ud]` and `.+(x)` fails.
    let msg = mint_err("=/  foo  |*(x=@ [.+(x) $(x [x x])])\n=>  foo\n$(x 0)");
    assert!(msg.contains("mull-nice"), "{msg}");
}

#[test]
fn c4_wet_rib_scan_compares_entries_from_the_same_subject() {
    // `f:d` mulls `f` against the redone door, whose body fires `g` from that
    // door; `g`'s body fires `h` from the same subject, so the rib scan meets
    // the entry for `g` with an equal subject and dox but another arm, and
    // `z.e` fires from that subject with another core's dox.
    mint_dump(include_str!(
        "../../../../test-assets/type-probes/coverage/c4/c4_wet_rib_scan.hoon"
    ));
}

#[test]
fn c4_wet_mulled_core_is_not_the_played_core() {
    // `^.` plays the core literal (through the wet `id`) and the wet caller's
    // body mulls it. hoon-138 builds the played core with `*seminoun` and the
    // mulled one with `++laze`, so nest cannot take the equal-coil shortcut and
    // meet(context, payload) rejects `x=%foo` against `x=@ud`.
    let src = |edit: &str| {
        format!(
            "=/  id  |*(a=* a)\n=/  wet  |*  x=*  ^.  id  =>  |%  ++  y  1  --  .(x {edit})\n(wet 5)"
        )
    };
    let msg = mint_err(&src("%foo"));
    assert!(msg.contains("mull-nice"), "{msg}");
    mint_dump(&src("6"));
}

#[test]
fn c4_wet_redo_recursive_argument_against_forked_sample() {
    // A `rec` argument against the sample `?(~ [i=@ t=*])`: the tail recursion
    // meets the same %hold with a different reduced reference.
    let (ty, _) = mint_dump(
        "=>  |%\n    +$  rec  $@(~ [i=@ t=rec])\n    --\n=/  gat  |*  a=?(~ [i=@ t=*])  a\n=/  v  ^-(rec [1 ~])\n(gat v)",
    );
    assert!(!ty.is_empty());
}

#[test]
fn c4_wet_redo_through_recursive_sample() {
    let (ty, _) = mint_dump(
        "=>  |%\n    +$  rec  $@(~ [i=@ t=rec])\n    --\n=/  gat  |*  a=rec  a\n(gat [1 2 ~])",
    );
    assert!(!ty.is_empty());
}

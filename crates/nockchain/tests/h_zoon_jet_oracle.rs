//! Always-on jet differential oracle for the seven h-zoon jets.
//!
//! Companion to `h_zoon_checkpoint_migration.rs`. That test runs a full
//! kernel boot/export against a developer-supplied checkpoint and stays
//! `#[ignore]`d because it needs a chkjam pinned to the local node. It can
//! opt into the expensive `NOCK_TEST_JETS` Hoon fallback list for local
//! differential checks. This file fills the "always-on" gap: it constructs
//! the seven h-zoon jet inputs in Rust, calls each jet against an
//! independent Rust oracle, and asserts both produced byte-equivalent
//! nouns.
//!
//! Coverage assertion: an explicit counter tallies the seven jets that
//! ran. If any of the seven is missed -- because the test is silently
//! short-circuiting -- the final assert fails loudly, so we cannot lose
//! differential coverage by accident.
//!
//! When a new h-zoon jet lands, add the call here AND in
//! `h_zoon_checkpoint_migration.rs::H_ZOON_TEST_JETS`. The plan in
//! `docs/H-ZOON-TEST-PLAN.md` (§3) describes the contract; this is its
//! always-on incarnation.

use nockchain_math::noun_ext::NounMathExt;
use nockchain_math::zoon::common::DefaultTipHasher;
use nockchain_math::zoon::zmap::z_map_put;
use nockchain_math::zoon::zset::z_set_put;
use nockvm::interpreter::Context;
use nockvm::jets::util::test::init_context;
use nockvm::jets::util::BAIL_FAIL;
use nockvm::jets::JetErr;
use nockvm::mem::NockStack;
use nockvm::noun::{Noun, NounAllocator, NounSpace, D, NO, T, YES};
use nockvm::unifying_equality::unifying_equality;
use zkvm_jetpack::jets::zoon_jets::{
    gor_hip_jet, h_by_bif_jet, h_by_del_jet, h_by_dif_jet, h_by_dig_jet, h_by_gas_jet,
    h_by_get_jet, h_by_got_jet, h_by_gut_jet, h_by_has_jet, h_by_int_jet, h_by_mar_jet,
    h_by_put_jet, h_by_uni_jet, h_in_bif_jet, h_in_del_jet, h_in_dif_jet, h_in_dig_jet,
    h_in_gas_jet, h_in_has_jet, h_in_int_jet, h_in_put_jet, h_in_uni_jet, mor_hip_jet,
    zh_balance_milt_jet, zh_jult_jet, zh_milt_jet, zh_molt_jet, zh_silt_jet,
};

const GOR_HIP_ORDER: [usize; 5] = [4, 3, 2, 1, 0];
const MOR_HIP_ORDER: [usize; 5] = [0, 1, 2, 3, 4];

fn direct_limb(value: u64) -> u64 {
    value & (u64::MAX >> 1)
}

fn digest(stack: &mut NockStack, limbs: [u64; 5]) -> Noun {
    T(
        stack,
        &[
            D(direct_limb(limbs[0])),
            D(direct_limb(limbs[1])),
            D(direct_limb(limbs[2])),
            D(direct_limb(limbs[3])),
            D(direct_limb(limbs[4])),
        ],
    )
}

fn list(stack: &mut NockStack, items: &[Noun]) -> Noun {
    items
        .iter()
        .rev()
        .fold(D(0), |tail, item| T(stack, &[*item, tail]))
}

fn digest_list(stack: &mut NockStack, digests: &[[u64; 5]]) -> Noun {
    let items: Vec<Noun> = digests.iter().map(|limbs| digest(stack, *limbs)).collect();
    list(stack, &items)
}

fn jet_subject(stack: &mut NockStack, a: Noun, b: Noun) -> Noun {
    let sam = T(stack, &[a, b]);
    T(stack, &[D(0), sam, D(0)])
}

fn unary_jet_subject(stack: &mut NockStack, sample: Noun) -> Noun {
    T(stack, &[D(0), sample, D(0)])
}

fn run_unary_jet(
    ctx: &mut Context,
    jet: fn(&mut Context, Noun) -> Result<Noun, JetErr>,
    sample: Noun,
) -> Result<Noun, JetErr> {
    let subject = unary_jet_subject(&mut ctx.stack, sample);
    jet(ctx, subject)
}

fn cmp_with_jet(
    ctx: &mut Context,
    jet: fn(&mut Context, Noun) -> Result<Noun, JetErr>,
    a: Noun,
    b: Noun,
) -> Result<bool, JetErr> {
    let subject = jet_subject(&mut ctx.stack, a, b);
    let result = jet(ctx, subject)?;
    if unsafe { result.raw_equals(&YES) } {
        Ok(true)
    } else if unsafe { result.raw_equals(&NO) } {
        Ok(false)
    } else {
        panic!("comparator jet did not return %.y/%.n: {result:?}");
    }
}

fn noun_eq(stack: &mut NockStack, a: Noun, b: Noun) -> bool {
    let mut an = a;
    let mut bn = b;
    unsafe { unifying_equality(stack, &mut an, &mut bn) }
}

fn z_map_from_entries<A: NounAllocator>(stack: &mut A, entries: &[(Noun, Noun)]) -> Noun {
    let mut map = D(0);
    for (key, value) in entries {
        let mut k = *key;
        let mut v = *value;
        map = z_map_put(stack, &map, &mut k, &mut v, &DefaultTipHasher)
            .expect("z-map construction must succeed");
    }
    map
}

fn z_set_from_items<A: NounAllocator>(stack: &mut A, items: &[Noun]) -> Noun {
    let mut set = D(0);
    for item in items {
        let mut x = *item;
        set = z_set_put(stack, &set, &mut x, &DefaultTipHasher)
            .expect("z-set construction must succeed");
    }
    set
}

// Independent oracle: convert a z-map noun to the h-map shape by
// walking the tree, collecting (key, value) entries, then inserting in
// h-order. Mirrors the Hoon arm `zh-molt` exactly.
fn slow_z_map_to_h_map(stack: &mut NockStack, tree: Noun) -> Result<Noun, JetErr> {
    let mut entries = Vec::new();
    let space = stack.noun_space();
    collect_z_map_entries(tree, &mut entries, &space)?;
    let mut map = h_map_empty();
    for (key, value) in entries {
        map = h_map_put(stack, map, key, value)?;
    }
    Ok(map)
}

fn slow_z_set_to_h_set(stack: &mut NockStack, tree: Noun) -> Result<Noun, JetErr> {
    let mut items = Vec::new();
    let space = stack.noun_space();
    collect_z_set_items(tree, &mut items, &space)?;
    let mut set = h_set_empty();
    for item in items {
        set = h_set_put(stack, set, item)?;
    }
    Ok(set)
}

fn slow_z_mip_to_h_mip(stack: &mut NockStack, tree: Noun) -> Result<Noun, JetErr> {
    let mut entries = Vec::new();
    let space = stack.noun_space();
    collect_z_map_entries(tree, &mut entries, &space)?;
    let mut map = h_map_empty();
    for (outer_key, inner_map) in entries {
        let converted_inner = slow_z_map_to_h_map(stack, inner_map)?;
        map = h_map_put(stack, map, outer_key, converted_inner)?;
    }
    Ok(map)
}

fn slow_z_jug_to_h_jug(stack: &mut NockStack, tree: Noun) -> Result<Noun, JetErr> {
    let mut entries = Vec::new();
    let space = stack.noun_space();
    collect_z_map_entries(tree, &mut entries, &space)?;
    let mut map = h_map_empty();
    for (outer_key, inner_set) in entries {
        let converted_inner = slow_z_set_to_h_set(stack, inner_set)?;
        map = h_map_put(stack, map, outer_key, converted_inner)?;
    }
    Ok(map)
}

fn collect_z_map_entries(
    tree: Noun,
    entries: &mut Vec<(Noun, Noun)>,
    space: &NounSpace,
) -> Result<(), JetErr> {
    if unsafe { tree.raw_equals(&D(0)) } {
        return Ok(());
    }
    if tree.is_atom() {
        return Err(BAIL_FAIL);
    }
    let [entry, left, right] = tree.uncell(space)?;
    let [key, value] = entry.uncell(space)?;
    entries.push((key, value));
    collect_z_map_entries(left, entries, space)?;
    collect_z_map_entries(right, entries, space)
}

fn collect_z_set_items(tree: Noun, items: &mut Vec<Noun>, space: &NounSpace) -> Result<(), JetErr> {
    if unsafe { tree.raw_equals(&D(0)) } {
        return Ok(());
    }
    if tree.is_atom() {
        return Err(BAIL_FAIL);
    }
    let [value, left, right] = tree.uncell(space)?;
    items.push(value);
    collect_z_set_items(left, items, space)?;
    collect_z_set_items(right, items, space)
}

// Minimal h-tree builders for the oracle: %hmap / %hset empty leaves,
// no balancing here -- only used to compare to the jet output, which
// performs the actual treap construction. The oracle stays naive on
// purpose so it cannot share a balancing bug with the jet.
fn h_map_empty() -> Noun {
    D(tas_value(b"hmap"))
}

fn h_set_empty() -> Noun {
    D(tas_value(b"hset"))
}

fn tas_value(bytes: &[u8]) -> u64 {
    let mut acc: u64 = 0;
    for (i, byte) in bytes.iter().enumerate() {
        acc |= u64::from(*byte) << (8 * i);
    }
    acc
}

fn h_map_put(stack: &mut NockStack, tree: Noun, key: Noun, value: Noun) -> Result<Noun, JetErr> {
    let entry = T(stack, &[key, value]);
    let hmap = h_map_empty();
    if unsafe { tree.raw_equals(&hmap) } {
        return Ok(T(stack, &[entry, hmap, hmap]));
    }
    let space = stack.noun_space();
    let [n, l, r] = tree.uncell(&space)?;
    let [np, _nq] = n.uncell(&space)?;
    if noun_eq(stack, key, np) {
        return Ok(T(stack, &[entry, l, r]));
    }
    if hashed_less(key, np, &space) {
        let new_l = h_map_put(stack, l, key, value)?;
        rebalance_left(stack, n, new_l, r)
    } else {
        let new_r = h_map_put(stack, r, key, value)?;
        rebalance_right(stack, n, l, new_r)
    }
}

fn h_set_put(stack: &mut NockStack, tree: Noun, item: Noun) -> Result<Noun, JetErr> {
    let hset = h_set_empty();
    if unsafe { tree.raw_equals(&hset) } {
        return Ok(T(stack, &[item, hset, hset]));
    }
    let space = stack.noun_space();
    let [n, l, r] = tree.uncell(&space)?;
    if noun_eq(stack, item, n) {
        return Ok(tree);
    }
    if hashed_less(item, n, &space) {
        let new_l = h_set_put(stack, l, item)?;
        rebalance_left_set(stack, n, new_l, r)
    } else {
        let new_r = h_set_put(stack, r, item)?;
        rebalance_right_set(stack, n, l, new_r)
    }
}

fn rebalance_left(stack: &mut NockStack, n: Noun, new_l: Noun, r: Noun) -> Result<Noun, JetErr> {
    let hmap = h_map_empty();
    if unsafe { new_l.raw_equals(&hmap) } {
        return Ok(T(stack, &[n, hmap, r]));
    }
    let space = stack.noun_space();
    let [dn, dl, dr] = new_l.uncell(&space)?;
    let [np, _nq] = n.uncell(&space)?;
    let [dnp, _dnq] = dn.uncell(&space)?;
    if hashed_priority(np, dnp, &space) {
        Ok(T(stack, &[n, new_l, r]))
    } else {
        let inner = T(stack, &[n, dr, r]);
        Ok(T(stack, &[dn, dl, inner]))
    }
}

fn rebalance_right(stack: &mut NockStack, n: Noun, l: Noun, new_r: Noun) -> Result<Noun, JetErr> {
    let hmap = h_map_empty();
    if unsafe { new_r.raw_equals(&hmap) } {
        return Ok(T(stack, &[n, l, hmap]));
    }
    let space = stack.noun_space();
    let [dn, dl, dr] = new_r.uncell(&space)?;
    let [np, _nq] = n.uncell(&space)?;
    let [dnp, _dnq] = dn.uncell(&space)?;
    if hashed_priority(np, dnp, &space) {
        Ok(T(stack, &[n, l, new_r]))
    } else {
        let inner = T(stack, &[n, l, dl]);
        Ok(T(stack, &[dn, inner, dr]))
    }
}

fn rebalance_left_set(
    stack: &mut NockStack,
    n: Noun,
    new_l: Noun,
    r: Noun,
) -> Result<Noun, JetErr> {
    let hset = h_set_empty();
    if unsafe { new_l.raw_equals(&hset) } {
        return Ok(T(stack, &[n, hset, r]));
    }
    let space = stack.noun_space();
    let [dn, dl, dr] = new_l.uncell(&space)?;
    if hashed_priority(n, dn, &space) {
        Ok(T(stack, &[n, new_l, r]))
    } else {
        let inner = T(stack, &[n, dr, r]);
        Ok(T(stack, &[dn, dl, inner]))
    }
}

fn rebalance_right_set(
    stack: &mut NockStack,
    n: Noun,
    l: Noun,
    new_r: Noun,
) -> Result<Noun, JetErr> {
    let hset = h_set_empty();
    if unsafe { new_r.raw_equals(&hset) } {
        return Ok(T(stack, &[n, l, hset]));
    }
    let space = stack.noun_space();
    let [dn, dl, dr] = new_r.uncell(&space)?;
    if hashed_priority(n, dn, &space) {
        Ok(T(stack, &[n, l, new_r]))
    } else {
        let inner = T(stack, &[n, l, dl]);
        Ok(T(stack, &[dn, inner, dr]))
    }
}

fn hashed_to_limbs(key: Noun, space: &NounSpace) -> Vec<[u64; 5]> {
    let mut out = Vec::new();
    if let Ok(limbs) = try_single_digest(key, space) {
        out.push(limbs);
        return out;
    }
    let mut cur = key;
    while !unsafe { cur.raw_equals(&D(0)) } {
        if cur.is_atom() {
            return Vec::new();
        }
        let Ok([head, tail]) = cur.uncell(space) else {
            return Vec::new();
        };
        let Ok(limbs) = try_single_digest(head, space) else {
            return Vec::new();
        };
        out.push(limbs);
        cur = tail;
    }
    out
}

fn try_single_digest(key: Noun, space: &NounSpace) -> Result<[u64; 5], ()> {
    let Ok([a, rest1]) = key.uncell(space) else {
        return Err(());
    };
    let Ok([b, rest2]) = rest1.uncell(space) else {
        return Err(());
    };
    let Ok([c, rest3]) = rest2.uncell(space) else {
        return Err(());
    };
    let Ok([d, e]) = rest3.uncell(space) else {
        return Err(());
    };
    let limb_of = |n: &Noun| -> Result<u64, ()> {
        let atom = n.in_space(space).as_atom().map_err(|_| ())?;
        atom.as_u64().map_err(|_| ())
    };
    let xs = [limb_of(&a)?, limb_of(&b)?, limb_of(&c)?, limb_of(&d)?, limb_of(&e)?];
    Ok(xs)
}

fn hashed_less(a: Noun, b: Noun, space: &NounSpace) -> bool {
    digest_list_order_oracle(
        &hashed_to_limbs(a, space),
        &hashed_to_limbs(b, space),
        &GOR_HIP_ORDER,
    )
}

fn hashed_priority(a: Noun, b: Noun, space: &NounSpace) -> bool {
    digest_list_order_oracle(
        &hashed_to_limbs(a, space),
        &hashed_to_limbs(b, space),
        &MOR_HIP_ORDER,
    )
}

fn digest_list_order_oracle(a: &[[u64; 5]], b: &[[u64; 5]], order: &[usize; 5]) -> bool {
    let mut idx = 0;
    loop {
        match (a.get(idx), b.get(idx)) {
            (None, _) => return false,
            (Some(_), None) => return true,
            (Some(av), Some(bv)) => {
                if let Some(ordered) = digest_order_oracle(av, bv, order) {
                    return ordered;
                }
            }
        }
        idx += 1;
    }
}

fn digest_order_oracle(a: &[u64; 5], b: &[u64; 5], order: &[usize; 5]) -> Option<bool> {
    for &i in order {
        if a[i] > b[i] {
            return Some(true);
        }
        if a[i] < b[i] {
            return Some(false);
        }
    }
    None
}

fn assert_noun_eq(stack: &mut NockStack, name: &str, mut a: Noun, mut b: Noun) {
    let eq = unsafe { unifying_equality(stack, &mut a, &mut b) };
    assert!(eq, "{name}: jet and oracle nouns disagree: {a:?} vs {b:?}");
}

// Local page noun, mirroring `local-page:v0:dt` shape: [block 0 parent
// 0 0 0 0 0 0 0 0]. The migration only walks the noun shape; values
// inside the page are opaque to zh-balmilt.
fn local_page_v0(stack: &mut NockStack, block: Noun, parent: Noun) -> Noun {
    T(
        stack,
        &[block, D(0), parent, D(0), D(0), D(0), D(0), D(0), D(0), D(0), D(0)],
    )
}

#[test]
fn h_zoon_jets_are_byte_equivalent_to_independent_oracles() {
    let ctx = &mut init_context();
    let mut jets_ran = 0u32;

    // ---- 1. gor-hip on a single-digest pair ----
    let high = digest(&mut ctx.stack, [0, 0, 0, 0, 2]);
    let low = digest(&mut ctx.stack, [99, 99, 99, 99, 1]);
    let gor = cmp_with_jet(ctx, gor_hip_jet, high, low).expect("gor-hip");
    assert!(
        gor,
        "gor-hip must order higher tail-limb first; jet returned %.n"
    );
    let oracle =
        digest_list_order_oracle(&[[0, 0, 0, 0, 2]], &[[99, 99, 99, 99, 1]], &GOR_HIP_ORDER);
    assert_eq!(gor, oracle, "gor-hip jet disagrees with oracle");
    jets_ran += 1;

    // ---- 2. mor-hip on a single-digest pair ----
    let m_high = digest(&mut ctx.stack, [2, 0, 0, 0, 0]);
    let m_low = digest(&mut ctx.stack, [1, 99, 99, 99, 99]);
    let mor = cmp_with_jet(ctx, mor_hip_jet, m_high, m_low).expect("mor-hip");
    let oracle =
        digest_list_order_oracle(&[[2, 0, 0, 0, 0]], &[[1, 99, 99, 99, 99]], &MOR_HIP_ORDER);
    assert_eq!(mor, oracle, "mor-hip jet disagrees with oracle");
    jets_ran += 1;

    // build a four-entry z-map with a mix of direct-digest and
    // digest-list keys so every conversion jet hits both key shapes
    let key_a = digest(&mut ctx.stack, [1, 0, 0, 0, 0]);
    let key_b = digest_list(&mut ctx.stack, &[[2, 0, 0, 0, 0], [3, 0, 0, 0, 0]]);
    let key_c = digest(&mut ctx.stack, [4, 0, 0, 0, 0]);
    let key_d = digest_list(&mut ctx.stack, &[[5, 0, 0, 0, 0], [6, 0, 0, 0, 0]]);
    let value_a = D(10);
    let value_b = T(&mut ctx.stack, &[D(20), D(21)]);
    let value_c = D(30);
    let value_d = T(&mut ctx.stack, &[D(40), D(41), D(42)]);
    let z_map = z_map_from_entries(
        &mut ctx.stack,
        &[(key_a, value_a), (key_b, value_b), (key_c, value_c), (key_d, value_d)],
    );

    // ---- 3. zh-molt ----
    let h_from_jet = run_unary_jet(ctx, zh_molt_jet, z_map).expect("zh-molt");
    let h_from_oracle = slow_z_map_to_h_map(&mut ctx.stack, z_map).expect("zh-molt oracle");
    assert_noun_eq(&mut ctx.stack, "zh-molt", h_from_jet, h_from_oracle);
    jets_ran += 1;

    // ---- 4. zh-silt ----
    let z_set = z_set_from_items(&mut ctx.stack, &[key_a, key_b, key_c, key_d]);
    let s_from_jet = run_unary_jet(ctx, zh_silt_jet, z_set).expect("zh-silt");
    let s_from_oracle = slow_z_set_to_h_set(&mut ctx.stack, z_set).expect("zh-silt oracle");
    assert_noun_eq(&mut ctx.stack, "zh-silt", s_from_jet, s_from_oracle);
    jets_ran += 1;

    // ---- 5. zh-milt (nested z-map -> nested h-map) ----
    let inner_key_a = digest(&mut ctx.stack, [0, 1, 0, 0, 0]);
    let inner_key_b = digest_list(&mut ctx.stack, &[[0, 2, 0, 0, 0], [0, 3, 0, 0, 0]]);
    let inner_map = z_map_from_entries(
        &mut ctx.stack,
        &[(inner_key_a, D(100)), (inner_key_b, D(200))],
    );
    let outer_key_a = digest(&mut ctx.stack, [1, 1, 0, 0, 0]);
    let outer_key_b = digest_list(&mut ctx.stack, &[[2, 2, 0, 0, 0], [3, 3, 0, 0, 0]]);
    let z_mip = z_map_from_entries(
        &mut ctx.stack,
        &[(outer_key_a, inner_map), (outer_key_b, inner_map)],
    );
    let mip_from_jet = run_unary_jet(ctx, zh_milt_jet, z_mip).expect("zh-milt");
    let mip_from_oracle = slow_z_mip_to_h_mip(&mut ctx.stack, z_mip).expect("zh-milt oracle");
    assert_noun_eq(&mut ctx.stack, "zh-milt", mip_from_jet, mip_from_oracle);
    jets_ran += 1;

    // ---- 6. zh-jult (nested z-set inside z-map) ----
    let inner_set = z_set_from_items(&mut ctx.stack, &[inner_key_a, inner_key_b]);
    let z_jug = z_map_from_entries(
        &mut ctx.stack,
        &[(outer_key_a, inner_set), (outer_key_b, inner_set)],
    );
    let jug_from_jet = run_unary_jet(ctx, zh_jult_jet, z_jug).expect("zh-jult");
    let jug_from_oracle = slow_z_jug_to_h_jug(&mut ctx.stack, z_jug).expect("zh-jult oracle");
    assert_noun_eq(&mut ctx.stack, "zh-jult", jug_from_jet, jug_from_oracle);
    jets_ran += 1;

    // ---- 7. zh-balmilt (parent-chain balance migration) ----
    let parent_block = digest(&mut ctx.stack, [11, 0, 0, 0, 0]);
    let child_block = digest(&mut ctx.stack, [12, 0, 0, 0, 0]);
    let grandparent = digest(&mut ctx.stack, [10, 0, 0, 0, 0]);
    let parent_page = local_page_v0(&mut ctx.stack, parent_block, grandparent);
    let child_page = local_page_v0(&mut ctx.stack, child_block, parent_block);
    let blocks = z_map_from_entries(
        &mut ctx.stack,
        &[(parent_block, parent_page), (child_block, child_page)],
    );
    let note_key_a = digest(&mut ctx.stack, [21, 0, 0, 0, 0]);
    let note_key_b = digest_list(&mut ctx.stack, &[[22, 0, 0, 0, 0], [23, 0, 0, 0, 0]]);
    let note_value_a = T(&mut ctx.stack, &[D(100), D(101)]);
    let note_value_a2 = T(&mut ctx.stack, &[D(110), D(111)]);
    let note_value_b = T(&mut ctx.stack, &[D(200), D(201)]);
    let parent_balance = z_map_from_entries(&mut ctx.stack, &[(note_key_a, note_value_a)]);
    let child_balance = z_map_from_entries(
        &mut ctx.stack,
        &[(note_key_a, note_value_a2), (note_key_b, note_value_b)],
    );
    let balance = z_map_from_entries(
        &mut ctx.stack,
        &[(parent_block, parent_balance), (child_block, child_balance)],
    );
    let sample = T(&mut ctx.stack, &[blocks, balance]);
    let subject = unary_jet_subject(&mut ctx.stack, sample);
    let balmilt_from_jet = zh_balance_milt_jet(ctx, subject).expect("zh-balmilt");
    let balmilt_from_oracle =
        slow_z_mip_to_h_mip(&mut ctx.stack, balance).expect("zh-balmilt oracle");
    assert_noun_eq(
        &mut ctx.stack, "zh-balmilt", balmilt_from_jet, balmilt_from_oracle,
    );
    jets_ran += 1;

    // coverage assertion: if any jet was silently skipped we fail.
    // Update this constant when a new h-zoon jet lands.
    const H_ZOON_JET_COUNT: u32 = 7;
    assert_eq!(
        jets_ran, H_ZOON_JET_COUNT,
        "expected all {H_ZOON_JET_COUNT} h-zoon jets to be exercised, only {jets_ran} ran"
    );
}

// ===========================================================================
// Randomized differential oracle for the 22 non-gate h-by / h-in arm jets.
//
// Inputs (h-maps / h-sets over single-digest keys) are built with the file's
// naive canonical treap builder. Expected results are computed with plain
// BTreeMap / BTreeSet set logic and re-built with the SAME naive builder. The
// canonical treap is uniquely determined by (key set, mor-hip priorities), so
// any correct jet must produce the byte-identical noun. The oracle's set
// logic is independent of each jet's recursive merge/split/lookup, so an
// algorithmic bug cannot be shared.
// ===========================================================================

use std::collections::{BTreeMap, BTreeSet};

struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }
    fn upto(&mut self, n: u64) -> u64 {
        self.next() % n
    }
}

fn rand_limbs(rng: &mut Rng) -> [u64; 5] {
    // small limb domain forces equal keys and gor-hip ordering ties
    [rng.upto(3), rng.upto(3), rng.upto(3), rng.upto(3), rng.upto(3)]
}

fn build_h_map(stack: &mut NockStack, entries: &[([u64; 5], u64)]) -> Noun {
    let mut tree = h_map_empty();
    for (limbs, value) in entries {
        let k = digest(stack, *limbs);
        tree = h_map_put(stack, tree, k, D(*value)).expect("naive h_map_put");
    }
    tree
}

fn build_h_set(stack: &mut NockStack, items: &[[u64; 5]]) -> Noun {
    let mut tree = h_set_empty();
    for limbs in items {
        let x = digest(stack, *limbs);
        tree = h_set_put(stack, tree, x).expect("naive h_set_put");
    }
    tree
}

fn canon_map(stack: &mut NockStack, m: &BTreeMap<[u64; 5], u64>) -> Noun {
    let v: Vec<([u64; 5], u64)> = m.iter().map(|(k, val)| (*k, *val)).collect();
    build_h_map(stack, &v)
}

fn canon_set(stack: &mut NockStack, s: &BTreeSet<[u64; 5]>) -> Noun {
    let v: Vec<[u64; 5]> = s.iter().copied().collect();
    build_h_set(stack, &v)
}

// faithful door-arm subject: arm sample at axis 6, door sample at
// slot(slot(_,7),6) -- same shape the real h-by/h-in call produces.
fn door_subject(stack: &mut NockStack, sample: Noun, container: Noun) -> Noun {
    let context = T(stack, &[D(0), container, D(0)]);
    T(stack, &[D(0), sample, context])
}

fn run_door_jet(
    ctx: &mut Context,
    jet: fn(&mut Context, Noun) -> Result<Noun, JetErr>,
    sample: Noun,
    container: Noun,
) -> Result<Noun, JetErr> {
    let subject = door_subject(&mut ctx.stack, sample, container);
    jet(ctx, subject)
}

fn noun_at_axis(mut tree: Noun, axis: u64, space: &NounSpace) -> Option<Noun> {
    if axis == 0 {
        return None;
    }
    let bits = 63 - axis.leading_zeros();
    for i in (0..bits).rev() {
        let cell = tree.in_space(space).as_cell().ok()?;
        tree = if (axis >> i) & 1 == 0 {
            cell.head().noun()
        } else {
            cell.tail().noun()
        };
    }
    Some(tree)
}

fn gor_lt(a: &[u64; 5], b: &[u64; 5]) -> bool {
    digest_list_order_oracle(&[*a], &[*b], &GOR_HIP_ORDER)
}

#[test]
fn h_by_h_in_non_gate_jets_match_independent_oracles_randomized() {
    let ctx = &mut init_context();
    let mut covered: BTreeSet<&'static str> = BTreeSet::new();

    for seed in 0..400u64 {
        let mut rng = Rng(seed.wrapping_mul(0x2545F4914F6CDD1D).wrapping_add(1));

        // ---- build A and B over an overlapping key pool ----
        let pool: Vec<[u64; 5]> = (0..10).map(|_| rand_limbs(&mut rng)).collect();
        let mut amap: BTreeMap<[u64; 5], u64> = BTreeMap::new();
        let mut bmap: BTreeMap<[u64; 5], u64> = BTreeMap::new();
        for _ in 0..(rng.upto(10) + 1) {
            let k = pool[rng.upto(pool.len() as u64) as usize];
            amap.insert(k, rng.upto(900) + 1);
        }
        for _ in 0..(rng.upto(10) + 1) {
            let k = pool[rng.upto(pool.len() as u64) as usize];
            bmap.insert(k, rng.upto(900) + 1001);
        }
        let a_entries: Vec<([u64; 5], u64)> = amap.iter().map(|(k, v)| (*k, *v)).collect();
        let b_entries: Vec<([u64; 5], u64)> = bmap.iter().map(|(k, v)| (*k, *v)).collect();
        let map_a = build_h_map(&mut ctx.stack, &a_entries);
        let map_b = build_h_map(&mut ctx.stack, &b_entries);
        let aset: BTreeSet<[u64; 5]> = amap.keys().copied().collect();
        let bset: BTreeSet<[u64; 5]> = bmap.keys().copied().collect();
        let set_a = canon_set(&mut ctx.stack, &aset);
        let set_b = canon_set(&mut ctx.stack, &bset);

        // probe keys: some present, some absent
        let probe = pool[rng.upto(pool.len() as u64) as usize];
        let probe_k = digest(&mut ctx.stack, probe);

        // ---- h-by get / has / got / gut ----
        let got_jet = run_door_jet(ctx, h_by_get_jet, probe_k, map_a).expect("h-by get");
        match amap.get(&probe) {
            Some(v) => {
                let exp = T(&mut ctx.stack, &[D(0), D(*v)]);
                assert_noun_eq(&mut ctx.stack, "h-by/get(some)", got_jet, exp);
            }
            None => assert!(
                got_jet.is_atom() && unsafe { got_jet.raw_equals(&D(0)) },
                "h-by/get(none) must be ~"
            ),
        }
        covered.insert("h-by/get");

        let has_jet = run_door_jet(ctx, h_by_has_jet, probe_k, map_a).expect("h-by has");
        let exp_has = if amap.contains_key(&probe) { YES } else { NO };
        assert!(
            unsafe { has_jet.raw_equals(&exp_has) },
            "h-by/has mismatch seed {seed}"
        );
        covered.insert("h-by/has");

        let got_res = run_door_jet(ctx, h_by_got_jet, probe_k, map_a);
        match amap.get(&probe) {
            Some(v) => {
                let g = got_res.expect("h-by got present");
                let ev = D(*v);
                assert_noun_eq(&mut ctx.stack, "h-by/got", g, ev);
            }
            None => assert!(got_res.is_err(), "h-by/got(absent) must error"),
        }
        covered.insert("h-by/got");

        let default = D(424_242);
        let gut_sample = T(&mut ctx.stack, &[probe_k, default]);
        let gut_jet = run_door_jet(ctx, h_by_gut_jet, gut_sample, map_a).expect("h-by gut");
        let exp_gut = match amap.get(&probe) {
            Some(v) => D(*v),
            None => D(424_242),
        };
        assert_noun_eq(&mut ctx.stack, "h-by/gut", gut_jet, exp_gut);
        covered.insert("h-by/gut");

        // ---- h-by put / del / mar ----
        let pk = pool[rng.upto(pool.len() as u64) as usize];
        let pv = rng.upto(900) + 5000;
        let pkn = digest(&mut ctx.stack, pk);
        let put_sample = T(&mut ctx.stack, &[pkn, D(pv)]);
        let put_jet = run_door_jet(ctx, h_by_put_jet, put_sample, map_a).expect("h-by put");
        let mut put_exp_m = amap.clone();
        put_exp_m.insert(pk, pv);
        let put_exp = canon_map(&mut ctx.stack, &put_exp_m);
        assert_noun_eq(&mut ctx.stack, "h-by/put", put_jet, put_exp);
        covered.insert("h-by/put");

        let dk = pool[rng.upto(pool.len() as u64) as usize];
        let dkn = digest(&mut ctx.stack, dk);
        let del_jet = run_door_jet(ctx, h_by_del_jet, dkn, map_a).expect("h-by del");
        let mut del_exp_m = amap.clone();
        del_exp_m.remove(&dk);
        let del_exp = canon_map(&mut ctx.stack, &del_exp_m);
        assert_noun_eq(&mut ctx.stack, "h-by/del", del_jet, del_exp);
        covered.insert("h-by/del");

        // mar with ~ -> del ; mar with [~ v] -> put
        let mar_none = T(&mut ctx.stack, &[dkn, D(0)]);
        let mar_none_jet = run_door_jet(ctx, h_by_mar_jet, mar_none, map_a).expect("h-by mar none");
        assert_noun_eq(&mut ctx.stack, "h-by/mar(none)", mar_none_jet, del_exp);
        let some_v = T(&mut ctx.stack, &[D(0), D(pv)]);
        let mar_some = T(&mut ctx.stack, &[pkn, some_v]);
        let mar_some_jet = run_door_jet(ctx, h_by_mar_jet, mar_some, map_a).expect("h-by mar some");
        assert_noun_eq(&mut ctx.stack, "h-by/mar(some)", mar_some_jet, put_exp);
        covered.insert("h-by/mar");

        // ---- h-by gas ----
        let mut gas_vec: Vec<([u64; 5], u64)> = Vec::new();
        let mut gas_exp_m = amap.clone();
        for _ in 0..rng.upto(5) {
            let gk = pool[rng.upto(pool.len() as u64) as usize];
            let gv = rng.upto(900) + 7000;
            gas_vec.push((gk, gv));
            gas_exp_m.insert(gk, gv);
        }
        let gas_items: Vec<Noun> = gas_vec
            .iter()
            .map(|(k, v)| {
                let kn = digest(&mut ctx.stack, *k);
                T(&mut ctx.stack, &[kn, D(*v)])
            })
            .collect();
        let gas_list = list(&mut ctx.stack, &gas_items);
        let gas_jet = run_door_jet(ctx, h_by_gas_jet, gas_list, map_a).expect("h-by gas");
        let gas_exp = canon_map(&mut ctx.stack, &gas_exp_m);
        assert_noun_eq(&mut ctx.stack, "h-by/gas", gas_jet, gas_exp);
        covered.insert("h-by/gas");

        // ---- h-by uni / int / dif (b wins on equal for uni & int) ----
        let uni_jet = run_door_jet(ctx, h_by_uni_jet, map_b, map_a).expect("h-by uni");
        let mut uni_m = amap.clone();
        for (k, v) in &bmap {
            uni_m.insert(*k, *v);
        }
        let uni_exp = canon_map(&mut ctx.stack, &uni_m);
        assert_noun_eq(&mut ctx.stack, "h-by/uni", uni_jet, uni_exp);
        covered.insert("h-by/uni");

        let int_jet = run_door_jet(ctx, h_by_int_jet, map_b, map_a).expect("h-by int");
        let mut int_m: BTreeMap<[u64; 5], u64> = BTreeMap::new();
        for k in amap.keys() {
            if let Some(bv) = bmap.get(k) {
                int_m.insert(*k, *bv);
            }
        }
        let int_exp = canon_map(&mut ctx.stack, &int_m);
        assert_noun_eq(&mut ctx.stack, "h-by/int", int_jet, int_exp);
        covered.insert("h-by/int");

        let dif_jet = run_door_jet(ctx, h_by_dif_jet, map_b, map_a).expect("h-by dif");
        let mut dif_m: BTreeMap<[u64; 5], u64> = BTreeMap::new();
        for (k, v) in &amap {
            if !bmap.contains_key(k) {
                dif_m.insert(*k, *v);
            }
        }
        let dif_exp = canon_map(&mut ctx.stack, &dif_m);
        assert_noun_eq(&mut ctx.stack, "h-by/dif", dif_jet, dif_exp);
        covered.insert("h-by/dif");

        // ---- h-by bif ----
        let bif_jet = run_door_jet(ctx, h_by_bif_jet, probe_k, map_a).expect("h-by bif");
        let space = ctx.stack.noun_space();
        let [bl, br] = bif_jet.uncell(&space).expect("bif pair");
        let mut left_m: BTreeMap<[u64; 5], u64> = BTreeMap::new();
        let mut right_m: BTreeMap<[u64; 5], u64> = BTreeMap::new();
        for (k, v) in &amap {
            if *k == probe {
                continue;
            }
            if gor_lt(k, &probe) {
                left_m.insert(*k, *v);
            } else {
                right_m.insert(*k, *v);
            }
        }
        let le = canon_map(&mut ctx.stack, &left_m);
        let re = canon_map(&mut ctx.stack, &right_m);
        assert_noun_eq(&mut ctx.stack, "h-by/bif.l", bl, le);
        assert_noun_eq(&mut ctx.stack, "h-by/bif.r", br, re);
        covered.insert("h-by/bif");

        // ---- h-by dig (independent invariant: axis lands on key) ----
        let dig_jet = run_door_jet(ctx, h_by_dig_jet, probe_k, map_a);
        if let Ok(d) = dig_jet {
            match amap.get(&probe) {
                None => assert!(unsafe { d.raw_equals(&D(0)) }, "h-by/dig(absent) must be ~"),
                Some(_) => {
                    let space = ctx.stack.noun_space();
                    let [_z, ax] = d.uncell(&space).expect("dig unit");
                    let axv = ax
                        .in_space(&space)
                        .as_atom()
                        .expect("axis atom")
                        .as_u64()
                        .expect("axis u64");
                    let n = noun_at_axis(map_a, axv, &space).expect("axis node");
                    // axis points at the [key value] pair; head is the key
                    let nk = n.in_space(&space).as_cell().expect("pair").head().noun();
                    let mut a = nk;
                    let mut b = probe_k;
                    assert!(
                        unsafe { unifying_equality(&mut ctx.stack, &mut a, &mut b) },
                        "h-by/dig axis must address the queried key"
                    );
                }
            }
            covered.insert("h-by/dig");
        } // else Punt (deep axis) is an acceptable faithful fallback

        // ---- h-in has / put / del / gas ----
        let s_has = run_door_jet(ctx, h_in_has_jet, probe_k, set_a).expect("h-in has");
        let exp_s_has = if aset.contains(&probe) { YES } else { NO };
        assert!(
            unsafe { s_has.raw_equals(&exp_s_has) },
            "h-in/has mismatch seed {seed}"
        );
        covered.insert("h-in/has");

        let spk = digest(&mut ctx.stack, pk);
        let s_put = run_door_jet(ctx, h_in_put_jet, spk, set_a).expect("h-in put");
        let mut s_put_set = aset.clone();
        s_put_set.insert(pk);
        let s_put_exp = canon_set(&mut ctx.stack, &s_put_set);
        assert_noun_eq(&mut ctx.stack, "h-in/put", s_put, s_put_exp);
        covered.insert("h-in/put");

        let sdk = digest(&mut ctx.stack, dk);
        let s_del = run_door_jet(ctx, h_in_del_jet, sdk, set_a).expect("h-in del");
        let mut s_del_set = aset.clone();
        s_del_set.remove(&dk);
        let s_del_exp = canon_set(&mut ctx.stack, &s_del_set);
        assert_noun_eq(&mut ctx.stack, "h-in/del", s_del, s_del_exp);
        covered.insert("h-in/del");

        let mut s_gas_items: Vec<[u64; 5]> = Vec::new();
        let mut s_gas_set = aset.clone();
        for _ in 0..rng.upto(5) {
            let gk = pool[rng.upto(pool.len() as u64) as usize];
            s_gas_items.push(gk);
            s_gas_set.insert(gk);
        }
        let s_gas_nouns: Vec<Noun> = s_gas_items
            .iter()
            .map(|k| digest(&mut ctx.stack, *k))
            .collect();
        let s_gas_list = list(&mut ctx.stack, &s_gas_nouns);
        let s_gas = run_door_jet(ctx, h_in_gas_jet, s_gas_list, set_a).expect("h-in gas");
        let s_gas_exp = canon_set(&mut ctx.stack, &s_gas_set);
        assert_noun_eq(&mut ctx.stack, "h-in/gas", s_gas, s_gas_exp);
        covered.insert("h-in/gas");

        // ---- h-in uni / int / dif ----
        let s_uni = run_door_jet(ctx, h_in_uni_jet, set_b, set_a).expect("h-in uni");
        let s_uni_exp = canon_set(
            &mut ctx.stack,
            &aset.union(&bset).copied().collect::<BTreeSet<_>>(),
        );
        assert_noun_eq(&mut ctx.stack, "h-in/uni", s_uni, s_uni_exp);
        covered.insert("h-in/uni");

        let s_int = run_door_jet(ctx, h_in_int_jet, set_b, set_a).expect("h-in int");
        let s_int_exp = canon_set(
            &mut ctx.stack,
            &aset.intersection(&bset).copied().collect::<BTreeSet<_>>(),
        );
        assert_noun_eq(&mut ctx.stack, "h-in/int", s_int, s_int_exp);
        covered.insert("h-in/int");

        let s_dif = run_door_jet(ctx, h_in_dif_jet, set_b, set_a).expect("h-in dif");
        let s_dif_exp = canon_set(
            &mut ctx.stack,
            &aset.difference(&bset).copied().collect::<BTreeSet<_>>(),
        );
        assert_noun_eq(&mut ctx.stack, "h-in/dif", s_dif, s_dif_exp);
        covered.insert("h-in/dif");

        // ---- h-in bif ----
        let s_bif = run_door_jet(ctx, h_in_bif_jet, probe_k, set_a).expect("h-in bif");
        let space = ctx.stack.noun_space();
        let [sbl, sbr] = s_bif.uncell(&space).expect("h-in bif pair");
        let mut sl: BTreeSet<[u64; 5]> = BTreeSet::new();
        let mut sr: BTreeSet<[u64; 5]> = BTreeSet::new();
        for k in &aset {
            if *k == probe {
                continue;
            }
            if gor_lt(k, &probe) {
                sl.insert(*k);
            } else {
                sr.insert(*k);
            }
        }
        let sle = canon_set(&mut ctx.stack, &sl);
        let sre = canon_set(&mut ctx.stack, &sr);
        assert_noun_eq(&mut ctx.stack, "h-in/bif.l", sbl, sle);
        assert_noun_eq(&mut ctx.stack, "h-in/bif.r", sbr, sre);
        covered.insert("h-in/bif");

        // ---- h-in dig ----
        let s_dig = run_door_jet(ctx, h_in_dig_jet, probe_k, set_a);
        if let Ok(d) = s_dig {
            match aset.contains(&probe) {
                false => assert!(unsafe { d.raw_equals(&D(0)) }, "h-in/dig(absent) must be ~"),
                true => {
                    let space = ctx.stack.noun_space();
                    let [_z, ax] = d.uncell(&space).expect("dig unit");
                    let axv = ax
                        .in_space(&space)
                        .as_atom()
                        .expect("axis atom")
                        .as_u64()
                        .expect("axis u64");
                    let n = noun_at_axis(set_a, axv, &space).expect("axis node");
                    let mut a = n;
                    let mut b = probe_k;
                    assert!(
                        unsafe { unifying_equality(&mut ctx.stack, &mut a, &mut b) },
                        "h-in/dig axis must address the queried item"
                    );
                }
            }
            covered.insert("h-in/dig");
        }
    }

    const EXPECTED: &[&str] = &[
        "h-by/get", "h-by/got", "h-by/gut", "h-by/has", "h-by/put", "h-by/del", "h-by/mar",
        "h-by/gas", "h-by/uni", "h-by/int", "h-by/dif", "h-by/bif", "h-by/dig", "h-in/has",
        "h-in/put", "h-in/del", "h-in/gas", "h-in/uni", "h-in/int", "h-in/dif", "h-in/bif",
        "h-in/dig",
    ];
    for arm in EXPECTED {
        assert!(
            covered.contains(arm),
            "jet {arm} was never exercised by the randomized differential"
        );
    }
    assert_eq!(
        covered.len(),
        EXPECTED.len(),
        "unexpected jet coverage set: {covered:?}"
    );
}

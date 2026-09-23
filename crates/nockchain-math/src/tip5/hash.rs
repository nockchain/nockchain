use nockvm::jets::util::BAIL_FAIL;
use nockvm::jets::JetErr;
use nockvm::noun::{Noun, NounAllocator, NounSpace};
use noun_serde::{NounDecode, NounEncode};

use super::*;
use crate::based;
use crate::belt::{montify, Belt};
use crate::poly::Poly;

// assert that input is made of base field elements
pub fn assert_all_based(vecbelt: &Vec<Belt>) {
    vecbelt.iter().for_each(|b| based!(b.0));
}

// calc q and r for vecbelt, based on RATE
pub fn tip5_calc_q_r(input_vec: &[Belt]) -> (usize, usize) {
    let lent_input = input_vec.len();
    let (q, r) = (lent_input / RATE, lent_input % RATE);
    (q, r)
}

// pad vecbelt with ~[1 0 ... 0] to be a multiple of rate
pub fn tip5_pad_vecbelt(input_vec: &mut Vec<Belt>, r: usize) {
    input_vec.push(Belt(1));
    for _i in 0..(RATE - r) - 1 {
        input_vec.push(Belt(0));
    }
}

// monitify vecbelt (bring into montgomery space)
pub fn tip5_montify_vecbelt(input_vec: &mut [Belt]) {
    for belt in input_vec.iter_mut() {
        *belt = Belt(montify(belt.0));
    }
}

// calc digest
pub fn tip5_calc_digest(sponge: &[u64; 16]) -> [u64; 5] {
    let mut digest = [0u64; DIGEST_LENGTH];
    for i in 0..DIGEST_LENGTH {
        digest[i] = mont_reduction(sponge[i] as u128);
    }
    digest
}

// absorb complete input
pub fn tip5_absorb_input(input_vec: &mut Vec<Belt>, sponge: &mut [u64; 16], q: usize) {
    let mut cnt_q = q;
    let mut input_to_absorb = input_vec.as_slice();
    loop {
        let (scag_input, slag_input) = input_to_absorb.split_at(RATE);
        tip5_absorb_rate(sponge, scag_input);

        if cnt_q == 0 {
            break;
        }
        cnt_q -= 1;
        input_to_absorb = slag_input;
    }
}

// absorb one part of input (size RATE)
pub fn tip5_absorb_rate(sponge: &mut [u64; 16], input: &[Belt]) {
    assert_eq!(input.len(), RATE);

    for copy_pos in 0..RATE {
        sponge[copy_pos] = input[copy_pos].0;
    }

    permute(sponge);
}

#[inline(always)]
fn absorb_preprocess(input: &[u64]) -> Vec<u64> {
    let r = input.len() % RATE;
    let mut padded = vec![0; input.len() + RATE - r];
    padded[..input.len()].copy_from_slice(input);
    padded[input.len()] = MONT_ONE;
    padded
}

#[inline(always)]
fn hash_ten_cell_mont(input: [u64; RATE]) -> [u64; DIGEST_LENGTH] {
    let mut sponge: [u64; STATE_SIZE] = [0; STATE_SIZE];
    sponge[..RATE].copy_from_slice(&input);
    for item in sponge.iter_mut().skip(RATE) {
        *item = MONT_ONE;
    }

    permute(&mut sponge);

    sponge[..DIGEST_LENGTH]
        .try_into()
        .expect("digest length should match")
}

#[inline(always)]
pub fn hash_ten_cell(input: [u64; RATE]) -> [u64; DIGEST_LENGTH] {
    let mut monted = [0; RATE];
    for i in 0..RATE {
        based!(input[i]);
        monted[i] = montify(input[i]);
    }

    let mut res = hash_ten_cell_mont(monted);
    for item in &mut res {
        *item = mont_reduction(*item as u128);
    }
    res
}

#[inline(always)]
pub fn hash_belts_slice(input: &[u64]) -> [u64; DIGEST_LENGTH] {
    let q = input.len() / RATE;
    let mut padded = absorb_preprocess(input);

    for i in 0..input.len() {
        based!(input[i]);
        padded[i] = montify(input[i]);
    }

    let mut sponge: [u64; STATE_SIZE] = [0; STATE_SIZE];
    let mut padded_ref = &padded[..];
    for _ in 0..=q {
        sponge[..RATE].copy_from_slice(&padded_ref[..RATE]);
        permute(&mut sponge);
        padded_ref = &padded_ref[RATE..];
    }

    tip5_calc_digest(&sponge)
}

#[inline(always)]
pub fn absorb(sponge: &mut [u64; STATE_SIZE], input: &[u64]) {
    let q = input.len() / RATE;
    let mut padded = absorb_preprocess(input);

    for i in 0..input.len() {
        based!(input[i]);
        padded[i] = montify(input[i]);
    }

    let mut padded_ref = &padded[..];
    for _ in 0..=q {
        sponge[..RATE].copy_from_slice(&padded_ref[..RATE]);
        permute(sponge);
        padded_ref = &padded_ref[RATE..];
    }
}

#[inline(always)]
pub fn squeeze(sponge: &mut [u64; STATE_SIZE]) -> [u64; RATE] {
    let mut res = [0; RATE];
    for i in 0..RATE {
        res[i] = mont_reduction(sponge[i] as u128);
    }
    permute(sponge);
    res
}

pub fn hash_varlen(input_vec: &mut Vec<Belt>) -> [u64; 5] {
    assert_all_based(input_vec);
    let input = input_vec.iter().map(|belt| belt.0).collect::<Vec<_>>();
    hash_belts_slice(&input)
}

pub fn create_init_sponge_variable() -> [u64; STATE_SIZE] {
    [0u64; STATE_SIZE]
}
pub fn create_init_sponge_fixed() -> [u64; STATE_SIZE] {
    [
        0u64, 0u64, 0u64, 0u64, 0u64, 0u64, 0u64, 0u64, 0u64, 0u64, 4294967295u64, 4294967295u64,
        4294967295u64, 4294967295u64, 4294967295u64, 4294967295u64,
    ]
}

pub fn hash_10(input_vec: &mut Vec<Belt>) -> [u64; 5] {
    // check input
    let (q, r) = tip5_calc_q_r(input_vec);
    assert_eq!(q, 1);
    assert_eq!(r, 0);
    assert_all_based(input_vec);

    let input: [u64; RATE] = input_vec
        .iter()
        .map(|belt| belt.0)
        .collect::<Vec<_>>()
        .try_into()
        .expect("hash_10 input should have RATE elements");
    hash_ten_cell(input)
}

pub fn hash_noun_varlen<A: NounAllocator>(
    stack: &mut A,
    n: Noun,
    space: &NounSpace,
) -> Result<Noun, JetErr> {
    let digest = hash_noun_varlen_digest(stack, n, space)?;
    Ok(digest.to_noun(stack))
}

pub fn hash_noun_varlen_digest<A: NounAllocator>(
    _stack: &mut A,
    n: Noun,
    space: &NounSpace,
) -> Result<[u64; 5], JetErr> {
    let mut leaf_vec = Vec::new();
    let mut dyck_vec = Vec::new();
    dfs_dyck(n, &mut leaf_vec, &mut dyck_vec, space)?;

    let mut input = Vec::with_capacity(1 + leaf_vec.len() + dyck_vec.len());
    input.push(leaf_vec.len() as u64);
    input.extend(leaf_vec);
    input.extend(dyck_vec);

    Ok(hash_belts_slice(&input))
}

pub fn hash_belts_list<A: NounAllocator>(
    alloc: &mut A,
    input: Noun,
    space: &NounSpace,
) -> Result<Noun, JetErr> {
    let mut input_vec = <Vec<Belt>>::from_noun(&input, space)?;
    let digest = hash_varlen(&mut input_vec);
    let res = digest.to_noun(alloc);
    Ok(res)
}

/// Pre-order dyck walk over a noun: `0` on entering a cell, `1` after its
/// head subtree, leaf atoms collected in encounter order.
///
/// Iterative and budget-bounded: network-decoded nouns can be arbitrarily
/// deep and structurally shared (a jam DAG of `k` physical nodes implies a
/// logical tree of up to `2^k`), so the walk must neither recurse on the
/// worker thread's stack nor expand an unbounded logical tree — the digest
/// is defined over the logical tree, so an oversized expansion is an error
/// rather than a truncated hash.
const DYCK_WALK_MAX_NODES: usize = 1 << 20;

fn dfs_dyck(
    root: Noun,
    leaf_vec: &mut Vec<u64>,
    dyck_vec: &mut Vec<u64>,
    space: &NounSpace,
) -> Result<(), JetErr> {
    enum Task {
        Walk(Noun),
        CloseHead,
    }

    let mut work = vec![Task::Walk(root)];
    let mut count = 0usize;
    while let Some(task) = work.pop() {
        count += 1;
        if count > DYCK_WALK_MAX_NODES {
            return Err(BAIL_FAIL);
        }
        match task {
            Task::Walk(current) => {
                if current.is_atom() {
                    leaf_vec.push(current.in_space(space).as_atom()?.as_u64()?);
                } else {
                    let current_cell = current.in_space(space).as_cell()?;
                    dyck_vec.push(0);
                    // LIFO: head subtree fully walks, then its closing `1`,
                    // then the tail — the same order the recursion produced.
                    work.push(Task::Walk(current_cell.tail().noun()));
                    work.push(Task::CloseHead);
                    work.push(Task::Walk(current_cell.head().noun()));
                }
            }
            Task::CloseHead => dyck_vec.push(1),
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use nockvm::mem::NockStack;
    use nockvm::noun::{D, T};

    use super::*;

    #[test]
    fn tip5_hash_varlen_public_vectors() {
        let mut empty = vec![];
        assert_eq!(
            hash_varlen(&mut empty),
            [
                11048995573592393898, 6655187932135147625, 8573492257662932655,
                4379820112787053727, 3881663824627898703,
            ]
        );

        let mut one = vec![Belt(2)];
        assert_eq!(
            hash_varlen(&mut one),
            [
                8342164316692288712, 12061287490523852513, 4038969618836824144,
                5830796451787599265, 468390350313364562,
            ]
        );

        let mut two = vec![Belt(5), Belt(26)];
        assert_eq!(
            hash_varlen(&mut two),
            [
                4045697570544439560, 13674194094340317530, 13743008867885290460,
                6020910684025273897, 3362765570390427021,
            ]
        );

        let mut ten = vec![
            Belt(1),
            Belt(2448),
            Belt(1),
            Belt(0),
            Belt(0),
            Belt(0),
            Belt(0),
            Belt(0),
            Belt(0),
            Belt(0),
        ];
        assert_eq!(
            hash_varlen(&mut ten),
            [
                12811986333282368874, 13601598673786067780, 3807788325936413287,
                5511165615113400862, 11490077061305916457,
            ]
        );
    }

    #[test]
    fn hash_noun_varlen_digest_walks_deep_chains_iteratively() {
        // GHSA-xjmj-ppvv-9hfj (latent crash component): a deep spine noun
        // (reachable via crafted gossip before validation) must digest
        // without overflowing the hashing thread's stack. Walk order is
        // pinned by `hash_noun_varlen_digest_uses_public_shape_encoding`.
        let depth = 200_000;
        let digest = std::thread::Builder::new()
            .stack_size(2 * 1024 * 1024)
            .spawn(move || {
                let mut stack = NockStack::new(nockvm::mem::NOCK_STACK_SIZE_SMALL, 0);
                let mut noun = D(0);
                for i in 0..depth {
                    noun = T(&mut stack, &[D(1 + (i % 63)), noun]);
                }
                let space = stack.noun_space();
                let mut arena = NockStack::new(nockvm::mem::NOCK_STACK_SIZE_SMALL, 0);
                hash_noun_varlen_digest(&mut arena, noun, &space)
                    .expect("deep chain digests iteratively")
            })
            .expect("spawn worker thread")
            .join()
            .expect("hash walk must not overflow the stack");
        assert_ne!(digest, [0, 0, 0, 0, 0]);
    }

    #[test]
    fn hash_noun_varlen_digest_rejects_dag_bomb_expansion() {
        // GHSA-g3rv-687v-p7h9: a shared DAG `a_k = [a_(k-1) a_(k-1)]` of a
        // few dozen physical nodes implies a 2^k-node logical tree. The
        // digest is defined over the logical expansion, so the walk is
        // budget-bounded and errors instead of expanding unboundedly.
        let mut stack = NockStack::new(nockvm::mem::NOCK_STACK_SIZE_SMALL, 0);
        let mut noun = D(0);
        for _ in 0..30 {
            let cell = nockvm::noun::Cell::new(&mut stack, noun, noun);
            noun = cell.as_noun();
        }
        let space = stack.noun_space();
        let mut arena = NockStack::new(nockvm::mem::NOCK_STACK_SIZE_SMALL, 0);
        assert!(
            hash_noun_varlen_digest(&mut arena, noun, &space).is_err(),
            "a 2^30-node logical expansion must be rejected at the budget"
        );

        // A small shared DAG still digests fine.
        let mut stack = NockStack::new(nockvm::mem::NOCK_STACK_SIZE_TINY, 0);
        let shared = T(&mut stack, &[D(7), D(7)]);
        let noun = T(&mut stack, &[shared, shared]);
        let space = stack.noun_space();
        let mut arena = NockStack::new(nockvm::mem::NOCK_STACK_SIZE_TINY, 0);
        assert!(hash_noun_varlen_digest(&mut arena, noun, &space).is_ok());
    }

    #[test]
    fn hash_noun_varlen_digest_uses_public_shape_encoding() {
        let stack = &mut NockStack::new(1 << 20, 0);
        let noun = T(stack, &[D(2), D(5), D(0)]);
        let space = stack.noun_space();

        assert_eq!(
            hash_noun_varlen_digest(stack, noun, &space).expect("hash noun"),
            hash_belts_slice(&[3, 2, 5, 0, 0, 1, 0, 1])
        );
    }
}

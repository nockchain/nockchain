use nockchain_math::belt::Belt;
use nockchain_math::owned_based_noun::{hash_owned_based_noun_varlen, OwnedBasedNoun};
use nockchain_math::tip5;
use nockchain_math::zoon::zset::ZSetHasher;

use crate::tx_engine::common::Hash;

mod noun;
pub use noun::{bpoly_to_list, hash_hashable};

/// Errors raised by the tip5 hash-hashable jet wrapper.
#[derive(Debug, thiserror::Error)]
pub enum HashableJetError {
    #[error("hash-hashable jet failed: {0}")]
    Jet(String),
    #[error("hash-hashable digest decode failed: {0}")]
    DigestDecode(String),
}

/// Errors raised while encoding direct hashable digests.
#[derive(Debug, thiserror::Error)]
pub enum HashableEncodingError {
    #[error("leaf atom is not based: {0}")]
    LeafAtomNotBased(u64),
}

/// Computes the consensus hashable digest directly, without materializing an
/// allocator-backed noun tree first.
pub trait Hashable {
    type Error;

    fn hash_digest(&self) -> Result<Hash, Self::Error>;
}

/// Generic digest entrypoint for all handwritten `Hashable` values.
pub fn hash_hashable_value<T>(value: &T) -> Result<[u64; 5], T::Error>
where
    T: Hashable,
{
    Ok(value.hash_digest()?.to_array())
}

/// Hashes a `%leaf` whose payload is a single based atom.
pub fn hash_leaf_atom(atom: u64) -> Result<Hash, HashableEncodingError> {
    let belt = Belt::try_from(&atom).map_err(|_| HashableEncodingError::LeafAtomNotBased(atom))?;
    Ok(hash_leaf_belt(belt))
}

/// Hashes a `%leaf` whose payload is a single based field element.
/// This is the allocator-free closed form of `hash_noun_varlen` for an atom:
/// `leaf-sequence(atom) = [atom]`, `dyck(atom) = ~`, so the hashed belt list is
/// exactly `[1 atom]`.
pub fn hash_leaf_belt(belt: Belt) -> Hash {
    let mut input = vec![Belt(1), belt];
    Hash::from_limbs(&tip5::hash::hash_varlen(&mut input))
}

/// Hashes the canonical null leaf `%leaf ~`.
pub fn hash_leaf_null() -> Hash {
    hash_leaf_belt(Belt(0))
}

/// Hashes a generic hashable pair by hashing the 10 child digest limbs.
pub fn hash_pair(left: &Hash, right: &Hash) -> Hash {
    let mut input = left
        .to_array()
        .into_iter()
        .chain(right.to_array())
        .map(Belt)
        .collect::<Vec<_>>();
    Hash::from_limbs(&tip5::hash::hash_10(&mut input))
}

/// Hashes a hashable `(unit noun)` where the payload is a based atom.
pub fn hash_unit_belt(value: Option<Belt>) -> Hash {
    match value {
        None => hash_leaf_null(),
        Some(value) => {
            let none_leaf = hash_leaf_null();
            let value_leaf = hash_leaf_belt(value);
            hash_pair(&none_leaf, &value_leaf)
        }
    }
}

/// Hashes a `%list` payload from its already-hashed items.
pub fn hash_list(items: &[Hash]) -> Hash {
    let noun = OwnedBasedNoun::list(
        items
            .iter()
            .map(|item| {
                OwnedBasedNoun::tuple_atoms(&item.to_array())
                    .expect("hash digest limbs are always valid based atoms")
            })
            .collect(),
    );
    Hash::from_limbs(&hash_owned_based_noun_varlen(&noun))
}

/// Hashes a `%mary` payload from its step, original array length, and
/// step-normalized belt payload.
pub fn hash_mary(
    step: u64,
    array_len: u64,
    normalized_belts: &[Belt],
) -> Result<Hash, HashableEncodingError> {
    let step_hash = hash_leaf_atom(step)?;
    let len_hash = hash_leaf_atom(array_len)?;
    let mut belt_list = normalized_belts.to_vec();
    let belts_hash = Hash::from_limbs(&tip5::hash::hash_varlen(&mut belt_list));
    Ok(hash_pair(&step_hash, &hash_pair(&len_hash, &belts_hash)))
}

#[derive(Debug, Clone, Copy, Default)]
pub struct HashableTreeHasher;

impl ZSetHasher<Hash> for HashableTreeHasher {
    type Output = Hash;

    fn empty(&self) -> Self::Output {
        hash_leaf_null()
    }

    fn node(&self, digest: &Hash, left: Self::Output, right: Self::Output) -> Self::Output {
        let branches = hash_pair(&left, &right);
        hash_pair(digest, &branches)
    }
}

#[cfg(test)]
mod tests {
    use nockapp::noun::slab::{NockJammer, NounSlab};
    use nockchain_math::belt::{Belt, PRIME};
    use nockchain_math::noun_ext::NounMathExt;
    use nockchain_math::poly::BPolyVec;
    use nockchain_math::shape::do_leaf_sequence;
    use nockchain_math::structs::HoonList;
    use nockchain_math::tip5;
    use nockchain_math::zoon::zset::ZSet;
    use nockvm::ext::make_tas;
    use nockvm::mem::NockStack;
    use nockvm::noun::{Atom, Noun, NounAllocator, D, T};
    use nockvm_macros::tas;
    use noun_serde::{NounDecode, NounEncode};

    use super::{
        bpoly_to_list, hash_hashable, hash_hashable_value, hash_leaf_atom, hash_list, hash_mary,
        hash_pair, Hashable, HashableEncodingError, HashableTreeHasher,
    };
    use crate::tx_engine::common::Hash;

    fn manual_hash_ten_cell<A: NounAllocator>(allocator: &mut A, ten_cell: Noun) -> Noun {
        let mut leaf = Vec::<u64>::new();
        do_leaf_sequence(ten_cell, &mut leaf).expect("manual ten-cell leaf sequence should work");
        let mut leaf_belt = leaf.into_iter().map(Belt).collect();
        tip5::hash::hash_10(&mut leaf_belt).to_noun(allocator)
    }

    fn manual_hash_hashable<A: NounAllocator>(allocator: &mut A, noun: Noun) -> Noun {
        let cell = noun.as_cell().expect("hashable noun must be a cell");
        let head = cell.head();
        let tail = cell.tail();

        if let Ok(tag) = head.as_direct() {
            match tag.data() {
                tas!(b"hash") => return tail,
                tas!(b"leaf") => {
                    return tip5::hash::hash_noun_varlen(allocator, tail)
                        .expect("manual leaf hashing should succeed");
                }
                tas!(b"list") => {
                    let mut hashed_list = D(0);
                    let items = HoonList::try_from(tail).expect("manual list payload must decode");
                    let hashed_items = items
                        .into_iter()
                        .map(|item| manual_hash_hashable(allocator, item))
                        .collect::<Vec<_>>();
                    for item in hashed_items.into_iter().rev() {
                        hashed_list = T(allocator, &[item, hashed_list]);
                    }
                    return tip5::hash::hash_noun_varlen(allocator, hashed_list)
                        .expect("manual list hashing should succeed");
                }
                _ => {}
            }
        }

        let left = manual_hash_hashable(allocator, head);
        let right = manual_hash_hashable(allocator, tail);
        let pair = T(allocator, &[left, right]);
        manual_hash_ten_cell(allocator, pair)
    }

    fn digest_hash(noun: Noun) -> Hash {
        Hash::from_noun(&noun).expect("digest noun should decode")
    }

    fn tagged_hashable<A: NounAllocator>(allocator: &mut A, tag: &str, payload: Noun) -> Noun {
        let tag = make_tas(allocator, tag).as_noun();
        T(allocator, &[tag, payload])
    }

    fn zset_noun_hashable<A: NounAllocator>(allocator: &mut A, noun: Noun) -> Noun {
        if unsafe { noun.raw_equals(&D(0)) } {
            return tagged_hashable(allocator, "leaf", D(0));
        }

        let [digest, left, right] = noun.uncell().expect("z-set tree must be a node");
        let hash_node = tagged_hashable(allocator, "hash", digest);
        let left = zset_noun_hashable(allocator, left);
        let right = zset_noun_hashable(allocator, right);
        T(allocator, &[hash_node, left, right])
    }

    struct TestLeafHashable(u64);

    impl Hashable for TestLeafHashable {
        type Error = HashableEncodingError;

        fn hash_digest(&self) -> Result<Hash, Self::Error> {
            hash_leaf_atom(self.0)
        }
    }

    #[test]
    fn bpoly_to_list_encodes_large_belt_as_atom() {
        let mut stack = NockStack::new(8 << 10 << 10, 0);
        let len_noun = Atom::new(&mut stack, 1).as_noun();
        let belt_noun = Atom::new(&mut stack, PRIME - 1).as_noun();
        let sam = T(&mut stack, &[len_noun, belt_noun]);

        let list = bpoly_to_list(&mut stack, sam).expect("bpoly list conversion should succeed");
        let cell = list.as_cell().expect("expected non-empty list");
        let head = cell.head().as_atom().expect("expected atom head");
        let value = head.as_u64().expect("expected u64 atom");

        assert_eq!(value, PRIME - 1);
        assert!(unsafe { cell.tail().raw_equals(&D(0)) });
    }

    #[test]
    fn hash_hashable_leaf_matches_manual_spec() {
        let mut slab: NounSlab<NockJammer> = NounSlab::new();
        let hashable = tagged_hashable(&mut slab, "leaf", D(42));
        let actual = hash_hashable(&mut slab, hashable).expect("leaf hash should succeed");

        let mut expected_slab: NounSlab<NockJammer> = NounSlab::new();
        let expected_hashable = tagged_hashable(&mut expected_slab, "leaf", D(42));
        let expected = manual_hash_hashable(&mut expected_slab, expected_hashable);

        assert_eq!(digest_hash(actual), digest_hash(expected));
    }

    #[test]
    fn hash_hashable_hash_passthrough_matches_manual_spec() {
        let expected_hash = Hash::from_limbs(&[7, 11, 13, 17, 19]);

        let mut slab: NounSlab<NockJammer> = NounSlab::new();
        let payload = expected_hash.to_noun(&mut slab);
        let hashable = tagged_hashable(&mut slab, "hash", payload);
        let actual = hash_hashable(&mut slab, hashable).expect("hash passthrough should succeed");

        assert_eq!(digest_hash(actual), expected_hash);
    }

    #[test]
    fn hash_hashable_pair_recursion_matches_manual_spec() {
        let mut slab: NounSlab<NockJammer> = NounSlab::new();
        let left = tagged_hashable(&mut slab, "leaf", D(3));
        let right = tagged_hashable(&mut slab, "leaf", D(9));
        let hashable = T(&mut slab, &[left, right]);
        let actual = hash_hashable(&mut slab, hashable).expect("pair hash should succeed");

        let mut expected_slab: NounSlab<NockJammer> = NounSlab::new();
        let left = tagged_hashable(&mut expected_slab, "leaf", D(3));
        let right = tagged_hashable(&mut expected_slab, "leaf", D(9));
        let expected_hashable = T(&mut expected_slab, &[left, right]);
        let expected = manual_hash_hashable(&mut expected_slab, expected_hashable);

        assert_eq!(digest_hash(actual), digest_hash(expected));
    }

    #[test]
    fn hash_hashable_list_matches_manual_spec() {
        let mut slab: NounSlab<NockJammer> = NounSlab::new();
        let item_one = tagged_hashable(&mut slab, "leaf", D(1));
        let item_two = tagged_hashable(&mut slab, "leaf", D(2));
        let list_tail = T(&mut slab, &[item_two, D(0)]);
        let list_payload = T(&mut slab, &[item_one, list_tail]);
        let hashable = tagged_hashable(&mut slab, "list", list_payload);
        let actual = hash_hashable(&mut slab, hashable).expect("list hash should succeed");

        let mut expected_slab: NounSlab<NockJammer> = NounSlab::new();
        let item_one = tagged_hashable(&mut expected_slab, "leaf", D(1));
        let item_two = tagged_hashable(&mut expected_slab, "leaf", D(2));
        let list_tail = T(&mut expected_slab, &[item_two, D(0)]);
        let list_payload = T(&mut expected_slab, &[item_one, list_tail]);
        let expected_hashable = tagged_hashable(&mut expected_slab, "list", list_payload);
        let expected = manual_hash_hashable(&mut expected_slab, expected_hashable);

        assert_eq!(digest_hash(actual), digest_hash(expected));
    }

    #[test]
    fn direct_hashable_helper_matches_manual_leaf_hash() {
        let actual =
            hash_hashable_value(&TestLeafHashable(42)).expect("direct helper should hash test");

        let mut expected_slab: NounSlab<NockJammer> = NounSlab::new();
        let expected_hashable = tagged_hashable(&mut expected_slab, "leaf", D(42));
        let expected = manual_hash_hashable(&mut expected_slab, expected_hashable);

        assert_eq!(Hash::from_limbs(&actual), digest_hash(expected));
    }

    #[test]
    fn direct_hash_set_matches_hashable_tree_semantics() {
        let hashes = vec![
            Hash::from_limbs(&[1, 2, 3, 4, 5]),
            Hash::from_limbs(&[7, 11, 13, 17, 19]),
            Hash::from_limbs(&[23, 29, 31, 37, 41]),
        ];
        let zset = ZSet::try_from_items(hashes).expect("hash z-set should build");

        let actual = zset.hash_with(&HashableTreeHasher);

        let mut slab: NounSlab<NockJammer> = NounSlab::new();
        let zset_noun = zset.to_noun(&mut slab);
        let hashable = zset_noun_hashable(&mut slab, zset_noun);
        let expected = hash_hashable(&mut slab, hashable).expect("noun helper should hash");

        assert_eq!(actual, digest_hash(expected));
    }

    #[test]
    fn direct_hash_list_matches_noun_hashable() {
        let items =
            vec![hash_leaf_atom(1).expect("based leaf"), hash_leaf_atom(2).expect("based leaf")];
        let actual = hash_list(&items);

        let mut slab: NounSlab<NockJammer> = NounSlab::new();
        let item_one = tagged_hashable(&mut slab, "leaf", D(1));
        let item_two = tagged_hashable(&mut slab, "leaf", D(2));
        let list_tail = T(&mut slab, &[item_two, D(0)]);
        let list_payload = T(&mut slab, &[item_one, list_tail]);
        let hashable = tagged_hashable(&mut slab, "list", list_payload);
        let expected = hash_hashable(&mut slab, hashable).expect("noun helper should hash");

        assert_eq!(actual, digest_hash(expected));
    }

    #[test]
    fn direct_hash_mary_matches_noun_hashable() {
        let bpoly = BPolyVec::from(vec![2, 4, 8]);
        let actual = hash_mary(1, 3, &bpoly.0).expect("direct mary helper should hash");

        let mut slab: NounSlab<NockJammer> = NounSlab::new();
        let array = bpoly.to_noun(&mut slab);
        let payload = T(&mut slab, &[D(1), array]);
        let hashable = tagged_hashable(&mut slab, "mary", payload);
        let expected = hash_hashable(&mut slab, hashable).expect("noun helper should hash");

        assert_eq!(actual, digest_hash(expected));
    }

    #[test]
    fn hash_pair_matches_manual_ten_cell() {
        let left = Hash::from_limbs(&[2, 3, 5, 7, 11]);
        let right = Hash::from_limbs(&[13, 17, 19, 23, 29]);
        let actual = hash_pair(&left, &right);

        let mut slab: NounSlab<NockJammer> = NounSlab::new();
        let left_noun = left.to_noun(&mut slab);
        let right_noun = right.to_noun(&mut slab);
        let pair = T(&mut slab, &[left_noun, right_noun]);
        let expected = manual_hash_ten_cell(&mut slab, pair);

        assert_eq!(actual, digest_hash(expected));
    }
}

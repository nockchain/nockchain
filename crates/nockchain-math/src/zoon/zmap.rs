use nockvm::interpreter::Context;
use nockvm::jets::util::slot;
use nockvm::jets::JetErr;
use nockvm::noun::{Noun, NounAllocator, D, T};
use nockvm::site::{site_slam, Site};
use noun_serde::{NounDecode, NounDecodeError, NounEncode};

use super::common::*;
use crate::noun_ext::NounMathExt;

pub trait ZMapHasher<K, V> {
    type Output: Clone;

    fn empty(&self) -> Self::Output;
    fn node(&self, key: &K, value: &V, left: Self::Output, right: Self::Output) -> Self::Output;
}

pub fn z_map_put<A: NounAllocator, H: TipHasher>(
    stack: &mut A,
    a: &Noun,
    b: &mut Noun,
    c: &mut Noun,
    hasher: &H,
) -> Result<Noun, JetErr> {
    if unsafe { a.raw_equals(&D(0)) } {
        let kv = T(stack, &[*b, *c]);
        Ok(T(stack, &[kv, D(0), D(0)]))
    } else {
        let [mut an, al, ar] = a.uncell()?;
        let [mut anp, mut anq] = an.uncell()?;
        if unsafe { stack.equals(b, &mut anp) } {
            if unsafe { stack.equals(c, &mut anq) } {
                return Ok(*a);
            }
            an = T(stack, &[*b, *c]);
            let anbc = T(stack, &[an, al, ar]);
            Ok(anbc)
        } else if gor_tip(stack, b, &mut anp, hasher)? {
            let d = z_map_put(stack, &al, b, c, hasher)?;
            let [dn, dl, dr] = d.uncell()?;
            let [mut dnp, _dnq] = dn.uncell()?;
            if mor_tip(stack, &mut anp, &mut dnp, hasher)? {
                Ok(T(stack, &[an, d, ar]))
            } else {
                let new_a = T(stack, &[an, dr, ar]);
                Ok(T(stack, &[dn, dl, new_a]))
            }
        } else {
            let d = z_map_put(stack, &ar, b, c, hasher)?;
            let [dn, dl, dr] = d.uncell()?;
            let [mut dnp, _dnq] = dn.uncell()?;
            if mor_tip(stack, &mut anp, &mut dnp, hasher)? {
                Ok(T(stack, &[an, al, d]))
            } else {
                let new_a = T(stack, &[an, al, dl]);
                Ok(T(stack, &[dn, new_a, dr]))
            }
        }
    }
}

/// Reduce a z-map using the gate's cached `Site`, mirroring Hoon `++rep`.
pub fn z_map_rep(context: &mut Context, map: &Noun, gate: &mut Noun) -> Result<Noun, JetErr> {
    let prod = slot(*gate, 13)?;
    let site = Site::new(context, gate);
    let mut reducer = |node: Noun, acc: Noun| -> Result<Noun, JetErr> {
        let sam = T(&mut context.stack, &[node, acc]);
        site_slam(context, &site, sam)
    };
    rep_fold(*map, prod, &mut reducer)
}

fn rep_fold<F>(tree: Noun, acc: Noun, reducer: &mut F) -> Result<Noun, JetErr>
where
    F: FnMut(Noun, Noun) -> Result<Noun, JetErr>,
{
    if unsafe { tree.raw_equals(&D(0)) } {
        return Ok(acc);
    }

    let [entry, left, right] = tree.uncell()?;
    let acc = reducer(entry, acc)?;
    let acc = rep_fold(left, acc, reducer)?;
    rep_fold(right, acc, reducer)
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ZMapEntry<K, V> {
    key: K,
    value: V,
    ordered_key: OrderedNoun,
}

impl<K: NounEncode, V> ZMapEntry<K, V> {
    fn encode(key: K, value: V) -> Result<Self, OwnedZoonError> {
        let ordered_key = OrderedNoun::encode(&key)?;
        Ok(Self {
            key,
            value,
            ordered_key,
        })
    }
}

impl<K, V> ZTreeValue for ZMapEntry<K, V> {
    type Output = (K, V);

    fn ordered(&self) -> &OrderedNoun {
        &self.ordered_key
    }

    fn same_slot(&self, other: &Self) -> bool {
        self.ordered_key.noun == other.ordered_key.noun
    }

    fn merge_duplicate(existing: Self, incoming: Self) -> Self {
        Self {
            key: incoming.key,
            value: incoming.value,
            ordered_key: existing.ordered_key,
        }
    }

    fn into_output(self) -> Self::Output {
        (self.key, self.value)
    }
}

impl<K: NounEncode, V: NounEncode> ZTreeEncode for ZMapEntry<K, V> {
    fn encode_payload<A: NounAllocator>(&self, allocator: &mut A) -> Noun {
        let key = self.key.to_noun(allocator);
        let value = self.value.to_noun(allocator);
        T(allocator, &[key, value])
    }
}

impl<K: NounDecode, V: NounDecode> ZTreeDecode for ZMapEntry<K, V> {
    const KIND: &'static str = "z-map";

    fn decode_payload(noun: &Noun) -> Result<Self, NounDecodeError> {
        let entry_cell = noun
            .as_cell()
            .map_err(|_| NounDecodeError::Custom("z-map entry must be a pair".into()))?;
        let raw_key = entry_cell.head();
        let raw_value = entry_cell.tail();
        let key = K::from_noun(&raw_key)?;
        let value = V::from_noun(&raw_value)?;
        let ordered_key = OrderedNoun::from_noun(raw_key).map_err(owned_zoon_decode_error)?;
        Ok(Self {
            key,
            value,
            ordered_key,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ZMap<K, V> {
    tree: ZTree<ZMapEntry<K, V>>,
}

impl<K, V> Default for ZMap<K, V> {
    fn default() -> Self {
        Self { tree: ZTree::Empty }
    }
}

impl<K, V> ZMap<K, V> {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn is_empty(&self) -> bool {
        matches!(self.tree, ZTree::Empty)
    }

    pub fn into_entries(self) -> Vec<(K, V)> {
        let mut out = Vec::new();
        self.tree.into_outputs(&mut out);
        out
    }

    pub fn fold_tree<R, F>(&self, empty: R, node: F) -> R
    where
        R: Clone,
        F: Fn(&K, &V, R, R) -> R,
    {
        self.tree.fold(&empty, &|payload, left, right| {
            node(&payload.key, &payload.value, left, right)
        })
    }

    pub fn hash_with<H>(&self, hasher: &H) -> H::Output
    where
        H: ZMapHasher<K, V>,
    {
        self.fold_tree(hasher.empty(), |key, value, left, right| {
            hasher.node(key, value, left, right)
        })
    }
}

impl<K: NounEncode, V> ZMap<K, V> {
    pub fn try_insert(&mut self, key: K, value: V) -> Result<bool, OwnedZoonError> {
        let candidate = ZMapEntry::encode(key, value)?;
        let (tree, added) = std::mem::take(&mut self.tree).insert(candidate);
        self.tree = tree;
        Ok(added)
    }

    pub fn try_from_entries<I>(entries: I) -> Result<Self, OwnedZoonError>
    where
        I: IntoIterator<Item = (K, V)>,
    {
        let mut map = Self::new();
        for (key, value) in entries {
            map.try_insert(key, value)?;
        }
        Ok(map)
    }
}

impl<K: NounEncode, V: NounEncode> NounEncode for ZMap<K, V> {
    fn to_noun<A: NounAllocator>(&self, allocator: &mut A) -> Noun {
        self.tree.to_noun(allocator)
    }
}

impl<K: NounDecode, V: NounDecode> NounDecode for ZMap<K, V> {
    fn from_noun(noun: &Noun) -> Result<Self, NounDecodeError> {
        Ok(Self {
            tree: ZTree::from_noun(noun)?,
        })
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use nockvm::mem::NockStack;
    use nockvm::noun::{Atom, Noun, NounAllocator, D, T};
    use noun_serde::{NounDecode, NounEncode};
    use quickcheck::QuickCheck;

    use super::{z_map_put, DefaultTipHasher, ZMap, ZMapHasher};
    use crate::belt::PRIME;
    use crate::zoon::common::test_support::BoundedTreeValue;

    struct DebugMapHasher;

    impl ZMapHasher<BoundedTreeValue, BoundedTreeValue> for DebugMapHasher {
        type Output = String;

        fn empty(&self) -> Self::Output {
            "~".into()
        }

        fn node(
            &self,
            key: &BoundedTreeValue,
            value: &BoundedTreeValue,
            left: Self::Output,
            right: Self::Output,
        ) -> Self::Output {
            format!("{key:?}=>{value:?}<{left}|{right}>")
        }
    }

    fn bounded_entries(
        entries: Vec<(BoundedTreeValue, BoundedTreeValue)>,
    ) -> Vec<(BoundedTreeValue, BoundedTreeValue)> {
        entries.into_iter().take(12).collect()
    }

    fn last_write_wins(
        entries: Vec<(BoundedTreeValue, BoundedTreeValue)>,
    ) -> Vec<(BoundedTreeValue, BoundedTreeValue)> {
        let mut deduped = BTreeMap::new();
        for (key, value) in entries {
            deduped.insert(key, value);
        }
        deduped.into_iter().collect()
    }

    fn nouns_equal(stack: &mut NockStack, left: Noun, right: Noun) -> bool {
        let mut left = left;
        let mut right = right;
        unsafe { stack.equals(&mut left, &mut right) }
    }

    #[test]
    fn owned_zmap_matches_raw_zmap_encoding() {
        let entries = vec![(7u64, 70u64), (3u64, 30u64), (11u64, 110u64)];
        let mut stack = NockStack::new(8 << 10 << 10, 0);
        let raw = entries.iter().fold(D(0), |acc, (key, value): &(u64, u64)| {
            let mut key = key.to_noun(&mut stack);
            let mut value = value.to_noun(&mut stack);
            z_map_put(&mut stack, &acc, &mut key, &mut value, &DefaultTipHasher)
                .expect("raw z-map put")
        });

        let owned = ZMap::try_from_entries(entries).expect("owned z-map should build");
        let mut owned_noun = owned.to_noun(&mut stack);
        let mut raw = raw;
        assert!(unsafe { stack.equals(&mut raw, &mut owned_noun) });
    }

    #[test]
    fn owned_zmap_updates_duplicate_keys() {
        let map = ZMap::try_from_entries(vec![(1u64, 10u64), (1u64, 11u64)])
            .expect("owned z-map should build");
        assert_eq!(map.into_entries(), vec![(1u64, 11u64)]);
    }

    #[test]
    fn owned_zmap_roundtrips_from_noun() {
        let mut stack = NockStack::new(8 << 10 << 10, 0);
        let original =
            ZMap::try_from_entries(vec![(9u64, 90u64), (2u64, 20u64)]).expect("build z-map");
        let noun = original.to_noun(&mut stack);
        let decoded = ZMap::<u64, u64>::from_noun(&noun).expect("decode z-map");
        let mut roundtrip = decoded.to_noun(&mut stack);
        let mut noun = noun;
        assert!(unsafe { stack.equals(&mut noun, &mut roundtrip) });
    }

    #[test]
    fn quickcheck_owned_zmap_matches_raw_zmap_for_bounded_nouns() {
        fn prop(entries: Vec<(BoundedTreeValue, BoundedTreeValue)>) -> bool {
            let entries = bounded_entries(entries);
            let mut stack = NockStack::new(8 << 10 << 10, 0);
            let raw = entries.iter().fold(D(0), |acc, (key, value)| {
                let mut key = key.to_noun(&mut stack);
                let mut value = value.to_noun(&mut stack);
                z_map_put(&mut stack, &acc, &mut key, &mut value, &DefaultTipHasher)
                    .expect("raw z-map put")
            });

            let owned = ZMap::try_from_entries(entries).expect("owned z-map should build");
            let owned_noun = owned.to_noun(&mut stack);
            nouns_equal(&mut stack, raw, owned_noun)
        }

        QuickCheck::new()
            .tests(256)
            .quickcheck(prop as fn(Vec<(BoundedTreeValue, BoundedTreeValue)>) -> bool);
    }

    #[test]
    fn quickcheck_owned_zmap_is_insertion_order_independent_after_dedup() {
        fn prop(entries: Vec<(BoundedTreeValue, BoundedTreeValue)>) -> bool {
            let unique = last_write_wins(bounded_entries(entries));
            let mut reversed = unique.clone();
            reversed.reverse();

            let left = ZMap::try_from_entries(unique).expect("left z-map should build");
            let right = ZMap::try_from_entries(reversed).expect("right z-map should build");
            let mut stack = NockStack::new(8 << 10 << 10, 0);
            let left_noun = left.to_noun(&mut stack);
            let right_noun = right.to_noun(&mut stack);
            nouns_equal(&mut stack, left_noun, right_noun)
        }

        QuickCheck::new()
            .tests(256)
            .quickcheck(prop as fn(Vec<(BoundedTreeValue, BoundedTreeValue)>) -> bool);
    }

    #[test]
    fn quickcheck_owned_zmap_uses_last_write_wins_for_duplicate_keys() {
        fn prop(entries: Vec<(BoundedTreeValue, BoundedTreeValue)>) -> bool {
            let entries = bounded_entries(entries);
            let canonical_entries = last_write_wins(entries.clone());

            let with_duplicates =
                ZMap::try_from_entries(entries).expect("z-map with duplicates should build");
            let canonical =
                ZMap::try_from_entries(canonical_entries).expect("canonical z-map should build");
            let mut stack = NockStack::new(8 << 10 << 10, 0);
            let with_duplicates_noun = with_duplicates.to_noun(&mut stack);
            let canonical_noun = canonical.to_noun(&mut stack);
            nouns_equal(&mut stack, with_duplicates_noun, canonical_noun)
        }

        QuickCheck::new()
            .tests(256)
            .quickcheck(prop as fn(Vec<(BoundedTreeValue, BoundedTreeValue)>) -> bool);
    }

    #[test]
    fn zmap_hash_with_is_insertion_order_independent_after_dedup() {
        let left = ZMap::try_from_entries(vec![
            (BoundedTreeValue::Atom(1), BoundedTreeValue::Atom(10)),
            (BoundedTreeValue::Atom(7), BoundedTreeValue::Atom(70)),
            (BoundedTreeValue::Atom(3), BoundedTreeValue::Atom(30)),
        ])
        .expect("left z-map should build");
        let right = ZMap::try_from_entries(vec![
            (BoundedTreeValue::Atom(3), BoundedTreeValue::Atom(30)),
            (BoundedTreeValue::Atom(1), BoundedTreeValue::Atom(10)),
            (BoundedTreeValue::Atom(7), BoundedTreeValue::Atom(70)),
        ])
        .expect("right z-map should build");

        assert_eq!(
            left.hash_with(&DebugMapHasher),
            right.hash_with(&DebugMapHasher)
        );
    }

    #[test]
    fn zmap_hash_with_uses_last_write_wins() {
        let with_duplicates = ZMap::try_from_entries(vec![
            (BoundedTreeValue::Atom(1), BoundedTreeValue::Atom(10)),
            (BoundedTreeValue::Atom(1), BoundedTreeValue::Atom(11)),
            (BoundedTreeValue::Atom(7), BoundedTreeValue::Atom(70)),
        ])
        .expect("z-map with duplicates should build");
        let canonical = ZMap::try_from_entries(vec![
            (BoundedTreeValue::Atom(1), BoundedTreeValue::Atom(11)),
            (BoundedTreeValue::Atom(7), BoundedTreeValue::Atom(70)),
        ])
        .expect("canonical z-map should build");

        assert_eq!(
            with_duplicates.hash_with(&DebugMapHasher),
            canonical.hash_with(&DebugMapHasher)
        );
    }

    #[test]
    fn zmap_decode_rejects_nonzero_atom() {
        assert!(ZMap::<u64, u64>::from_noun(&D(7)).is_err());
    }

    #[test]
    fn zmap_decode_rejects_non_pair_entry() {
        let mut stack = NockStack::new(8 << 10 << 10, 0);
        let malformed = T(&mut stack, &[D(1), D(0), D(0)]);
        assert!(ZMap::<u64, u64>::from_noun(&malformed).is_err());
    }

    #[test]
    fn zmap_decode_rejects_non_cell_branches() {
        let mut stack = NockStack::new(8 << 10 << 10, 0);
        let entry = T(&mut stack, &[D(1), D(2)]);
        let malformed = T(&mut stack, &[entry, D(0)]);
        assert!(ZMap::<u64, u64>::from_noun(&malformed).is_err());
    }

    #[test]
    fn zmap_decode_rejects_invalid_key_payload() {
        let mut stack = NockStack::new(8 << 10 << 10, 0);
        let key = T(&mut stack, &[D(1), D(2)]);
        let entry = T(&mut stack, &[key, D(3)]);
        let malformed = T(&mut stack, &[entry, D(0), D(0)]);
        assert!(ZMap::<u64, u64>::from_noun(&malformed).is_err());
    }

    #[test]
    fn zmap_decode_rejects_invalid_value_payload() {
        let mut stack = NockStack::new(8 << 10 << 10, 0);
        let value = T(&mut stack, &[D(1), D(2)]);
        let entry = T(&mut stack, &[D(3), value]);
        let malformed = T(&mut stack, &[entry, D(0), D(0)]);
        assert!(ZMap::<u64, u64>::from_noun(&malformed).is_err());
    }

    #[test]
    fn zmap_decode_rejects_non_based_atoms() {
        let mut stack = NockStack::new(8 << 10 << 10, 0);
        let key = Atom::new(&mut stack, PRIME).as_noun();
        let entry = T(&mut stack, &[key, D(3)]);
        let malformed = T(&mut stack, &[entry, D(0), D(0)]);
        assert!(ZMap::<u64, u64>::from_noun(&malformed).is_err());
    }

    #[test]
    fn zmap_try_from_entries_rejects_non_based_atoms() {
        assert!(ZMap::try_from_entries(vec![(PRIME, 1u64)]).is_err());
    }
}

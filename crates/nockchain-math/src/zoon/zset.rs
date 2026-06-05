use nockvm::jets::JetErr;
use nockvm::noun::{Noun, NounAllocator, D, T};
use noun_serde::{NounDecode, NounDecodeError, NounEncode};

use super::common::*;
use crate::noun_ext::NounMathExt;

pub trait ZSetHasher<T> {
    type Output: Clone;

    fn empty(&self) -> Self::Output;
    fn node(&self, value: &T, left: Self::Output, right: Self::Output) -> Self::Output;
}

pub fn z_set_put<A: NounAllocator, H: TipHasher>(
    stack: &mut A,
    a: &Noun,
    b: &mut Noun,
    hasher: &H,
) -> Result<Noun, JetErr> {
    if unsafe { a.raw_equals(&D(0)) } {
        Ok(T(stack, &[*b, D(0), D(0)]))
    } else {
        let [mut an, al, ar] = a.uncell()?;
        if unsafe { stack.equals(b, &mut an) } {
            Ok(*a)
        } else if gor_tip(stack, b, &mut an, hasher)? {
            let c = z_set_put(stack, &al, b, hasher)?;
            let [mut cn, cl, cr] = c.uncell()?;
            if mor_tip(stack, &mut an, &mut cn, hasher)? {
                Ok(T(stack, &[an, c, ar]))
            } else {
                let new_a = T(stack, &[an, cr, ar]);
                Ok(T(stack, &[cn, cl, new_a]))
            }
        } else {
            let c = z_set_put(stack, &ar, b, hasher)?;
            let [mut cn, cl, cr] = c.uncell()?;
            if mor_tip(stack, &mut an, &mut cn, hasher)? {
                Ok(T(stack, &[an, al, c]))
            } else {
                let new_a = T(stack, &[an, al, cl]);
                Ok(T(stack, &[cn, new_a, cr]))
            }
        }
    }
}

pub fn z_set_bif<A: NounAllocator, H: TipHasher>(
    stack: &mut A,
    a: &mut Noun,
    b: &mut Noun,
    hasher: &H,
) -> Result<Noun, JetErr> {
    fn do_bif<A: NounAllocator, H: TipHasher>(
        stack: &mut A,
        a: &mut Noun,
        b: &mut Noun,
        hasher: &H,
    ) -> Result<Noun, JetErr> {
        if unsafe { a.raw_equals(&D(0)) } {
            Ok(T(stack, &[*b, D(0), D(0)]))
        } else {
            let [mut n, mut l, mut r] = a.uncell()?;
            if unsafe { stack.equals(b, &mut n) } {
                Ok(*a)
            } else if gor_tip(stack, b, &mut n, hasher)? {
                // could also parameterize Hasher if needed
                let c = do_bif(stack, &mut l, b, hasher)?;
                let [cn, cl, cr] = c.uncell()?;
                let new_a = T(stack, &[n, cr, r]);
                Ok(T(stack, &[cn, cl, new_a]))
            } else {
                let c = do_bif(stack, &mut r, b, hasher)?;
                let [cn, cl, cr] = c.uncell()?;
                let new_a = T(stack, &[n, l, cl]);
                Ok(T(stack, &[cn, new_a, cr]))
            }
        }
    }
    let res = do_bif(stack, a, b, hasher)?;
    Ok(res.as_cell()?.tail())
}

pub fn z_set_dif<A: NounAllocator, H: TipHasher>(
    stack: &mut A,
    a: &mut Noun,
    b: &mut Noun,
    hasher: &H,
) -> Result<Noun, JetErr> {
    fn dif_helper<A: NounAllocator, H: TipHasher>(
        stack: &mut A,
        d: &mut Noun,
        e: &mut Noun,
        hasher: &H,
    ) -> Result<Noun, JetErr> {
        if unsafe { d.raw_equals(&D(0)) } {
            Ok(*e)
        } else if unsafe { e.raw_equals(&D(0)) } {
            Ok(*d)
        } else {
            let [mut dn, dl, mut dr] = d.uncell()?;
            let [mut en, mut el, er] = e.uncell()?;
            if mor_tip(stack, &mut dn, &mut en, hasher)? {
                let df = dif_helper(stack, &mut dr, e, hasher)?;
                Ok(T(stack, &[dn, dl, df]))
            } else {
                let df = dif_helper(stack, d, &mut el, hasher)?;
                Ok(T(stack, &[en, df, er]))
            }
        }
    }

    if unsafe { b.raw_equals(&D(0)) } {
        Ok(*a)
    } else {
        let [mut bn, mut bl, mut br] = b.uncell()?;
        let c = z_set_bif(stack, a, &mut bn, hasher)?; // could also be generic if needed
        let [mut cl, mut cr] = c.uncell()?;
        let mut d = z_set_dif(stack, &mut cl, &mut bl, hasher)?;
        let mut e = z_set_dif(stack, &mut cr, &mut br, hasher)?;
        dif_helper(stack, &mut d, &mut e, hasher)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ZSetValue<T> {
    value: T,
    ordered: OrderedNoun,
}

impl<T: NounEncode> ZSetValue<T> {
    fn encode(value: T) -> Result<Self, OwnedZoonError> {
        let ordered = OrderedNoun::encode(&value)?;
        Ok(Self { value, ordered })
    }
}

impl<T> ZTreeValue for ZSetValue<T> {
    type Output = T;

    fn ordered(&self) -> &OrderedNoun {
        &self.ordered
    }

    fn same_slot(&self, other: &Self) -> bool {
        self.ordered.noun == other.ordered.noun
    }

    fn merge_duplicate(existing: Self, _incoming: Self) -> Self {
        existing
    }

    fn into_output(self) -> Self::Output {
        self.value
    }
}

impl<T: NounEncode> ZTreeEncode for ZSetValue<T> {
    fn encode_payload<A: NounAllocator>(&self, allocator: &mut A) -> Noun {
        self.value.to_noun(allocator)
    }
}

impl<T: NounDecode> ZTreeDecode for ZSetValue<T> {
    const KIND: &'static str = "z-set";

    fn decode_payload(noun: &Noun) -> Result<Self, NounDecodeError> {
        let value = T::from_noun(noun)?;
        let ordered = OrderedNoun::from_noun(*noun).map_err(owned_zoon_decode_error)?;
        Ok(Self { value, ordered })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ZSet<T> {
    tree: ZTree<ZSetValue<T>>,
}

impl<T> Default for ZSet<T> {
    fn default() -> Self {
        Self { tree: ZTree::Empty }
    }
}

impl<T> ZSet<T> {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn is_empty(&self) -> bool {
        matches!(self.tree, ZTree::Empty)
    }

    pub fn into_items(self) -> Vec<T> {
        let mut out = Vec::new();
        self.tree.into_outputs(&mut out);
        out
    }

    pub fn len(&self) -> usize {
        self.tree.len()
    }

    pub fn iter(&self) -> impl Iterator<Item = &T> {
        self.tree.iter().map(|payload| &payload.value)
    }

    pub fn first(&self) -> Option<&T> {
        self.iter().next()
    }

    pub fn contains(&self, value: &T) -> bool
    where
        T: PartialEq,
    {
        self.iter().any(|item| item == value)
    }

    pub fn fold_tree<R, F>(&self, empty: R, node: F) -> R
    where
        R: Clone,
        F: Fn(&T, R, R) -> R,
    {
        self.tree.fold(&empty, &|payload, left, right| {
            node(&payload.value, left, right)
        })
    }

    pub fn hash_with<H>(&self, hasher: &H) -> H::Output
    where
        H: ZSetHasher<T>,
    {
        self.fold_tree(hasher.empty(), |value, left, right| {
            hasher.node(value, left, right)
        })
    }
}

impl<T: NounEncode> ZSet<T> {
    pub fn try_insert(&mut self, value: T) -> Result<bool, OwnedZoonError> {
        let candidate = ZSetValue::encode(value)?;
        let (tree, added) = std::mem::take(&mut self.tree).insert(candidate);
        self.tree = tree;
        Ok(added)
    }

    pub fn try_from_items<I>(items: I) -> Result<Self, OwnedZoonError>
    where
        I: IntoIterator<Item = T>,
    {
        let mut set = Self::new();
        for item in items {
            set.try_insert(item)?;
        }
        Ok(set)
    }
}

impl<T: NounEncode> NounEncode for ZSet<T> {
    fn to_noun<A: NounAllocator>(&self, allocator: &mut A) -> Noun {
        self.tree.to_noun(allocator)
    }
}

impl<T: NounDecode> NounDecode for ZSet<T> {
    fn from_noun(noun: &Noun) -> Result<Self, NounDecodeError> {
        Ok(Self {
            tree: ZTree::from_noun(noun)?,
        })
    }
}

impl<T> IntoIterator for ZSet<T> {
    type Item = T;
    type IntoIter = std::vec::IntoIter<T>;

    fn into_iter(self) -> Self::IntoIter {
        self.into_items().into_iter()
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use nockvm::mem::NockStack;
    use nockvm::noun::{Atom, Noun, NounAllocator, D, T};
    use noun_serde::{NounDecode, NounEncode};
    use quickcheck::QuickCheck;

    use super::{z_set_put, DefaultTipHasher, ZSet, ZSetHasher};
    use crate::belt::PRIME;
    use crate::zoon::common::test_support::BoundedTreeValue;

    struct DebugSetHasher;

    impl ZSetHasher<BoundedTreeValue> for DebugSetHasher {
        type Output = String;

        fn empty(&self) -> Self::Output {
            "~".into()
        }

        fn node(
            &self,
            value: &BoundedTreeValue,
            left: Self::Output,
            right: Self::Output,
        ) -> Self::Output {
            format!("{value:?}<{left}|{right}>")
        }
    }

    fn bounded_items(items: Vec<BoundedTreeValue>) -> Vec<BoundedTreeValue> {
        items.into_iter().take(12).collect()
    }

    fn nouns_equal(stack: &mut NockStack, left: Noun, right: Noun) -> bool {
        let mut left = left;
        let mut right = right;
        unsafe { stack.equals(&mut left, &mut right) }
    }

    #[test]
    fn owned_zset_matches_raw_zset_encoding() {
        let items = vec![7u64, 3u64, 11u64, 5u64];
        let mut stack = NockStack::new(8 << 10 << 10, 0);
        let raw = items.iter().fold(D(0), |acc, item: &u64| {
            let mut noun = item.to_noun(&mut stack);
            z_set_put(&mut stack, &acc, &mut noun, &DefaultTipHasher).expect("raw z-set put")
        });

        let owned = ZSet::try_from_items(items).expect("owned z-set should build");
        let mut owned_noun = owned.to_noun(&mut stack);
        let mut raw = raw;
        assert!(unsafe { stack.equals(&mut raw, &mut owned_noun) });
    }

    #[test]
    fn owned_zset_is_insertion_order_independent() {
        let mut stack = NockStack::new(8 << 10 << 10, 0);
        let left = ZSet::try_from_items(vec![1u64, 7u64, 3u64]).expect("left z-set");
        let right = ZSet::try_from_items(vec![7u64, 3u64, 1u64]).expect("right z-set");
        let mut left_noun = left.to_noun(&mut stack);
        let mut right_noun = right.to_noun(&mut stack);
        assert!(unsafe { stack.equals(&mut left_noun, &mut right_noun) });
    }

    #[test]
    fn owned_zset_roundtrips_from_noun() {
        let mut stack = NockStack::new(8 << 10 << 10, 0);
        let original = ZSet::try_from_items(vec![9u64, 2u64, 6u64]).expect("build z-set");
        let noun = original.to_noun(&mut stack);
        let decoded = ZSet::<u64>::from_noun(&noun).expect("decode z-set");
        let mut roundtrip = decoded.to_noun(&mut stack);
        let mut noun = noun;
        assert!(unsafe { stack.equals(&mut noun, &mut roundtrip) });
    }

    #[test]
    fn quickcheck_owned_zset_matches_raw_zset_for_bounded_nouns() {
        fn prop(items: Vec<BoundedTreeValue>) -> bool {
            let items = bounded_items(items);
            let mut stack = NockStack::new(8 << 10 << 10, 0);
            let raw = items.iter().fold(D(0), |acc, item| {
                let mut noun = item.to_noun(&mut stack);
                z_set_put(&mut stack, &acc, &mut noun, &DefaultTipHasher).expect("raw z-set put")
            });

            let owned = ZSet::try_from_items(items).expect("owned z-set should build");
            let owned_noun = owned.to_noun(&mut stack);
            nouns_equal(&mut stack, raw, owned_noun)
        }

        QuickCheck::new()
            .tests(256)
            .quickcheck(prop as fn(Vec<BoundedTreeValue>) -> bool);
    }

    #[test]
    fn quickcheck_owned_zset_is_insertion_order_independent_for_unique_items() {
        fn prop(items: Vec<BoundedTreeValue>) -> bool {
            let unique: Vec<_> = bounded_items(items)
                .into_iter()
                .collect::<BTreeSet<_>>()
                .into_iter()
                .collect();
            let mut reversed = unique.clone();
            reversed.reverse();

            let left = ZSet::try_from_items(unique).expect("left z-set should build");
            let right = ZSet::try_from_items(reversed).expect("right z-set should build");
            let mut stack = NockStack::new(8 << 10 << 10, 0);
            let left_noun = left.to_noun(&mut stack);
            let right_noun = right.to_noun(&mut stack);
            nouns_equal(&mut stack, left_noun, right_noun)
        }

        QuickCheck::new()
            .tests(256)
            .quickcheck(prop as fn(Vec<BoundedTreeValue>) -> bool);
    }

    #[test]
    fn quickcheck_owned_zset_ignores_duplicate_items() {
        fn prop(items: Vec<BoundedTreeValue>) -> bool {
            let items = bounded_items(items);
            let deduped: Vec<_> = items
                .iter()
                .cloned()
                .collect::<BTreeSet<_>>()
                .into_iter()
                .collect();

            let with_duplicates =
                ZSet::try_from_items(items).expect("z-set with duplicates should build");
            let deduped = ZSet::try_from_items(deduped).expect("deduped z-set should build");
            let mut stack = NockStack::new(8 << 10 << 10, 0);
            let with_duplicates_noun = with_duplicates.to_noun(&mut stack);
            let deduped_noun = deduped.to_noun(&mut stack);
            nouns_equal(&mut stack, with_duplicates_noun, deduped_noun)
        }

        QuickCheck::new()
            .tests(256)
            .quickcheck(prop as fn(Vec<BoundedTreeValue>) -> bool);
    }

    #[test]
    fn zset_hash_with_is_insertion_order_independent() {
        let left = ZSet::try_from_items(vec![
            BoundedTreeValue::Atom(1),
            BoundedTreeValue::Atom(7),
            BoundedTreeValue::Atom(3),
        ])
        .expect("left z-set should build");
        let right = ZSet::try_from_items(vec![
            BoundedTreeValue::Atom(7),
            BoundedTreeValue::Atom(3),
            BoundedTreeValue::Atom(1),
        ])
        .expect("right z-set should build");

        assert_eq!(
            left.hash_with(&DebugSetHasher),
            right.hash_with(&DebugSetHasher)
        );
    }

    #[test]
    fn zset_decode_rejects_nonzero_atom() {
        assert!(ZSet::<u64>::from_noun(&D(7)).is_err());
    }

    #[test]
    fn zset_decode_rejects_non_cell_branches() {
        let mut stack = NockStack::new(8 << 10 << 10, 0);
        let malformed = T(&mut stack, &[D(1), D(2)]);
        assert!(ZSet::<u64>::from_noun(&malformed).is_err());
    }

    #[test]
    fn zset_decode_rejects_invalid_item_payload() {
        let mut stack = NockStack::new(8 << 10 << 10, 0);
        let payload = T(&mut stack, &[D(1), D(2)]);
        let malformed = T(&mut stack, &[payload, D(0), D(0)]);
        assert!(ZSet::<u64>::from_noun(&malformed).is_err());
    }

    #[test]
    fn zset_decode_rejects_non_based_atoms() {
        let mut stack = NockStack::new(8 << 10 << 10, 0);
        let payload = Atom::new(&mut stack, PRIME).as_noun();
        let malformed = T(&mut stack, &[payload, D(0), D(0)]);
        assert!(ZSet::<u64>::from_noun(&malformed).is_err());
    }

    #[test]
    fn zset_try_from_items_rejects_non_based_atoms() {
        assert!(ZSet::try_from_items(vec![PRIME]).is_err());
    }
}

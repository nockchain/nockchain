use std::cmp::Ordering;

use nockapp::noun::slab::{NockJammer, NounSlab};
use nockchain_math::zoon::common::{tip, DefaultTipHasher};
use nockchain_types::tx_engine::common::Name;
use nockchain_types::tx_engine::v1::tx::{LockPrimitive, SpendCondition};
use noun_serde::NounEncode;

use crate::types::{CandidateNote, SelectionOrder};

/// Converts a note name into a lexicographically comparable base58 tuple key.
pub fn note_name_lex_key(name: &Name) -> (String, String) {
    (name.first.to_base58(), name.last.to_base58())
}

/// Compares note names using stable lexical ordering over base58 display forms.
pub fn compare_names_lex(a: &Name, b: &Name) -> Ordering {
    note_name_lex_key(a).cmp(&note_name_lex_key(b))
}

/// Sorts candidates by assets then name, preserving legacy deterministic behavior.
pub fn sort_candidates(candidates: &mut [CandidateNote], direction: SelectionOrder) {
    candidates.sort_by(|a, b| {
        let assets_cmp = a.assets().0.cmp(&b.assets().0);
        let assets_cmp = match direction {
            SelectionOrder::Ascending => assets_cmp,
            SelectionOrder::Descending => assets_cmp.reverse(),
        };
        assets_cmp.then_with(|| compare_names_lex(&a.identity().name, &b.identity().name))
    });
}

/// Canonicalizes spend-condition primitives and hash lists for stable matching.
pub fn canonicalize_spend_condition(spend_condition: &SpendCondition) -> SpendCondition {
    let mut primitives = spend_condition
        .iter()
        .cloned()
        .map(canonicalize_lock_primitive)
        .collect::<Vec<_>>();
    primitives.sort_by_key(lock_primitive_sort_key);
    SpendCondition::new(primitives)
}

fn canonicalize_lock_primitive(primitive: LockPrimitive) -> LockPrimitive {
    primitive
}

fn lock_primitive_sort_key(primitive: &LockPrimitive) -> [u64; 5] {
    tip5_sort_key(primitive)
}

fn tip5_sort_key<T: NounEncode>(value: &T) -> [u64; 5] {
    let mut slab: NounSlab<NockJammer> = NounSlab::new();
    let noun = value.to_noun(&mut slab);
    tip(&mut slab, noun, &DefaultTipHasher)
        .expect("tip5 hash should always succeed for noun-encodable primitives")
}

#[cfg(test)]
mod tests {
    use nockchain_math::belt::Belt;
    use nockchain_types::tx_engine::common::{BlockHeight, Hash, Nicks};
    use nockchain_types::tx_engine::v1::tx::{Hax, LockPrimitive, Pkh};

    use super::*;
    use crate::note_data::DecodedNoteData;
    use crate::types::{CandidateIdentity, CandidateNote, CandidateV1Note, RawNoteDataEntry};

    fn hash(v: u64) -> Hash {
        Hash::from_limbs(&[v, 0, 0, 0, 0])
    }

    fn candidate(assets: usize, first: u64, last: u64) -> CandidateNote {
        CandidateNote::V1(CandidateV1Note {
            identity: CandidateIdentity {
                name: Name::new(hash(first), hash(last)),
                origin_page: BlockHeight(Belt(1)),
            },
            assets: Nicks(assets),
            raw_note_data: Vec::<RawNoteDataEntry>::new(),
            decoded_note_data: DecodedNoteData(Vec::new()),
        })
    }

    #[test]
    fn legacy_sort_orders_by_assets_then_name() {
        let mut candidates = vec![candidate(10, 2, 1), candidate(5, 3, 1), candidate(10, 1, 1)];

        sort_candidates(&mut candidates, SelectionOrder::Ascending);

        assert_eq!(candidates[0].assets().0, 5);
        assert!(compare_names_lex(
            &candidates[1].identity().name,
            &candidates[2].identity().name
        )
        .is_le());
    }

    #[test]
    fn legacy_sort_descending_reverses_assets_only() {
        let mut candidates = vec![candidate(10, 2, 1), candidate(5, 3, 1), candidate(10, 1, 1)];

        sort_candidates(&mut candidates, SelectionOrder::Descending);

        assert_eq!(candidates[0].assets().0, 10);
        assert_eq!(candidates[1].assets().0, 10);
        assert!(compare_names_lex(
            &candidates[0].identity().name,
            &candidates[1].identity().name
        )
        .is_le());
    }

    #[test]
    fn canonicalize_spend_condition_dedups_and_sorts_hashes() {
        let h1 = hash(1);
        let h2 = hash(2);
        let h3 = hash(3);
        let sc = SpendCondition::new(vec![
            LockPrimitive::Hax(Hax::new(vec![h3.clone(), h2.clone(), h3.clone()])),
            LockPrimitive::Pkh(Pkh::new(2, vec![h2.clone(), h1.clone(), h2.clone()])),
            LockPrimitive::Burn,
        ]);

        let canonical = canonicalize_spend_condition(&sc);
        let canonical_primitives = canonical.iter().cloned().collect::<Vec<_>>();
        assert_eq!(canonical_primitives.len(), 3);

        let pkh = canonical_primitives
            .iter()
            .find_map(|primitive| match primitive {
                LockPrimitive::Pkh(pkh) => Some(pkh),
                _ => None,
            })
            .expect("pkh primitive should be present");
        assert_eq!(pkh.hashes.len(), 2);
        assert!(pkh.hashes.contains(&h1));
        assert!(pkh.hashes.contains(&h2));

        let hax = canonical_primitives
            .iter()
            .find_map(|primitive| match primitive {
                LockPrimitive::Hax(Hax(hashes)) => Some(hashes),
                _ => None,
            })
            .expect("hax primitive should be present");
        assert_eq!(hax.len(), 2);
        assert!(hax.contains(&hash(2)));
        assert!(hax.contains(&h3));

        assert!(canonical_primitives
            .iter()
            .any(|primitive| matches!(primitive, LockPrimitive::Burn)));

        let sort_keys = canonical_primitives
            .iter()
            .map(lock_primitive_sort_key)
            .collect::<Vec<_>>();
        let mut sorted_keys = sort_keys.clone();
        sorted_keys.sort();
        assert_eq!(sort_keys, sorted_keys);
    }
}

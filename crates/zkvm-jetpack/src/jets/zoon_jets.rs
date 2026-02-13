use nockchain_math::belt::Belt;
use nockchain_math::tip5::hash::{hash_10, hash_noun_varlen_digest};
use nockchain_math::zoon::common::{dor_tip, lth_tip};
use nockvm::interpreter::Context;
use nockvm::jets::util::slot;
use nockvm::jets::JetErr;
use nockvm::noun::{Noun, D, NO, T, YES};
use nockvm_macros::tas;
use noun_serde::NounDecode;

use crate::jets::tip5_jets::digest_to_noundigest;

const TIP_CACHE_TAG: u64 = tas!(b"zntip");
const DOUBLE_TIP_CACHE_TAG: u64 = tas!(b"zndtip");

pub fn dor_tip_jet(context: &mut Context, subject: Noun) -> Result<Noun, JetErr> {
    let sam = slot(subject, 6)?;
    let mut a = slot(sam, 2)?;
    let mut b = slot(sam, 3)?;

    Ok(bool_to_noun(dor_tip(&mut context.stack, &mut a, &mut b)?))
}

pub fn gor_tip_jet(context: &mut Context, subject: Noun) -> Result<Noun, JetErr> {
    let sam = slot(subject, 6)?;
    let mut a = slot(sam, 2)?;
    let mut b = slot(sam, 3)?;

    let a_tip = get_tip_digest(context, a)?;
    let b_tip = get_tip_digest(context, b)?;

    let ordered = if a_tip == b_tip {
        dor_tip(&mut context.stack, &mut a, &mut b)?
    } else {
        lth_tip(&a_tip, &b_tip)
    };

    Ok(bool_to_noun(ordered))
}

pub fn mor_tip_jet(context: &mut Context, subject: Noun) -> Result<Noun, JetErr> {
    let sam = slot(subject, 6)?;
    let mut a = slot(sam, 2)?;
    let mut b = slot(sam, 3)?;

    let a_tip = get_double_tip_digest(context, a)?;
    let b_tip = get_double_tip_digest(context, b)?;

    let ordered = if a_tip == b_tip {
        dor_tip(&mut context.stack, &mut a, &mut b)?
    } else {
        lth_tip(&a_tip, &b_tip)
    };

    Ok(bool_to_noun(ordered))
}

fn bool_to_noun(value: bool) -> Noun {
    if value {
        YES
    } else {
        NO
    }
}

fn cache_lookup_digest(
    context: &mut Context,
    tag: u64,
    noun: Noun,
) -> Result<Option<[u64; 5]>, JetErr> {
    let mut key = T(&mut context.stack, &[D(tag), noun]);
    match context.cache.lookup(&mut context.stack, &mut key) {
        Some(cached) => Ok(Some(<[u64; 5]>::from_noun(&cached)?)),
        None => Ok(None),
    }
}

fn cache_insert_digest(context: &mut Context, tag: u64, noun: Noun, digest: [u64; 5]) {
    let mut key = T(&mut context.stack, &[D(tag), noun]);
    let value = digest_to_noundigest(&mut context.stack, digest);
    context.cache = context.cache.insert(&mut context.stack, &mut key, value);
}

fn get_tip_digest(context: &mut Context, noun: Noun) -> Result<[u64; 5], JetErr> {
    if let Some(cached) = cache_lookup_digest(context, TIP_CACHE_TAG, noun)? {
        return Ok(cached);
    }

    let digest = hash_noun_varlen_digest(&mut context.stack, noun)?;
    cache_insert_digest(context, TIP_CACHE_TAG, noun, digest);
    Ok(digest)
}

fn get_double_tip_digest(context: &mut Context, noun: Noun) -> Result<[u64; 5], JetErr> {
    if let Some(cached) = cache_lookup_digest(context, DOUBLE_TIP_CACHE_TAG, noun)? {
        return Ok(cached);
    }

    let tip_digest = get_tip_digest(context, noun)?;
    let mut input: Vec<Belt> = Vec::with_capacity(10);
    input.extend(tip_digest.into_iter().map(Belt));
    input.extend(tip_digest.into_iter().map(Belt));
    let digest = hash_10(&mut input);

    cache_insert_digest(context, DOUBLE_TIP_CACHE_TAG, noun, digest);
    Ok(digest)
}

#[cfg(test)]
mod tests {
    use ibig::UBig;
    use nockvm::interpreter::Context;
    use nockvm::jets::util::test::{init_context, A};
    use nockvm::mem::NockStack;
    use nockvm::noun::{Noun, D, T};
    use nockvm::unifying_equality::unifying_equality;
    use quickcheck::{Arbitrary, Gen, QuickCheck};

    use super::*;

    #[test]
    fn dor_tip_matches_hoon_for_mixed_atom_cell_inputs() {
        let c = &mut init_context();
        let atom = D(7);
        let cell = T(&mut c.stack, &[D(1), D(2)]);

        assert!(
            cmp_with_jet(c, dor_tip_jet, atom, cell).expect("dor-tip jet should succeed"),
            "expected dor-tip atom<cell case to be true"
        );
        assert!(
            !cmp_with_jet(c, dor_tip_jet, cell, atom).expect("dor-tip jet should succeed"),
            "expected dor-tip cell<atom case to be false"
        );
    }

    #[test]
    fn gor_tip_reuses_tip_cache() {
        let c = &mut init_context();
        let a = T(&mut c.stack, &[D(1), D(2)]);
        let b = T(&mut c.stack, &[D(3), D(4)]);
        let subject = jet_subject(&mut c.stack, a, b);

        assert_eq!(cache_entries(c), 0);
        let _first = gor_tip_jet(c, subject).expect("gor-tip should succeed");
        let after_first = cache_entries(c);
        assert!(
            after_first >= 2,
            "expected tip cache entries for both nouns, found {after_first}"
        );

        let _second = gor_tip_jet(c, subject).expect("gor-tip should succeed");
        let after_second = cache_entries(c);
        assert_eq!(after_second, after_first);
    }

    #[test]
    fn mor_tip_reuses_tip_and_double_tip_cache() {
        let c = &mut init_context();
        let a = T(&mut c.stack, &[D(17), D(23)]);
        let b = T(&mut c.stack, &[D(19), D(29)]);
        let subject = jet_subject(&mut c.stack, a, b);

        assert_eq!(cache_entries(c), 0);
        let _first = mor_tip_jet(c, subject).expect("mor-tip should succeed");
        let after_mor = cache_entries(c);
        assert!(
            after_mor >= 4,
            "expected tip + double-tip cache entries for both nouns, found {after_mor}"
        );

        let _second = mor_tip_jet(c, subject).expect("mor-tip should succeed");
        let after_second_mor = cache_entries(c);
        assert_eq!(after_second_mor, after_mor);

        let _gor = gor_tip_jet(c, subject).expect("gor-tip should succeed");
        let after_gor = cache_entries(c);
        assert_eq!(after_gor, after_mor);
    }

    #[test]
    fn jets_error_on_malformed_sample_shape() {
        let c = &mut init_context();
        let malformed_subject = T(&mut c.stack, &[D(0), D(42), D(0)]);

        assert!(dor_tip_jet(c, malformed_subject).is_err());
        assert!(gor_tip_jet(c, malformed_subject).is_err());
        assert!(mor_tip_jet(c, malformed_subject).is_err());
    }

    #[test]
    fn gor_tip_errors_on_non_decodable_tip_cache_entry() {
        let c = &mut init_context();
        let a = T(&mut c.stack, &[D(1), D(2)]);
        let b = T(&mut c.stack, &[D(3), D(4)]);
        inject_bad_cache_value(c, TIP_CACHE_TAG, a);

        let subject = jet_subject(&mut c.stack, a, b);
        assert!(gor_tip_jet(c, subject).is_err());
    }

    #[test]
    fn mor_tip_errors_on_non_decodable_double_tip_cache_entry() {
        let c = &mut init_context();
        let a = T(&mut c.stack, &[D(5), D(6)]);
        let b = T(&mut c.stack, &[D(7), D(8)]);
        inject_bad_cache_value(c, DOUBLE_TIP_CACHE_TAG, a);

        let subject = jet_subject(&mut c.stack, a, b);
        assert!(mor_tip_jet(c, subject).is_err());
    }

    #[test]
    fn gor_and_mor_error_on_non_u64_atom_inputs() {
        let c = &mut init_context();
        let huge_atom = A(&mut c.stack, &(UBig::from(1u128) << 64));
        let other = D(1);

        let subject = jet_subject(&mut c.stack, huge_atom, other);
        assert!(gor_tip_jet(c, subject).is_err());
        assert!(mor_tip_jet(c, subject).is_err());
    }

    #[test]
    fn quickcheck_order_laws_for_dor_gor_mor() {
        fn prop(a: BoundedNounInput, b: BoundedNounInput, c_input: BoundedNounInput) -> bool {
            let context = &mut init_context();
            let an = bounded_noun_from_input(&mut context.stack, &a);
            let bn = bounded_noun_from_input(&mut context.stack, &b);
            let cn = bounded_noun_from_input(&mut context.stack, &c_input);

            let dor_ab =
                cmp_with_jet(context, dor_tip_jet, an, bn).expect("dor comparison should succeed");
            let dor_ba =
                cmp_with_jet(context, dor_tip_jet, bn, an).expect("dor comparison should succeed");
            let dor_bc =
                cmp_with_jet(context, dor_tip_jet, bn, cn).expect("dor comparison should succeed");
            let dor_ac =
                cmp_with_jet(context, dor_tip_jet, an, cn).expect("dor comparison should succeed");

            if !dor_ab && !dor_ba {
                return false;
            }
            if dor_ab && dor_ba && !noun_eq(context, an, bn) {
                return false;
            }
            if dor_ab && dor_bc && !dor_ac {
                return false;
            }

            for jet in [gor_tip_jet, mor_tip_jet] {
                let ab = cmp_with_jet(context, jet, an, bn).expect("comparison should succeed");
                let ba = cmp_with_jet(context, jet, bn, an).expect("comparison should succeed");
                let bc = cmp_with_jet(context, jet, bn, cn).expect("comparison should succeed");
                let ac = cmp_with_jet(context, jet, an, cn).expect("comparison should succeed");

                if !ab && !ba {
                    return false; // totality
                }
                if ab && ba && !noun_eq(context, an, bn) {
                    return false; // antisymmetry
                }
                if ab && bc && !ac {
                    return false; // transitivity
                }
            }

            true
        }

        QuickCheck::new()
            .tests(256)
            .quickcheck(prop as fn(BoundedNounInput, BoundedNounInput, BoundedNounInput) -> bool);
    }

    #[test]
    fn quickcheck_cold_vs_warm_cache_equivalence() {
        fn prop(a: BoundedNounInput, b: BoundedNounInput) -> bool {
            let mut cold = init_context();
            let cold_a = bounded_noun_from_input(&mut cold.stack, &a);
            let cold_b = bounded_noun_from_input(&mut cold.stack, &b);

            let cold_dor =
                cmp_with_jet(&mut cold, dor_tip_jet, cold_a, cold_b).expect("dor should succeed");
            let cold_gor =
                cmp_with_jet(&mut cold, gor_tip_jet, cold_a, cold_b).expect("gor should succeed");
            let cold_mor =
                cmp_with_jet(&mut cold, mor_tip_jet, cold_a, cold_b).expect("mor should succeed");

            let mut warm = init_context();
            let warm_a = bounded_noun_from_input(&mut warm.stack, &a);
            let warm_b = bounded_noun_from_input(&mut warm.stack, &b);

            let warm_first_dor =
                cmp_with_jet(&mut warm, dor_tip_jet, warm_a, warm_b).expect("dor should succeed");
            let warm_first_gor =
                cmp_with_jet(&mut warm, gor_tip_jet, warm_a, warm_b).expect("gor should succeed");
            let warm_first_mor =
                cmp_with_jet(&mut warm, mor_tip_jet, warm_a, warm_b).expect("mor should succeed");

            let warm_second_dor =
                cmp_with_jet(&mut warm, dor_tip_jet, warm_a, warm_b).expect("dor should succeed");
            let warm_second_gor =
                cmp_with_jet(&mut warm, gor_tip_jet, warm_a, warm_b).expect("gor should succeed");
            let warm_second_mor =
                cmp_with_jet(&mut warm, mor_tip_jet, warm_a, warm_b).expect("mor should succeed");

            cold_dor == warm_first_dor
                && cold_gor == warm_first_gor
                && cold_mor == warm_first_mor
                && warm_first_dor == warm_second_dor
                && warm_first_gor == warm_second_gor
                && warm_first_mor == warm_second_mor
        }

        QuickCheck::new()
            .tests(256)
            .quickcheck(prop as fn(BoundedNounInput, BoundedNounInput) -> bool);
    }

    fn cache_entries(context: &Context) -> usize {
        context.cache.iter().map(|pairs| pairs.len()).sum()
    }

    fn inject_bad_cache_value(context: &mut Context, tag: u64, noun: Noun) {
        let mut key = T(&mut context.stack, &[D(tag), noun]);
        context.cache = context.cache.insert(&mut context.stack, &mut key, D(7));
    }

    fn cmp_with_jet(
        context: &mut Context,
        jet: fn(&mut Context, Noun) -> Result<Noun, JetErr>,
        a: Noun,
        b: Noun,
    ) -> Result<bool, JetErr> {
        let subject = jet_subject(&mut context.stack, a, b);
        let result = jet(context, subject)?;
        if unsafe { result.raw_equals(&YES) } {
            Ok(true)
        } else if unsafe { result.raw_equals(&NO) } {
            Ok(false)
        } else {
            panic!("comparison jet should return %.y/%.n, got: {result:?}");
        }
    }

    fn noun_eq(context: &mut Context, a: Noun, b: Noun) -> bool {
        let mut an = a;
        let mut bn = b;
        unsafe { unifying_equality(&mut context.stack, &mut an, &mut bn) }
    }

    fn jet_subject(stack: &mut nockvm::mem::NockStack, a: Noun, b: Noun) -> Noun {
        let sam = T(stack, &[a, b]);
        T(stack, &[D(0), sam, D(0)])
    }

    #[derive(Clone, Debug)]
    struct BoundedNounInput {
        seed: u64,
        depth: u8,
    }

    impl Arbitrary for BoundedNounInput {
        fn arbitrary(g: &mut Gen) -> Self {
            Self {
                seed: u64::arbitrary(g),
                depth: (u8::arbitrary(g) % 6) + 1,
            }
        }
    }

    fn bounded_noun_from_input(stack: &mut NockStack, input: &BoundedNounInput) -> Noun {
        let mut state = if input.seed == 0 { 1 } else { input.seed };
        bounded_noun_from_state(stack, &mut state, input.depth)
    }

    fn bounded_noun_from_state(stack: &mut NockStack, state: &mut u64, depth: u8) -> Noun {
        let token = next_u64(state);
        let atom = D(token % 1_000_000_000);

        if depth == 0 || (token & 0b11) == 0 {
            atom
        } else {
            let left = bounded_noun_from_state(stack, state, depth - 1);
            let right = bounded_noun_from_state(stack, state, depth - 1);
            T(stack, &[left, right])
        }
    }

    fn next_u64(state: &mut u64) -> u64 {
        let mut x = *state;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        *state = x;
        x
    }
}

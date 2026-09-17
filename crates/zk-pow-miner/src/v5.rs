//! Native version-5 nonce evaluation used as the CUDA correctness oracle.

use ibig::UBig;
use nockapp::noun::slab::NounSlab;
use nockchain_math::noun_ext::NounMathExtHandle;
use nockchain_math::tip5::hash::{absorb, hash_belts_slice, hash_ten_cell, tip5_calc_digest};
use nockvm::noun::NounAllocator;
use thiserror::Error;
use zkvm_jetpack::form::belt::PRIME;
use zkvm_jetpack::form::tog::{belts, hash_proof_data, Tog};
use zkvm_jetpack::form::ProofData;

const MAX_PUZZLE_LEN: u64 = 1 << 16;
const DIGEST_LIMBS: usize = 5;

#[derive(Debug, Error)]
pub enum V5Error {
    #[error("expected a five-belt digest")]
    BadDigest,
    #[error("digest limb {index} is not a Goldilocks field element")]
    UnbasedDigest { index: usize },
    #[error("target is neither an atom nor a %bn bignum")]
    BadTarget,
    #[error("target %bn limb list is malformed")]
    BadTargetList,
    #[error("target %bn limb does not fit in u32")]
    BadTargetLimb,
    #[error("puzzle length must be a nonzero power of two no greater than {MAX_PUZZLE_LEN}")]
    BadPuzzleLength,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(C)]
pub struct V5Job {
    pub commitment: [u64; DIGEST_LIMBS],
    pub target: [u64; DIGEST_LIMBS],
    pub puzzle_tag_hash: [u64; DIGEST_LIMBS],
    pub length_hash: [u64; DIGEST_LIMBS],
    pub pow_tag_hash: [u64; DIGEST_LIMBS],
    pub pow_len: u32,
}

impl V5Job {
    pub fn new(
        commitment: [u64; DIGEST_LIMBS],
        target: [u64; DIGEST_LIMBS],
        pow_len: u64,
    ) -> Result<Self, V5Error> {
        if pow_len == 0 || !pow_len.is_power_of_two() || pow_len > MAX_PUZZLE_LEN {
            return Err(V5Error::BadPuzzleLength);
        }
        Ok(Self {
            commitment,
            target,
            puzzle_tag_hash: hash_term(b"puzzle"),
            length_hash: hash_leaf(pow_len),
            pow_tag_hash: hash_term(b"zkpow-v5"),
            pow_len: pow_len as u32,
        })
    }

    pub fn from_candidate_parts(
        commitment: &NounSlab,
        target: &NounSlab,
        pow_len: u64,
    ) -> Result<Self, V5Error> {
        Self::new(
            decode_digest_slab(commitment)?,
            decode_target_slab(target)?,
            pow_len,
        )
    }

    #[inline]
    pub fn digest(&self, nonce: [u64; DIGEST_LIMBS]) -> [u64; DIGEST_LIMBS] {
        v5_pow_digest(self.commitment, nonce, u64::from(self.pow_len))
    }

    #[inline]
    pub fn meets_target(&self, digest: &[u64; DIGEST_LIMBS]) -> bool {
        digest_le(digest, &self.target)
    }
}

pub fn decode_digest_slab(slab: &NounSlab) -> Result<[u64; DIGEST_LIMBS], V5Error> {
    let space = slab.noun_space();
    let root = unsafe { *slab.root() };
    let nouns = root
        .in_space(&space)
        .uncell::<DIGEST_LIMBS>()
        .map_err(|_| V5Error::BadDigest)?;
    let mut digest = [0; DIGEST_LIMBS];
    for (index, noun) in nouns.into_iter().enumerate() {
        let value = noun
            .as_atom()
            .map_err(|_| V5Error::BadDigest)?
            .as_u64()
            .map_err(|_| V5Error::BadDigest)?;
        if value >= PRIME {
            return Err(V5Error::UnbasedDigest { index });
        }
        digest[index] = value;
    }
    Ok(digest)
}

pub fn decode_target_slab(target: &NounSlab) -> Result<[u64; DIGEST_LIMBS], V5Error> {
    let space = target.noun_space();
    let root = unsafe { *target.root() };
    let value = if let Ok(cell) = root.in_space(&space).as_cell() {
        if !cell.head().eq_bytes("bn") {
            return Err(V5Error::BadTarget);
        }
        let mut value = UBig::from(0u8);
        let mut factor = UBig::from(1u8);
        let radix = UBig::from(1u64) << 32;
        let mut list = cell.tail().noun();
        loop {
            if let Ok(end) = list.in_space(&space).as_atom() {
                if end.as_u64().map_err(|_| V5Error::BadTargetList)? != 0 {
                    return Err(V5Error::BadTargetList);
                }
                break;
            }
            let item = list
                .in_space(&space)
                .as_cell()
                .map_err(|_| V5Error::BadTargetList)?;
            let limb = item
                .head()
                .as_atom()
                .map_err(|_| V5Error::BadTargetLimb)?
                .as_u64()
                .map_err(|_| V5Error::BadTargetLimb)?;
            let limb = u32::try_from(limb).map_err(|_| V5Error::BadTargetLimb)?;
            value += UBig::from(limb) * &factor;
            factor *= &radix;
            list = item.tail().noun();
        }
        value
    } else {
        let atom = root
            .in_space(&space)
            .as_atom()
            .map_err(|_| V5Error::BadTarget)?;
        UBig::from_le_bytes(atom.as_ne_bytes())
    };
    Ok(target_to_base_p(value))
}

pub fn nonce_slab(nonce: [u64; DIGEST_LIMBS]) -> NounSlab {
    use nockapp::noun::AtomExt;
    use nockvm::noun::{Atom, T};

    let mut slab = NounSlab::new();
    let nouns = nonce.map(|limb| {
        <Atom as AtomExt>::from_value(&mut slab, limb)
            .expect("based u64 nonce limb fits in an atom")
            .as_noun()
    });
    let root = T(&mut slab, &nouns);
    slab.set_root(root);
    slab
}

pub fn add_nonce(mut nonce: [u64; DIGEST_LIMBS], increment: u64) -> [u64; DIGEST_LIMBS] {
    let mut carry = u128::from(increment);
    for limb in &mut nonce {
        if carry == 0 {
            break;
        }
        let sum = u128::from(*limb) + carry;
        *limb = (sum % u128::from(PRIME)) as u64;
        carry = sum / u128::from(PRIME);
    }
    nonce
}

pub fn v5_pow_digest(
    commitment: [u64; DIGEST_LIMBS],
    nonce: [u64; DIGEST_LIMBS],
    pow_len: u64,
) -> [u64; DIGEST_LIMBS] {
    let mut seed = [0; 10];
    seed[..DIGEST_LIMBS].copy_from_slice(&commitment);
    seed[DIGEST_LIMBS..].copy_from_slice(&nonce);
    let mut sponge = [0; 16];
    absorb(&mut sponge, &seed);
    let mut rng = Tog { sponge };
    let mut product = belts(&mut rng, pow_len as u32)
        .into_iter()
        .map(|belt| belt.0)
        .collect::<Vec<_>>();
    product.reverse();

    let mut leaf = Vec::with_capacity(product.len() + 1);
    leaf.extend_from_slice(&product);
    leaf.push(0);
    let mut dyck = Vec::with_capacity(product.len() * 2);
    for _ in 0..product.len() {
        dyck.extend_from_slice(&[0, 1]);
    }
    let object = ProofData::Puzzle {
        com: commitment,
        nonce,
        len: pow_len,
        leaf,
        dyck,
    };
    let object_hash = hash_proof_data(&object);

    let mut proof_sponge = [0; 16];
    absorb(&mut proof_sponge, &object_hash);
    let pow_digest = tip5_calc_digest(&proof_sponge);

    let tag_hash = hash_term(b"zkpow-v5");
    let mut domain_pair = [0; 10];
    domain_pair[..DIGEST_LIMBS].copy_from_slice(&tag_hash);
    domain_pair[DIGEST_LIMBS..].copy_from_slice(&pow_digest);
    hash_ten_cell(domain_pair)
}

#[inline]
pub fn digest_le(left: &[u64; DIGEST_LIMBS], right: &[u64; DIGEST_LIMBS]) -> bool {
    for index in (0..DIGEST_LIMBS).rev() {
        match left[index].cmp(&right[index]) {
            std::cmp::Ordering::Less => return true,
            std::cmp::Ordering::Greater => return false,
            std::cmp::Ordering::Equal => {}
        }
    }
    true
}

fn hash_leaf(value: u64) -> [u64; DIGEST_LIMBS] {
    hash_belts_slice(&[1, value])
}

fn hash_term(bytes: &[u8]) -> [u64; DIGEST_LIMBS] {
    let mut value = 0u64;
    for &byte in bytes.iter().rev() {
        value = (value << 8) | u64::from(byte);
    }
    hash_leaf(value)
}

fn target_to_base_p(mut target: UBig) -> [u64; DIGEST_LIMBS] {
    let prime = UBig::from(PRIME);
    let mut limbs = [0; DIGEST_LIMBS];
    for limb in &mut limbs {
        let remainder = &target % &prime;
        *limb = u64::try_from(remainder).expect("remainder below Goldilocks prime fits in u64");
        target /= &prime;
    }
    if target != UBig::from(0u8) {
        [PRIME - 1; DIGEST_LIMBS]
    } else {
        limbs
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nonce_increment_carries_in_base_p() {
        assert_eq!(add_nonce([PRIME - 1, 2, 3, 4, 5], 1), [0, 3, 3, 4, 5]);
    }

    #[test]
    fn digest_comparison_uses_most_significant_base_p_limb() {
        assert!(digest_le(&[PRIME - 1, 0, 0, 0, 0], &[0, 1, 0, 0, 0]));
        assert!(!digest_le(&[0, 1, 0, 0, 0], &[PRIME - 1, 0, 0, 0, 0]));
    }

    #[test]
    fn prepared_constants_match_digest_path() {
        let job = V5Job::new([1, 2, 3, 4, 5], [PRIME - 1; 5], 64).expect("job");
        assert_eq!(job.puzzle_tag_hash, hash_term(b"puzzle"));
        assert_eq!(job.length_hash, hash_leaf(64));
        assert_eq!(job.pow_tag_hash, hash_term(b"zkpow-v5"));
        assert!(job.meets_target(&job.digest([6, 7, 8, 9, 10])));
    }
}

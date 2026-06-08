use std::cell::RefCell;

use nockapp::noun::slab::{NockJammer, NounSlab};
use nockapp::utils::make_tas;
use nockchain_math::belt::Belt;
use nockchain_math::crypto::cheetah::{
    belt_schnorr_t8_to_ubig, ch_add, ch_neg, ch_scal_big, trunc_g_order, A_GEN, F6_ZERO, G_ORDER,
};
use nockchain_math::tip5::hash::hash_varlen;
use nockchain_math::zoon::zmap::ZMap;
use nockchain_math::zoon::zset::ZSet;
use nockvm::noun::{Noun, NounAllocator, D, T};
use noun_serde::{NounDecode, NounEncode};

use crate::tx_engine::common::{Hash, SchnorrPubkey, SchnorrSignature, Source};
use crate::tx_engine::v1::hashable::{
    hash_leaf_digest, hashable_hash_noun, hashable_leaf_noun, noun_hashable,
};
use crate::tx_engine::v1::note::{NoteData, NoteDataValue};
use crate::tx_engine::v1::tx::{PkhSignatureEntry, Seed, Seeds, Spend1};

pub trait SigHashable {
    fn sig_hashable_noun<A: NounAllocator>(
        &self,
        allocator: &mut A,
    ) -> Result<Noun, Spend1SignatureVerificationError>;

    fn sig_hash_digest(&self) -> Result<Hash, Spend1SignatureVerificationError> {
        let mut slab: NounSlab<NockJammer> = NounSlab::new();
        let hashable = self.sig_hashable_noun(&mut slab)?;
        let digest = crate::tx_engine::v1::hash_hashable(&mut slab, hashable).map_err(|err| {
            Spend1SignatureVerificationError::SignatureHash(format!(
                "failed to hash spend-1 signature noun: {err:?}"
            ))
        })?;
        let space = slab.noun_space();
        Hash::from_noun(&digest, &space).map_err(|err| {
            Spend1SignatureVerificationError::SignatureHash(format!(
                "failed to decode spend-1 signature hash: {err}"
            ))
        })
    }
}

impl SigHashable for Spend1 {
    fn sig_hashable_noun<A: NounAllocator>(
        &self,
        allocator: &mut A,
    ) -> Result<Noun, Spend1SignatureVerificationError> {
        let seeds_hashable = self.seeds.sig_hashable_noun(allocator)?;
        let fee_hashable = hashable_leaf_value(allocator, &self.fee);
        Ok(T(allocator, &[seeds_hashable, fee_hashable]))
    }
}

impl SigHashable for Seeds {
    fn sig_hashable_noun<A: NounAllocator>(
        &self,
        allocator: &mut A,
    ) -> Result<Noun, Spend1SignatureVerificationError> {
        let allocator = RefCell::new(allocator);
        let seeds = ZSet::try_from_items(self.0.clone()).map_err(|err| {
            Spend1SignatureVerificationError::SignatureHash(format!(
                "failed to encode spend-1 seeds z-set: {err}"
            ))
        })?;
        seeds.try_fold_tree(
            || {
                let mut allocator = allocator.borrow_mut();
                Ok(hashable_leaf_noun(*allocator, D(0)))
            },
            |seed, left, right| {
                let mut allocator = allocator.borrow_mut();
                let head = seed.sig_hashable_noun(*allocator)?;
                Ok(T(*allocator, &[head, left, right]))
            },
        )
    }
}

impl SigHashable for Seed {
    fn sig_hashable_noun<A: NounAllocator>(
        &self,
        allocator: &mut A,
    ) -> Result<Noun, Spend1SignatureVerificationError> {
        let output_source = self.output_source.sig_hashable_noun(allocator)?;
        let lock_root = hashable_hash_noun(allocator, &self.lock_root);
        let note_data = hashable_hash_noun(allocator, &self.note_data.sig_hash_digest()?);
        let gift = hashable_leaf_value(allocator, &self.gift);
        let parent_hash = hashable_hash_noun(allocator, &self.parent_hash);
        Ok(T(
            allocator,
            &[output_source, lock_root, note_data, gift, parent_hash],
        ))
    }
}

impl SigHashable for Option<Source> {
    fn sig_hashable_noun<A: NounAllocator>(
        &self,
        allocator: &mut A,
    ) -> Result<Noun, Spend1SignatureVerificationError> {
        Ok(match self {
            None => hashable_leaf_noun(allocator, D(0)),
            Some(source) => {
                let none_leaf = hashable_leaf_noun(allocator, D(0));
                let source_hashable = source.sig_hashable_noun(allocator)?;
                T(allocator, &[none_leaf, source_hashable])
            }
        })
    }
}

impl SigHashable for Source {
    fn sig_hashable_noun<A: NounAllocator>(
        &self,
        allocator: &mut A,
    ) -> Result<Noun, Spend1SignatureVerificationError> {
        let hash = hashable_hash_noun(allocator, &self.hash);
        let is_coinbase = hashable_leaf_value(allocator, &self.is_coinbase);
        Ok(T(allocator, &[hash, is_coinbase]))
    }
}

impl SigHashable for NoteData {
    fn sig_hashable_noun<A: NounAllocator>(
        &self,
        allocator: &mut A,
    ) -> Result<Noun, Spend1SignatureVerificationError> {
        let allocator = RefCell::new(allocator);
        let note_data = ZMap::try_from_entries(
            self.0
                .iter()
                .map(|entry| (NoteDataKey(entry.key.clone()), entry.value.clone())),
        )
        .map_err(|err| {
            Spend1SignatureVerificationError::SignatureHash(format!(
                "failed to encode spend-1 note-data z-map: {err}"
            ))
        })?;
        note_data.try_fold_tree(
            || {
                let mut allocator = allocator.borrow_mut();
                Ok(hashable_leaf_noun(*allocator, D(0)))
            },
            |key, value, left, right| {
                let mut allocator = allocator.borrow_mut();
                let key_noun = key.to_noun(*allocator);
                let key = hashable_leaf_noun(*allocator, key_noun);
                let value = value.sig_hashable_noun(*allocator)?;
                let head = T(*allocator, &[key, value]);
                Ok(T(*allocator, &[head, left, right]))
            },
        )
    }
}

impl SigHashable for NoteDataValue {
    fn sig_hashable_noun<A: NounAllocator>(
        &self,
        allocator: &mut A,
    ) -> Result<Noun, Spend1SignatureVerificationError> {
        Ok(match self {
            Self::Noun(noun) => {
                let noun = noun.to_noun(allocator);
                noun_hashable(allocator, noun)
            }
            _ => {
                let noun = self.to_noun(allocator);
                noun_hashable(allocator, noun)
            }
        })
    }
}

impl SchnorrPubkey {
    /// Computes the tx-engine PKH for this Schnorr pubkey.
    pub fn pkh_hash(&self) -> Result<Hash, Spend1SignatureVerificationError> {
        let mut slab: NounSlab<NockJammer> = NounSlab::new();
        let noun = self.to_noun(&mut slab);
        hash_leaf_digest(&mut slab, noun).map_err(|err| {
            Spend1SignatureVerificationError::PubkeyHash(format!(
                "failed to hash spend-1 signer pubkey leaf: {err}"
            ))
        })
    }
}

impl SchnorrSignature {
    fn chal_ubig(&self) -> ibig::UBig {
        belt_schnorr_t8_to_ubig(&self.chal)
    }

    fn sig_ubig(&self) -> ibig::UBig {
        belt_schnorr_t8_to_ubig(&self.sig)
    }

    /// Every 32-bit limb of both `chal` and `sig` must be a canonical digit
    /// (`< 2^32`), mirroring `based:belt-schnorr` in Hoon. The scalar is
    /// reconstructed from these limbs as base-2^32 digits, so requiring
    /// canonical limbs makes the encoding unique.
    pub fn is_canonical(&self) -> bool {
        const LIMB_BOUND: u64 = 1 << 32;
        self.chal
            .iter()
            .chain(self.sig.iter())
            .all(|limb| limb.0 < LIMB_BOUND)
    }
}

impl Spend1 {
    /// Verifies one `%pkh` witness signature entry against this canonical `%1` spend.
    pub fn verify_pkh_signature(
        &self,
        entry: &PkhSignatureEntry,
    ) -> Result<(), Spend1SignatureVerificationError> {
        let pubkey_hash = entry.pubkey.pkh_hash()?;
        if pubkey_hash != entry.pkh {
            return Err(Spend1SignatureVerificationError::PubkeyHashMismatch {
                actual: pubkey_hash.to_base58(),
                expected: entry.pkh.to_base58(),
            });
        }

        // reject non-canonical limb encodings
        if !entry.signature.is_canonical() {
            return Err(Spend1SignatureVerificationError::NonCanonicalSignature);
        }

        let sig_hash = self.sig_hash_digest()?;
        let chal = entry.signature.chal_ubig();
        let sig = entry.signature.sig_ubig();
        // match the Hoon `+verify:affine:schnorr` scalar-range guards
        // (`0 < scalar < g-order`) before any curve arithmetic.
        let zero = ibig::UBig::from(0u8);
        if chal == zero || sig == zero || chal >= *G_ORDER || sig >= *G_ORDER {
            return Err(Spend1SignatureVerificationError::InvalidSignature);
        }
        let left = ch_scal_big(&sig, &A_GEN)
            .map_err(|_| Spend1SignatureVerificationError::SignatureArithmetic)?;
        let right = ch_neg(
            &ch_scal_big(&chal, &entry.pubkey.0)
                .map_err(|_| Spend1SignatureVerificationError::SignatureArithmetic)?,
        );
        let sum = ch_add(&left, &right)
            .map_err(|_| Spend1SignatureVerificationError::SignatureArithmetic)?;
        if sum.x == F6_ZERO {
            return Err(Spend1SignatureVerificationError::InvalidSignature);
        }

        let mut hashable = vec![Belt(0); 6 * 4 + 5];
        hashable[0..6].copy_from_slice(&sum.x.0);
        hashable[6..12].copy_from_slice(&sum.y.0);
        hashable[12..18].copy_from_slice(&entry.pubkey.0.x.0);
        hashable[18..24].copy_from_slice(&entry.pubkey.0.y.0);
        hashable[24..].copy_from_slice(&sig_hash.0);

        let truncated_hash = trunc_g_order(&hash_varlen(&mut hashable));
        if truncated_hash == chal {
            Ok(())
        } else {
            Err(Spend1SignatureVerificationError::InvalidSignature)
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum Spend1SignatureVerificationError {
    #[error("failed to hash signer pubkey: {0}")]
    PubkeyHash(String),
    #[error("signer pubkey hash mismatch: pubkey hashed to {actual}, stored hash was {expected}")]
    PubkeyHashMismatch { actual: String, expected: String },
    #[error("failed to compute spend-1 signature hash: {0}")]
    SignatureHash(String),
    #[error("failed to verify spend-1 Schnorr signature")]
    SignatureArithmetic,
    #[error("invalid spend-1 Schnorr signature")]
    InvalidSignature,
    #[error("non-canonical spend-1 Schnorr signature (limb >= 2^32)")]
    NonCanonicalSignature,
}

#[derive(Debug, Clone)]
struct NoteDataKey(String);

impl NounEncode for NoteDataKey {
    fn to_noun<A: NounAllocator>(&self, allocator: &mut A) -> Noun {
        make_tas(allocator, &self.0).as_noun()
    }
}

fn hashable_leaf_value<T: NounEncode, A: NounAllocator>(allocator: &mut A, value: &T) -> Noun {
    let noun = value.to_noun(allocator);
    hashable_leaf_noun(allocator, noun)
}

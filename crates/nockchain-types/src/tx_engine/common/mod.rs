pub mod page;

use anyhow::Result;
use nockchain_math::belt::{Belt, PRIME};
use nockchain_math::crypto::cheetah::{CheetahError, CheetahPoint};
use nockchain_math::noun_ext::NounMathExt;
use nockchain_math::zoon::common::DefaultTipHasher;
use nockchain_math::zoon::zmap;
use nockvm::noun::{Noun, NounAllocator, D};
use noun_serde::{NounDecode, NounDecodeError, NounEncode};
use num_bigint::BigUint;
pub use page::{BigNum, BlockId, CoinbaseSplit, Page, PageMsg};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Hash, NounDecode, NounEncode)]
pub struct SchnorrPubkey(pub CheetahPoint);

impl SchnorrPubkey {
    pub const BYTES_BASE58: usize = 132;

    pub fn to_base58(&self) -> Result<String, CheetahError> {
        CheetahPoint::into_base58(&self.0)
    }

    pub fn from_base58(b58: &str) -> Result<Self, CheetahError> {
        Ok(SchnorrPubkey(CheetahPoint::from_base58(b58)?))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, NounDecode, NounEncode)]
pub struct SchnorrSignature {
    pub chal: [Belt; 8],
    pub sig: [Belt; 8],
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Signature(pub Vec<(SchnorrPubkey, SchnorrSignature)>);

impl NounEncode for Signature {
    fn to_noun<A: NounAllocator>(&self, stack: &mut A) -> Noun {
        self.0.iter().fold(D(0), |map, (pubkey, sig)| {
            let mut key = pubkey.to_noun(stack);
            let mut value = sig.to_noun(stack);
            zmap::z_map_put(stack, &map, &mut key, &mut value, &DefaultTipHasher)
                .expect("z-map put for signature should not fail")
        })
    }
}

impl NounDecode for Signature {
    fn from_noun(noun: &Noun) -> Result<Self, NounDecodeError> {
        if let Ok(atom) = noun.as_atom() {
            if atom.as_u64()? == 0 {
                return Ok(Signature(Vec::new()));
            }
            return Err(NounDecodeError::Custom("signature node not a cell".into()));
        }

        let entries = nockchain_math::structs::HoonMapIter::from(*noun)
            .filter(|entry| entry.is_cell())
            .map(|entry| {
                let [key, value] = entry
                    .uncell()
                    .map_err(|_| NounDecodeError::Custom("signature entry not a pair".into()))?;
                let pubkey = SchnorrPubkey::from_noun(&key)?;
                let signature = SchnorrSignature::from_noun(&value)?;
                Ok((pubkey, signature))
            })
            .collect::<Result<Vec<_>, NounDecodeError>>()?;

        Ok(Signature(entries))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, NounEncode, NounDecode, Serialize, Deserialize)]
pub struct BlockHeight(pub Belt);

#[derive(Debug, Clone, PartialEq, Eq, NounEncode, NounDecode, Serialize, Deserialize)]
pub struct BlockHeightDelta(pub Belt);

#[derive(Debug, Clone, PartialEq, Eq, NounDecode, NounEncode)]
pub struct Nicks(pub usize);

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Version {
    V0,
    V1,
    V2,
}

impl From<Version> for u32 {
    fn from(version: Version) -> Self {
        match version {
            Version::V0 => 0,
            Version::V1 => 1,
            Version::V2 => 2,
        }
    }
}

impl From<u32> for Version {
    fn from(version: u32) -> Self {
        match version {
            0 => Version::V0,
            1 => Version::V1,
            2 => Version::V2,
            _ => panic!("Invalid version"),
        }
    }
}

impl NounEncode for Version {
    fn to_noun<A: NounAllocator>(&self, _stack: &mut A) -> Noun {
        match self {
            Version::V0 => D(0),
            Version::V1 => D(1),
            Version::V2 => D(2),
        }
    }
}

impl NounDecode for Version {
    fn from_noun(noun: &Noun) -> Result<Self, NounDecodeError> {
        match noun.as_atom()?.as_direct() {
            Ok(ver) if ver.data() == 0 => Ok(Version::V0),
            Ok(ver) if ver.data() == 1 => Ok(Version::V1),
            Ok(ver) if ver.data() == 2 => Ok(Version::V2),
            _ => Err(NounDecodeError::InvalidEnumVariant),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, NounEncode, NounDecode)]
pub struct Source {
    pub hash: Hash,
    pub is_coinbase: bool,
}

#[derive(Debug, thiserror::Error)]
pub enum HashDecodeError {
    #[error("Provided base58 corresponds to a value too large to be a tip5 hash (likely a v0 pubkey instead of a v1 pkh)")]
    ProvidedValueTooLarge,
    #[error("base58 decode error: {0}")]
    Base58(#[from] bs58::decode::Error),
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, NounDecode, NounEncode, Serialize, Deserialize)]
pub struct Hash(pub [Belt; 5]);

impl Hash {
    pub fn to_base58(&self) -> String {
        fn base_p_to_decimal<const N: usize>(belts: [Belt; N]) -> String {
            let prime = BigUint::from(PRIME);
            let mut result = BigUint::from(0u8);

            for (i, value) in belts.iter().enumerate() {
                let pow = prime.pow(i as u32);
                result += BigUint::from(value.0) * pow;
            }

            bs58::encode(result.to_bytes_be()).into_string()
        }

        base_p_to_decimal(self.0)
    }

    pub fn from_base58(s: &str) -> Result<Self, HashDecodeError> {
        let bytes = bs58::decode(s).into_vec()?;
        let mut value = BigUint::from_bytes_be(&bytes);
        let prime = BigUint::from(PRIME);
        let mut belts = [Belt(0); 5];
        for belt in &mut belts {
            let rem = &value % &prime;
            let rem_u64: u64 = rem
                .try_into()
                .map_err(|_| HashDecodeError::ProvidedValueTooLarge)?;
            *belt = Belt(rem_u64);
            value /= &prime;
        }
        if value > prime {
            return Err(HashDecodeError::ProvidedValueTooLarge);
        }
        Ok(Hash(belts))
    }

    pub fn to_array(&self) -> [u64; 5] {
        [self.0[0].0, self.0[1].0, self.0[2].0, self.0[3].0, self.0[4].0]
    }
}

/// Peek response for the heaviest block ID.
/// Wraps `(unit (unit Hash))` - the Hoon peek response encoding.
#[derive(Debug, Clone, PartialEq, Eq, NounDecode, NounEncode)]
pub struct Heavy(pub Option<Option<Option<Hash>>>);

impl Heavy {
    /// Convert to Base58 string if the heavy block ID exists.
    pub fn to_base58(&self) -> Option<String> {
        match &self.0 {
            Some(Some(Some(hash))) => Some(hash.to_base58()),
            _ => None,
        }
    }
}

#[derive(NounEncode, NounDecode, Clone, Debug, PartialEq, Eq)]
pub struct Name {
    pub first: Hash,
    pub last: Hash,
    null: usize,
}

impl Name {
    pub fn new(first: Hash, last: Hash) -> Self {
        Self {
            first,
            last,
            null: 0,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, NounEncode, NounDecode)]
pub struct TimelockRangeAbsolute {
    pub min: Option<BlockHeight>,
    pub max: Option<BlockHeight>,
}

impl TimelockRangeAbsolute {
    pub fn new(min: Option<BlockHeight>, max: Option<BlockHeight>) -> Self {
        let min = min.filter(|height| (height.0).0 != 0);
        let max = max.filter(|height| (height.0).0 != 0);
        Self { min, max }
    }

    pub fn none() -> Self {
        Self {
            min: None,
            max: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, NounEncode, NounDecode)]
pub struct TimelockRangeRelative {
    pub min: Option<BlockHeightDelta>,
    pub max: Option<BlockHeightDelta>,
}

impl TimelockRangeRelative {
    pub fn new(min: Option<BlockHeightDelta>, max: Option<BlockHeightDelta>) -> Self {
        let min = min.filter(|height| (height.0).0 != 0);
        let max = max.filter(|height| (height.0).0 != 0);
        Self { min, max }
    }

    pub fn none() -> Self {
        Self {
            min: None,
            max: None,
        }
    }
}

pub type TxId = Hash;

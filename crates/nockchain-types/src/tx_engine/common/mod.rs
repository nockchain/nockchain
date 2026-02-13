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
    #[error("expected {expected} bytes for tip5 hash, got {actual}")]
    InvalidByteLength { expected: usize, actual: usize },
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, NounDecode, NounEncode, Serialize, Deserialize)]
pub struct Hash(pub [Belt; 5]);

impl Hash {
    pub fn to_base58(&self) -> String {
        bs58::encode(self.to_be_bytes()).into_string()
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

    /// Decode a tip5 hash from a big-endian 32-byte value using base-p decomposition.
    pub fn from_be_bytes(bytes: &[u8; 32]) -> Self {
        let mut value = BigUint::from_bytes_be(bytes);
        let prime = BigUint::from(PRIME);
        let mut belts = [Belt(0); 5];
        for belt in &mut belts {
            let rem: u64 = (&value % &prime)
                .try_into()
                .expect("remainder must fit in u64");
            *belt = Belt(rem);
            value /= &prime;
        }
        Hash(belts)
    }

    /// Convert this tip5 hash to its atom representation (big integer).
    /// This is the inverse of `from_be_bytes` for values that fit in 32 bytes.
    /// Encodes limbs as a base-p number: a + b*p + c*p^2 + d*p^3 + e*p^4.
    // TODO: Unify this implementation with the digest_to_atom jet.
    pub fn to_atom(&self) -> BigUint {
        let prime = BigUint::from(PRIME);
        let mut result = BigUint::from(0u8);
        let mut power = BigUint::from(1u8);
        for belt in &self.0 {
            result += BigUint::from(belt.0) * &power;
            power *= &prime;
        }
        result
    }

    /// Convert this tip5 hash to big-endian bytes of its atom representation.
    pub fn to_be_bytes(&self) -> Vec<u8> {
        self.to_atom().to_bytes_be()
    }

    pub fn from_limbs(limbs: &[u64; 5]) -> Self {
        Hash([Belt(limbs[0]), Belt(limbs[1]), Belt(limbs[2]), Belt(limbs[3]), Belt(limbs[4])])
    }

    pub fn to_be_limb_bytes(&self) -> [u8; 40] {
        let limbs = self.to_array();
        let mut out = [0u8; 40];
        for (i, limb) in limbs.iter().enumerate() {
            out[i * 8..(i + 1) * 8].copy_from_slice(&limb.to_be_bytes());
        }
        out
    }

    pub fn from_be_limb_bytes(bytes: &[u8]) -> Result<Self, HashDecodeError> {
        if bytes.len() != 40 {
            return Err(HashDecodeError::InvalidByteLength {
                expected: 40,
                actual: bytes.len(),
            });
        }
        let mut limbs = [0u64; 5];
        for (i, limb) in limbs.iter_mut().enumerate() {
            let start = i * 8;
            let chunk: [u8; 8] = bytes[start..start + 8].try_into().map_err(|_| {
                HashDecodeError::InvalidByteLength {
                    expected: 40,
                    actual: bytes.len(),
                }
            })?;
            *limb = u64::from_be_bytes(chunk);
        }
        Ok(Self::from_limbs(&limbs))
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

#[cfg(test)]
mod tests {
    use nockchain_math::belt::{Belt, PRIME};
    use num_bigint::BigUint;

    use super::{Hash, HashDecodeError};

    fn biguint_to_be_32(value: &BigUint) -> [u8; 32] {
        let mut out = [0u8; 32];
        let bytes = value.to_bytes_be();
        assert!(
            bytes.len() <= 32,
            "expected value that fits in 32 bytes, got {} bytes",
            bytes.len()
        );
        out[32 - bytes.len()..].copy_from_slice(&bytes);
        out
    }

    #[test]
    fn tip5_be_limb_bytes_roundtrip() {
        let hash = Hash([Belt(1), Belt(2), Belt(3), Belt(4), Belt(5)]);
        let bytes = hash.to_be_limb_bytes();
        let roundtrip = Hash::from_be_limb_bytes(&bytes).expect("valid be limb bytes");
        assert_eq!(hash, roundtrip);
    }

    #[test]
    fn tip5_be_limb_bytes_rejects_wrong_length() {
        let bytes = vec![0u8; 39];
        let err = Hash::from_be_limb_bytes(&bytes).expect_err("invalid length");
        assert!(matches!(
            err,
            HashDecodeError::InvalidByteLength {
                expected: 40,
                actual: 39
            }
        ));
    }

    #[test]
    fn tip5_limbs_roundtrip() {
        let test_cases = vec![
            Hash([Belt(0), Belt(0), Belt(0), Belt(0), Belt(0)]),
            Hash([Belt(1), Belt(0), Belt(0), Belt(0), Belt(0)]),
            Hash([Belt(1), Belt(2), Belt(3), Belt(4), Belt(5)]),
            Hash([Belt(100), Belt(200), Belt(300), Belt(400), Belt(500)]),
            Hash([
                Belt(PRIME - 1),
                Belt(PRIME - 1),
                Belt(PRIME - 1),
                Belt(PRIME - 1),
                Belt(PRIME - 1),
            ]),
        ];

        for original in test_cases {
            let limbs = original.to_array();
            let reconstructed = Hash::from_limbs(&limbs);
            assert_eq!(original, reconstructed, "hash should roundtrip exactly");
        }
    }

    #[test]
    fn tip5_from_be_bytes_known_vectors() {
        let one = {
            let mut bytes = [0u8; 32];
            bytes[31] = 1;
            bytes
        };

        // Bridge vector with explicit bytes and limbs.
        //
        // Calculation:
        // n = 1 + 2*p + 3*p^2 + 4*p^3 + 1*p^4, where p = PRIME
        // n (32-byte big-endian) =
        // 0xfffffffc0000000dffffffe40000002dffffffce0000002cffffffe80000000b
        let bridge_limbs = [1, 2, 3, 4, 1];
        let bridge_bytes = [
            0xff, 0xff, 0xff, 0xfc, 0x00, 0x00, 0x00, 0x0d, 0xff, 0xff, 0xff, 0xe4, 0x00, 0x00,
            0x00, 0x2d, 0xff, 0xff, 0xff, 0xce, 0x00, 0x00, 0x00, 0x2c, 0xff, 0xff, 0xff, 0xe8,
            0x00, 0x00, 0x00, 0x0b,
        ];

        let prime = BigUint::from(PRIME);
        let p2 = &prime * &prime;
        let p3 = &p2 * &prime;
        let p4 = &p3 * &prime;
        let bridge_value = BigUint::from(1u64)
            + BigUint::from(2u64) * &prime
            + BigUint::from(3u64) * &p2
            + BigUint::from(4u64) * &p3
            + BigUint::from(1u64) * &p4;
        assert_eq!(
            biguint_to_be_32(&bridge_value),
            bridge_bytes,
            "bridge vector bytes should match documented base-p expansion",
        );

        let test_vectors: [([u8; 32], [u64; 5]); 2] =
            [(one, [1, 0, 0, 0, 0]), (bridge_bytes, bridge_limbs)];

        for (input, expected_limbs) in test_vectors {
            let digest = Hash::from_be_bytes(&input);
            assert_eq!(
                digest.to_array(),
                expected_limbs,
                "unexpected base-p limbs for input bytes"
            );

            let atom = digest.to_atom();
            assert_eq!(
                biguint_to_be_32(&atom),
                input,
                "bytes -> tip5 -> atom should roundtrip back to the original 32 bytes"
            );
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

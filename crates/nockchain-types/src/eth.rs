use alloy::primitives::Address as AlloyAddress;
use hex::FromHex;
use nockvm::noun::{Atom, IndirectAtom, Noun, NounAllocator};
use noun_serde::{NounDecode, NounDecodeError, NounEncode};
use thiserror::Error;

/// 20-byte Ethereum-compatible address wrapper.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct EthAddress(pub [u8; Self::LEN]);

impl EthAddress {
    pub const LEN: usize = 20;
    pub const ZERO: Self = Self([0u8; Self::LEN]);

    pub fn as_bytes(&self) -> &[u8; Self::LEN] {
        &self.0
    }

    pub fn as_slice(&self) -> &[u8] {
        &self.0
    }

    /// Parses a hex string (optional `0x` prefix, underscores ignored) into an address.
    pub fn from_hex_str(raw: &str) -> Result<Self, EthAddressParseError> {
        let trimmed = raw.trim();
        if trimmed.is_empty() {
            return Err(EthAddressParseError::Empty);
        }

        let without_prefix = trimmed
            .strip_prefix("0x")
            .or_else(|| trimmed.strip_prefix("0X"))
            .unwrap_or(trimmed);
        let cleaned: String = without_prefix.chars().filter(|c| *c != '_').collect();
        let len = cleaned.len();
        if len != Self::LEN * 2 {
            return Err(EthAddressParseError::WrongLength(len));
        }
        if !cleaned.chars().all(|c| c.is_ascii_hexdigit()) {
            return Err(EthAddressParseError::InvalidCharacters);
        }

        let bytes = <[u8; Self::LEN]>::from_hex(&cleaned)
            .map_err(|err| EthAddressParseError::InvalidHex(err.to_string()))?;
        Ok(Self(bytes))
    }
}

impl From<[u8; EthAddress::LEN]> for EthAddress {
    fn from(value: [u8; EthAddress::LEN]) -> Self {
        Self(value)
    }
}

impl From<EthAddress> for [u8; EthAddress::LEN] {
    fn from(value: EthAddress) -> Self {
        value.0
    }
}

impl From<AlloyAddress> for EthAddress {
    fn from(value: AlloyAddress) -> Self {
        let mut bytes = [0u8; EthAddress::LEN];
        bytes.copy_from_slice(value.as_slice());
        Self(bytes)
    }
}

impl From<EthAddress> for AlloyAddress {
    fn from(value: EthAddress) -> Self {
        AlloyAddress::from_slice(&value.0)
    }
}

impl std::fmt::Display for EthAddress {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "0x")?;
        for byte in &self.0 {
            write!(f, "{:02x}", byte)?;
        }
        Ok(())
    }
}

impl NounEncode for EthAddress {
    fn to_noun<A: NounAllocator>(&self, allocator: &mut A) -> Noun {
        let mut le_bytes = self.0;
        le_bytes.reverse();
        let trimmed_len = le_bytes
            .iter()
            .rposition(|&b| b != 0)
            .map(|i| i + 1)
            .unwrap_or(0);

        if trimmed_len == 0 {
            return Atom::new(allocator, 0).as_noun();
        }

        unsafe {
            let mut atom = IndirectAtom::new_raw_bytes(allocator, trimmed_len, le_bytes.as_ptr());
            atom.normalize_as_atom().as_noun()
        }
    }
}

impl NounDecode for EthAddress {
    fn from_noun(noun: &Noun) -> Result<Self, NounDecodeError> {
        let atom = noun.as_atom().map_err(|_| NounDecodeError::ExpectedAtom)?;
        let bytes = atom.as_ne_bytes();
        let mut buf = [0u8; EthAddress::LEN];
        for (i, byte) in bytes.iter().enumerate() {
            if i < EthAddress::LEN {
                buf[EthAddress::LEN - 1 - i] = *byte;
            } else if *byte != 0 {
                return Err(NounDecodeError::Custom(
                    "EthAddress noun is longer than 20 bytes".into(),
                ));
            }
        }
        Ok(Self(buf))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum EthAddressParseError {
    #[error("EVM address cannot be empty")]
    Empty,
    #[error("EVM address must contain exactly 40 hex characters (20 bytes), got length {0}")]
    WrongLength(usize),
    #[error("EVM address must be valid hex (0-9, a-f)")]
    InvalidCharacters,
    #[error("Failed to parse EVM address: {0}")]
    InvalidHex(String),
}

#[cfg(test)]
mod tests {
    use ibig::UBig;
    use nockapp::noun::slab::{NockJammer, NounSlab};
    use nockvm::noun::Atom;

    use super::*;

    #[test]
    fn parse_hex_strings() {
        let addr = EthAddress::from_hex_str("0xaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa")
            .expect("parse works");
        assert_eq!(addr.as_bytes(), &[0xaa; EthAddress::LEN]);

        let addr = EthAddress::from_hex_str("AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA")
            .expect("upper case works");
        assert_eq!(addr.as_bytes(), &[0xaa; EthAddress::LEN]);
    }

    #[test]
    fn noun_roundtrip() {
        let mut slab = NounSlab::<NockJammer>::new();
        let addr = EthAddress::from([0x11; EthAddress::LEN]);
        let noun = addr.to_noun(&mut slab);
        let decoded = EthAddress::from_noun(&noun).expect("decode");
        assert_eq!(decoded, addr);
    }

    #[test]
    fn noun_encoding_preserves_byte_order() {
        let addr = EthAddress([
            0x01, 0x23, 0x45, 0x67, 0x89, 0xab, 0xcd, 0xef, 0xfe, 0xdc, 0xba, 0x98, 0x76, 0x54,
            0x32, 0x10, 0xaa, 0xbb, 0xcc, 0xdd,
        ]);

        let mut slab = NounSlab::<NockJammer>::new();
        let noun = addr.to_noun(&mut slab);
        let atom = noun
            .as_atom()
            .expect("expected EthAddress noun to be an atom");
        let be_bytes = {
            let mut bytes = atom.to_be_bytes();
            if bytes.len() < EthAddress::LEN {
                let mut padded = vec![0u8; EthAddress::LEN];
                padded[EthAddress::LEN - bytes.len()..].copy_from_slice(&bytes);
                bytes = padded;
            } else if bytes.len() > EthAddress::LEN {
                bytes = bytes[bytes.len() - EthAddress::LEN..].to_vec();
            }
            bytes
        };

        assert_eq!(be_bytes.as_slice(), addr.as_slice());
    }

    #[test]
    fn noun_decoding_preserves_byte_order() {
        let addr = EthAddress([
            0x01, 0x23, 0x45, 0x67, 0x89, 0xab, 0xcd, 0xef, 0xfe, 0xdc, 0xba, 0x98, 0x76, 0x54,
            0x32, 0x10, 0xaa, 0xbb, 0xcc, 0xdd,
        ]);

        let value = UBig::from_be_bytes(addr.as_slice());
        let mut slab = NounSlab::<NockJammer>::new();
        let noun = Atom::from_ubig(&mut slab, &value).as_noun();
        let decoded = EthAddress::from_noun(&noun).expect("decode");

        assert_eq!(decoded, addr);
    }

    #[test]
    fn display_is_lower_hex() {
        let addr =
            EthAddress::from_hex_str("0x0123456789abcdef0123456789abcdef01234567").expect("parse");
        assert_eq!(
            addr.to_string(),
            "0x0123456789abcdef0123456789abcdef01234567"
        );
    }
}

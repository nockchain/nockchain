use nockchain_types::tx_engine::v0;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub struct PageKeyAddress {
    pub address: String,
    pub height: u64,
    pub block_id: v0::Hash,
}

impl PageKeyAddress {
    pub fn new(address: String, height: u64, block_id: v0::Hash) -> Self {
        PageKeyAddress {
            address,
            height,
            block_id,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PageCursorAddress {
    pub key: PageKeyAddress,
    pub last_first: v0::Hash,
    pub last_last: v0::Hash,
}

impl PageCursorAddress {
    pub fn new(
        address: String,
        height: &v0::BlockHeight,
        block_id: &v0::Hash,
        name: &v0::Name,
    ) -> Self {
        PageCursorAddress {
            key: PageKeyAddress {
                address,
                height: height.0 .0,
                block_id: block_id.clone(),
            },
            last_first: name.first.clone(),
            last_last: name.last.clone(),
        }
    }
    pub fn key(&self) -> &PageKeyAddress {
        &self.key
    }
}

fn belts_to_array(belts: &[nockchain_math::belt::Belt; 5]) -> [u64; 5] {
    [belts[0].0, belts[1].0, belts[2].0, belts[3].0, belts[4].0]
}

// TODO: We can make the cursor opaque if folks abuse it
pub fn encode_cursor_address(cur: &PageCursorAddress) -> String {
    let bytes =
        bincode::serde::encode_to_vec(cur, bincode::config::standard()).expect("cursor encode");
    hex::encode(bytes)
}

pub fn decode_cursor_address(s: &str) -> Option<PageCursorAddress> {
    let bytes = hex::decode(s).ok()?;
    let (cur, _len): (PageCursorAddress, usize) =
        bincode::serde::decode_from_slice(&bytes, bincode::config::standard()).ok()?;
    Some(cur)
}

pub fn name_key(name: &v0::Name) -> ([u64; 5], [u64; 5]) {
    (belts_to_array(&name.first.0), belts_to_array(&name.last.0))
}

pub fn cmp_name(a: &v0::Name, b: &v0::Name) -> std::cmp::Ordering {
    let (af, al) = name_key(a);
    let (bf, bl) = name_key(b);
    af.cmp(&bf).then(al.cmp(&bl))
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub struct PageKeyFirstName {
    pub first_name: v0::Hash,
    pub height: u64,
    pub block_id: v0::Hash,
}

impl PageKeyFirstName {
    pub fn new(first_name: v0::Hash, height: u64, block_id: v0::Hash) -> Self {
        PageKeyFirstName {
            first_name,
            height,
            block_id,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PageCursorFirstName {
    pub key: PageKeyFirstName,
    pub last_first: v0::Hash,
    pub last_last: v0::Hash,
}

impl PageCursorFirstName {
    pub fn new(
        first_name: &v0::Hash,
        height: &v0::BlockHeight,
        block_id: &v0::Hash,
        name: &v0::Name,
    ) -> Self {
        PageCursorFirstName {
            key: PageKeyFirstName {
                first_name: first_name.clone(),
                height: height.0 .0,
                block_id: block_id.clone(),
            },
            last_first: name.first.clone(),
            last_last: name.last.clone(),
        }
    }
    pub fn key(&self) -> &PageKeyFirstName {
        &self.key
    }
}

pub fn encode_cursor_first_name(cur: &PageCursorFirstName) -> String {
    let bytes =
        bincode::serde::encode_to_vec(cur, bincode::config::standard()).expect("cursor encode");
    hex::encode(bytes)
}

pub fn decode_cursor_first_name(s: &str) -> Option<PageCursorFirstName> {
    let bytes = hex::decode(s).ok()?;
    let (cur, _len): (PageCursorFirstName, usize) =
        bincode::serde::decode_from_slice(&bytes, bincode::config::standard()).ok()?;
    Some(cur)
}

#[cfg(test)]
mod tests {
    use nockchain_math::belt::Belt;
    use nockchain_math::crypto::cheetah::A_GEN;

    use super::*;

    #[test]
    fn test_cursor_roundtrip() {
        let cur = PageCursorAddress {
            key: PageKeyAddress {
                address: A_GEN.into_base58().unwrap(),
                height: 42,
                block_id: v0::Hash([Belt(1), Belt(2), Belt(3), Belt(4), Belt(5)]),
            },
            last_first: v0::Hash([Belt(10), Belt(20), Belt(30), Belt(40), Belt(50)]),
            last_last: v0::Hash([Belt(11), Belt(22), Belt(33), Belt(44), Belt(55)]),
        };
        let s = encode_cursor_address(&cur);
        let cur2 = decode_cursor_address(&s).expect("decode ok");
        assert_eq!(cur.key.height, cur2.key.height);
        assert_eq!(cur.key.block_id, cur2.key.block_id);
        assert_eq!(cur.last_first, cur2.last_first);
        assert_eq!(cur.last_last, cur2.last_last);
    }

    #[test]
    fn test_cursor_decode_invalid() {
        assert!(decode_cursor_address("not-hex").is_none());
    }
}

use nockvm::ext::AtomExt;
use nockvm::noun::{Noun, NounAllocator, NounHandle, NounSpace};
use noun_serde::{NounDecode, NounDecodeError, NounEncode};
use num_bigint::BigUint;

use super::{Hash, TxId};

pub type BlockId = Hash;

fn atom_payload_bytes(atom: nockvm::noun::AtomHandle<'_>) -> Vec<u8> {
    let mut bytes = atom.as_ne_bytes().to_vec();
    while bytes.last() == Some(&0) {
        bytes.pop();
    }
    bytes
}

/// Decode a z-set into a Vec
/// z-set structure: either `~` (atom 0) for empty, or `[n=item l=tree r=tree]`
fn decode_zset<T: NounDecode>(noun: &NounHandle) -> Result<Vec<T>, NounDecodeError> {
    let mut result = Vec::new();
    collect_zset_items(noun, &mut result)?;
    Ok(result)
}

fn collect_zset_items<T: NounDecode>(
    noun: &NounHandle,
    result: &mut Vec<T>,
) -> Result<(), NounDecodeError> {
    // Empty set is atom 0
    if let Ok(atom) = noun.as_atom() {
        if atom.as_u64() == Ok(0) {
            return Ok(());
        }
        // Non-zero atom - shouldn't happen in a valid z-set
        return Err(NounDecodeError::Custom(
            "z-set: unexpected non-zero atom".into(),
        ));
    }

    // Non-empty set: [n=item l=tree r=tree]
    let cell = noun.as_cell()?;
    let n = cell.head();
    let lr = cell.tail().as_cell()?;
    let l = lr.head();
    let r = lr.tail();

    // Recursively collect from left subtree
    collect_zset_items(&l, result)?;

    // Add the node item
    let item = T::from_noun_handle(&n)?;
    result.push(item);

    // Recursively collect from right subtree
    collect_zset_items(&r, result)?;

    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BigNum(pub BigUint);

impl BigNum {
    pub fn from_u64(value: u64) -> Self {
        BigNum(BigUint::from(value))
    }
}

impl NounEncode for BigNum {
    fn to_noun<A: NounAllocator>(&self, allocator: &mut A) -> Noun {
        let bytes = self.0.to_bytes_le();
        if bytes.is_empty() {
            return nockvm::noun::Atom::new(allocator, 0).as_noun();
        }
        nockvm::noun::Atom::from_bytes(allocator, &bytes).as_noun()
    }
}

impl NounDecode for BigNum {
    fn from_noun(noun: &Noun, space: &NounSpace) -> Result<Self, NounDecodeError> {
        // Bignum in Hoon is [%bn p=(list u32)] - a tagged cell with u32 chunks
        if let Ok(cell) = noun.as_cell() {
            // Check for %bn tag
            if let Ok(tag) = cell.in_space(space).head().as_atom() {
                // %bn = 0x6e62 = 28258 in little-endian ('bn' as cord)
                let tag_val = tag.as_u64().unwrap_or(u64::MAX);
                if tag_val == 28258 {
                    // Decode tail as list of u32 chunks (LSB first)
                    let mut chunks: Vec<u32> = Vec::new();
                    let mut current = cell.in_space(space).tail();
                    while let Ok(list_cell) = current.as_cell() {
                        let chunk = list_cell
                            .head()
                            .as_atom()
                            .map_err(|_| NounDecodeError::Custom("BigNum: chunk not atom".into()))?
                            .as_u64()
                            .map_err(|_| {
                                NounDecodeError::Custom("BigNum: chunk too large for u64".into())
                            })?;
                        chunks.push(chunk as u32);
                        current = list_cell.tail();
                    }
                    // current should now be 0 (end of list)
                    if let Ok(end) = current.as_atom() {
                        if end.as_u64() == Ok(0) {
                            // Reconstruct the number: chunks are in LSB order, each is 32 bits
                            let mut result = BigUint::from(0u8);
                            let mut factor = BigUint::from(1u8);
                            let base = BigUint::from(1u64) << 32;
                            for chunk in chunks {
                                result += BigUint::from(chunk) * &factor;
                                factor *= &base;
                            }
                            return Ok(BigNum(result));
                        }
                    }
                    return Err(NounDecodeError::Custom(
                        "BigNum: list not terminated with 0".into(),
                    ));
                }
                // Cell but tag not 'bn' (28258) - report what tag we got
                return Err(NounDecodeError::Custom(format!(
                    "BigNum: cell with unknown tag val={} (expected 28258 for %bn)",
                    tag_val
                )));
            }
            // Cell but head not atom
            return Err(NounDecodeError::Custom(
                "BigNum: cell but head not atom (expected %bn tag)".into(),
            ));
        }
        // Fallback: try as raw atom (for compatibility)
        let atom = noun.as_atom().map_err(|_| {
            NounDecodeError::Custom("BigNum: expected atom or [%bn list] cell".into())
        })?;
        let atom_handle = atom.in_space(space);
        let bytes = atom_handle.as_ne_bytes();
        let biguint = BigUint::from_bytes_le(bytes);
        Ok(BigNum(biguint))
    }
}

pub type PageMsg = Vec<u32>;

/// Coinbase split - v0 is a byte list, v1 is a tagged z-map
/// For now we store the version tag and skip detailed parsing of v1 maps
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CoinbaseSplit {
    /// V0: list of bytes (legacy format)
    V0(Vec<u8>),
    /// V1: [%1 (z-map hash coins)] - we don't parse the map details for now
    V1,
}

impl NounEncode for CoinbaseSplit {
    fn to_noun<A: NounAllocator>(&self, allocator: &mut A) -> Noun {
        match self {
            CoinbaseSplit::V0(bytes) => bytes.to_noun(allocator),
            CoinbaseSplit::V1 => {
                // Encode as [%1 ~] for v1 (empty map placeholder)
                let tag = nockvm::noun::D(1);
                let empty_map = nockvm::noun::D(0);
                nockvm::noun::T(allocator, &[tag, empty_map])
            }
        }
    }
}

impl NounDecode for CoinbaseSplit {
    fn from_noun(noun: &Noun, space: &NounSpace) -> Result<Self, NounDecodeError> {
        // Check if it's a tagged cell [%0 data] or [%1 data]
        if let Ok(cell) = noun.as_cell() {
            if let Ok(tag_atom) = cell.in_space(space).head().as_atom() {
                if let Ok(tag) = tag_atom.as_u64() {
                    match tag {
                        0 => {
                            // V0: [%0 byte-list]
                            let bytes = Vec::<u8>::from_noun_handle(&cell.in_space(space).tail())?;
                            return Ok(CoinbaseSplit::V0(bytes));
                        }
                        1 => {
                            // V1: [%1 z-map] - we skip parsing the map for now
                            return Ok(CoinbaseSplit::V1);
                        }
                        _ => {}
                    }
                }
            }
        }
        // Fallback: try to decode as raw byte list (untagged v0)
        if let Ok(bytes) = Vec::<u8>::from_noun(noun, space) {
            return Ok(CoinbaseSplit::V0(bytes));
        }
        // If all else fails, treat as v1 (unknown structure)
        Ok(CoinbaseSplit::V1)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Page {
    pub digest: BlockId,
    pub pow: Option<Vec<u8>>,
    pub parent: BlockId,
    pub tx_ids: Vec<TxId>,
    pub coinbase: CoinbaseSplit,
    pub timestamp: u64,
    pub epoch_counter: u64,
    pub target: BigNum,
    pub accumulated_work: BigNum,
    pub height: u64,
    pub msg: PageMsg,
}

impl NounEncode for Page {
    fn to_noun<A: NounAllocator>(&self, allocator: &mut A) -> Noun {
        // TODO: should not be hardcoding v1 here, need a better solution
        let version = nockvm::noun::D(1);
        let digest = self.digest.to_noun(allocator);

        let pow = if let Some(ref pow_data) = self.pow {
            let bytes_noun = pow_data.to_noun(allocator);
            nockvm::noun::T(allocator, &[nockvm::noun::D(0), bytes_noun])
        } else {
            nockvm::noun::D(0)
        };

        let parent = self.parent.to_noun(allocator);
        let tx_ids = self.tx_ids.to_noun(allocator);
        let coinbase = self.coinbase.to_noun(allocator);
        let timestamp = nockvm::noun::Atom::new(allocator, self.timestamp).as_noun();
        let epoch_counter = nockvm::noun::Atom::new(allocator, self.epoch_counter).as_noun();
        let target = self.target.to_noun(allocator);
        let accumulated_work = self.accumulated_work.to_noun(allocator);
        let height = nockvm::noun::Atom::new(allocator, self.height).as_noun();
        let msg = self.msg.to_noun(allocator);

        nockvm::noun::T(
            allocator,
            &[
                version, digest, pow, parent, tx_ids, coinbase, timestamp, epoch_counter, target,
                accumulated_work, height, msg,
            ],
        )
    }
}

impl NounDecode for Page {
    // TODO: Purge these custom Page NounDecode/NounEncode implementations in favor of
    // the standard noun-serde path once it can represent this shape directly.
    fn from_noun(noun: &Noun, space: &NounSpace) -> Result<Self, NounDecodeError> {
        let cell = noun
            .as_cell()
            .map_err(|_| NounDecodeError::Custom("Page: expected cell at root".into()))?;

        // Check if this is a v1 page (head is atom %1) or v0 page (head is cell/digest)
        let (digest, rest_after_digest) =
            if cell.in_space(space).head().is_atom() {
                // v1 page: [%1 digest pow parent ...]
                let version = cell
                    .in_space(space)
                    .head()
                    .as_atom()
                    .map_err(|_| NounDecodeError::Custom("Page: version tag not atom".into()))?
                    .as_u64()
                    .map_err(|_| NounDecodeError::Custom("Page: version tag too large".into()))?;
                if version != 1 {
                    return Err(NounDecodeError::Custom(format!(
                        "Page: unknown version: {}",
                        version
                    )));
                }
                // Skip version tag, get digest from tail
                let rest = cell.in_space(space).tail().as_cell().map_err(|_| {
                    NounDecodeError::Custom("Page: expected cell after version".into())
                })?;
                let digest = BlockId::from_noun_handle(&rest.head())
                    .map_err(|e| NounDecodeError::Custom(format!("Page.digest: {}", e)))?;
                let rest_after = rest.tail().as_cell().map_err(|_| {
                    NounDecodeError::Custom("Page: expected cell after digest".into())
                })?;
                (digest, rest_after)
            } else {
                // v0 page: [digest pow parent ...]
                let digest = BlockId::from_noun_handle(&cell.in_space(space).head())
                    .map_err(|e| NounDecodeError::Custom(format!("Page.digest(v0): {}", e)))?;
                let rest_after = cell.in_space(space).tail().as_cell().map_err(|_| {
                    NounDecodeError::Custom("Page: expected cell after digest(v0)".into())
                })?;
                (digest, rest_after)
            };

        // POW: (unit proof). The proof payload is opaque to this crate, but
        // page noun roundtrips must preserve its bytes exactly.
        let pow_noun = rest_after_digest.head();
        let pow = if pow_noun.is_atom() {
            let atom = pow_noun
                .as_atom()
                .map_err(|_| NounDecodeError::Custom("Page.pow: expected atom".into()))?;
            if atom.as_u64() == Ok(0) {
                None
            } else {
                // Non-zero atom - treat as raw proof bytes
                Some(atom_payload_bytes(atom))
            }
        } else {
            // Unit `some`: `[~ proof]`, where this Rust encoder stores
            // `proof` as a byte list.
            let cell = pow_noun
                .as_cell()
                .map_err(|_| NounDecodeError::Custom("Page.pow: expected cell".into()))?;
            let tag = cell
                .head()
                .as_atom()
                .map_err(|_| NounDecodeError::Custom("Page.pow: unit tag not atom".into()))?
                .as_u64()
                .map_err(|_| NounDecodeError::Custom("Page.pow: unit tag too large".into()))?;
            if tag != 0 {
                return Err(NounDecodeError::Custom(format!(
                    "Page.pow: invalid unit tag {tag}"
                )));
            }
            let payload = cell.tail();
            if payload.is_atom() {
                let atom = payload
                    .as_atom()
                    .map_err(|_| NounDecodeError::Custom("Page.pow: payload not atom".into()))?;
                Some(atom_payload_bytes(atom))
            } else {
                Some(
                    Vec::<u8>::from_noun_handle(&payload)
                        .map_err(|e| NounDecodeError::Custom(format!("Page.pow: {}", e)))?,
                )
            }
        };

        let rest = rest_after_digest
            .tail()
            .as_cell()
            .map_err(|_| NounDecodeError::Custom("Page: expected cell after pow".into()))?;

        let parent = BlockId::from_noun_handle(&rest.head())
            .map_err(|e| NounDecodeError::Custom(format!("Page.parent: {}", e)))?;

        let rest = rest
            .tail()
            .as_cell()
            .map_err(|_| NounDecodeError::Custom("Page: expected cell after parent".into()))?;

        let tx_ids = decode_zset::<TxId>(&rest.head())
            .map_err(|e| NounDecodeError::Custom(format!("Page.tx_ids: {}", e)))?;

        let rest = rest
            .tail()
            .as_cell()
            .map_err(|_| NounDecodeError::Custom("Page: expected cell after tx_ids".into()))?;

        let coinbase = CoinbaseSplit::from_noun_handle(&rest.head())
            .map_err(|e| NounDecodeError::Custom(format!("Page.coinbase: {}", e)))?;

        let rest = rest
            .tail()
            .as_cell()
            .map_err(|_| NounDecodeError::Custom("Page: expected cell after coinbase".into()))?;

        let timestamp = rest
            .head()
            .as_atom()
            .map_err(|_| NounDecodeError::Custom("Page.timestamp: expected atom".into()))?
            .as_u64()
            .map_err(|_| NounDecodeError::Custom("Page.timestamp: too large".into()))?;

        let rest = rest
            .tail()
            .as_cell()
            .map_err(|_| NounDecodeError::Custom("Page: expected cell after timestamp".into()))?;

        let epoch_counter = rest
            .head()
            .as_atom()
            .map_err(|_| NounDecodeError::Custom("Page.epoch_counter: expected atom".into()))?
            .as_u64()
            .map_err(|_| NounDecodeError::Custom("Page.epoch_counter: too large".into()))?;

        let rest = rest.tail().as_cell().map_err(|_| {
            NounDecodeError::Custom("Page: expected cell after epoch_counter".into())
        })?;

        let target = BigNum::from_noun_handle(&rest.head())
            .map_err(|e| NounDecodeError::Custom(format!("Page.target: {}", e)))?;

        let rest = rest
            .tail()
            .as_cell()
            .map_err(|_| NounDecodeError::Custom("Page: expected cell after target".into()))?;

        let accumulated_work = BigNum::from_noun(&rest.head().noun(), space)
            .map_err(|e| NounDecodeError::Custom(format!("Page.accumulated_work: {}", e)))?;

        let rest = rest.tail().as_cell().map_err(|_| {
            NounDecodeError::Custom("Page: expected cell after accumulated_work".into())
        })?;

        let height = rest
            .head()
            .as_atom()
            .map_err(|_| NounDecodeError::Custom("Page.height: expected atom".into()))?
            .as_u64()
            .map_err(|_| NounDecodeError::Custom("Page.height: too large".into()))?;

        let msg = PageMsg::from_noun_handle(&rest.tail())
            .map_err(|e| NounDecodeError::Custom(format!("Page.msg: {}", e)))?;

        Ok(Page {
            digest,
            pow,
            parent,
            tx_ids,
            coinbase,
            timestamp,
            epoch_counter,
            target,
            accumulated_work,
            height,
            msg,
        })
    }
}

#[cfg(test)]
mod tests {
    use nockapp::noun::slab::NounSlab;
    use nockchain_math::belt::Belt;
    use nockvm::ext::AtomExt;
    use nockvm::noun::{Atom, NounAllocator};
    use noun_serde::{NounDecode, NounEncode};

    use super::{BigNum, CoinbaseSplit, Page};
    use crate::tx_engine::common::Hash;

    fn test_hash(seed: u64) -> Hash {
        Hash([Belt(seed), Belt(seed + 1), Belt(seed + 2), Belt(seed + 3), Belt(seed + 4)])
    }

    #[test]
    fn page_pow_unit_roundtrip_preserves_opaque_bytes() {
        let proof_bytes = (0..=255).collect::<Vec<u8>>();
        let page = Page {
            digest: test_hash(10),
            pow: Some(proof_bytes.clone()),
            parent: test_hash(20),
            tx_ids: vec![],
            coinbase: CoinbaseSplit::V0(vec![1, 2, 3]),
            timestamp: 123,
            epoch_counter: 456,
            target: BigNum::from_u64(789),
            accumulated_work: BigNum::from_u64(1_000),
            height: 42,
            msg: vec![7, 8, 9],
        };

        let mut slab: NounSlab = NounSlab::new();
        let noun = page.to_noun(&mut slab);
        let space = slab.noun_space();
        let decoded = Page::from_noun(&noun, &space).expect("page should decode");

        assert_eq!(decoded.pow, Some(proof_bytes));
        assert_eq!(decoded.digest, page.digest);
        assert_eq!(decoded.parent, page.parent);
        assert_eq!(decoded.msg, page.msg);
    }

    #[test]
    fn page_pow_unit_atom_decodes_as_opaque_jam_bytes() {
        let proof_bytes = vec![0xa1, 0xb2, 0xc3, 0xd4, 0xe5];
        let page = Page {
            digest: test_hash(30),
            pow: None,
            parent: test_hash(40),
            tx_ids: vec![],
            coinbase: CoinbaseSplit::V0(vec![]),
            timestamp: 321,
            epoch_counter: 654,
            target: BigNum::from_u64(987),
            accumulated_work: BigNum::from_u64(2_000),
            height: 43,
            msg: vec![],
        };

        let mut slab: NounSlab = NounSlab::new();
        let pow_atom = Atom::from_bytes(&mut slab, &proof_bytes).as_noun();
        let pow = nockvm::noun::T(&mut slab, &[nockvm::noun::D(0), pow_atom]);
        let digest = page.digest.to_noun(&mut slab);
        let parent = page.parent.to_noun(&mut slab);
        let tx_ids = page.tx_ids.to_noun(&mut slab);
        let coinbase = page.coinbase.to_noun(&mut slab);
        let timestamp = Atom::new(&mut slab, page.timestamp).as_noun();
        let epoch_counter = Atom::new(&mut slab, page.epoch_counter).as_noun();
        let target = page.target.to_noun(&mut slab);
        let accumulated_work = page.accumulated_work.to_noun(&mut slab);
        let height = Atom::new(&mut slab, page.height).as_noun();
        let msg = page.msg.to_noun(&mut slab);
        let noun = nockvm::noun::T(
            &mut slab,
            &[
                nockvm::noun::D(1),
                digest,
                pow,
                parent,
                tx_ids,
                coinbase,
                timestamp,
                epoch_counter,
                target,
                accumulated_work,
                height,
                msg,
            ],
        );
        let space = slab.noun_space();
        let decoded = Page::from_noun(&noun, &space).expect("page should decode");

        assert_eq!(decoded.pow, Some(proof_bytes));
    }
}

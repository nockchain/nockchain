use ibig::UBig;
use nockvm::noun::{Noun, NounAllocator};
use noun_serde::{NounDecode, NounDecodeError, NounEncode};

use super::{Hash, TxId};

pub type BlockId = Hash;

/// Decode a z-set into a Vec
/// z-set structure: either `~` (atom 0) for empty, or `[n=item l=tree r=tree]`
fn decode_zset<T: NounDecode>(noun: &Noun) -> Result<Vec<T>, NounDecodeError> {
    let mut result = Vec::new();
    collect_zset_items(noun, &mut result)?;
    Ok(result)
}

fn collect_zset_items<T: NounDecode>(
    noun: &Noun,
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
    let item = T::from_noun(&n)?;
    result.push(item);

    // Recursively collect from right subtree
    collect_zset_items(&r, result)?;

    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BigNum(pub UBig);

impl BigNum {
    pub fn from_u64(value: u64) -> Self {
        BigNum(UBig::from(value))
    }
}

impl NounEncode for BigNum {
    fn to_noun<A: NounAllocator>(&self, allocator: &mut A) -> Noun {
        nockvm::noun::Atom::from_ubig(allocator, &self.0).as_noun()
    }
}

impl NounDecode for BigNum {
    fn from_noun(noun: &Noun) -> Result<Self, NounDecodeError> {
        // Bignum in Hoon is [%bn p=(list u32)] - a tagged cell with u32 chunks
        if let Ok(cell) = noun.as_cell() {
            // Check for %bn tag
            if let Ok(tag) = cell.head().as_atom() {
                // %bn = 0x6e62 = 28258 in little-endian ('bn' as cord)
                let tag_val = tag.as_u64().unwrap_or(u64::MAX);
                if tag_val == 28258 {
                    // Decode tail as list of u32 chunks (LSB first)
                    let mut chunks: Vec<u32> = Vec::new();
                    let mut current = cell.tail();
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
                            let mut result = UBig::from(0u32);
                            for (i, chunk) in chunks.iter().enumerate() {
                                let shift = i * 32;
                                result += UBig::from(*chunk) << shift;
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
        let bytes = atom.as_ne_bytes();
        let ubig = UBig::from_le_bytes(bytes);
        Ok(BigNum(ubig))
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
    fn from_noun(noun: &Noun) -> Result<Self, NounDecodeError> {
        // Check if it's a tagged cell [%0 data] or [%1 data]
        if let Ok(cell) = noun.as_cell() {
            if let Ok(tag_atom) = cell.head().as_atom() {
                if let Ok(tag) = tag_atom.as_u64() {
                    match tag {
                        0 => {
                            // V0: [%0 byte-list]
                            let bytes = Vec::<u8>::from_noun(&cell.tail())?;
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
        if let Ok(bytes) = Vec::<u8>::from_noun(noun) {
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
    fn from_noun(noun: &Noun) -> Result<Self, NounDecodeError> {
        let cell = noun
            .as_cell()
            .map_err(|_| NounDecodeError::Custom("Page: expected cell at root".into()))?;

        // Check if this is a v1 page (head is atom %1) or v0 page (head is cell/digest)
        let (digest, rest_after_digest) =
            if cell.head().is_atom() {
                // v1 page: [%1 digest pow parent ...]
                let version = cell
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
                let rest = cell.tail().as_cell().map_err(|_| {
                    NounDecodeError::Custom("Page: expected cell after version".into())
                })?;
                let digest = BlockId::from_noun(&rest.head())
                    .map_err(|e| NounDecodeError::Custom(format!("Page.digest: {}", e)))?;
                let rest_after = rest.tail().as_cell().map_err(|_| {
                    NounDecodeError::Custom("Page: expected cell after digest".into())
                })?;
                (digest, rest_after)
            } else {
                // v0 page: [digest pow parent ...]
                let digest = BlockId::from_noun(&cell.head())
                    .map_err(|e| NounDecodeError::Custom(format!("Page.digest(v0): {}", e)))?;
                let rest_after = cell.tail().as_cell().map_err(|_| {
                    NounDecodeError::Custom("Page: expected cell after digest(v0)".into())
                })?;
                (digest, rest_after)
            };

        // POW: (unit proof) - we just detect presence, don't decode the proof structure
        let pow_noun = rest_after_digest.head();
        let pow = if pow_noun.is_atom() {
            let atom = pow_noun
                .as_atom()
                .map_err(|_| NounDecodeError::Custom("Page.pow: expected atom".into()))?;
            if atom.as_u64() == Ok(0) {
                None
            } else {
                // Non-zero atom - treat as raw proof bytes
                Some(atom.as_ne_bytes().to_vec())
            }
        } else {
            // Cell means [~ proof] - proof is present but we don't decode it
            Some(vec![])
        };

        let rest = rest_after_digest
            .tail()
            .as_cell()
            .map_err(|_| NounDecodeError::Custom("Page: expected cell after pow".into()))?;

        let parent = BlockId::from_noun(&rest.head())
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

        let coinbase = CoinbaseSplit::from_noun(&rest.head())
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

        let target = BigNum::from_noun(&rest.head())
            .map_err(|e| NounDecodeError::Custom(format!("Page.target: {}", e)))?;

        let rest = rest
            .tail()
            .as_cell()
            .map_err(|_| NounDecodeError::Custom("Page: expected cell after target".into()))?;

        let accumulated_work = BigNum::from_noun(&rest.head())
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

        let msg = PageMsg::from_noun(&rest.tail())
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

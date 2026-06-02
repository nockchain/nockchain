//! Types for speed-of-light benchmark data.
//!
//! The extractor is archive-oriented: it only needs stable block metadata
//! plus raw jam bytes, not full historical page/transaction decoding.

use nockchain_types::tx_engine::common::{BlockHeight, Hash};
use nockvm::noun::Noun;
use noun_serde::NounDecodeError;
use serde::{Deserialize, Serialize};

use super::noun_compat;

/// Height wrapper for speed-of-light data structures.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct SolHeight(pub u64);

impl SolHeight {
    pub const ZERO: SolHeight = SolHeight(0);
    pub const MAX: SolHeight = SolHeight(u64::MAX);

    pub fn as_u64(self) -> u64 {
        self.0
    }

    pub fn saturating_add(self, rhs: u64) -> SolHeight {
        SolHeight(self.0.saturating_add(rhs))
    }

    pub fn saturating_sub(self, rhs: u64) -> SolHeight {
        SolHeight(self.0.saturating_sub(rhs))
    }
}

impl From<u64> for SolHeight {
    fn from(value: u64) -> Self {
        SolHeight(value)
    }
}

impl From<SolHeight> for u64 {
    fn from(value: SolHeight) -> Self {
        value.0
    }
}

impl std::fmt::Display for SolHeight {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl Default for SolHeight {
    fn default() -> Self {
        SolHeight::ZERO
    }
}

/// Proof version cutover heights (from hoon/apps/dumbnet/lib/consensus.hoon)
pub const PROOF_VERSION_1_START: u64 = 6_750;
pub const PROOF_VERSION_2_START: u64 = 12_000;

/// Proof version for a block
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[repr(u8)]
pub enum ProofVersion {
    V0 = 0,
    V1 = 1,
    V2 = 2,
}

impl ProofVersion {
    /// Map a block height to its proof version using consensus cutovers.
    pub fn for_height(height: SolHeight) -> Self {
        if height.0 >= PROOF_VERSION_2_START {
            ProofVersion::V2
        } else if height.0 >= PROOF_VERSION_1_START {
            ProofVersion::V1
        } else {
            ProofVersion::V0
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            ProofVersion::V0 => "v0",
            ProofVersion::V1 => "v1",
            ProofVersion::V2 => "v2",
        }
    }
}

impl std::fmt::Display for ProofVersion {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

/// Stable metadata needed by archive writing without full block decode.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ArchiveBlockSummary {
    pub height: SolHeight,
    pub block_id: Hash,
    pub tx_count: usize,
    pub proof_version: ProofVersion,
}

/// Summarize a raw block-range entry noun for archive metadata only.
///
/// This intentionally stays shallow: it reads `[height [block-id [page txs]]]`
/// without decoding the page or transaction payload values.
pub(crate) fn summarize_archive_entry(
    entry_noun: Noun,
    space: &noun_compat::NounSpace,
) -> Result<ArchiveBlockSummary, NounDecodeError> {
    let height_noun = noun_compat::noun_head(entry_noun, space)?;
    let height = noun_compat::decode_with_space::<BlockHeight>(&height_noun, space)?;

    let tail = noun_compat::noun_tail(entry_noun, space)?;
    let block_id_noun = noun_compat::noun_head(tail, space)?;
    let block_id = noun_compat::decode_with_space::<Hash>(&block_id_noun, space)?;

    let page_and_txs = noun_compat::noun_tail(tail, space)?;
    let txs_noun = noun_compat::noun_tail(page_and_txs, space)?;
    let tx_count = tx_map_len(&txs_noun, space)?;
    let height = SolHeight(height.0 .0);

    Ok(ArchiveBlockSummary {
        height,
        block_id,
        tx_count,
        proof_version: ProofVersion::for_height(height),
    })
}

pub(crate) fn tx_map_len(
    txs_noun: &Noun,
    space: &noun_compat::NounSpace,
) -> Result<usize, NounDecodeError> {
    if txs_noun.is_atom() {
        if noun_compat::atom_is_zero(txs_noun, space)? {
            return Ok(0);
        }
        return Err(NounDecodeError::ExpectedCell);
    }

    Ok(noun_compat::hoon_map_entries(*txs_noun, space)
        .into_iter()
        .filter(|entry| entry.is_cell())
        .count())
}

#[cfg(test)]
mod tests {
    use nockapp::noun::slab::NounSlab;
    use nockchain_math::belt::Belt;
    use nockchain_math::zoon::common::DefaultTipHasher;
    use nockchain_math::zoon::zmap;
    use nockvm::noun::{D, T};
    use noun_serde::NounEncode;

    use super::*;

    fn dummy_hash(v: u64) -> Hash {
        Hash([Belt(v), Belt(v + 1), Belt(v + 2), Belt(v + 3), Belt(v + 4)])
    }

    fn tx_map_with_atom_payloads(slab: &mut NounSlab, entries: &[(Hash, u64)]) -> Noun {
        entries.iter().fold(D(0), |map, (tx_id, payload)| {
            let mut key = tx_id.to_noun(slab);
            let mut value = D(*payload);
            zmap::z_map_put(slab, &map, &mut key, &mut value, &DefaultTipHasher)
                .expect("tx z-map insert should succeed")
        })
    }

    fn v0_page_noun(slab: &mut NounSlab, parent: Hash, timestamp: u64, height: u64) -> Noun {
        let digest = dummy_hash(height + 10_000).to_noun(slab);
        let pow = D(0);
        let parent = parent.to_noun(slab);
        let tx_ids = D(0);
        let coinbase = D(0);
        let timestamp = D(timestamp);
        let epoch_counter = D(0);
        let target = D(0);
        let accumulated_work = D(0);
        let height = BlockHeight(Belt(height)).to_noun(slab);
        let msg = D(0);

        T(
            slab,
            &[
                digest, pow, parent, tx_ids, coinbase, timestamp, epoch_counter, target,
                accumulated_work, height, msg,
            ],
        )
    }

    fn v1_page_noun(slab: &mut NounSlab, parent: Hash, timestamp: u64, height: u64) -> Noun {
        let version = D(1);
        let digest = dummy_hash(height + 20_000).to_noun(slab);
        let pow = D(0);
        let parent = parent.to_noun(slab);
        let tx_ids = D(0);
        let coinbase = T(slab, &[D(1), D(0)]);
        let timestamp = D(timestamp);
        let epoch_counter = D(0);
        let target = D(0);
        let accumulated_work = D(0);
        let height = BlockHeight(Belt(height)).to_noun(slab);
        let msg = D(0);

        T(
            slab,
            &[
                version, digest, pow, parent, tx_ids, coinbase, timestamp, epoch_counter, target,
                accumulated_work, height, msg,
            ],
        )
    }

    fn block_range_entry_noun(
        slab: &mut NounSlab,
        height: u64,
        block_id: Hash,
        page: Noun,
        txs: Noun,
    ) -> Noun {
        let height = BlockHeight(Belt(height)).to_noun(slab);
        let block_id = block_id.to_noun(slab);
        let page_and_txs = T(slab, &[page, txs]);
        let tail = T(slab, &[block_id, page_and_txs]);
        T(slab, &[height, tail])
    }

    fn slab_space<J>(slab: &NounSlab<J>) -> noun_compat::NounSpace {
        noun_compat::space_for_slab(slab)
    }

    #[test]
    fn test_proof_version_for_height_boundaries() {
        assert_eq!(ProofVersion::for_height(SolHeight(0)), ProofVersion::V0);
        assert_eq!(
            ProofVersion::for_height(SolHeight(PROOF_VERSION_1_START - 1)),
            ProofVersion::V0
        );
        assert_eq!(
            ProofVersion::for_height(SolHeight(PROOF_VERSION_1_START)),
            ProofVersion::V1
        );
        assert_eq!(
            ProofVersion::for_height(SolHeight(PROOF_VERSION_2_START - 1)),
            ProofVersion::V1
        );
        assert_eq!(
            ProofVersion::for_height(SolHeight(PROOF_VERSION_2_START)),
            ProofVersion::V2
        );
    }

    #[test]
    fn test_tx_map_len_returns_zero_for_zero_atom() {
        let slab: NounSlab = NounSlab::new();
        assert_eq!(
            tx_map_len(&D(0), &slab_space(&slab)).expect("zero should decode"),
            0
        );
    }

    #[test]
    fn test_tx_map_len_counts_map_entries() {
        let mut slab = NounSlab::new();
        let txs = tx_map_with_atom_payloads(
            &mut slab,
            &[(dummy_hash(7_000), 11), (dummy_hash(7_001), 22), (dummy_hash(7_002), 33)],
        );

        assert_eq!(
            tx_map_len(&txs, &slab_space(&slab)).expect("tx map length should decode"),
            3
        );
    }

    #[test]
    fn test_summarize_archive_entry_reads_height_block_id_and_tx_count() {
        let mut slab = NounSlab::new();
        let height = 42;
        let block_id = dummy_hash(8_000);
        let parent_id = dummy_hash(7_999);
        let page = v0_page_noun(&mut slab, parent_id, 1_700_000_042, height);
        let txs =
            tx_map_with_atom_payloads(&mut slab, &[(dummy_hash(8_100), 5), (dummy_hash(8_101), 6)]);
        let entry = block_range_entry_noun(&mut slab, height, block_id.clone(), page, txs);

        let summary = summarize_archive_entry(entry, &slab_space(&slab))
            .expect("summary decode should succeed");

        assert_eq!(summary.height, SolHeight(height));
        assert_eq!(summary.block_id.to_base58(), block_id.to_base58());
        assert_eq!(summary.tx_count, 2);
        assert_eq!(summary.proof_version, ProofVersion::V0);
    }

    #[test]
    fn test_summarize_archive_entry_v0_page_counts_atom_payload_txs() {
        let mut slab = NounSlab::new();
        let height = 12;
        let block_id = dummy_hash(1_000);
        let parent_id = dummy_hash(999);
        let page = v0_page_noun(&mut slab, parent_id.clone(), 1_700_000_012, height);
        let txs = tx_map_with_atom_payloads(
            &mut slab,
            &[(dummy_hash(2_000), 11), (dummy_hash(2_001), 22), (dummy_hash(2_002), 33)],
        );
        let entry = block_range_entry_noun(&mut slab, height, block_id.clone(), page, txs);

        let summary = summarize_archive_entry(entry, &slab_space(&slab))
            .expect("summary decode should succeed");

        assert_eq!(summary.height, SolHeight(height));
        assert_eq!(summary.block_id.to_base58(), block_id.to_base58());
        assert_eq!(summary.tx_count, 3);
        assert_eq!(summary.proof_version, ProofVersion::V0);
    }

    #[test]
    fn test_summarize_archive_entry_v1_page_preserves_height_and_proof_version() {
        let mut slab = NounSlab::new();
        let height = PROOF_VERSION_1_START;
        let block_id = dummy_hash(3_000);
        let parent_id = dummy_hash(2_999);
        let page = v1_page_noun(&mut slab, parent_id, 1_800_000_000, height);
        let txs = tx_map_with_atom_payloads(&mut slab, &[(dummy_hash(4_000), 99)]);
        let entry = block_range_entry_noun(&mut slab, height, block_id.clone(), page, txs);

        let summary = summarize_archive_entry(entry, &slab_space(&slab))
            .expect("summary decode should succeed");

        assert_eq!(summary.height, SolHeight(height));
        assert_eq!(summary.block_id.to_base58(), block_id.to_base58());
        assert_eq!(summary.tx_count, 1);
        assert_eq!(summary.proof_version, ProofVersion::V1);
    }

    #[test]
    fn test_summarize_archive_entry_v2_height_uses_v2_proof_version() {
        let mut slab = NounSlab::new();
        let height = PROOF_VERSION_2_START;
        let block_id = dummy_hash(5_000);
        let parent_id = dummy_hash(4_999);
        let page = v1_page_noun(&mut slab, parent_id, 1_900_000_000, height);
        let txs = tx_map_with_atom_payloads(&mut slab, &[]);
        let entry = block_range_entry_noun(&mut slab, height, block_id.clone(), page, txs);

        let summary = summarize_archive_entry(entry, &slab_space(&slab))
            .expect("summary decode should succeed");

        assert_eq!(summary.height, SolHeight(height));
        assert_eq!(summary.block_id.to_base58(), block_id.to_base58());
        assert_eq!(summary.tx_count, 0);
        assert_eq!(summary.proof_version, ProofVersion::V2);
    }
}

use super::archive::{ArchiveError, ArchiveFilter, SolArchiveReader};
use super::final_tip::ExpectedFinalTip;
use super::types::{ProofVersion, SolHeight};

#[derive(Debug, Clone)]
pub struct ReplayWindowOptions {
    pub filter: ArchiveFilter,
    pub skip_genesis: bool,
    pub block_limit: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SelectedReplayBlock {
    pub height: SolHeight,
    pub block_hash: String,
    pub proof_version: ProofVersion,
    pub tx_count: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReplayWindow {
    pub blocks: Vec<SelectedReplayBlock>,
    pub contiguous: bool,
    pub first_gap_height: Option<SolHeight>,
    pub expected_final_tip: Option<ExpectedFinalTip>,
}

pub fn select_replay_window(
    reader: &SolArchiveReader,
    options: ReplayWindowOptions,
) -> Result<ReplayWindow, ArchiveError> {
    let mut blocks = Vec::new();
    for entry in reader.iter_filtered(options.filter) {
        if options.skip_genesis && entry.height == SolHeight::ZERO {
            continue;
        }
        if options
            .block_limit
            .is_some_and(|limit| blocks.len() as u64 >= limit)
        {
            break;
        }
        blocks.push(SelectedReplayBlock {
            height: entry.height,
            block_hash: entry.block_id.to_base58(),
            proof_version: entry.proof_version,
            tx_count: entry.tx_count,
        });
    }

    let first_gap_height = first_gap_height(&blocks);
    let contiguous = first_gap_height.is_none();
    let expected_final_tip = if contiguous {
        blocks.last().map(|block| ExpectedFinalTip {
            height: block.height.as_u64(),
            hash: block.block_hash.clone(),
        })
    } else {
        None
    };

    Ok(ReplayWindow {
        blocks,
        contiguous,
        first_gap_height,
        expected_final_tip,
    })
}

fn first_gap_height(blocks: &[SelectedReplayBlock]) -> Option<SolHeight> {
    blocks.windows(2).find_map(|pair| {
        let expected = pair[0].height.saturating_add(1);
        (expected != pair[1].height).then_some(expected)
    })
}

#[cfg(test)]
mod tests {
    use nockchain_math::belt::Belt;
    use nockchain_types::tx_engine::common::Hash;

    use super::*;
    use crate::speed_of_light::archive::{RawTxPayload, SolArchiveWriter};

    fn dummy_hash(v: u64) -> Hash {
        Hash([Belt(v), Belt(v + 1), Belt(v + 2), Belt(v + 3), Belt(v + 4)])
    }

    #[test]
    fn window_applies_skip_limit_and_contiguity() {
        let mut writer = SolArchiveWriter::new();
        for height in 0u64..3 {
            let raw_tx = [height as u8, 0xAA];
            writer
                .add_block_with_raw_txs(
                    SolHeight(height),
                    dummy_hash(height),
                    ProofVersion::V0,
                    &[height as u8],
                    [RawTxPayload {
                        tx_id: dummy_hash(100 + height),
                        jam_bytes: &raw_tx,
                    }],
                )
                .expect("add archive block");
        }
        let reader = SolArchiveReader::from_bytes(writer.to_bytes().expect("serialize"))
            .expect("read archive");

        let window = select_replay_window(
            &reader,
            ReplayWindowOptions {
                filter: ArchiveFilter::default(),
                skip_genesis: true,
                block_limit: Some(1),
            },
        )
        .expect("replay window");

        assert_eq!(window.blocks.len(), 1);
        assert_eq!(window.blocks[0].height, SolHeight(1));
        assert!(window.contiguous);
        assert_eq!(
            window.expected_final_tip,
            Some(ExpectedFinalTip {
                height: 1,
                hash: dummy_hash(1).to_base58(),
            })
        );
    }
}

//! Mempool snapshot inspector for speed-of-light archives.

use std::collections::{HashMap, HashSet};

use nockchain_types::tx_engine::common::Hash;
use thiserror::Error;

use super::archive::{ArchiveError, MempoolSnapshotEntry, SolArchiveReader};
use super::types::SolHeight;

#[derive(Debug, Error)]
pub enum InspectorError {
    #[error("Archive error: {0}")]
    Archive(#[from] ArchiveError),

    #[error("Archive does not contain mempool snapshots")]
    MempoolNotPresent,
}

/// A contiguous presence range for a transaction in the mempool snapshots.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TxPresenceRange {
    pub tx_id: Hash,
    pub heard_at: SolHeight,
    pub start_height: SolHeight,
    pub end_height: SolHeight,
}

/// A contiguous stale range for a transaction (age >= retain).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StaleTxRange {
    pub tx_id: Hash,
    pub heard_at: SolHeight,
    pub start_height: SolHeight,
    pub end_height: SolHeight,
}

/// Find stale transaction ranges (age >= retain) from mempool snapshots.
pub fn find_stale_ranges(
    reader: &SolArchiveReader,
    retain: u64,
) -> Result<Vec<StaleTxRange>, InspectorError> {
    if !reader.has_mempool() {
        return Err(InspectorError::MempoolNotPresent);
    }

    let presence = build_presence_ranges(reader)?;
    let mut stale_ranges = Vec::new();

    for range in presence {
        let stale_threshold = range.heard_at.saturating_add(retain);
        if range.end_height < stale_threshold {
            continue;
        }

        let start = if range.start_height > stale_threshold {
            range.start_height
        } else {
            stale_threshold
        };
        stale_ranges.push(StaleTxRange {
            tx_id: range.tx_id,
            heard_at: range.heard_at,
            start_height: start,
            end_height: range.end_height,
        });
    }

    stale_ranges.sort_by_key(|range| (range.start_height, range.tx_id.to_array()));
    Ok(stale_ranges)
}

fn build_presence_ranges(
    reader: &SolArchiveReader,
) -> Result<Vec<TxPresenceRange>, InspectorError> {
    let mut entries: Vec<MempoolSnapshotEntry> = reader.mempool_snapshot_entries().to_vec();
    entries.sort_by_key(|entry| entry.height);

    let mut active: HashMap<Hash, ActiveTx> = HashMap::new();
    let mut ranges = Vec::new();
    let mut prev_height: Option<SolHeight> = None;

    for entry in entries {
        let height = entry.height;

        if let Some(prev) = prev_height {
            if height > prev.saturating_add(1) {
                close_all_ranges(prev, &mut active, &mut ranges);
            }
        }

        prev_height = Some(height);

        let snapshot = reader.get_mempool_snapshot(height)?.unwrap_or_default();

        let mut seen = HashSet::new();
        for tx in snapshot {
            seen.insert(tx.tx_id.clone());
            match active.get_mut(&tx.tx_id) {
                Some(state) => {
                    if tx.heard_at < state.heard_at {
                        state.heard_at = tx.heard_at;
                    }
                }
                None => {
                    active.insert(
                        tx.tx_id.clone(),
                        ActiveTx {
                            heard_at: tx.heard_at,
                            start_height: height,
                        },
                    );
                }
            }
        }

        let mut to_remove = Vec::new();
        for (tx_id, state) in active.iter() {
            if !seen.contains(tx_id) {
                to_remove.push((tx_id.clone(), *state));
            }
        }

        for (tx_id, state) in to_remove {
            active.remove(&tx_id);
            ranges.push(TxPresenceRange {
                tx_id,
                heard_at: state.heard_at,
                start_height: state.start_height,
                end_height: height.saturating_sub(1),
            });
        }
    }

    if let Some(last_height) = prev_height {
        close_all_ranges(last_height, &mut active, &mut ranges);
    }

    Ok(ranges)
}

#[derive(Debug, Clone, Copy)]
struct ActiveTx {
    heard_at: SolHeight,
    start_height: SolHeight,
}

fn close_all_ranges(
    height: SolHeight,
    active: &mut HashMap<Hash, ActiveTx>,
    ranges: &mut Vec<TxPresenceRange>,
) {
    for (tx_id, state) in active.drain() {
        ranges.push(TxPresenceRange {
            tx_id,
            heard_at: state.heard_at,
            start_height: state.start_height,
            end_height: height,
        });
    }
}

#[cfg(test)]
mod tests {
    use nockchain_math::belt::Belt;
    use nockchain_types::tx_engine::common::Hash;

    use super::*;
    use crate::speed_of_light::archive::SolArchiveWriter;
    use crate::speed_of_light::{MempoolTxEntry, ProofVersion, PROOF_VERSION_1_START};

    fn dummy_hash(v: u64) -> Hash {
        Hash([Belt(v), Belt(v + 1), Belt(v + 2), Belt(v + 3), Belt(v + 4)])
    }

    #[test]
    fn test_find_stale_ranges_basic() {
        let mut writer = SolArchiveWriter::new();

        for height in 0u64..=4 {
            writer
                .add_block_with_raw_txs(
                    SolHeight(height),
                    dummy_hash(height),
                    ProofVersion::for_height(SolHeight(height)),
                    &[height as u8; 4],
                    std::iter::empty(),
                )
                .unwrap();
        }

        let tx_a = dummy_hash(100);
        let tx_b = dummy_hash(200);

        writer
            .add_mempool_snapshot(
                SolHeight(0),
                &[MempoolTxEntry {
                    tx_id: tx_a.clone(),
                    heard_at: SolHeight(0),
                }],
            )
            .unwrap();
        writer
            .add_mempool_snapshot(
                SolHeight(1),
                &[
                    MempoolTxEntry {
                        tx_id: tx_a.clone(),
                        heard_at: SolHeight(0),
                    },
                    MempoolTxEntry {
                        tx_id: tx_b.clone(),
                        heard_at: SolHeight(1),
                    },
                ],
            )
            .unwrap();
        writer
            .add_mempool_snapshot(
                SolHeight(2),
                &[
                    MempoolTxEntry {
                        tx_id: tx_a.clone(),
                        heard_at: SolHeight(0),
                    },
                    MempoolTxEntry {
                        tx_id: tx_b.clone(),
                        heard_at: SolHeight(1),
                    },
                ],
            )
            .unwrap();
        writer
            .add_mempool_snapshot(
                SolHeight(3),
                &[MempoolTxEntry {
                    tx_id: tx_b.clone(),
                    heard_at: SolHeight(1),
                }],
            )
            .unwrap();
        writer.add_mempool_snapshot(SolHeight(4), &[]).unwrap();

        let bytes = writer.to_bytes().unwrap();
        let reader = SolArchiveReader::from_bytes(bytes).unwrap();

        let ranges = find_stale_ranges(&reader, 2).unwrap();

        let expected = vec![
            StaleTxRange {
                tx_id: tx_a,
                heard_at: SolHeight(0),
                start_height: SolHeight(2),
                end_height: SolHeight(2),
            },
            StaleTxRange {
                tx_id: tx_b,
                heard_at: SolHeight(1),
                start_height: SolHeight(3),
                end_height: SolHeight(3),
            },
        ];

        assert_eq!(ranges, expected);
    }

    #[test]
    fn test_find_stale_ranges_reads_mempool_snapshots() {
        let mut writer = SolArchiveWriter::new();

        for height in 0u64..=2 {
            writer
                .add_block_with_raw_txs(
                    SolHeight(height),
                    dummy_hash(height),
                    ProofVersion::for_height(SolHeight(height)),
                    &[height as u8; 4],
                    std::iter::empty(),
                )
                .unwrap();
        }

        let tx = dummy_hash(100);
        writer
            .add_mempool_snapshot(
                SolHeight(0),
                &[MempoolTxEntry {
                    tx_id: tx.clone(),
                    heard_at: SolHeight(0),
                }],
            )
            .unwrap();
        writer
            .add_mempool_snapshot(
                SolHeight(1),
                &[MempoolTxEntry {
                    tx_id: tx.clone(),
                    heard_at: SolHeight(0),
                }],
            )
            .unwrap();
        writer.add_mempool_snapshot(SolHeight(2), &[]).unwrap();

        let reader = SolArchiveReader::from_bytes(writer.to_bytes().unwrap()).unwrap();

        let ranges = find_stale_ranges(&reader, 1).unwrap();
        assert_eq!(
            ranges,
            vec![StaleTxRange {
                tx_id: tx,
                heard_at: SolHeight(0),
                start_height: SolHeight(1),
                end_height: SolHeight(1),
            }]
        );
    }

    #[test]
    fn test_find_stale_ranges_requires_mempool() {
        let mut writer = SolArchiveWriter::new();
        writer
            .add_block_with_tx_count_for_test(
                SolHeight(PROOF_VERSION_1_START),
                dummy_hash(1),
                0,
                ProofVersion::V1,
                &[0xAA; 8],
            )
            .unwrap();

        let reader = SolArchiveReader::from_bytes(writer.to_bytes().unwrap()).unwrap();
        let err = find_stale_ranges(&reader, 20).unwrap_err();
        assert!(matches!(err, InspectorError::MempoolNotPresent));
    }
}

use std::collections::BTreeMap;
use std::fmt::Debug;

use nockchain_types::tx_engine::common::{BlockHeight, Hash, Name};
use nockchain_types::tx_engine::v1::note::BalanceUpdate;
use thiserror::Error;

use crate::determinism::compare_names_lex;
use crate::types::CandidateNote;

#[derive(Debug, Clone, PartialEq, Eq)]
/// Snapshot identity metadata shared by all pages in one consistent balance view.
pub struct SnapshotMetadata {
    pub height: BlockHeight,
    pub block_id: Hash,
}

#[derive(Debug, Clone, PartialEq, Eq)]
/// Deduplicated, normalized snapshot used as planner candidate input.
pub struct NormalizedSnapshot {
    pub metadata: SnapshotMetadata,
    pub candidates: Vec<CandidateNote>,
}

#[derive(Debug, Clone, PartialEq, Eq, Error)]
/// Errors indicating that fetched pages do not represent one consistent snapshot.
pub enum SnapshotConsistencyError {
    #[error("missing snapshot metadata")]
    MissingMetadata,
    #[error("snapshot height drifted across pages")]
    HeightDrift,
    #[error("snapshot block-id drifted across pages")]
    BlockIdDrift,
}

#[derive(Debug, Clone, PartialEq, Eq, Error)]
/// Errors raised while deduplicating notes from snapshot pages.
pub enum CandidateNormalizationError {
    #[error("duplicate note has non-identical payload for name {first}/{last}")]
    DuplicateNameMismatch { first: String, last: String },
}

#[derive(Debug, Clone, PartialEq, Eq, Error)]
/// Combined normalization error for metadata consistency and candidate payloads.
pub enum NormalizeSnapshotError {
    #[error(transparent)]
    Snapshot(#[from] SnapshotConsistencyError),
    #[error(transparent)]
    Candidate(#[from] CandidateNormalizationError),
}

#[derive(Debug, Error)]
/// End-to-end collection failure while fetching and normalizing balance snapshots.
pub enum CollectSnapshotError<E>
where
    E: std::fmt::Display + Debug,
{
    #[error("snapshot page fetch failed: {0}")]
    Fetch(E),
    #[error(transparent)]
    Normalize(#[from] NormalizeSnapshotError),
    #[error("snapshot drift retries exhausted after {attempts} attempts: {last_error}")]
    SnapshotDriftRetriesExhausted {
        attempts: usize,
        last_error: SnapshotConsistencyError,
    },
}

/// Source capable of fetching a full set of balance pages for one snapshot attempt.
pub trait SnapshotPageFetcher {
    type Error: std::fmt::Display + Debug;

    fn fetch_pages(&mut self) -> Result<Vec<BalanceUpdate>, Self::Error>;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct NoteNameKey {
    first: [u64; 5],
    last: [u64; 5],
}

/// Fetches pages and retries drift errors until a stable normalized snapshot is produced.
pub fn collect_snapshot_with_retry<F>(
    fetcher: &mut F,
    max_retries: usize,
) -> Result<NormalizedSnapshot, CollectSnapshotError<F::Error>>
where
    F: SnapshotPageFetcher,
{
    let mut attempts = 0usize;
    loop {
        attempts += 1;
        let pages = fetcher.fetch_pages().map_err(CollectSnapshotError::Fetch)?;
        match normalize_balance_pages(&pages) {
            Ok(snapshot) => return Ok(snapshot),
            Err(NormalizeSnapshotError::Snapshot(err))
                if matches!(
                    err,
                    SnapshotConsistencyError::HeightDrift | SnapshotConsistencyError::BlockIdDrift
                ) =>
            {
                if attempts <= max_retries {
                    continue;
                }
                return Err(CollectSnapshotError::SnapshotDriftRetriesExhausted {
                    attempts,
                    last_error: err,
                });
            }
            Err(err) => return Err(CollectSnapshotError::Normalize(err)),
        }
    }
}

/// Normalizes balance pages into one metadata stamp and deduplicated candidate set.
pub fn normalize_balance_pages(
    pages: &[BalanceUpdate],
) -> Result<NormalizedSnapshot, NormalizeSnapshotError> {
    let metadata = enforce_single_snapshot(
        &pages
            .iter()
            .map(|page| SnapshotMetadata {
                height: page.height.clone(),
                block_id: page.block_id.clone(),
            })
            .collect::<Vec<_>>(),
    )?;

    let mut deduped = BTreeMap::<NoteNameKey, CandidateNote>::new();
    for page in pages {
        for (name, note) in &page.notes.0 {
            let candidate = CandidateNote::from_note(name, note);
            let key = note_name_key(name);
            if let Some(existing) = deduped.get(&key) {
                if existing != &candidate {
                    return Err(CandidateNormalizationError::DuplicateNameMismatch {
                        first: name.first.to_base58(),
                        last: name.last.to_base58(),
                    }
                    .into());
                }
                continue;
            }
            deduped.insert(key, candidate);
        }
    }

    let mut candidates = deduped.into_values().collect::<Vec<_>>();
    candidates.sort_by(|a, b| compare_names_lex(&a.identity().name, &b.identity().name));
    Ok(NormalizedSnapshot {
        metadata,
        candidates,
    })
}

/// Validates all provided pages were produced at one height and block id.
pub fn enforce_single_snapshot(
    pages: &[SnapshotMetadata],
) -> Result<SnapshotMetadata, SnapshotConsistencyError> {
    let first = pages
        .first()
        .ok_or(SnapshotConsistencyError::MissingMetadata)?;
    for page in pages.iter().skip(1) {
        if page.height != first.height {
            return Err(SnapshotConsistencyError::HeightDrift);
        }
        if page.block_id != first.block_id {
            return Err(SnapshotConsistencyError::BlockIdDrift);
        }
    }
    Ok(first.clone())
}

fn note_name_key(name: &Name) -> NoteNameKey {
    NoteNameKey {
        first: name.first.to_array(),
        last: name.last.to_array(),
    }
}

#[cfg(test)]
mod tests {
    use nockchain_math::belt::Belt;
    use nockchain_math::owned_based_noun::OwnedBasedNoun;
    use nockchain_types::tx_engine::common::Nicks;
    use nockchain_types::tx_engine::v1::note::{Balance, Note, NoteData, NoteDataEntry, NoteV1};

    use super::*;

    fn hash(v: u64) -> Hash {
        Hash::from_limbs(&[v, 0, 0, 0, 0])
    }

    fn name(v: u64) -> Name {
        Name::new(hash(v), hash(v + 100))
    }

    fn note_v1(name: Name, origin_page: u64, assets: u64, key: &str, value: u64) -> Note {
        let note_data = NoteData::new(vec![NoteDataEntry::new(
            key.to_string(),
            OwnedBasedNoun::try_atom(value).expect("fixture raw note-data value must be based"),
        )]);
        Note::V1(NoteV1::new(
            BlockHeight(Belt(origin_page)),
            name,
            note_data,
            Nicks(assets as usize),
        ))
    }

    fn page(height: u64, block_id: u64, notes: Vec<(Name, Note)>) -> BalanceUpdate {
        BalanceUpdate {
            height: BlockHeight(Belt(height)),
            block_id: hash(block_id),
            notes: Balance(notes),
        }
    }

    #[test]
    fn normalize_balance_pages_dedupes_identical_entries() {
        let n = name(1);
        let p1 = page(10, 99, vec![(n.clone(), note_v1(n.clone(), 10, 5, "k", 1))]);
        let p2 = page(10, 99, vec![(n.clone(), note_v1(n.clone(), 10, 5, "k", 1))]);

        let normalized = normalize_balance_pages(&[p1, p2]).expect("normalized");
        assert_eq!(normalized.candidates.len(), 1);
        assert_eq!(normalized.metadata.height, BlockHeight(Belt(10)));
        assert_eq!(normalized.metadata.block_id, hash(99));
    }

    #[test]
    fn normalize_balance_pages_rejects_duplicate_payload_mismatch() {
        let n = name(2);
        let p1 = page(10, 99, vec![(n.clone(), note_v1(n.clone(), 10, 5, "k", 1))]);
        let p2 = page(10, 99, vec![(n.clone(), note_v1(n.clone(), 10, 6, "k", 1))]);

        let error = normalize_balance_pages(&[p1, p2]).expect_err("expected mismatch");
        assert!(matches!(
            error,
            NormalizeSnapshotError::Candidate(
                CandidateNormalizationError::DuplicateNameMismatch { .. }
            )
        ));
    }

    struct SequenceFetcher {
        responses: Vec<Result<Vec<BalanceUpdate>, &'static str>>,
        idx: usize,
    }

    impl SnapshotPageFetcher for SequenceFetcher {
        type Error = &'static str;

        fn fetch_pages(&mut self) -> Result<Vec<BalanceUpdate>, Self::Error> {
            let response = self
                .responses
                .get(self.idx)
                .cloned()
                .expect("missing response");
            self.idx += 1;
            response
        }
    }

    #[test]
    fn collect_snapshot_with_retry_recovers_from_drift() {
        let n = name(1);
        let drift = vec![
            page(10, 99, vec![(n.clone(), note_v1(n.clone(), 10, 1, "k", 1))]),
            page(10, 100, vec![(name(2), note_v1(name(2), 10, 2, "k", 2))]),
        ];
        let stable = vec![page(
            10,
            99,
            vec![
                (n.clone(), note_v1(n.clone(), 10, 1, "k", 1)),
                (name(2), note_v1(name(2), 10, 2, "k", 2)),
            ],
        )];

        let mut fetcher = SequenceFetcher {
            responses: vec![Ok(drift), Ok(stable)],
            idx: 0,
        };

        let snapshot = collect_snapshot_with_retry(&mut fetcher, 2).expect("retry succeeded");
        assert_eq!(snapshot.candidates.len(), 2);
        assert_eq!(fetcher.idx, 2);
    }

    #[test]
    fn collect_snapshot_with_retry_exhausts_on_persistent_drift() {
        let n = name(1);
        let drift_a = vec![
            page(10, 99, vec![(n.clone(), note_v1(n.clone(), 10, 1, "k", 1))]),
            page(10, 100, vec![(name(2), note_v1(name(2), 10, 2, "k", 2))]),
        ];
        let drift_b = vec![
            page(
                10,
                200,
                vec![(n.clone(), note_v1(n.clone(), 10, 1, "k", 1))],
            ),
            page(10, 201, vec![(name(2), note_v1(name(2), 10, 2, "k", 2))]),
        ];

        let mut fetcher = SequenceFetcher {
            responses: vec![Ok(drift_a), Ok(drift_b)],
            idx: 0,
        };

        let error = collect_snapshot_with_retry(&mut fetcher, 1).expect_err("expected drift");
        assert!(matches!(
            error,
            CollectSnapshotError::SnapshotDriftRetriesExhausted { .. }
        ));
        assert_eq!(fetcher.idx, 2);
    }
}

use std::collections::BTreeSet;
use std::sync::{Arc, RwLock};
use std::time::{Duration, SystemTime};

use async_trait::async_trait;
use nockapp::noun::slab::{NockJammer, NounSlab};
use nockapp::{Bytes, NounAllocator};
use nockapp_grpc::services::private_nockapp::client::PrivateNockAppGrpcClient;
use nockchain_types::tx_engine::common::Name;
use nockchain_types::tx_engine::v1::note::BalanceUpdate;
use noun_serde::{NounDecode, NounEncode};
use wallet_tx_builder::adapter::{
    normalize_balance_pages, NormalizeSnapshotError, NormalizedSnapshot,
};

use crate::shared::errors::BridgeError;
use crate::shared::types::Tip5Hash;
const SNAPSHOT_DRIFT_MAX_RETRIES: usize = 2;

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct BridgeOwnedNoteSelectors {
    pub first_names: Vec<String>,
}

impl BridgeOwnedNoteSelectors {
    pub fn normalized(&self) -> Self {
        let mut first_names = self
            .first_names
            .iter()
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty())
            .collect::<Vec<_>>();
        first_names.sort();
        first_names.dedup();

        Self { first_names }
    }

    pub fn is_empty(&self) -> bool {
        self.first_names.is_empty()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfirmedBridgeNoteSnapshot {
    pub refreshed_at: SystemTime,
    pub normalized: NormalizedSnapshot,
}

impl ConfirmedBridgeNoteSnapshot {
    pub fn height(&self) -> u64 {
        self.normalized.metadata.height.0 .0
    }

    pub fn block_id(&self) -> &Tip5Hash {
        &self.normalized.metadata.block_id
    }

    pub fn is_stale(&self, now: SystemTime, stale_after: Duration) -> bool {
        now.duration_since(self.refreshed_at)
            .unwrap_or(Duration::ZERO)
            >= stale_after
    }

    pub fn matches_confirmed_block(&self, height: u64, block_id: &Tip5Hash) -> bool {
        self.height() == height && self.block_id() == block_id
    }
}

#[async_trait]
pub trait BridgeNoteSnapshotSource: Send + Sync {
    async fn fetch_pages(
        &self,
        selectors: &BridgeOwnedNoteSelectors,
    ) -> Result<Vec<BalanceUpdate>, BridgeError>;
}

#[derive(Debug, Clone)]
pub struct PrivateNockAppSnapshotSource {
    endpoint: String,
}

impl PrivateNockAppSnapshotSource {
    pub fn new(endpoint: String) -> Self {
        Self { endpoint }
    }
}

#[async_trait]
impl BridgeNoteSnapshotSource for PrivateNockAppSnapshotSource {
    async fn fetch_pages(
        &self,
        selectors: &BridgeOwnedNoteSelectors,
    ) -> Result<Vec<BalanceUpdate>, BridgeError> {
        let selectors = selectors.normalized();
        if selectors.is_empty() {
            return Ok(Vec::new());
        }

        let mut client = PrivateNockAppGrpcClient::connect(self.endpoint.clone())
            .await
            .map_err(|err| BridgeError::EventMonitoring(err.to_string()))?;
        let mut request_index = 0i32;
        let mut pages = Vec::new();

        for first_name in &selectors.first_names {
            if let Some(page) = fetch_private_balance_page(
                &mut client, request_index, "balance-by-first-name", first_name,
            )
            .await?
            {
                pages.push(page);
            }
            request_index = request_index.wrapping_add(1);
        }

        Ok(pages)
    }
}

pub struct BridgeNoteSnapshotService {
    source: Arc<dyn BridgeNoteSnapshotSource>,
    selectors: BridgeOwnedNoteSelectors,
    stale_after: Duration,
    cache: Arc<RwLock<Option<ConfirmedBridgeNoteSnapshot>>>,
    nockchain_confirmation_depth: u64,
}

impl Clone for BridgeNoteSnapshotService {
    fn clone(&self) -> Self {
        Self {
            source: self.source.clone(),
            selectors: self.selectors.clone(),
            stale_after: self.stale_after,
            cache: self.cache.clone(),
            nockchain_confirmation_depth: self.nockchain_confirmation_depth,
        }
    }
}

impl BridgeNoteSnapshotService {
    pub fn new(
        source: Arc<dyn BridgeNoteSnapshotSource>,
        selectors: BridgeOwnedNoteSelectors,
        stale_after: Duration,
    ) -> Self {
        Self {
            source,
            selectors: selectors.normalized(),
            stale_after,
            cache: Arc::new(RwLock::new(None)),
            nockchain_confirmation_depth: 0,
        }
    }

    pub fn new_private(
        endpoint: String,
        selectors: BridgeOwnedNoteSelectors,
        stale_after: Duration,
    ) -> Self {
        Self::new(
            Arc::new(PrivateNockAppSnapshotSource::new(endpoint)),
            selectors,
            stale_after,
        )
    }

    pub fn with_nockchain_confirmation_depth(mut self, depth: u64) -> Self {
        self.nockchain_confirmation_depth = depth;
        self
    }

    pub fn selectors(&self) -> &BridgeOwnedNoteSelectors {
        &self.selectors
    }

    pub fn snapshot(&self) -> Option<ConfirmedBridgeNoteSnapshot> {
        self.cache.read().ok().and_then(|guard| guard.clone())
    }

    pub async fn refresh(&self) -> Result<Option<ConfirmedBridgeNoteSnapshot>, BridgeError> {
        if self.selectors.is_empty() {
            self.replace_snapshot(None);
            return Ok(None);
        }

        let mut attempts = 0usize;
        loop {
            attempts = attempts.saturating_add(1);
            let pages = self.source.fetch_pages(&self.selectors).await?;
            if pages.is_empty() {
                self.replace_snapshot(None);
                return Ok(None);
            }

            match normalize_balance_pages(&pages) {
                Ok(normalized) => {
                    let snapshot = ConfirmedBridgeNoteSnapshot {
                        refreshed_at: SystemTime::now(),
                        normalized,
                    };
                    self.replace_snapshot(Some(snapshot.clone()));
                    return Ok(Some(snapshot));
                }
                Err(NormalizeSnapshotError::Snapshot(
                    wallet_tx_builder::adapter::SnapshotConsistencyError::HeightDrift
                    | wallet_tx_builder::adapter::SnapshotConsistencyError::BlockIdDrift,
                )) if attempts <= SNAPSHOT_DRIFT_MAX_RETRIES => continue,
                Err(err) => {
                    return Err(BridgeError::Runtime(format!(
                        "failed to normalize bridge note snapshot: {err}"
                    )))
                }
            }
        }
    }

    pub async fn refresh_if_stale(
        &self,
        now: SystemTime,
    ) -> Result<Option<ConfirmedBridgeNoteSnapshot>, BridgeError> {
        if let Some(snapshot) = self.snapshot() {
            if !snapshot.is_stale(now, self.stale_after) {
                return Ok(Some(snapshot));
            }
        }
        self.refresh().await
    }

    pub async fn refresh_on_confirmed_block(
        &self,
        height: u64,
        block_id: &Tip5Hash,
    ) -> Result<Option<ConfirmedBridgeNoteSnapshot>, BridgeError> {
        if let Some(snapshot) = self.snapshot() {
            if snapshot.matches_confirmed_block(height, block_id)
                && !snapshot.is_stale(SystemTime::now(), self.stale_after)
            {
                return Ok(Some(snapshot));
            }
        }
        self.refresh().await
    }

    /// Filters out notes that are not safe to spend from the current bridge
    /// node view. Notes newer than the configured safe Nockchain tip may be
    /// orphaned, and reserved notes are already committed to active withdrawal
    /// attempts.
    pub fn spendable_snapshot(&self, reserved_inputs: &[Name]) -> Option<NormalizedSnapshot> {
        self.snapshot().map(|snapshot| {
            filter_spendable_inputs(
                &snapshot.normalized, reserved_inputs, self.nockchain_confirmation_depth,
            )
        })
    }

    fn replace_snapshot(&self, snapshot: Option<ConfirmedBridgeNoteSnapshot>) {
        if let Ok(mut guard) = self.cache.write() {
            *guard = snapshot;
        }
    }
}

pub fn filter_reserved_inputs(
    snapshot: &NormalizedSnapshot,
    reserved_inputs: &[Name],
) -> NormalizedSnapshot {
    filter_spendable_inputs(snapshot, reserved_inputs, 0)
}

pub fn filter_spendable_inputs(
    snapshot: &NormalizedSnapshot,
    reserved_inputs: &[Name],
    nockchain_confirmation_depth: u64,
) -> NormalizedSnapshot {
    let reserved = reserved_inputs
        .iter()
        .map(note_name_key)
        .collect::<BTreeSet<_>>();
    let safe_tip = (snapshot.metadata.height.0)
        .0
        .saturating_sub(nockchain_confirmation_depth);

    let candidates = snapshot
        .candidates
        .iter()
        .filter(|candidate| (candidate.identity().origin_page.0).0 <= safe_tip)
        .filter(|candidate| !reserved.contains(&note_name_key(&candidate.identity().name)))
        .cloned()
        .collect();

    NormalizedSnapshot {
        metadata: snapshot.metadata.clone(),
        candidates,
    }
}

async fn fetch_private_balance_page(
    client: &mut PrivateNockAppGrpcClient,
    request_index: i32,
    root: &str,
    value: &str,
) -> Result<Option<BalanceUpdate>, BridgeError> {
    let mut path_slab = NounSlab::<NockJammer>::new();
    let path_noun = vec![root.to_string(), value.to_string()].to_noun(&mut path_slab);
    path_slab.set_root(path_noun);
    let path_bytes = path_slab.jam().to_vec();

    let response = client
        .peek(request_index, path_bytes)
        .await
        .map_err(|err| BridgeError::EventMonitoring(err.to_string()))?;

    decode_private_balance_payload(response)
}

fn decode_private_balance_payload(response: Vec<u8>) -> Result<Option<BalanceUpdate>, BridgeError> {
    let mut slab: NounSlab<NockJammer> = NounSlab::new();
    let noun = slab
        .cue_into(Bytes::from(response))
        .map_err(|err| BridgeError::Runtime(format!("failed to cue balance response: {err}")))?;
    let space = slab.noun_space();
    let payload: Option<Option<BalanceUpdate>> =
        Option::<Option<BalanceUpdate>>::from_noun(&noun, &space).map_err(|err| {
            BridgeError::Runtime(format!("failed to decode private balance response: {err}"))
        })?;
    Ok(payload.flatten())
}

fn note_name_key(name: &Name) -> ([u64; 5], [u64; 5]) {
    (name.first.to_array(), name.last.to_array())
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;
    use std::sync::Mutex;

    use nockchain_math::belt::Belt;
    use nockchain_math::owned_based_noun::OwnedBasedNoun;
    use nockchain_types::tx_engine::common::{BlockHeight, Hash, Nicks};
    use nockchain_types::tx_engine::v1::note::{Balance, Note, NoteData, NoteDataEntry, NoteV1};
    use wallet_tx_builder::types::CandidateNote;

    use super::*;

    #[derive(Debug)]
    struct FakeSnapshotSource {
        responses: Mutex<VecDeque<Result<Vec<BalanceUpdate>, BridgeError>>>,
        calls: Mutex<usize>,
    }

    impl FakeSnapshotSource {
        fn new(responses: Vec<Result<Vec<BalanceUpdate>, BridgeError>>) -> Self {
            Self {
                responses: Mutex::new(VecDeque::from(responses)),
                calls: Mutex::new(0),
            }
        }

        fn calls(&self) -> usize {
            *self.calls.lock().expect("calls lock")
        }
    }

    #[async_trait]
    impl BridgeNoteSnapshotSource for FakeSnapshotSource {
        async fn fetch_pages(
            &self,
            _selectors: &BridgeOwnedNoteSelectors,
        ) -> Result<Vec<BalanceUpdate>, BridgeError> {
            *self.calls.lock().expect("calls lock") += 1;
            self.responses
                .lock()
                .expect("responses lock")
                .pop_front()
                .expect("missing fake response")
        }
    }

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

    fn selectors() -> BridgeOwnedNoteSelectors {
        BridgeOwnedNoteSelectors {
            first_names: vec!["first-a".to_string(), " first-a ".to_string()],
        }
    }

    #[tokio::test]
    async fn refresh_retries_snapshot_drift_and_caches_normalized_snapshot() {
        let note = name(1);
        let drift = vec![
            page(
                10,
                99,
                vec![(note.clone(), note_v1(note.clone(), 10, 5, "k", 1))],
            ),
            page(10, 100, vec![(name(2), note_v1(name(2), 10, 7, "k", 2))]),
        ];
        let stable = vec![page(
            10,
            99,
            vec![
                (note.clone(), note_v1(note.clone(), 10, 5, "k", 1)),
                (name(2), note_v1(name(2), 10, 7, "k", 2)),
            ],
        )];

        let source = Arc::new(FakeSnapshotSource::new(vec![Ok(drift), Ok(stable)]));
        let service =
            BridgeNoteSnapshotService::new(source.clone(), selectors(), Duration::from_secs(60));

        let snapshot = service
            .refresh()
            .await
            .expect("refresh succeeds")
            .expect("snapshot exists");

        assert_eq!(snapshot.height(), 10);
        assert_eq!(snapshot.block_id(), &hash(99));
        assert_eq!(snapshot.normalized.candidates.len(), 2);
        assert_eq!(source.calls(), 2);
        assert_eq!(
            service.snapshot().expect("cached snapshot").normalized,
            snapshot.normalized
        );
    }

    #[tokio::test]
    async fn refresh_if_stale_uses_cache_until_stale_threshold() {
        let note = name(3);
        let first = vec![page(
            20,
            199,
            vec![(note.clone(), note_v1(note.clone(), 20, 8, "k", 3))],
        )];
        let second = vec![page(
            21,
            200,
            vec![(note.clone(), note_v1(note.clone(), 21, 8, "k", 3))],
        )];
        let third = vec![page(
            22,
            201,
            vec![(note.clone(), note_v1(note.clone(), 22, 8, "k", 3))],
        )];

        let source = Arc::new(FakeSnapshotSource::new(vec![
            Ok(first),
            Ok(second),
            Ok(third),
        ]));
        let service =
            BridgeNoteSnapshotService::new(source.clone(), selectors(), Duration::from_secs(300));

        let initial = service
            .refresh()
            .await
            .expect("initial refresh")
            .expect("initial snapshot");
        let reused = service
            .refresh_if_stale(initial.refreshed_at)
            .await
            .expect("refresh if stale")
            .expect("reused snapshot");

        assert_eq!(source.calls(), 1);
        assert_eq!(reused.normalized, initial.normalized);

        let always_stale =
            BridgeNoteSnapshotService::new(source.clone(), selectors(), Duration::ZERO);
        always_stale
            .refresh()
            .await
            .expect("seed zero-stale service")
            .expect("snapshot");
        always_stale
            .refresh_if_stale(SystemTime::now())
            .await
            .expect("forced stale refresh")
            .expect("snapshot");
        assert_eq!(source.calls(), 3);
    }

    #[tokio::test]
    async fn refresh_on_confirmed_block_skips_matching_fresh_snapshot() {
        let note = name(4);
        let source = Arc::new(FakeSnapshotSource::new(vec![Ok(vec![page(
            30,
            299,
            vec![(note.clone(), note_v1(note.clone(), 30, 9, "k", 4))],
        )])]));
        let service =
            BridgeNoteSnapshotService::new(source.clone(), selectors(), Duration::from_secs(300));
        let snapshot = service.refresh().await.expect("refresh").expect("snapshot");

        let reused = service
            .refresh_on_confirmed_block(snapshot.height(), snapshot.block_id())
            .await
            .expect("refresh on confirmed block")
            .expect("snapshot");

        assert_eq!(source.calls(), 1);
        assert_eq!(reused.normalized, snapshot.normalized);
    }

    #[tokio::test]
    async fn spendable_snapshot_filters_reserved_inputs() {
        let note_a = name(10);
        let note_b = name(11);
        let source = Arc::new(FakeSnapshotSource::new(vec![Ok(vec![page(
            40,
            399,
            vec![
                (note_a.clone(), note_v1(note_a.clone(), 40, 10, "k", 10)),
                (note_b.clone(), note_v1(note_b.clone(), 40, 11, "k", 11)),
            ],
        )])]));
        let service = BridgeNoteSnapshotService::new(source, selectors(), Duration::from_secs(300));
        service.refresh().await.expect("refresh").expect("snapshot");

        let spendable = service
            .spendable_snapshot(std::slice::from_ref(&note_a))
            .expect("spendable snapshot");

        assert_eq!(spendable.metadata.height, BlockHeight(Belt(40)));
        assert_eq!(spendable.metadata.block_id, hash(399));
        assert_eq!(spendable.candidates.len(), 1);
        assert!(matches!(
            &spendable.candidates[0],
            CandidateNote::V1(note) if note.identity.name == note_b
        ));
    }

    #[tokio::test]
    async fn spendable_snapshot_filters_notes_newer_than_safe_tip() {
        let confirmed_note = name(20);
        let edge_note = name(21);
        let unsafe_note = name(22);
        let source = Arc::new(FakeSnapshotSource::new(vec![Ok(vec![page(
            100,
            999,
            vec![
                (
                    confirmed_note.clone(),
                    note_v1(confirmed_note.clone(), 90, 10, "k", 20),
                ),
                (
                    edge_note.clone(),
                    note_v1(edge_note.clone(), 95, 11, "k", 21),
                ),
                (
                    unsafe_note.clone(),
                    note_v1(unsafe_note.clone(), 96, 12, "k", 22),
                ),
            ],
        )])]));
        let service = BridgeNoteSnapshotService::new(source, selectors(), Duration::from_secs(300))
            .with_nockchain_confirmation_depth(5);
        service.refresh().await.expect("refresh").expect("snapshot");

        let spendable = service.spendable_snapshot(&[]).expect("spendable snapshot");
        let spendable_names = spendable
            .candidates
            .iter()
            .map(|candidate| candidate.identity().name.clone())
            .collect::<Vec<_>>();

        assert_eq!(spendable_names, vec![confirmed_note, edge_note]);
    }

    #[tokio::test]
    async fn spendable_snapshot_combines_safe_tip_and_reserved_filters() {
        let reserved_safe_note = name(30);
        let unsafe_note = name(31);
        let spendable_note = name(32);
        let source = Arc::new(FakeSnapshotSource::new(vec![Ok(vec![page(
            200,
            1999,
            vec![
                (
                    reserved_safe_note.clone(),
                    note_v1(reserved_safe_note.clone(), 190, 10, "k", 30),
                ),
                (
                    unsafe_note.clone(),
                    note_v1(unsafe_note.clone(), 196, 11, "k", 31),
                ),
                (
                    spendable_note.clone(),
                    note_v1(spendable_note.clone(), 194, 12, "k", 32),
                ),
            ],
        )])]));
        let service = BridgeNoteSnapshotService::new(source, selectors(), Duration::from_secs(300))
            .with_nockchain_confirmation_depth(5);
        service.refresh().await.expect("refresh").expect("snapshot");

        let spendable = service
            .spendable_snapshot(std::slice::from_ref(&reserved_safe_note))
            .expect("spendable snapshot");

        assert_eq!(spendable.candidates.len(), 1);
        assert!(matches!(
            &spendable.candidates[0],
            CandidateNote::V1(note) if note.identity.name == spendable_note
        ));
    }
}

#![allow(clippy::unwrap_used)]

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use alloy::primitives::Address;
use async_trait::async_trait;
use bridge::deposit::base::DepositSubmissionResult;
use bridge::deposit::cache::{ProposalCache, ProposalStatus, SignatureData};
use bridge::deposit::log::{DepositLog, DepositLogEntry, DepositLogInsertOutcome};
use bridge::deposit::ports::BaseContractPort;
use bridge::deposit::posting::{
    posting_tick_once, PostingTickConfig, PostingTickContext, PostingTickControl, PostingTickInput,
    PostingTickNodeState, PostingTickPorts, PostingTickState,
};
use bridge::deposit::signing::{
    signing_tick_once, SigningLocalStopMode, SigningTickConfig, SigningTickContext,
    SigningTickControl, SigningTickInput, SigningTickNodeState, SigningTickPorts, SigningTickState,
};
use bridge::deposit::types::{DepositId, DepositSubmission, NockDepositRequestData};
use bridge::observability::health::SharedHealthState;
use bridge::observability::status::{BridgeStatus, BridgeStatusState};
use bridge::shared::config::NonceEpochConfig;
use bridge::shared::errors::BridgeError;
use bridge::shared::proposer::active_proposer;
use bridge::shared::runtime::{
    BridgeEvent, BridgeRuntime, CauseBuildOutcome, CauseBuilder, EventEnvelope,
};
use bridge::shared::signing::BridgeSigner;
use bridge::shared::stop::StopController;
use bridge::shared::types::{
    zero_tip5_hash, AtomBytes, EthAddress, NodeConfig, NodeInfo, SchnorrSecretKey, Tip5Hash,
};
use nockchain_math::belt::Belt;
use nockchain_types::tx_engine::common::Hash as NockPkh;
use nockchain_types::v1::Name;
use tempfile::TempDir;

const TEST_PRIVATE_KEY: &str = "4c0883a69102937d6231471b5dbb6204fe5129617082792ae468d01a3f362318";

#[derive(Clone)]
struct CountingBaseContract {
    get_last_nonce_calls: Arc<AtomicUsize>,
    is_processed_calls: Arc<AtomicUsize>,
    submit_calls: Arc<AtomicUsize>,
    last_nonce: u64,
    fail_get_last_nonce: bool,
    scripted_last_nonce: Arc<Mutex<VecDeque<Result<u64, BridgeError>>>>,
    scripted_processed: Arc<Mutex<VecDeque<Result<bool, BridgeError>>>>,
    scripted_submit: Arc<Mutex<VecDeque<Result<DepositSubmissionResult, BridgeError>>>>,
}

impl CountingBaseContract {
    fn with_last_nonce(last_nonce: u64) -> Self {
        Self {
            get_last_nonce_calls: Arc::new(AtomicUsize::new(0)),
            is_processed_calls: Arc::new(AtomicUsize::new(0)),
            submit_calls: Arc::new(AtomicUsize::new(0)),
            last_nonce,
            fail_get_last_nonce: false,
            scripted_last_nonce: Arc::new(Mutex::new(VecDeque::new())),
            scripted_processed: Arc::new(Mutex::new(VecDeque::new())),
            scripted_submit: Arc::new(Mutex::new(VecDeque::new())),
        }
    }

    fn with_nonce_query_error() -> Self {
        Self {
            get_last_nonce_calls: Arc::new(AtomicUsize::new(0)),
            is_processed_calls: Arc::new(AtomicUsize::new(0)),
            submit_calls: Arc::new(AtomicUsize::new(0)),
            last_nonce: 0,
            fail_get_last_nonce: true,
            scripted_last_nonce: Arc::new(Mutex::new(VecDeque::new())),
            scripted_processed: Arc::new(Mutex::new(VecDeque::new())),
            scripted_submit: Arc::new(Mutex::new(VecDeque::new())),
        }
    }

    fn script_last_nonce_responses(
        &self,
        responses: impl IntoIterator<Item = Result<u64, BridgeError>>,
    ) {
        self.scripted_last_nonce
            .lock()
            .expect("scripted_last_nonce lock poisoned")
            .extend(responses);
    }

    fn script_processed_responses(
        &self,
        responses: impl IntoIterator<Item = Result<bool, BridgeError>>,
    ) {
        self.scripted_processed
            .lock()
            .expect("scripted_processed lock poisoned")
            .extend(responses);
    }

    fn script_submit_responses(
        &self,
        responses: impl IntoIterator<Item = Result<DepositSubmissionResult, BridgeError>>,
    ) {
        self.scripted_submit
            .lock()
            .expect("scripted_submit lock poisoned")
            .extend(responses);
    }

    fn get_last_nonce_calls(&self) -> usize {
        self.get_last_nonce_calls.load(Ordering::SeqCst)
    }

    fn submit_calls(&self) -> usize {
        self.submit_calls.load(Ordering::SeqCst)
    }

    fn is_processed_calls(&self) -> usize {
        self.is_processed_calls.load(Ordering::SeqCst)
    }
}

#[async_trait]
impl BaseContractPort for CountingBaseContract {
    async fn submit_deposit(
        &self,
        _submission: DepositSubmission,
    ) -> Result<DepositSubmissionResult, BridgeError> {
        self.submit_calls.fetch_add(1, Ordering::SeqCst);
        if let Some(response) = self
            .scripted_submit
            .lock()
            .expect("scripted_submit lock poisoned")
            .pop_front()
        {
            return response;
        }
        Ok(DepositSubmissionResult {
            tx_hash: "in-memory".to_string(),
            block_number: 0,
        })
    }

    async fn get_last_deposit_nonce(&self) -> Result<u64, BridgeError> {
        self.get_last_nonce_calls.fetch_add(1, Ordering::SeqCst);
        if let Some(response) = self
            .scripted_last_nonce
            .lock()
            .expect("scripted_last_nonce lock poisoned")
            .pop_front()
        {
            return response;
        }
        if self.fail_get_last_nonce {
            Err(BridgeError::BaseBridgeQuery(
                "simulated nonce query error".to_string(),
            ))
        } else {
            Ok(self.last_nonce)
        }
    }

    async fn is_deposit_processed(&self, _tx_id: &Tip5Hash) -> Result<bool, BridgeError> {
        self.is_processed_calls.fetch_add(1, Ordering::SeqCst);
        if let Some(response) = self
            .scripted_processed
            .lock()
            .expect("scripted_processed lock poisoned")
            .pop_front()
        {
            return response;
        }
        Ok(false)
    }
}

struct NoopCauseBuilder;

impl CauseBuilder for NoopCauseBuilder {
    fn build_poke(
        &self,
        _event: &EventEnvelope<BridgeEvent>,
    ) -> Result<CauseBuildOutcome, BridgeError> {
        Ok(CauseBuildOutcome::Deferred("test".to_string()))
    }
}

fn test_bridge_status() -> BridgeStatus {
    let health: SharedHealthState = Arc::new(std::sync::RwLock::new(Vec::new()));
    BridgeStatus::new(health)
}

fn test_node_config(node_id: u64) -> NodeConfig {
    NodeConfig {
        node_id,
        nodes: vec![
            NodeInfo {
                ip: "localhost:8001".to_string(),
                eth_pubkey: AtomBytes(vec![0u8; 20]),
                nock_pkh: NockPkh::from_base58(
                    "2222222222222222222222222222222222222222222222222222",
                )
                .expect("valid test pkh 0"),
            },
            NodeInfo {
                ip: "localhost:8002".to_string(),
                eth_pubkey: AtomBytes(vec![1u8; 20]),
                nock_pkh: NockPkh::from_base58(
                    "3333333333333333333333333333333333333333333333333333",
                )
                .expect("valid test pkh 1"),
            },
            NodeInfo {
                ip: "localhost:8003".to_string(),
                eth_pubkey: AtomBytes(vec![2u8; 20]),
                nock_pkh: NockPkh::from_base58(
                    "4444444444444444444444444444444444444444444444444444",
                )
                .expect("valid test pkh 2"),
            },
        ],
        bridge_lock_root: NockPkh::from_base58(
            "AcsPkuhXQoGeEsF91yynpm1kcW17PQ2Z1MEozgx7YnDPkZwrtzLuuqd",
        )
        .expect("bridge lock root"),
        my_eth_key: AtomBytes(vec![]),
        my_nock_key: SchnorrSecretKey([Belt(0); 8]),
    }
}

fn sample_proposal(nonce: u64) -> NockDepositRequestData {
    NockDepositRequestData {
        tx_id: Tip5Hash([Belt(1 + nonce), Belt(2), Belt(3), Belt(4), Belt(5)]),
        name: Name::new(zero_tip5_hash(), zero_tip5_hash()),
        recipient: EthAddress::ZERO,
        amount: 1_000,
        block_height: 100,
        as_of: zero_tip5_hash(),
        nonce,
    }
}

fn seed_ready_proposal(cache: &ProposalCache, nonce: u64) -> DepositId {
    let mut proposal = sample_proposal(nonce);
    proposal.as_of = Tip5Hash([Belt(10_000 + nonce), Belt(0), Belt(0), Belt(0), Belt(0)]);
    let proposal_hash = proposal.compute_proposal_hash();
    let deposit_id = DepositId::from_effect_payload(&proposal);

    let signer1 = Address::from([1u8; 20]);
    let signer2 = Address::from([2u8; 20]);
    let signer3 = Address::from([3u8; 20]);

    cache
        .add_signature(
            &deposit_id,
            SignatureData {
                signer_address: signer1,
                signature: vec![1, 1, 1],
                proposal_hash,
                is_mine: true,
            },
            Some(proposal.clone()),
            move |_, _| Some(signer1),
        )
        .expect("seed sig1");

    cache
        .add_signature(
            &deposit_id,
            SignatureData {
                signer_address: signer2,
                signature: vec![2, 2, 2],
                proposal_hash,
                is_mine: false,
            },
            None,
            move |_, _| Some(signer2),
        )
        .expect("seed sig2");

    cache
        .add_signature(
            &deposit_id,
            SignatureData {
                signer_address: signer3,
                signature: vec![3, 3, 3],
                proposal_hash,
                is_mine: false,
            },
            None,
            move |_, _| Some(signer3),
        )
        .expect("seed sig3");

    assert!(cache.is_ready(&deposit_id).expect("ready check"));
    deposit_id
}

fn seed_collecting_proposal_with_my_signature(
    cache: &ProposalCache,
    signer_address: Address,
) -> DepositId {
    let proposal = sample_proposal(1);
    let proposal_hash = proposal.compute_proposal_hash();
    let deposit_id = DepositId::from_effect_payload(&proposal);

    cache
        .add_signature(
            &deposit_id,
            SignatureData {
                signer_address,
                signature: vec![9, 9, 9],
                proposal_hash,
                is_mine: true,
            },
            Some(proposal),
            move |_, _| Some(signer_address),
        )
        .expect("seed collecting my sig");

    assert!(!cache.is_ready(&deposit_id).expect("ready check"));
    deposit_id
}

fn non_proposer_node_id_for_height(height: u64) -> u64 {
    let base = test_node_config(0);
    let node_pkhs: Vec<_> = base
        .nodes
        .iter()
        .map(|node| node.nock_pkh.clone())
        .collect();
    let proposer = active_proposer(height, &node_pkhs);
    ((proposer + 1) % node_pkhs.len()) as u64
}

fn proposer_node_id_for_height(height: u64) -> u64 {
    let base = test_node_config(0);
    let node_pkhs: Vec<_> = base
        .nodes
        .iter()
        .map(|node| node.nock_pkh.clone())
        .collect();
    active_proposer(height, &node_pkhs) as u64
}

async fn seed_deposit_log_from_proposal(log: &DepositLog, proposal: &NockDepositRequestData) {
    let entry = DepositLogEntry {
        block_height: proposal.block_height,
        tx_id: proposal.tx_id.clone(),
        as_of: proposal.as_of.clone(),
        name: proposal.name.clone(),
        recipient: proposal.recipient,
        amount_to_mint: proposal.amount,
    };
    let outcome = log
        .insert_entry(&entry)
        .await
        .expect("seed deposit log entry");
    assert!(matches!(
        outcome,
        DepositLogInsertOutcome::Inserted | DepositLogInsertOutcome::ExistingMatch
    ));
}

#[tokio::test]
async fn posting_tick_not_my_turn_skips_submit_without_sleep() {
    let proposal_cache = Arc::new(ProposalCache::new());
    let deposit_id = seed_ready_proposal(&proposal_cache, 1);
    let base_contract = Arc::new(CountingBaseContract::with_last_nonce(0));
    let status_state = BridgeStatusState::new();
    let bridge_status = test_bridge_status();
    let node_config = test_node_config(non_proposer_node_id_for_height(100));

    let context = PostingTickContext::new(
        PostingTickPorts::new(proposal_cache.clone(), base_contract.clone()),
        PostingTickNodeState::new(node_config),
        PostingTickControl::new(bridge_status, status_state),
        PostingTickConfig::new(300),
    );
    let mut state = PostingTickState::default();

    let outcome = posting_tick_once(
        &context,
        &mut state,
        PostingTickInput {
            now: SystemTime::now(),
        },
    )
    .await;

    assert_eq!(outcome.ready_proposals, 1);
    assert_eq!(outcome.submitted, 0);
    assert_eq!(base_contract.submit_calls(), 0);
    assert!(!proposal_cache.is_confirmed(&deposit_id));
}

#[tokio::test]
async fn posting_tick_failover_zero_submits_without_sleep() {
    let proposal_cache = Arc::new(ProposalCache::new());
    let deposit_id = seed_ready_proposal(&proposal_cache, 1);
    let base_contract = Arc::new(CountingBaseContract::with_last_nonce(0));
    let status_state = BridgeStatusState::new();
    let bridge_status = test_bridge_status();
    let node_config = test_node_config(non_proposer_node_id_for_height(100));

    let context = PostingTickContext::new(
        PostingTickPorts::new(proposal_cache.clone(), base_contract.clone()),
        PostingTickNodeState::new(node_config),
        PostingTickControl::new(bridge_status, status_state),
        PostingTickConfig::new(0),
    );
    let mut state = PostingTickState::default();

    let outcome = posting_tick_once(
        &context,
        &mut state,
        PostingTickInput {
            now: SystemTime::now(),
        },
    )
    .await;

    assert_eq!(outcome.ready_proposals, 1);
    assert_eq!(outcome.submitted, 1);
    assert_eq!(base_contract.submit_calls(), 1);
    assert!(proposal_cache.is_confirmed(&deposit_id));
}

#[tokio::test]
async fn posting_tick_replay_submits_ready_proposals_in_nonce_order() {
    let proposal_cache = Arc::new(ProposalCache::new());
    let deposit_id_1 = seed_ready_proposal(&proposal_cache, 1);
    let deposit_id_2 = seed_ready_proposal(&proposal_cache, 2);
    let base_contract = Arc::new(CountingBaseContract::with_last_nonce(0));
    base_contract.script_last_nonce_responses([Ok(0), Ok(1)]);
    base_contract.script_submit_responses([
        Ok(DepositSubmissionResult {
            tx_hash: "tx-1".to_string(),
            block_number: 101,
        }),
        Ok(DepositSubmissionResult {
            tx_hash: "tx-2".to_string(),
            block_number: 102,
        }),
    ]);
    let status_state = BridgeStatusState::new();
    let bridge_status = test_bridge_status();
    let node_config = test_node_config(proposer_node_id_for_height(100));

    let context = PostingTickContext::new(
        PostingTickPorts::new(proposal_cache.clone(), base_contract.clone()),
        PostingTickNodeState::new(node_config),
        PostingTickControl::new(bridge_status, status_state),
        PostingTickConfig::new(300),
    );
    let mut state = PostingTickState::default();
    let now = UNIX_EPOCH + Duration::from_secs(1_700_000_000);

    let tick1 = posting_tick_once(&context, &mut state, PostingTickInput { now }).await;
    assert_eq!(tick1.ready_proposals, 2);
    assert_eq!(tick1.submitted, 1);
    assert_eq!(base_contract.get_last_nonce_calls(), 1);
    assert_eq!(base_contract.submit_calls(), 1);
    assert!(proposal_cache.is_confirmed(&deposit_id_1));
    assert!(!proposal_cache.is_confirmed(&deposit_id_2));

    let tick2 = posting_tick_once(
        &context,
        &mut state,
        PostingTickInput {
            now: now + Duration::from_secs(1),
        },
    )
    .await;
    assert_eq!(tick2.ready_proposals, 1);
    assert_eq!(tick2.submitted, 1);
    assert_eq!(base_contract.get_last_nonce_calls(), 2);
    assert_eq!(base_contract.submit_calls(), 2);
    assert!(proposal_cache.is_confirmed(&deposit_id_2));

    let tick3 = posting_tick_once(
        &context,
        &mut state,
        PostingTickInput {
            now: now + Duration::from_secs(2),
        },
    )
    .await;
    assert_eq!(tick3.ready_proposals, 0);
    assert_eq!(tick3.submitted, 0);
    assert_eq!(
        base_contract.get_last_nonce_calls(),
        2,
        "no chain nonce query should happen when no proposals are ready"
    );
}

#[tokio::test]
async fn signing_tick_without_tip_skips_chain_nonce_query() {
    let base_contract = Arc::new(CountingBaseContract::with_last_nonce(0));
    let deposit_log_dir = TempDir::new().expect("tempdir");
    let deposit_log_path = deposit_log_dir.path().join("deposit-log.sqlite");
    let deposit_log = Arc::new(
        DepositLog::open(deposit_log_path)
            .await
            .expect("open deposit log"),
    );
    let (runtime, runtime_handle) = BridgeRuntime::new(Arc::new(NoopCauseBuilder));
    let _runtime = runtime;
    let runtime_handle = Arc::new(runtime_handle);
    let nonce_epoch = NonceEpochConfig {
        base: 0,
        start_height: 1,
        start_tx_id: None,
    };
    let proposal_cache = Arc::new(ProposalCache::new());
    let signer = Arc::new(
        BridgeSigner::new(format!("0x{}", TEST_PRIVATE_KEY)).expect("valid test signer key"),
    );
    let bridge_status = test_bridge_status();
    let (stop_controller, stop) = StopController::new();

    let context = SigningTickContext::new(
        SigningTickPorts::new(
            runtime_handle,
            base_contract.clone(),
            deposit_log,
            proposal_cache,
        ),
        SigningTickNodeState::new(signer, HashSet::new(), Vec::new(), 0, HashMap::new()),
        SigningTickControl::new(bridge_status, stop_controller, stop),
        SigningTickConfig::new(
            &nonce_epoch,
            bridge::core::loop_policy::SigningLoopPolicy::default(),
        ),
    );
    let mut state = SigningTickState::new(SystemTime::now());

    let outcome = signing_tick_once(
        &context,
        &mut state,
        SigningTickInput {
            now: SystemTime::now(),
            tip_height: None,
        },
    )
    .await;

    assert_eq!(outcome.regossip_broadcasts, 0);
    assert_eq!(outcome.initial_broadcasts, 0);
    assert_eq!(base_contract.get_last_nonce_calls(), 1);
}

#[tokio::test]
async fn signing_tick_regossip_collecting_my_signature_with_tip() {
    let base_contract = Arc::new(CountingBaseContract::with_last_nonce(0));
    let deposit_log_dir = TempDir::new().expect("tempdir");
    let deposit_log_path = deposit_log_dir.path().join("deposit-log.sqlite");
    let deposit_log = Arc::new(
        DepositLog::open(deposit_log_path)
            .await
            .expect("open deposit log"),
    );
    let (runtime, runtime_handle) = BridgeRuntime::new(Arc::new(NoopCauseBuilder));
    let _runtime = runtime;
    let runtime_handle = Arc::new(runtime_handle);
    let nonce_epoch = NonceEpochConfig {
        base: 0,
        start_height: 1,
        start_tx_id: None,
    };
    let proposal_cache = Arc::new(ProposalCache::new());
    let signer = Arc::new(
        BridgeSigner::new(format!("0x{}", TEST_PRIVATE_KEY)).expect("valid test signer key"),
    );
    let _deposit_id = seed_collecting_proposal_with_my_signature(&proposal_cache, signer.address());
    let bridge_status = test_bridge_status();
    let (stop_controller, stop) = StopController::new();

    let context = SigningTickContext::new(
        SigningTickPorts::new(
            runtime_handle,
            base_contract.clone(),
            deposit_log,
            proposal_cache,
        ),
        SigningTickNodeState::new(signer, HashSet::new(), Vec::new(), 0, HashMap::new()),
        SigningTickControl::new(bridge_status, stop_controller, stop),
        SigningTickConfig::new(
            &nonce_epoch,
            bridge::core::loop_policy::SigningLoopPolicy::default(),
        ),
    );
    let mut state = SigningTickState::default();

    let outcome = signing_tick_once(
        &context,
        &mut state,
        SigningTickInput {
            now: SystemTime::now(),
            tip_height: Some(1),
        },
    )
    .await;

    assert_eq!(outcome.regossip_broadcasts, 1);
    assert_eq!(outcome.initial_broadcasts, 0);
    assert_eq!(base_contract.get_last_nonce_calls(), 1);
}

#[tokio::test]
async fn signing_tick_replay_waits_then_signs_then_skips_already_signed() {
    let base_contract = Arc::new(CountingBaseContract::with_last_nonce(0));
    base_contract.script_last_nonce_responses([Ok(0), Ok(0)]);
    base_contract.script_processed_responses([Ok(false)]);
    let deposit_log_dir = TempDir::new().expect("tempdir");
    let deposit_log_path = deposit_log_dir.path().join("deposit-log.sqlite");
    let deposit_log = Arc::new(
        DepositLog::open(deposit_log_path)
            .await
            .expect("open deposit log"),
    );
    let proposal = sample_proposal(1);
    seed_deposit_log_from_proposal(&deposit_log, &proposal).await;
    let deposit_id = DepositId::from_effect_payload(&proposal);
    let (runtime, runtime_handle) = BridgeRuntime::new(Arc::new(NoopCauseBuilder));
    let _runtime = runtime;
    let runtime_handle = Arc::new(runtime_handle);
    let nonce_epoch = NonceEpochConfig {
        base: 0,
        start_height: 1,
        start_tx_id: None,
    };
    let proposal_cache = Arc::new(ProposalCache::new());
    let signer = Arc::new(
        BridgeSigner::new(format!("0x{}", TEST_PRIVATE_KEY)).expect("valid test signer key"),
    );
    let mut valid_addresses = HashSet::new();
    valid_addresses.insert(signer.address());
    let bridge_status = test_bridge_status();
    let (stop_controller, stop) = StopController::new();

    let context = SigningTickContext::new(
        SigningTickPorts::new(
            runtime_handle,
            base_contract.clone(),
            deposit_log,
            proposal_cache.clone(),
        ),
        SigningTickNodeState::new(signer, valid_addresses, Vec::new(), 0, HashMap::new()),
        SigningTickControl::new(bridge_status, stop_controller, stop),
        SigningTickConfig::new(
            &nonce_epoch,
            bridge::core::loop_policy::SigningLoopPolicy::default(),
        ),
    );
    let start = UNIX_EPOCH + Duration::from_secs(1_700_000_000);
    let mut state = SigningTickState::new(start);

    let tick1 = signing_tick_once(
        &context,
        &mut state,
        SigningTickInput {
            now: start,
            tip_height: None,
        },
    )
    .await;
    assert_eq!(tick1.regossip_broadcasts, 0);
    assert_eq!(tick1.initial_broadcasts, 0);
    assert_eq!(base_contract.get_last_nonce_calls(), 1);
    assert_eq!(base_contract.is_processed_calls(), 0);

    let tick2 = signing_tick_once(
        &context,
        &mut state,
        SigningTickInput {
            now: start + Duration::from_secs(1),
            tip_height: Some(1),
        },
    )
    .await;
    assert_eq!(tick2.regossip_broadcasts, 0);
    assert_eq!(tick2.initial_broadcasts, 1);
    assert_eq!(base_contract.get_last_nonce_calls(), 2);
    assert_eq!(base_contract.is_processed_calls(), 1);

    let state_after_tick2 = proposal_cache
        .get_state(&deposit_id)
        .expect("proposal cache read")
        .expect("proposal state should exist after signing");
    assert_eq!(state_after_tick2.status, ProposalStatus::Collecting);
    assert!(
        state_after_tick2.my_signature.is_some(),
        "tick2 should persist our signature for replay determinism"
    );

    let tick3 = signing_tick_once(
        &context,
        &mut state,
        SigningTickInput {
            now: start + Duration::from_secs(2),
            tip_height: Some(1),
        },
    )
    .await;
    assert_eq!(tick3.regossip_broadcasts, 0);
    assert_eq!(tick3.initial_broadcasts, 0);
    assert_eq!(base_contract.get_last_nonce_calls(), 3);
    assert_eq!(
        base_contract.is_processed_calls(),
        1,
        "already-signed candidate should skip processedDeposits query on replay tick"
    );
}

#[tokio::test]
async fn signing_tick_nonce_query_error_returns_without_actions() {
    let base_contract = Arc::new(CountingBaseContract::with_nonce_query_error());
    let deposit_log_dir = TempDir::new().expect("tempdir");
    let deposit_log_path = deposit_log_dir.path().join("deposit-log.sqlite");
    let deposit_log = Arc::new(
        DepositLog::open(deposit_log_path)
            .await
            .expect("open deposit log"),
    );
    let (runtime, runtime_handle) = BridgeRuntime::new(Arc::new(NoopCauseBuilder));
    let _runtime = runtime;
    let runtime_handle = Arc::new(runtime_handle);
    let nonce_epoch = NonceEpochConfig {
        base: 0,
        start_height: 1,
        start_tx_id: None,
    };
    let proposal_cache = Arc::new(ProposalCache::new());
    let signer = Arc::new(
        BridgeSigner::new(format!("0x{}", TEST_PRIVATE_KEY)).expect("valid test signer key"),
    );
    let bridge_status = test_bridge_status();
    let (stop_controller, stop) = StopController::new();

    let context = SigningTickContext::new(
        SigningTickPorts::new(
            runtime_handle,
            base_contract.clone(),
            deposit_log,
            proposal_cache,
        ),
        SigningTickNodeState::new(signer, HashSet::new(), Vec::new(), 0, HashMap::new()),
        SigningTickControl::new(bridge_status, stop_controller, stop),
        SigningTickConfig::new(
            &nonce_epoch,
            bridge::core::loop_policy::SigningLoopPolicy::default(),
        ),
    );
    let mut state = SigningTickState::new(SystemTime::now());

    let outcome = signing_tick_once(
        &context,
        &mut state,
        SigningTickInput {
            now: SystemTime::now(),
            tip_height: Some(1),
        },
    )
    .await;

    assert_eq!(outcome.regossip_broadcasts, 0);
    assert_eq!(outcome.initial_broadcasts, 0);
    assert_eq!(base_contract.get_last_nonce_calls(), 1);
}

#[tokio::test]
async fn signing_tick_nonce_epoch_mismatch_triggers_local_stop_without_runtime_peek() {
    let base_contract = Arc::new(CountingBaseContract::with_last_nonce(0));
    let deposit_log_dir = TempDir::new().expect("tempdir");
    let deposit_log_path = deposit_log_dir.path().join("deposit-log.sqlite");
    let deposit_log = Arc::new(
        DepositLog::open(deposit_log_path)
            .await
            .expect("open deposit log"),
    );
    let (runtime, runtime_handle) = BridgeRuntime::new(Arc::new(NoopCauseBuilder));
    let _runtime = runtime;
    let runtime_handle = Arc::new(runtime_handle);
    let nonce_epoch = NonceEpochConfig {
        base: 1,
        start_height: 1,
        start_tx_id: None,
    };
    let proposal_cache = Arc::new(ProposalCache::new());
    let signer = Arc::new(
        BridgeSigner::new(format!("0x{}", TEST_PRIVATE_KEY)).expect("valid test signer key"),
    );
    let bridge_status = test_bridge_status();
    let (stop_controller, stop) = StopController::new();
    let stop_probe = stop.clone();

    let context = SigningTickContext::new(
        SigningTickPorts::new(
            runtime_handle,
            base_contract.clone(),
            deposit_log,
            proposal_cache,
        ),
        SigningTickNodeState::new(signer, HashSet::new(), Vec::new(), 0, HashMap::new()),
        SigningTickControl::new(bridge_status, stop_controller, stop)
            .with_local_stop_mode(SigningLocalStopMode::LocalTriggerOnly),
        SigningTickConfig::new(
            &nonce_epoch,
            bridge::core::loop_policy::SigningLoopPolicy::default(),
        ),
    );
    let mut state = SigningTickState::new(SystemTime::now());

    let outcome = signing_tick_once(
        &context,
        &mut state,
        SigningTickInput {
            now: SystemTime::now(),
            tip_height: Some(1),
        },
    )
    .await;

    assert_eq!(outcome.regossip_broadcasts, 0);
    assert_eq!(outcome.initial_broadcasts, 0);
    assert_eq!(base_contract.get_last_nonce_calls(), 1);
    assert!(stop_probe.is_stopped());
}

#[tokio::test]
async fn signing_tick_sqlite_count_error_triggers_local_stop_without_runtime_peek() {
    let base_contract = Arc::new(CountingBaseContract::with_last_nonce(0));
    let deposit_log_dir = TempDir::new().expect("tempdir");
    let deposit_log_path = deposit_log_dir.path().join("deposit-log.sqlite");
    let deposit_log = Arc::new(
        DepositLog::open(deposit_log_path)
            .await
            .expect("open deposit log"),
    );
    let (runtime, runtime_handle) = BridgeRuntime::new(Arc::new(NoopCauseBuilder));
    let _runtime = runtime;
    let runtime_handle = Arc::new(runtime_handle);
    let nonce_epoch = NonceEpochConfig {
        base: 0,
        start_height: u64::MAX,
        start_tx_id: None,
    };
    let proposal_cache = Arc::new(ProposalCache::new());
    let signer = Arc::new(
        BridgeSigner::new(format!("0x{}", TEST_PRIVATE_KEY)).expect("valid test signer key"),
    );
    let bridge_status = test_bridge_status();
    let (stop_controller, stop) = StopController::new();
    let stop_probe = stop.clone();

    let context = SigningTickContext::new(
        SigningTickPorts::new(
            runtime_handle,
            base_contract.clone(),
            deposit_log,
            proposal_cache,
        ),
        SigningTickNodeState::new(signer, HashSet::new(), Vec::new(), 0, HashMap::new()),
        SigningTickControl::new(bridge_status, stop_controller, stop)
            .with_local_stop_mode(SigningLocalStopMode::LocalTriggerOnly),
        SigningTickConfig::new(
            &nonce_epoch,
            bridge::core::loop_policy::SigningLoopPolicy::default(),
        ),
    );
    let mut state = SigningTickState::new(SystemTime::now());

    let outcome = signing_tick_once(
        &context,
        &mut state,
        SigningTickInput {
            now: SystemTime::now(),
            tip_height: Some(u64::MAX),
        },
    )
    .await;

    assert_eq!(outcome.regossip_broadcasts, 0);
    assert_eq!(outcome.initial_broadcasts, 0);
    assert_eq!(base_contract.get_last_nonce_calls(), 1);
    assert!(stop_probe.is_stopped());
}

#![allow(clippy::unwrap_used)]

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, RwLock};
use std::time::{Duration, SystemTime};

use alloy::primitives::Address;
use async_trait::async_trait;
use bridge::core::loop_policy::PostingLoopPolicy;
use bridge::deposit::base::DepositSubmissionResult;
use bridge::deposit::cache::{ProposalCache, SignatureData};
use bridge::deposit::ports::BaseContractPort;
use bridge::deposit::posting::run_posting_loop_with_policy;
use bridge::deposit::types::{DepositId, DepositSubmission, NockDepositRequestData};
use bridge::observability::health::SharedHealthState;
use bridge::observability::status::{BridgeStatus, BridgeStatusState};
use bridge::shared::errors::BridgeError;
use bridge::shared::stop::{StopController, StopInfo, StopSource};
use bridge::shared::types::{
    zero_tip5_hash, AtomBytes, EthAddress, NodeConfig, NodeInfo, SchnorrSecretKey, Tip5Hash,
};
use nockchain_math::belt::Belt;
use nockchain_types::tx_engine::common::Hash as NockPkh;
use nockchain_types::v1::Name;

#[derive(Clone, Default)]
struct CountingBaseContract {
    get_last_nonce_calls: Arc<AtomicUsize>,
    is_processed_calls: Arc<AtomicUsize>,
    submit_calls: Arc<AtomicUsize>,
}

impl CountingBaseContract {
    fn get_last_nonce_calls(&self) -> usize {
        self.get_last_nonce_calls.load(Ordering::SeqCst)
    }

    fn submit_calls(&self) -> usize {
        self.submit_calls.load(Ordering::SeqCst)
    }
}

#[async_trait]
impl BaseContractPort for CountingBaseContract {
    async fn submit_deposit(
        &self,
        _submission: DepositSubmission,
    ) -> Result<DepositSubmissionResult, BridgeError> {
        self.submit_calls.fetch_add(1, Ordering::SeqCst);
        Ok(DepositSubmissionResult {
            tx_hash: "in-memory".to_string(),
            block_number: 0,
        })
    }

    async fn get_last_deposit_nonce(&self) -> Result<u64, BridgeError> {
        self.get_last_nonce_calls.fetch_add(1, Ordering::SeqCst);
        Ok(0)
    }

    async fn is_deposit_processed(&self, _tx_id: &Tip5Hash) -> Result<bool, BridgeError> {
        self.is_processed_calls.fetch_add(1, Ordering::SeqCst);
        Ok(false)
    }
}

fn test_bridge_status() -> BridgeStatus {
    let health: SharedHealthState = Arc::new(RwLock::new(Vec::new()));
    BridgeStatus::new(health)
}

fn trigger_stop_now(controller: &StopController) {
    controller.trigger(StopInfo {
        reason: "test stop".to_string(),
        last: None,
        source: StopSource::Local,
        at: SystemTime::now(),
    });
}

fn test_node_config() -> NodeConfig {
    NodeConfig {
        node_id: 0,
        nodes: vec![NodeInfo {
            ip: "localhost:8001".to_string(),
            eth_pubkey: AtomBytes(vec![0u8; 20]),
            nock_pkh: NockPkh::from_base58("2222222222222222222222222222222222222222222222222222")
                .expect("valid test nock pkh"),
        }],
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

fn seed_ready_proposal(cache: &ProposalCache) -> DepositId {
    let proposal = sample_proposal(1);
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

#[tokio::test]
async fn posting_loop_stopped_state_with_ready_proposal_skips_base_contract_calls() {
    let proposal_cache = Arc::new(ProposalCache::new());
    let base_contract = Arc::new(CountingBaseContract::default());
    let bridge_status = test_bridge_status();
    let status_state = BridgeStatusState::new();
    let node_config = test_node_config();
    let (stop_controller, stop_handle) = StopController::new();
    trigger_stop_now(&stop_controller);

    let _deposit_id = seed_ready_proposal(&proposal_cache);
    assert_eq!(
        proposal_cache
            .ready_proposals()
            .expect("ready proposals")
            .len(),
        1
    );

    let policy = PostingLoopPolicy {
        tick_interval: Duration::from_millis(5),
        failover_backoff_secs: 1,
    };

    let task = tokio::spawn(run_posting_loop_with_policy(
        proposal_cache,
        base_contract.clone(),
        node_config,
        bridge_status,
        stop_handle,
        status_state,
        policy,
    ));

    tokio::time::sleep(Duration::from_millis(35)).await;

    assert_eq!(base_contract.get_last_nonce_calls(), 0);
    assert_eq!(base_contract.submit_calls(), 0);

    task.abort();
    let _ = task.await;
}

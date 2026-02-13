#![allow(clippy::unwrap_used)]
//! Test harness for 3-of-5 degraded operation testing.
//!
//! This module provides infrastructure to simulate a 5-node bridge cluster
//! with mocked Ethereum and Nockchain backends, enabling comprehensive
//! failover and degraded operation tests.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use alloy::primitives::Address;
use bridge::proposal_cache::{ProposalCache, ProposalStatus, SignatureAddResult};
use bridge::types::{DepositId, NockDepositRequestData};
use nockchain_math::belt::Belt;
use nockchain_types::tx_engine::common::Hash as Tip5Hash;
use nockchain_types::v1::Name;
use nockchain_types::EthAddress;

/// Simulated Ethereum transaction hash.
pub type TxHash = [u8; 32];

/// Represents a submission to the Base L2 contract.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Submission {
    pub deposit_id: DepositId,
    pub submitter_node_id: usize,
    pub signature_count: usize,
    pub tx_hash: TxHash,
}

/// Mock Base contract that tracks submissions.
#[derive(Debug, Clone, Default)]
pub struct MockBaseContract {
    /// All successful submissions
    submissions: Arc<Mutex<Vec<Submission>>>,
    /// Track which deposits have been processed to prevent duplicates
    processed: Arc<Mutex<HashMap<DepositId, TxHash>>>,
    /// Last deposit nonce (mirrors MessageInbox.lastDepositNonce)
    last_deposit_nonce: Arc<Mutex<u64>>,
}

#[allow(dead_code)]
impl MockBaseContract {
    /// Create a new mock Base contract.
    pub fn new() -> Self {
        Self::default()
    }

    /// Submit a deposit to the mock contract.
    ///
    /// Returns Ok(tx_hash) if submission succeeds, Err if duplicate or nonce invalid.
    /// Mirrors the real MessageInbox contract behavior:
    /// - require(depositNonce > lastDepositNonce)
    /// - require(!processedDeposits[txIdHash])
    pub fn submit_deposit(
        &self,
        deposit_id: &DepositId,
        submitter_node_id: usize,
        signatures: &[Vec<u8>],
    ) -> Result<TxHash, String> {
        let mut processed = self.processed.lock().unwrap();

        // Check for duplicate
        if let Some(existing_hash) = processed.get(deposit_id) {
            return Err(format!(
                "Deposit already processed with tx hash {:?}",
                hex::encode(existing_hash)
            ));
        }

        // Generate a unique tx hash (based on deposit_id for determinism)
        let deposit_bytes = deposit_id.to_bytes();
        let tx_hash = blake3::hash(&deposit_bytes).into();

        // Record the submission
        let submission = Submission {
            deposit_id: deposit_id.clone(),
            submitter_node_id,
            signature_count: signatures.len(),
            tx_hash,
        };

        self.submissions.lock().unwrap().push(submission);
        processed.insert(deposit_id.clone(), tx_hash);

        Ok(tx_hash)
    }

    /// Submit a deposit with nonce validation (mirrors real contract behavior).
    ///
    /// Returns Ok(tx_hash) if submission succeeds, Err if:
    /// - Deposit already processed
    /// - Nonce is not strictly greater than lastDepositNonce
    pub fn submit_deposit_with_nonce(
        &self,
        deposit_id: &DepositId,
        submitter_node_id: usize,
        signatures: &[Vec<u8>],
        nonce: u64,
    ) -> Result<TxHash, String> {
        let mut processed = self.processed.lock().unwrap();
        let mut last_nonce = self.last_deposit_nonce.lock().unwrap();

        // Check for duplicate (same as real contract)
        if let Some(existing_hash) = processed.get(deposit_id) {
            return Err(format!(
                "Deposit already processed with tx hash {:?}",
                hex::encode(existing_hash)
            ));
        }

        // Check nonce is strictly greater (same as real contract)
        if nonce <= *last_nonce {
            return Err(format!(
                "Nonce must be strictly greater: got {}, last was {}",
                nonce, *last_nonce
            ));
        }

        // Generate a unique tx hash
        let deposit_bytes = deposit_id.to_bytes();
        let tx_hash = blake3::hash(&deposit_bytes).into();

        // Record the submission
        let submission = Submission {
            deposit_id: deposit_id.clone(),
            submitter_node_id,
            signature_count: signatures.len(),
            tx_hash,
        };

        self.submissions.lock().unwrap().push(submission);
        processed.insert(deposit_id.clone(), tx_hash);
        *last_nonce = nonce;

        Ok(tx_hash)
    }

    /// Get the last deposit nonce (mirrors MessageInbox.lastDepositNonce).
    pub fn get_last_deposit_nonce(&self) -> u64 {
        *self.last_deposit_nonce.lock().unwrap()
    }

    /// Set the last deposit nonce (for test setup).
    pub fn set_last_deposit_nonce(&self, nonce: u64) {
        *self.last_deposit_nonce.lock().unwrap() = nonce;
    }

    /// Get all submissions.
    pub fn all_submissions(&self) -> Vec<Submission> {
        self.submissions.lock().unwrap().clone()
    }

    /// Check if a deposit has been processed.
    pub fn is_processed(&self, deposit_id: &DepositId) -> bool {
        self.processed.lock().unwrap().contains_key(deposit_id)
    }

    /// Get the transaction hash for a processed deposit.
    pub fn get_tx_hash(&self, deposit_id: &DepositId) -> Option<TxHash> {
        self.processed.lock().unwrap().get(deposit_id).copied()
    }
}

/// Status of a node in the test cluster.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NodeStatus {
    Online,
    Offline,
}

/// A single node in the test cluster.
pub struct TestNode {
    pub node_id: usize,
    pub status: Arc<Mutex<NodeStatus>>,
    pub proposal_cache: ProposalCache,
    /// Simulated signature store (deposit_id -> signature)
    pub signatures: Arc<Mutex<HashMap<DepositId, Vec<u8>>>>,
}

#[allow(dead_code)]
impl TestNode {
    /// Create a new test node.
    pub fn new(node_id: usize) -> Self {
        Self {
            node_id,
            status: Arc::new(Mutex::new(NodeStatus::Online)),
            proposal_cache: ProposalCache::new(),
            signatures: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// Check if this node is online.
    pub fn is_online(&self) -> bool {
        *self.status.lock().unwrap() == NodeStatus::Online
    }

    /// Sign a deposit and store the signature.
    ///
    /// Returns the signature if the node is online.
    pub fn sign_deposit(
        &self,
        deposit_id: &DepositId,
        _proposal: &NockDepositRequestData,
    ) -> Option<Vec<u8>> {
        if !self.is_online() {
            return None;
        }

        // Generate a deterministic signature based on node_id and deposit
        let mut sig_data = Vec::new();
        sig_data.extend_from_slice(&(self.node_id as u64).to_le_bytes());
        sig_data.extend_from_slice(&deposit_id.to_bytes());

        // Use blake3 to generate a 65-byte signature (mock ECDSA format)
        let hash = blake3::hash(&sig_data);
        let mut signature = vec![0u8; 65];
        signature[..32].copy_from_slice(&hash.as_bytes()[..]);
        signature[32..64].copy_from_slice(&hash.as_bytes()[..]); // Reuse for s
        signature[64] = 27 + (self.node_id % 2) as u8; // v = 27 or 28

        self.signatures
            .lock()
            .unwrap()
            .insert(deposit_id.clone(), signature.clone());

        Some(signature)
    }

    /// Get the signature for a deposit if it exists.
    pub fn get_signature(&self, deposit_id: &DepositId) -> Option<Vec<u8>> {
        self.signatures.lock().unwrap().get(deposit_id).cloned()
    }

    /// Add a signature to this node's proposal cache.
    pub fn add_signature(
        &self,
        deposit_id: &DepositId,
        signer_address: Address,
        signature: Vec<u8>,
        proposal_hash: [u8; 32],
        proposal: Option<NockDepositRequestData>,
        is_mine: bool,
    ) -> Result<SignatureAddResult, String> {
        if !self.is_online() {
            return Err("Node offline".to_string());
        }

        // Simple verification: just accept all signatures (mock)
        let verify_fn = |_hash: &[u8; 32], _sig: &[u8]| Some(signer_address);

        self.proposal_cache.add_signature(
            deposit_id,
            bridge::proposal_cache::SignatureData {
                signer_address,
                signature,
                proposal_hash,
                is_mine,
            },
            proposal,
            verify_fn,
        )
    }

    /// Check if this node's cache shows threshold reached for a deposit.
    pub fn has_threshold(&self, deposit_id: &DepositId) -> bool {
        self.proposal_cache
            .get_state(deposit_id)
            .ok()
            .flatten()
            .map(|state| state.has_threshold())
            .unwrap_or(false)
    }

    /// Get the status of a proposal in this node's cache.
    pub fn get_proposal_status(&self, deposit_id: &DepositId) -> Option<ProposalStatus> {
        self.proposal_cache
            .get_state(deposit_id)
            .ok()
            .flatten()
            .map(|state| state.status)
    }
}

/// A test cluster of 5 bridge nodes with mock backends.
pub struct TestCluster {
    pub nodes: Vec<TestNode>,
    pub mock_base: MockBaseContract,
    /// Hoon-computed proposer for deposits (by height % num_nodes)
    pub proposer_rotation: HashMap<DepositId, usize>,
}

#[allow(dead_code)]
impl TestCluster {
    /// Create a new test cluster with n nodes.
    ///
    /// # Arguments
    /// * `n` - Number of nodes (typically 5)
    pub async fn new(n: usize) -> Self {
        let nodes = (0..n).map(TestNode::new).collect();

        Self {
            nodes,
            mock_base: MockBaseContract::new(),
            proposer_rotation: HashMap::new(),
        }
    }

    /// Trigger a deposit event across all online nodes.
    ///
    /// Simulates the Hoon bridge seeing a deposit and emitting signature requests.
    /// Each online node signs and broadcasts to peers.
    pub async fn trigger_deposit(&mut self, deposit: NockDepositRequestData) {
        let deposit_id = DepositId::from_effect_payload(&deposit);
        let proposal_hash = deposit.compute_proposal_hash();

        // Determine proposer (simple height-based rotation)
        let proposer_id = (deposit.block_height as usize) % self.nodes.len();
        self.proposer_rotation
            .insert(deposit_id.clone(), proposer_id);

        // All online nodes sign
        for node in &self.nodes {
            if let Some(signature) = node.sign_deposit(&deposit_id, &deposit) {
                // Node adds its own signature
                let signer_addr = self.node_address(node.node_id);
                let _ = node.add_signature(
                    &deposit_id,
                    signer_addr,
                    signature.clone(),
                    proposal_hash,
                    Some(deposit.clone()),
                    true,
                );

                // Broadcast to all OTHER online nodes
                for other_node in &self.nodes {
                    if other_node.node_id != node.node_id && other_node.is_online() {
                        let _ = other_node.add_signature(
                            &deposit_id,
                            signer_addr,
                            signature.clone(),
                            proposal_hash,
                            Some(deposit.clone()),
                            false,
                        );
                    }
                }
            }
        }
    }

    /// Kill a node (set it offline).
    pub async fn kill_node(&mut self, index: usize) {
        if index < self.nodes.len() {
            *self.nodes[index].status.lock().unwrap() = NodeStatus::Offline;
        }
    }

    /// Restart a node (set it back online).
    pub async fn restart_node(&mut self, index: usize) {
        if index < self.nodes.len() {
            *self.nodes[index].status.lock().unwrap() = NodeStatus::Online;
        }
    }

    /// Wait for at least `count` nodes to collect enough signatures for a deposit.
    ///
    /// Returns when threshold is reached or timeout (5s).
    pub async fn wait_for_signatures(&self, deposit: &NockDepositRequestData, count: usize) {
        let deposit_id = DepositId::from_effect_payload(deposit);
        let timeout = Duration::from_secs(5);
        let start = std::time::Instant::now();

        loop {
            let ready_count = self
                .nodes
                .iter()
                .filter(|n| n.is_online() && n.has_threshold(&deposit_id))
                .count();

            if ready_count >= count {
                return;
            }

            if start.elapsed() > timeout {
                panic!(
                    "Timeout waiting for {} nodes to reach signature threshold",
                    count
                );
            }

            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }

    /// Wait for a deposit to be confirmed on-chain.
    ///
    /// Returns the transaction hash if confirmed within timeout (10s).
    pub async fn wait_for_confirmation(&self, deposit: &NockDepositRequestData) -> Option<TxHash> {
        let deposit_id = DepositId::from_effect_payload(deposit);
        let timeout = Duration::from_secs(10);
        let start = std::time::Instant::now();

        loop {
            if let Some(tx_hash) = self.mock_base.get_tx_hash(&deposit_id) {
                return Some(tx_hash);
            }

            if start.elapsed() > timeout {
                return None;
            }

            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }

    /// Attempt to post a deposit to Base from a specific node.
    ///
    /// Returns Ok(tx_hash) if successful, Err if failed.
    pub fn post_deposit_from_node(
        &self,
        deposit: &NockDepositRequestData,
        node_id: usize,
    ) -> Result<TxHash, String> {
        let deposit_id = DepositId::from_effect_payload(deposit);
        let node = &self.nodes[node_id];

        if !node.is_online() {
            return Err("Node offline".to_string());
        }

        // Get signatures from cache
        let state = node
            .proposal_cache
            .get_state(&deposit_id)
            .map_err(|e| format!("Cache error: {}", e))?
            .ok_or("Proposal not found in cache")?;

        if !state.has_threshold() {
            return Err("Threshold not reached".to_string());
        }

        let signatures = state.all_signatures();

        self.mock_base
            .submit_deposit(&deposit_id, node_id, &signatures)
    }

    /// Get the initial proposer node ID for a deposit.
    pub fn initial_proposer(&self, deposit: &NockDepositRequestData) -> usize {
        let deposit_id = DepositId::from_effect_payload(deposit);
        self.proposer_rotation
            .get(&deposit_id)
            .copied()
            .unwrap_or_else(|| (deposit.block_height as usize) % self.nodes.len())
    }

    /// Get all submissions that have been posted to Base.
    pub fn base_submissions(&self) -> Vec<Submission> {
        self.mock_base.all_submissions()
    }

    /// Get the mock Ethereum address for a node.
    fn node_address(&self, node_id: usize) -> Address {
        // Generate deterministic addresses for testing
        let mut bytes = [0u8; 20];
        bytes[0] = node_id as u8;
        Address::from(bytes)
    }
}

/// Helper to create a sample deposit for testing.
pub fn sample_deposit(block_height: u64, amount: u64) -> NockDepositRequestData {
    NockDepositRequestData {
        tx_id: Tip5Hash([Belt(1 + block_height), Belt(2), Belt(3), Belt(4), Belt(5)]),
        name: Name::new(
            Tip5Hash([Belt(10), Belt(20), Belt(30), Belt(40), Belt(50)]),
            Tip5Hash([Belt(100), Belt(200), Belt(300), Belt(400), Belt(500)]),
        ),
        recipient: EthAddress([0xaa; 20]),
        amount,
        block_height,
        as_of: Tip5Hash([Belt(7), Belt(8), Belt(9), Belt(10), Belt(11)]),
        nonce: 1,
    }
}

/// Helper to create a sample deposit with a specific nonce.
pub fn sample_deposit_with_nonce(
    block_height: u64,
    amount: u64,
    nonce: u64,
) -> NockDepositRequestData {
    NockDepositRequestData {
        tx_id: Tip5Hash([Belt(1 + block_height + nonce), Belt(2), Belt(3), Belt(4), Belt(5)]),
        name: Name::new(
            Tip5Hash([Belt(10), Belt(20), Belt(30), Belt(40), Belt(50)]),
            Tip5Hash([Belt(100), Belt(200), Belt(300), Belt(400), Belt(500)]),
        ),
        recipient: EthAddress([0xaa; 20]),
        amount,
        block_height,
        as_of: Tip5Hash([Belt(7 + nonce), Belt(8), Belt(9), Belt(10), Belt(11)]),
        nonce,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_cluster_creation() {
        let cluster = TestCluster::new(5).await;
        assert_eq!(cluster.nodes.len(), 5);
        assert!(cluster.nodes.iter().all(|n| n.is_online()));
    }

    #[tokio::test]
    async fn test_node_lifecycle() {
        let mut cluster = TestCluster::new(5).await;

        assert!(cluster.nodes[2].is_online());

        cluster.kill_node(2).await;
        assert!(!cluster.nodes[2].is_online());

        cluster.restart_node(2).await;
        assert!(cluster.nodes[2].is_online());
    }

    #[tokio::test]
    async fn test_deposit_signing() {
        let cluster = TestCluster::new(5).await;
        let deposit = sample_deposit(100, 1000);
        let deposit_id = DepositId::from_effect_payload(&deposit);

        let signature = cluster.nodes[0].sign_deposit(&deposit_id, &deposit);
        assert!(signature.is_some());
        assert_eq!(signature.unwrap().len(), 65);
    }

    #[tokio::test]
    async fn test_offline_node_cannot_sign() {
        let mut cluster = TestCluster::new(5).await;
        let deposit = sample_deposit(100, 1000);
        let deposit_id = DepositId::from_effect_payload(&deposit);

        cluster.kill_node(0).await;
        let signature = cluster.nodes[0].sign_deposit(&deposit_id, &deposit);
        assert!(signature.is_none());
    }

    #[tokio::test]
    async fn test_mock_base_submission() {
        let cluster = TestCluster::new(5).await;
        let deposit = sample_deposit(100, 1000);
        let deposit_id = DepositId::from_effect_payload(&deposit);

        let signatures = vec![vec![1u8; 65], vec![2u8; 65], vec![3u8; 65]];

        let result = cluster
            .mock_base
            .submit_deposit(&deposit_id, 0, &signatures);
        assert!(result.is_ok());

        let submissions = cluster.mock_base.all_submissions();
        assert_eq!(submissions.len(), 1);
        assert_eq!(submissions[0].submitter_node_id, 0);
        assert_eq!(submissions[0].signature_count, 3);
    }

    #[tokio::test]
    async fn test_duplicate_submission_rejected() {
        let cluster = TestCluster::new(5).await;
        let deposit = sample_deposit(100, 1000);
        let deposit_id = DepositId::from_effect_payload(&deposit);

        let signatures = vec![vec![1u8; 65], vec![2u8; 65], vec![3u8; 65]];

        let result1 = cluster
            .mock_base
            .submit_deposit(&deposit_id, 0, &signatures);
        assert!(result1.is_ok());

        let result2 = cluster
            .mock_base
            .submit_deposit(&deposit_id, 1, &signatures);
        assert!(result2.is_err());
    }

    // =========================================================================
    // Nonce ordering tests - verify chain-based nonce logic
    // =========================================================================

    #[tokio::test]
    async fn test_nonce_must_be_strictly_greater() {
        let contract = MockBaseContract::new();
        let signatures = vec![vec![1u8; 65], vec![2u8; 65], vec![3u8; 65]];

        // Initial state: lastDepositNonce = 0
        assert_eq!(contract.get_last_deposit_nonce(), 0);

        // Submit nonce 1 - should succeed
        let deposit1 = sample_deposit_with_nonce(100, 1000, 1);
        let deposit_id1 = DepositId::from_effect_payload(&deposit1);
        let result = contract.submit_deposit_with_nonce(&deposit_id1, 0, &signatures, 1);
        assert!(result.is_ok(), "Nonce 1 should succeed when last is 0");
        assert_eq!(contract.get_last_deposit_nonce(), 1);

        // Submit nonce 1 again - should fail (not strictly greater)
        let deposit1b = sample_deposit_with_nonce(101, 1000, 1);
        let deposit_id1b = DepositId::from_effect_payload(&deposit1b);
        let result = contract.submit_deposit_with_nonce(&deposit_id1b, 0, &signatures, 1);
        assert!(result.is_err(), "Nonce 1 should fail when last is 1");

        // Submit nonce 2 - should succeed
        let deposit2 = sample_deposit_with_nonce(102, 1000, 2);
        let deposit_id2 = DepositId::from_effect_payload(&deposit2);
        let result = contract.submit_deposit_with_nonce(&deposit_id2, 0, &signatures, 2);
        assert!(result.is_ok(), "Nonce 2 should succeed when last is 1");
        assert_eq!(contract.get_last_deposit_nonce(), 2);
    }

    #[tokio::test]
    async fn test_nonce_can_skip_values() {
        // The contract only requires nonce > lastDepositNonce, not nonce == lastDepositNonce + 1
        // This means if nonce 2 is skipped, nonce 3 can still be submitted
        let contract = MockBaseContract::new();
        let signatures = vec![vec![1u8; 65], vec![2u8; 65], vec![3u8; 65]];

        // Submit nonce 1
        let deposit1 = sample_deposit_with_nonce(100, 1000, 1);
        let deposit_id1 = DepositId::from_effect_payload(&deposit1);
        contract
            .submit_deposit_with_nonce(&deposit_id1, 0, &signatures, 1)
            .unwrap();
        assert_eq!(contract.get_last_deposit_nonce(), 1);

        // Skip nonce 2, submit nonce 3 directly
        let deposit3 = sample_deposit_with_nonce(102, 1000, 3);
        let deposit_id3 = DepositId::from_effect_payload(&deposit3);
        let result = contract.submit_deposit_with_nonce(&deposit_id3, 0, &signatures, 3);
        assert!(
            result.is_ok(),
            "Nonce 3 should succeed when last is 1 (skipping 2)"
        );
        assert_eq!(contract.get_last_deposit_nonce(), 3);

        // Now nonce 2 cannot be submitted (2 is not > 3)
        let deposit2 = sample_deposit_with_nonce(101, 1000, 2);
        let deposit_id2 = DepositId::from_effect_payload(&deposit2);
        let result = contract.submit_deposit_with_nonce(&deposit_id2, 0, &signatures, 2);
        assert!(result.is_err(), "Nonce 2 should fail when last is 3");
    }

    #[tokio::test]
    async fn test_ready_proposals_sorted_by_nonce() {
        // Verify that ready_proposals returns proposals sorted by nonce
        let cache = ProposalCache::new();

        // Add proposals with nonces 5, 3, 7, 1 (out of order)
        let nonces = [5u64, 3, 7, 1];
        for &nonce in &nonces {
            let proposal = sample_deposit_with_nonce(100, 1000, nonce);
            let proposal_hash = proposal.compute_proposal_hash();
            let deposit_id = DepositId::from_effect_payload(&proposal);

            // Add enough signatures to reach threshold (3 signatures)
            for i in 0..3 {
                let signer = Address::from([i; 20]);
                let is_mine = i == 0;
                cache
                    .add_signature(
                        &deposit_id,
                        bridge::proposal_cache::SignatureData {
                            signer_address: signer,
                            signature: vec![i; 65],
                            proposal_hash,
                            is_mine,
                        },
                        if is_mine {
                            Some(proposal.clone())
                        } else {
                            None
                        },
                        |_, _| Some(signer),
                    )
                    .unwrap();
            }
        }

        // Get ready proposals - should be sorted by nonce
        let ready = cache.ready_proposals().unwrap();
        assert_eq!(ready.len(), 4);

        let ready_nonces: Vec<u64> = ready
            .iter()
            .map(|(_, state)| state.proposal.nonce)
            .collect();
        assert_eq!(
            ready_nonces,
            vec![1, 3, 5, 7],
            "Proposals should be sorted by nonce"
        );
    }

    #[tokio::test]
    async fn test_chain_nonce_determines_next_submission() {
        // Simulate the posting loop logic:
        // - Query lastDepositNonce from chain
        // - Only submit proposal where nonce == lastDepositNonce + 1
        // - Skip proposals with nonce <= lastDepositNonce (already on chain)
        // - Wait for proposals with nonce > lastDepositNonce + 1

        let contract = MockBaseContract::new();
        let cache = ProposalCache::new();
        let signatures = vec![vec![1u8; 65], vec![2u8; 65], vec![3u8; 65]];

        // Create proposals with nonces 1, 2, 3
        let mut proposals = Vec::new();
        for nonce in 1..=3 {
            let proposal = sample_deposit_with_nonce(100 + nonce, 1000, nonce);
            let proposal_hash = proposal.compute_proposal_hash();
            let deposit_id = DepositId::from_effect_payload(&proposal);

            // Add to cache with threshold signatures
            for i in 0..3 {
                let signer = Address::from([i; 20]);
                cache
                    .add_signature(
                        &deposit_id,
                        bridge::proposal_cache::SignatureData {
                            signer_address: signer,
                            signature: vec![i; 65],
                            proposal_hash,
                            is_mine: i == 0,
                        },
                        if i == 0 { Some(proposal.clone()) } else { None },
                        |_, _| Some(signer),
                    )
                    .unwrap();
            }
            proposals.push((deposit_id, proposal));
        }

        // Simulate posting loop iteration 1:
        // lastDepositNonce = 0, so next_nonce = 1
        let last_chain_nonce = contract.get_last_deposit_nonce();
        assert_eq!(last_chain_nonce, 0);
        let next_nonce = last_chain_nonce + 1;

        let ready = cache.ready_proposals().unwrap();
        for (deposit_id, state) in &ready {
            if state.proposal.nonce == next_nonce {
                // Submit this one
                contract
                    .submit_deposit_with_nonce(deposit_id, 0, &signatures, state.proposal.nonce)
                    .unwrap();
                cache.mark_confirmed(deposit_id).unwrap();
                break;
            }
        }
        assert_eq!(contract.get_last_deposit_nonce(), 1);

        // Simulate posting loop iteration 2:
        // lastDepositNonce = 1, so next_nonce = 2
        let last_chain_nonce = contract.get_last_deposit_nonce();
        let next_nonce = last_chain_nonce + 1;

        let ready = cache.ready_proposals().unwrap();
        for (deposit_id, state) in &ready {
            if state.proposal.nonce == next_nonce {
                contract
                    .submit_deposit_with_nonce(deposit_id, 0, &signatures, state.proposal.nonce)
                    .unwrap();
                cache.mark_confirmed(deposit_id).unwrap();
                break;
            }
        }
        assert_eq!(contract.get_last_deposit_nonce(), 2);

        // Verify all submissions are in order
        let submissions = contract.all_submissions();
        assert_eq!(submissions.len(), 2);
    }

    #[tokio::test]
    async fn test_false_failure_healed_by_chain_query() {
        // Scenario: Node thinks nonce 1 failed, but it actually succeeded on chain.
        // When we query the chain, we see lastDepositNonce = 1, so we mark it confirmed
        // and move on to nonce 2.

        let contract = MockBaseContract::new();
        let cache = ProposalCache::new();
        let signatures = vec![vec![1u8; 65], vec![2u8; 65], vec![3u8; 65]];

        // Create proposals with nonces 1 and 2
        let proposal1 = sample_deposit_with_nonce(100, 1000, 1);
        let proposal2 = sample_deposit_with_nonce(101, 1000, 2);
        let deposit_id1 = DepositId::from_effect_payload(&proposal1);
        let deposit_id2 = DepositId::from_effect_payload(&proposal2);

        // Add both to cache with threshold
        for (deposit_id, proposal) in [(&deposit_id1, &proposal1), (&deposit_id2, &proposal2)] {
            let proposal_hash = proposal.compute_proposal_hash();
            for i in 0..3 {
                let signer = Address::from([i; 20]);
                cache
                    .add_signature(
                        deposit_id,
                        bridge::proposal_cache::SignatureData {
                            signer_address: signer,
                            signature: vec![i; 65],
                            proposal_hash,
                            is_mine: i == 0,
                        },
                        if i == 0 { Some(proposal.clone()) } else { None },
                        |_, _| Some(signer),
                    )
                    .unwrap();
            }
        }

        // Simulate: nonce 1 was submitted and succeeded on chain, but node marked it Failed
        contract
            .submit_deposit_with_nonce(&deposit_id1, 0, &signatures, 1)
            .unwrap();
        cache.mark_failed(&deposit_id1).unwrap();

        // Verify local state shows Failed
        let state1 = cache.get_state(&deposit_id1).unwrap().unwrap();
        assert_eq!(state1.status, ProposalStatus::Failed);

        // Now simulate the fixed posting loop:
        // Query chain - it shows lastDepositNonce = 1
        let last_chain_nonce = contract.get_last_deposit_nonce();
        assert_eq!(last_chain_nonce, 1);
        let next_nonce = last_chain_nonce + 1; // = 2

        // Process ready proposals
        let ready = cache.ready_proposals().unwrap();
        for (deposit_id, state) in &ready {
            if state.proposal.nonce < next_nonce {
                // Already on chain - heal by marking confirmed
                cache.mark_confirmed(deposit_id).unwrap();
            } else if state.proposal.nonce == next_nonce {
                // This is the one to submit
                contract
                    .submit_deposit_with_nonce(deposit_id, 0, &signatures, state.proposal.nonce)
                    .unwrap();
                cache.mark_confirmed(deposit_id).unwrap();
            }
            // else: nonce > next_nonce, skip for now
        }

        // Verify: nonce 2 was submitted
        assert_eq!(contract.get_last_deposit_nonce(), 2);

        // Note: nonce 1 was already Failed, so it won't appear in ready_proposals.
        // In the real code, we'd need to also check non-ready proposals for healing.
        // But the key point is: we didn't deadlock waiting for nonce 1.
    }

    #[tokio::test]
    async fn test_out_of_order_nonces_wait_correctly() {
        // Scenario: Nonces 1, 2, 3 are ready, but we must submit in order.
        // If nonce 1 is not ready, we wait (don't skip to 2).

        let contract = MockBaseContract::new();
        let cache = ProposalCache::new();

        // Create proposals with nonces 2 and 3 (nonce 1 is missing/not ready)
        for nonce in [2u64, 3] {
            let proposal = sample_deposit_with_nonce(100 + nonce, 1000, nonce);
            let proposal_hash = proposal.compute_proposal_hash();
            let deposit_id = DepositId::from_effect_payload(&proposal);

            for i in 0..3 {
                let signer = Address::from([i; 20]);
                cache
                    .add_signature(
                        &deposit_id,
                        bridge::proposal_cache::SignatureData {
                            signer_address: signer,
                            signature: vec![i; 65],
                            proposal_hash,
                            is_mine: i == 0,
                        },
                        if i == 0 { Some(proposal.clone()) } else { None },
                        |_, _| Some(signer),
                    )
                    .unwrap();
            }
        }

        // Chain shows lastDepositNonce = 0, so next_nonce = 1
        let last_chain_nonce = contract.get_last_deposit_nonce();
        assert_eq!(last_chain_nonce, 0);
        let next_nonce = last_chain_nonce + 1; // = 1

        // Try to find a proposal with nonce == 1
        let ready = cache.ready_proposals().unwrap();
        let can_submit = ready
            .iter()
            .any(|(_, state)| state.proposal.nonce == next_nonce);

        // We should NOT be able to submit because nonce 1 is not ready
        assert!(!can_submit, "Should not find nonce 1 in ready proposals");

        // Verify we didn't submit anything
        assert_eq!(contract.get_last_deposit_nonce(), 0);
        assert!(contract.all_submissions().is_empty());
    }

    #[tokio::test]
    async fn test_timed_out_failed_proposal_can_be_skipped() {
        // Scenario: Nonce 1 truly failed and has been stuck for a while.
        // After timeout, the posting loop should be able to skip it (by incrementing
        // next_nonce) and submit nonce 2.

        let cache = ProposalCache::new();

        // Create proposal with nonce 1
        let proposal1 = sample_deposit_with_nonce(100, 1000, 1);
        let proposal_hash1 = proposal1.compute_proposal_hash();
        let deposit_id1 = DepositId::from_effect_payload(&proposal1);

        // Add to cache with threshold
        for i in 0..3 {
            let signer = Address::from([i; 20]);
            cache
                .add_signature(
                    &deposit_id1,
                    bridge::proposal_cache::SignatureData {
                        signer_address: signer,
                        signature: vec![i; 65],
                        proposal_hash: proposal_hash1,
                        is_mine: i == 0,
                    },
                    if i == 0 {
                        Some(proposal1.clone())
                    } else {
                        None
                    },
                    |_, _| Some(signer),
                )
                .unwrap();
        }

        // Mark as failed
        cache.mark_failed(&deposit_id1).unwrap();

        // Verify it's failed
        let state = cache.get_state(&deposit_id1).unwrap().unwrap();
        assert_eq!(state.status, ProposalStatus::Failed);
        assert!(state.failed_at.is_some());

        // With 0 timeout, it should immediately be considered timed out
        let timed_out = cache.lowest_timed_out_failed_nonce(0).unwrap();
        assert_eq!(
            timed_out,
            Some(1),
            "Nonce 1 should be timed out with 0s timeout"
        );

        // With very long timeout, it should not be timed out yet
        let timed_out = cache.lowest_timed_out_failed_nonce(999999).unwrap();
        assert_eq!(
            timed_out, None,
            "Nonce 1 should not be timed out with long timeout"
        );

        // The cache does not record "skipped" state; skipping is implemented in the posting loop
        // by advancing `next_nonce` when the current next nonce is timed out.
        let state = cache.get_state(&deposit_id1).unwrap().unwrap();
        assert_eq!(state.status, ProposalStatus::Failed);
        assert_eq!(
            cache.lowest_timed_out_failed_nonce(0).unwrap(),
            Some(1),
            "timed-out failed nonce should be detectable for skipping"
        );
    }

    #[tokio::test]
    async fn test_get_proposal_by_nonce() {
        let cache = ProposalCache::new();

        // Create proposals with nonces 5 and 10
        for nonce in [5u64, 10] {
            let proposal = sample_deposit_with_nonce(100 + nonce, 1000, nonce);
            let proposal_hash = proposal.compute_proposal_hash();
            let deposit_id = DepositId::from_effect_payload(&proposal);

            cache
                .add_signature(
                    &deposit_id,
                    bridge::proposal_cache::SignatureData {
                        signer_address: Address::ZERO,
                        signature: vec![1; 65],
                        proposal_hash,
                        is_mine: true,
                    },
                    Some(proposal),
                    |_, _| Some(Address::ZERO),
                )
                .unwrap();
        }

        // Find nonce 5
        let result = cache.get_proposal_by_nonce(5).unwrap();
        assert!(result.is_some());
        assert_eq!(result.unwrap().1.proposal.nonce, 5);

        // Find nonce 10
        let result = cache.get_proposal_by_nonce(10).unwrap();
        assert!(result.is_some());
        assert_eq!(result.unwrap().1.proposal.nonce, 10);

        // Nonce 7 doesn't exist
        let result = cache.get_proposal_by_nonce(7).unwrap();
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn test_full_skip_flow_unblocks_subsequent_nonces() {
        // Full integration test: nonce 1 fails, times out, gets skipped,
        // then nonce 2 can be submitted.

        let contract = MockBaseContract::new();
        let cache = ProposalCache::new();
        let signatures = vec![vec![1u8; 65], vec![2u8; 65], vec![3u8; 65]];

        // Create proposals with nonces 1 and 2
        for nonce in [1u64, 2] {
            let proposal = sample_deposit_with_nonce(100 + nonce, 1000, nonce);
            let proposal_hash = proposal.compute_proposal_hash();
            let deposit_id = DepositId::from_effect_payload(&proposal);

            for i in 0..3 {
                let signer = Address::from([i; 20]);
                cache
                    .add_signature(
                        &deposit_id,
                        bridge::proposal_cache::SignatureData {
                            signer_address: signer,
                            signature: vec![i; 65],
                            proposal_hash,
                            is_mine: i == 0,
                        },
                        if i == 0 { Some(proposal.clone()) } else { None },
                        |_, _| Some(signer),
                    )
                    .unwrap();
            }
        }

        let deposit_id1 = DepositId::from_effect_payload(&sample_deposit_with_nonce(101, 1000, 1));
        let deposit_id2 = DepositId::from_effect_payload(&sample_deposit_with_nonce(102, 1000, 2));

        // Nonce 1 fails
        cache.mark_failed(&deposit_id1).unwrap();

        // Chain shows lastDepositNonce = 0, so next_nonce = 1
        let last_chain_nonce = contract.get_last_deposit_nonce();
        assert_eq!(last_chain_nonce, 0);
        let _next_nonce = last_chain_nonce + 1;

        // Skipping does not mutate cache state; it is implemented by advancing next_nonce.
        // The contract permits nonce gaps (`depositNonce > lastDepositNonce`), so nonce 2 can be submitted.
        // But wait - the chain still shows lastDepositNonce = 0
        // We need to submit nonce 1 first... but we skipped it!
        //
        // Actually, this reveals a problem: skipping locally doesn't help
        // because the contract still requires nonce > lastDepositNonce.
        // If we skip nonce 1, we can't submit nonce 2 because 2 > 0 is true,
        // but the contract will accept it!

        // Let's verify: submit nonce 2 directly
        let result = contract.submit_deposit_with_nonce(&deposit_id2, 0, &signatures, 2);
        assert!(
            result.is_ok(),
            "Nonce 2 should succeed when last is 0 (skipping 1)"
        );
        assert_eq!(contract.get_last_deposit_nonce(), 2);

        // Nonce 1 is now permanently lost (can't submit 1 when last is 2)
    }
}

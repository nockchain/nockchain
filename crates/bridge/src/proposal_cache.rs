use std::collections::HashMap;
use std::mem::size_of;
use std::sync::{Arc, RwLock};
use std::time::{SystemTime, UNIX_EPOCH};

use alloy::primitives::Address;

use crate::types::{DepositId, NockDepositRequestData};

/// Signature threshold for multisig aggregation (3-of-5).
pub const SIGNATURE_THRESHOLD: usize = 3;

/// Status of a proposal in the cache.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ProposalStatus {
    /// Collecting signatures from peers
    Collecting,
    /// Threshold reached, ready to post on-chain
    Ready,
    /// Currently being posted to Ethereum
    Posting,
    /// Successfully confirmed on-chain
    Confirmed,
    /// Failed to post or invalid
    Failed,
}

/// Result of adding a signature to a proposal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SignatureAddResult {
    /// Signature added successfully
    Added,
    /// Signature already exists from this address
    Duplicate,
    /// Signature verification failed
    Invalid(String),
    /// Threshold reached, proposal is now ready
    ThresholdReached,
    /// Deposit already confirmed on-chain
    Stale,
}

/// State for a single proposal being aggregated.
#[derive(Debug, Clone)]
pub struct ProposalState {
    /// The proposal payload (deposit data)
    pub proposal: NockDepositRequestData,
    /// Keccak256 hash of the proposal for signature verification
    pub proposal_hash: [u8; 32],
    /// Our own signature (if we signed it)
    pub my_signature: Option<Vec<u8>>,
    /// Signatures from peers, keyed by their Ethereum address
    pub peer_signatures: HashMap<Address, Vec<u8>>,
    /// Unix timestamp when this proposal was created
    pub created_at: u64,
    /// Unix timestamp when threshold was reached (for failover timing)
    pub ready_at: Option<u64>,
    /// Unix timestamp when this proposal failed (for skip timeout)
    pub failed_at: Option<u64>,
    /// Current status of the proposal
    pub status: ProposalStatus,
}

impl ProposalState {
    /// Create a new proposal state.
    pub fn new(proposal: NockDepositRequestData) -> Self {
        let proposal_hash = proposal.compute_proposal_hash();
        let created_at = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();

        Self {
            proposal,
            proposal_hash,
            my_signature: None,
            peer_signatures: HashMap::new(),
            created_at,
            ready_at: None,
            failed_at: None,
            status: ProposalStatus::Collecting,
        }
    }

    /// Check if this proposal has reached the signature threshold.
    pub fn has_threshold(&self) -> bool {
        let total_sigs =
            self.peer_signatures.len() + if self.my_signature.is_some() { 1 } else { 0 };
        total_sigs >= SIGNATURE_THRESHOLD
    }

    /// Get all signatures (mine + peers) for posting to Ethereum.
    pub fn all_signatures(&self) -> Vec<Vec<u8>> {
        let mut sigs = Vec::new();
        if let Some(ref my_sig) = self.my_signature {
            sigs.push(my_sig.clone());
        }
        for sig in self.peer_signatures.values() {
            sigs.push(sig.clone());
        }
        sigs
    }
}

/// Data for adding a signature to a proposal.
#[derive(Debug, Clone)]
pub struct SignatureData {
    /// The Ethereum address of the signer
    pub signer_address: Address,
    /// The 65-byte ECDSA signature (r, s, v)
    pub signature: Vec<u8>,
    /// The expected proposal hash (for queuing if deposit unknown)
    pub proposal_hash: [u8; 32],
    /// Whether this is our own signature
    pub is_mine: bool,
}

/// A pending signature waiting for its proposal to be processed.
#[derive(Debug, Clone)]
pub struct PendingSignature {
    pub signer_address: Address,
    pub signature: Vec<u8>,
    pub proposal_hash: [u8; 32],
    pub received_at: u64,
}

#[derive(Debug, Clone)]
pub struct PendingSignatureMismatch {
    pub signer_address: Address,
    pub expected_hash: [u8; 32],
    pub received_hash: [u8; 32],
}

#[derive(Debug, Clone, Default)]
pub struct PendingSignatureReport {
    pub applied: usize,
    pub mismatched: Vec<PendingSignatureMismatch>,
}

#[derive(Debug, Clone, Default)]
pub struct ProposalCacheMetricsSnapshot {
    pub proposal_total: usize,
    pub collecting: usize,
    pub ready: usize,
    pub posting: usize,
    pub confirmed: usize,
    pub failed: usize,
    pub total_peer_signatures: usize,
    pub max_peer_signatures_per_proposal: usize,
    pub proposals_with_my_signature: usize,
    pub pending_signature_deposit_count: usize,
    pub pending_signature_total: usize,
    pub oldest_age_secs: u64,
    pub oldest_confirmed_age_secs: u64,
    pub oldest_failed_age_secs: u64,
    pub pending_oldest_age_secs: u64,
    pub approx_state_bytes: usize,
    pub approx_peer_signature_bytes: usize,
    pub approx_my_signature_bytes: usize,
    pub approx_pending_signature_bytes: usize,
    pub approx_total_bytes: usize,
}

/// Thread-safe cache for aggregating signatures on proposals.
///
/// This cache stores proposals keyed by DepositId and tracks signatures from
/// multiple bridge nodes until the threshold is reached.
///
/// Signatures that arrive before we've processed the deposit ourselves are
/// queued in `pending_signatures` and applied when we create the proposal.
#[derive(Debug, Clone)]
pub struct ProposalCache {
    inner: Arc<RwLock<HashMap<DepositId, ProposalState>>>,
    /// Signatures received before we processed the deposit ourselves
    pending_signatures: Arc<RwLock<HashMap<DepositId, Vec<PendingSignature>>>>,
}

impl Default for ProposalCache {
    fn default() -> Self {
        Self::new()
    }
}

impl ProposalCache {
    /// Create a new empty proposal cache.
    pub fn new() -> Self {
        Self {
            inner: Arc::new(RwLock::new(HashMap::new())),
            pending_signatures: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Add a signature for a proposal.
    ///
    /// # Arguments
    /// * `deposit_id` - The unique identifier for the deposit/proposal
    /// * `sig_data` - The signature data (signer address, signature bytes, hash, ownership)
    /// * `proposal` - The proposal data (needed if this is the first signature)
    /// * `verify_fn` - Function to verify signature validity
    ///
    /// # Returns
    /// Result indicating whether the signature was added and if threshold was reached.
    pub fn add_signature<F>(
        &self,
        deposit_id: &DepositId,
        sig_data: SignatureData,
        proposal: Option<NockDepositRequestData>,
        verify_fn: F,
    ) -> Result<SignatureAddResult, String>
    where
        F: FnOnce(&[u8; 32], &[u8]) -> Option<Address>,
    {
        let metrics = crate::metrics::init_metrics();
        // If we have proposal data, pre-emptively evict any existing proposal that uses
        // the same nonce but a different proposal_hash. This prevents stale entries from
        // blocking the fresh proposal at that nonce.
        let mut cache = match self.inner.write() {
            Ok(guard) => guard,
            Err(e) => {
                return Err(format!("Lock poisoned: {}", e));
            }
        };

        if let Some(ref proposal) = proposal {
            let incoming_nonce = proposal.nonce;
            let incoming_hash = proposal.compute_proposal_hash();

            cache.retain(|existing_id, state| {
                let same_nonce = state.proposal.nonce == incoming_nonce;
                let different_hash = state.proposal_hash != incoming_hash;
                if same_nonce && different_hash {
                    tracing::warn!(
                        target: "bridge.cache",
                        existing_deposit_id = %hex::encode(existing_id.to_bytes()),
                        existing_nonce = state.proposal.nonce,
                        existing_hash = %hex::encode(state.proposal_hash),
                        incoming_nonce = incoming_nonce,
                        incoming_hash = %hex::encode(incoming_hash),
                        "evicting stale proposal at nonce in favor of incoming proposal"
                    );
                    false
                } else {
                    true
                }
            });
        }

        // Get existing entry or create new one if we have proposal data
        let state = match cache.entry(deposit_id.clone()) {
            std::collections::hash_map::Entry::Occupied(entry) => entry.into_mut(),
            std::collections::hash_map::Entry::Vacant(entry) => {
                // For new entries, we need proposal data
                let Some(proposal) = proposal else {
                    // Peer sent signature for deposit we haven't processed yet
                    // Queue it for later application
                    drop(cache); // Release cache lock before acquiring pending lock
                    let mut pending = self
                        .pending_signatures
                        .write()
                        .map_err(|e| format!("Pending lock poisoned: {}", e))?;

                    let received_at = SystemTime::now()
                        .duration_since(UNIX_EPOCH)
                        .unwrap_or_default()
                        .as_secs();

                    let pending_sig = PendingSignature {
                        signer_address: sig_data.signer_address,
                        signature: sig_data.signature,
                        proposal_hash: sig_data.proposal_hash,
                        received_at,
                    };

                    pending
                        .entry(deposit_id.clone())
                        .or_insert_with(Vec::new)
                        .push(pending_sig);
                    metrics
                        .proposal_cache_pending_signature_queued_unknown_deposit
                        .increment();

                    tracing::info!(
                        target: "bridge.cache",
                        deposit_id = %hex::encode(deposit_id.to_bytes()),
                        signer = %sig_data.signer_address,
                        "Queued signature for unknown deposit - will apply when processed"
                    );
                    return Ok(SignatureAddResult::Added);
                };
                entry.insert(ProposalState::new(proposal))
            }
        };

        // Early reject if already confirmed on-chain
        if state.status == ProposalStatus::Confirmed {
            return Ok(SignatureAddResult::Stale);
        }

        // Verify signature recovers to expected address
        let recovered = match verify_fn(&state.proposal_hash, &sig_data.signature) {
            Some(address) => address,
            None => {
                metrics.proposal_cache_signature_verify_failed.increment();
                return Err("Signature verification failed".to_string());
            }
        };

        if recovered != sig_data.signer_address {
            metrics.proposal_cache_signature_address_mismatch.increment();
            return Err(format!(
                "Signature address mismatch: expected {}, recovered {}",
                sig_data.signer_address, recovered
            ));
        }

        // Check for duplicate
        if sig_data.is_mine && state.my_signature.is_some() {
            metrics.proposal_cache_signature_duplicate.increment();
            return Ok(SignatureAddResult::Duplicate);
        }
        if !sig_data.is_mine && state.peer_signatures.contains_key(&sig_data.signer_address) {
            metrics.proposal_cache_signature_duplicate.increment();
            return Ok(SignatureAddResult::Duplicate);
        }

        // Add signature
        if sig_data.is_mine {
            state.my_signature = Some(sig_data.signature);
        } else {
            state
                .peer_signatures
                .insert(sig_data.signer_address, sig_data.signature);
        }

        // Check if threshold reached
        if state.has_threshold() && state.status == ProposalStatus::Collecting {
            state.status = ProposalStatus::Ready;
            state.ready_at = Some(
                SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_secs(),
            );
            Ok(SignatureAddResult::ThresholdReached)
        } else {
            Ok(SignatureAddResult::Added)
        }
    }

    /// Apply any pending signatures that were received before we processed the deposit.
    ///
    /// Call this after creating a new proposal entry to apply signatures that
    /// arrived before we processed the deposit ourselves.
    pub fn apply_pending_signatures<F>(
        &self,
        deposit_id: &DepositId,
        verify_fn: F,
    ) -> Result<PendingSignatureReport, String>
    where
        F: Fn(&[u8; 32], &[u8]) -> Option<Address>,
    {
        let metrics = crate::metrics::init_metrics();
        // First, take any pending signatures for this deposit
        let pending_sigs = {
            let mut pending = self
                .pending_signatures
                .write()
                .map_err(|e| format!("Pending lock poisoned: {}", e))?;
            pending.remove(deposit_id).unwrap_or_default()
        };

        if pending_sigs.is_empty() {
            return Ok(PendingSignatureReport::default());
        }

        let mut cache = self
            .inner
            .write()
            .map_err(|e| format!("Lock poisoned: {}", e))?;

        let Some(state) = cache.get_mut(deposit_id) else {
            return Err("Deposit not found in cache".to_string());
        };

        let mut report = PendingSignatureReport::default();
        for pending in pending_sigs {
            // Verify proposal hash matches
            if pending.proposal_hash != state.proposal_hash {
                tracing::warn!(
                    target: "bridge.cache",
                    deposit_id = %hex::encode(deposit_id.to_bytes()),
                    signer = %pending.signer_address,
                    expected_hash = %hex::encode(state.proposal_hash),
                    received_hash = %hex::encode(pending.proposal_hash),
                    "Pending signature has wrong proposal hash - discarding"
                );
                report.mismatched.push(PendingSignatureMismatch {
                    signer_address: pending.signer_address,
                    expected_hash: state.proposal_hash,
                    received_hash: pending.proposal_hash,
                });
                metrics.proposal_cache_pending_signature_mismatched.increment();
                continue;
            }

            // Skip if already have signature from this address
            if state.peer_signatures.contains_key(&pending.signer_address) {
                continue;
            }

            // Verify signature
            let Some(recovered) = verify_fn(&state.proposal_hash, &pending.signature) else {
                tracing::warn!(
                    target: "bridge.cache",
                    deposit_id = %hex::encode(deposit_id.to_bytes()),
                    signer = %pending.signer_address,
                    "Pending signature verification failed - discarding"
                );
                metrics.proposal_cache_pending_signature_verify_failed.increment();
                continue;
            };

            if recovered != pending.signer_address {
                tracing::warn!(
                    target: "bridge.cache",
                    deposit_id = %hex::encode(deposit_id.to_bytes()),
                    expected = %pending.signer_address,
                    recovered = %recovered,
                    "Pending signature address mismatch - discarding"
                );
                metrics
                    .proposal_cache_pending_signature_address_mismatch
                    .increment();
                continue;
            }

            // Add the signature
            state
                .peer_signatures
                .insert(pending.signer_address, pending.signature);
            report.applied += 1;
            metrics.proposal_cache_pending_signature_applied.increment();

            tracing::info!(
                target: "bridge.cache",
                deposit_id = %hex::encode(deposit_id.to_bytes()),
                signer = %pending.signer_address,
                "Applied pending signature"
            );
        }

        // Check if threshold reached after applying pending signatures
        if report.applied > 0 && state.has_threshold() && state.status == ProposalStatus::Collecting
        {
            state.status = ProposalStatus::Ready;
            state.ready_at = Some(
                SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_secs(),
            );
            tracing::info!(
                target: "bridge.cache",
                deposit_id = %hex::encode(deposit_id.to_bytes()),
                "Threshold reached after applying pending signatures"
            );
        }

        Ok(report)
    }

    /// Check if a deposit is already known in the cache.
    pub fn is_known(&self, deposit_id: &DepositId) -> bool {
        if let Ok(cache) = self.inner.read() {
            cache.contains_key(deposit_id)
        } else {
            false
        }
    }

    /// Check if a deposit is already confirmed on-chain.
    pub fn is_confirmed(&self, deposit_id: &DepositId) -> bool {
        if let Ok(cache) = self.inner.read() {
            cache
                .get(deposit_id)
                .map(|state| state.status == ProposalStatus::Confirmed)
                .unwrap_or(false)
        } else {
            false
        }
    }

    /// Check if a proposal is ready to be posted (threshold reached).
    pub fn is_ready(&self, deposit_id: &DepositId) -> Result<bool, String> {
        let cache = self
            .inner
            .read()
            .map_err(|e| format!("Lock poisoned: {}", e))?;
        Ok(cache
            .get(deposit_id)
            .map(|state| state.status == ProposalStatus::Ready)
            .unwrap_or(false))
    }

    /// Get all signatures for a proposal that's ready to post.
    pub fn get_signatures_for_posting(
        &self,
        deposit_id: &DepositId,
    ) -> Result<Option<Vec<Vec<u8>>>, String> {
        let cache = self
            .inner
            .read()
            .map_err(|e| format!("Lock poisoned: {}", e))?;
        Ok(cache.get(deposit_id).and_then(|state| {
            if state.status == ProposalStatus::Ready && state.has_threshold() {
                Some(state.all_signatures())
            } else {
                None
            }
        }))
    }

    /// Mark a proposal as currently being posted to Ethereum.
    pub fn mark_posting(&self, deposit_id: &DepositId) -> Result<(), String> {
        let mut cache = self
            .inner
            .write()
            .map_err(|e| format!("Lock poisoned: {}", e))?;
        if let Some(state) = cache.get_mut(deposit_id) {
            state.status = ProposalStatus::Posting;
        }
        Ok(())
    }

    /// Mark a proposal as confirmed on-chain.
    pub fn mark_confirmed(&self, deposit_id: &DepositId) -> Result<(), String> {
        let mut cache = self
            .inner
            .write()
            .map_err(|e| format!("Lock poisoned: {}", e))?;
        if let Some(state) = cache.get_mut(deposit_id) {
            state.status = ProposalStatus::Confirmed;
        }
        Ok(())
    }

    /// Mark a proposal as failed.
    pub fn mark_failed(&self, deposit_id: &DepositId) -> Result<(), String> {
        let mut cache = self
            .inner
            .write()
            .map_err(|e| format!("Lock poisoned: {}", e))?;
        if let Some(state) = cache.get_mut(deposit_id) {
            state.status = ProposalStatus::Failed;
            state.failed_at = Some(
                SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_secs(),
            );
        }
        Ok(())
    }

    /// Garbage collect old confirmed or failed proposals.
    ///
    /// # Arguments
    /// * `max_age_secs` - Remove proposals older than this many seconds
    pub fn gc(&self, max_age_secs: u64) -> Result<usize, String> {
        let metrics = crate::metrics::init_metrics();
        metrics.proposal_cache_gc_runs.increment();
        let mut cache = self
            .inner
            .write()
            .map_err(|e| format!("Lock poisoned: {}", e))?;
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();

        let mut removed = 0;
        cache.retain(|_, state| {
            let age = now.saturating_sub(state.created_at);
            let should_remove = age >= max_age_secs
                && (state.status == ProposalStatus::Confirmed
                    || state.status == ProposalStatus::Failed);
            if should_remove {
                removed += 1;
            }
            !should_remove
        });

        metrics.proposal_cache_gc_last_removed.swap(removed as f64);
        Ok(removed)
    }

    /// Get the current state of a proposal (for debugging/monitoring).
    pub fn get_state(&self, deposit_id: &DepositId) -> Result<Option<ProposalState>, String> {
        let cache = self
            .inner
            .read()
            .map_err(|e| format!("Lock poisoned: {}", e))?;
        Ok(cache.get(deposit_id).cloned())
    }

    /// Get count of proposals in each status.
    pub fn status_counts(&self) -> Result<HashMap<ProposalStatus, usize>, String> {
        let cache = self
            .inner
            .read()
            .map_err(|e| format!("Lock poisoned: {}", e))?;
        let mut counts = HashMap::new();
        for state in cache.values() {
            *counts.entry(state.status).or_insert(0) += 1;
        }
        Ok(counts)
    }

    /// Build a point-in-time snapshot of proposal-cache memory pressure indicators.
    pub fn metrics_snapshot(&self) -> Result<ProposalCacheMetricsSnapshot, String> {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        let mut snapshot = ProposalCacheMetricsSnapshot::default();

        {
            let cache = self
                .inner
                .read()
                .map_err(|e| format!("Lock poisoned: {}", e))?;
            snapshot.proposal_total = cache.len();

            for state in cache.values() {
                let age = now.saturating_sub(state.created_at);
                snapshot.oldest_age_secs = snapshot.oldest_age_secs.max(age);
                snapshot.approx_state_bytes = snapshot
                    .approx_state_bytes
                    .saturating_add(size_of::<DepositId>() + size_of::<ProposalState>());

                match state.status {
                    ProposalStatus::Collecting => {
                        snapshot.collecting = snapshot.collecting.saturating_add(1)
                    }
                    ProposalStatus::Ready => snapshot.ready = snapshot.ready.saturating_add(1),
                    ProposalStatus::Posting => {
                        snapshot.posting = snapshot.posting.saturating_add(1)
                    }
                    ProposalStatus::Confirmed => {
                        snapshot.confirmed = snapshot.confirmed.saturating_add(1);
                        snapshot.oldest_confirmed_age_secs =
                            snapshot.oldest_confirmed_age_secs.max(age);
                    }
                    ProposalStatus::Failed => {
                        snapshot.failed = snapshot.failed.saturating_add(1);
                        let failed_age = state
                            .failed_at
                            .map(|failed_at| now.saturating_sub(failed_at))
                            .unwrap_or(age);
                        snapshot.oldest_failed_age_secs =
                            snapshot.oldest_failed_age_secs.max(failed_age);
                    }
                }

                if let Some(sig) = &state.my_signature {
                    snapshot.proposals_with_my_signature =
                        snapshot.proposals_with_my_signature.saturating_add(1);
                    snapshot.approx_my_signature_bytes = snapshot
                        .approx_my_signature_bytes
                        .saturating_add(size_of::<Vec<u8>>() + sig.len());
                }

                let peer_count = state.peer_signatures.len();
                snapshot.total_peer_signatures =
                    snapshot.total_peer_signatures.saturating_add(peer_count);
                snapshot.max_peer_signatures_per_proposal =
                    snapshot.max_peer_signatures_per_proposal.max(peer_count);

                for sig in state.peer_signatures.values() {
                    snapshot.approx_peer_signature_bytes =
                        snapshot.approx_peer_signature_bytes.saturating_add(
                            size_of::<Address>() + size_of::<Vec<u8>>() + sig.len(),
                        );
                }
            }
        }

        {
            let pending = self
                .pending_signatures
                .read()
                .map_err(|e| format!("Pending lock poisoned: {}", e))?;
            snapshot.pending_signature_deposit_count = pending.len();

            for pending_sigs in pending.values() {
                snapshot.pending_signature_total = snapshot
                    .pending_signature_total
                    .saturating_add(pending_sigs.len());
                for pending_sig in pending_sigs {
                    snapshot.pending_oldest_age_secs = snapshot
                        .pending_oldest_age_secs
                        .max(now.saturating_sub(pending_sig.received_at));
                    snapshot.approx_pending_signature_bytes = snapshot
                        .approx_pending_signature_bytes
                        .saturating_add(size_of::<PendingSignature>() + pending_sig.signature.len());
                }
            }
        }

        snapshot.approx_total_bytes = snapshot
            .approx_state_bytes
            .saturating_add(snapshot.approx_peer_signature_bytes)
            .saturating_add(snapshot.approx_my_signature_bytes)
            .saturating_add(snapshot.approx_pending_signature_bytes);

        Ok(snapshot)
    }

    /// Collect proposals that are still collecting signatures but already have our signature.
    ///
    /// Used to re-gossip our own signature to late/briefly-offline peers.
    pub fn collecting_with_my_sig(&self) -> Result<Vec<(DepositId, ProposalState)>, String> {
        let cache = self
            .inner
            .read()
            .map_err(|e| format!("Lock poisoned: {}", e))?;

        Ok(cache
            .iter()
            .filter(|(_, state)| {
                state.status == ProposalStatus::Collecting && state.my_signature.is_some()
            })
            .map(|(id, state)| (id.clone(), state.clone()))
            .collect())
    }

    /// Get all proposals that are ready to be posted (threshold reached, status=Ready).
    /// Returns iterator of (DepositId, ProposalState) pairs.
    pub fn ready_proposals(&self) -> Result<Vec<(DepositId, ProposalState)>, String> {
        let cache = self
            .inner
            .read()
            .map_err(|e| format!("Lock poisoned: {}", e))?;
        let mut ready: Vec<(DepositId, ProposalState)> = cache
            .iter()
            .filter(|(_, state)| state.status == ProposalStatus::Ready && state.has_threshold())
            .map(|(id, state)| (id.clone(), state.clone()))
            .collect();

        // Ensure deterministic posting order: lowest nonce first.
        ready.sort_by_key(|(_, state)| state.proposal.nonce);

        Ok(ready)
    }

    /// Get the lowest nonce among proposals that have been Failed for longer than the timeout.
    ///
    /// These proposals should be skipped - they've been stuck too long and are blocking
    /// subsequent nonces. Returns None if no proposals have timed out.
    pub fn lowest_timed_out_failed_nonce(&self, timeout_secs: u64) -> Result<Option<u64>, String> {
        let cache = self
            .inner
            .read()
            .map_err(|e| format!("Lock poisoned: {}", e))?;

        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();

        Ok(cache
            .values()
            .filter(|state| {
                state.status == ProposalStatus::Failed
                    && state
                        .failed_at
                        .map(|t| now.saturating_sub(t) >= timeout_secs)
                        .unwrap_or(false)
            })
            .map(|state| state.proposal.nonce)
            .min())
    }

    /// Get the proposal with a specific nonce (for skip handling).
    pub fn get_proposal_by_nonce(
        &self,
        nonce: u64,
    ) -> Result<Option<(DepositId, ProposalState)>, String> {
        let cache = self
            .inner
            .read()
            .map_err(|e| format!("Lock poisoned: {}", e))?;

        Ok(cache
            .iter()
            .find(|(_, state)| state.proposal.nonce == nonce)
            .map(|(id, state)| (id.clone(), state.clone())))
    }

    /// Check if a specific node has signed this deposit.
    ///
    /// Used for health-aware failover: if the proposer hasn't even signed yet,
    /// they're definitely not ready to post, so we can accelerate failover.
    ///
    /// # Arguments
    /// * `deposit_id` - The deposit to check
    /// * `signer_address` - The Ethereum address to check for
    ///
    /// # Returns
    /// `true` if this address has signed the deposit (either as us or a peer)
    pub fn has_signature(&self, deposit_id: &DepositId, signer_address: Address) -> bool {
        let Ok(cache) = self.inner.read() else {
            return false;
        };

        cache
            .get(deposit_id)
            .map(|state| {
                // Check if it's our signature and the address matches
                // Note: We can't directly verify our address without eth_key context,
                // so we check peer signatures only for now
                state.peer_signatures.contains_key(&signer_address)
            })
            .unwrap_or(false)
    }
}

#[cfg(test)]
mod tests {
    use nockchain_math::belt::Belt;
    use nockchain_types::tx_engine::common::Hash as Tip5Hash;
    use nockchain_types::v1::Name;

    use super::*;
    use crate::types::{zero_tip5_hash, EthAddress};

    fn test_proposal() -> NockDepositRequestData {
        NockDepositRequestData {
            tx_id: Tip5Hash([Belt(1), Belt(2), Belt(3), Belt(4), Belt(5)]),
            name: Name::new(zero_tip5_hash(), zero_tip5_hash()),
            recipient: EthAddress::ZERO,
            amount: 1000,
            block_height: 100,
            as_of: zero_tip5_hash(),
            nonce: 1,
        }
    }

    fn mock_verify(_hash: &[u8; 32], _sig: &[u8]) -> Option<Address> {
        Some(Address::ZERO)
    }

    #[test]
    fn test_proposal_state_new() {
        let proposal = test_proposal();
        let state = ProposalState::new(proposal.clone());
        assert_eq!(state.status, ProposalStatus::Collecting);
        assert!(state.my_signature.is_none());
        assert_eq!(state.peer_signatures.len(), 0);
        assert!(!state.has_threshold());
    }

    #[test]
    fn test_proposal_state_threshold() {
        let proposal = test_proposal();
        let mut state = ProposalState::new(proposal);

        // Add own signature
        state.my_signature = Some(vec![1, 2, 3]);
        assert!(!state.has_threshold());

        // Add two peer signatures
        state
            .peer_signatures
            .insert(Address::from([1u8; 20]), vec![4, 5, 6]);
        state
            .peer_signatures
            .insert(Address::from([2u8; 20]), vec![7, 8, 9]);

        // Now should have threshold (3 total)
        assert!(state.has_threshold());
    }

    #[test]
    fn test_cache_add_signature() {
        let cache = ProposalCache::new();
        let proposal = test_proposal();
        let proposal_hash = proposal.compute_proposal_hash();
        let deposit_id = DepositId::from_effect_payload(&proposal);

        // Add first signature (ours)
        let result = cache
            .add_signature(
                &deposit_id,
                SignatureData {
                    signer_address: Address::ZERO,
                    signature: vec![1, 2, 3],
                    proposal_hash,
                    is_mine: true,
                },
                Some(proposal.clone()),
                mock_verify,
            )
            .unwrap();
        assert_eq!(result, SignatureAddResult::Added);

        // Add second signature
        let result = cache
            .add_signature(
                &deposit_id,
                SignatureData {
                    signer_address: Address::from([1u8; 20]),
                    signature: vec![4, 5, 6],
                    proposal_hash,
                    is_mine: false,
                },
                None,
                |_, _| Some(Address::from([1u8; 20])),
            )
            .unwrap();
        assert_eq!(result, SignatureAddResult::Added);

        // Add third signature - should reach threshold
        let result = cache
            .add_signature(
                &deposit_id,
                SignatureData {
                    signer_address: Address::from([2u8; 20]),
                    signature: vec![7, 8, 9],
                    proposal_hash,
                    is_mine: false,
                },
                None,
                |_, _| Some(Address::from([2u8; 20])),
            )
            .unwrap();
        assert_eq!(result, SignatureAddResult::ThresholdReached);

        // Check ready status
        assert!(cache.is_ready(&deposit_id).unwrap());
    }

    #[test]
    fn test_cache_duplicate_signature() {
        let cache = ProposalCache::new();
        let proposal = test_proposal();
        let proposal_hash = proposal.compute_proposal_hash();
        let deposit_id = DepositId::from_effect_payload(&proposal);

        // Add first signature
        cache
            .add_signature(
                &deposit_id,
                SignatureData {
                    signer_address: Address::ZERO,
                    signature: vec![1, 2, 3],
                    proposal_hash,
                    is_mine: true,
                },
                Some(proposal),
                mock_verify,
            )
            .unwrap();

        // Try to add same signature again
        let result = cache
            .add_signature(
                &deposit_id,
                SignatureData {
                    signer_address: Address::ZERO,
                    signature: vec![1, 2, 3],
                    proposal_hash,
                    is_mine: true,
                },
                None,
                mock_verify,
            )
            .unwrap();
        assert_eq!(result, SignatureAddResult::Duplicate);
    }

    #[test]
    fn test_cache_get_signatures_for_posting() {
        let cache = ProposalCache::new();
        let proposal = test_proposal();
        let proposal_hash = proposal.compute_proposal_hash();
        let deposit_id = DepositId::from_effect_payload(&proposal);

        // Add enough signatures to reach threshold
        cache
            .add_signature(
                &deposit_id,
                SignatureData {
                    signer_address: Address::ZERO,
                    signature: vec![1, 2, 3],
                    proposal_hash,
                    is_mine: true,
                },
                Some(proposal),
                mock_verify,
            )
            .unwrap();
        cache
            .add_signature(
                &deposit_id,
                SignatureData {
                    signer_address: Address::from([1u8; 20]),
                    signature: vec![4, 5, 6],
                    proposal_hash,
                    is_mine: false,
                },
                None,
                |_, _| Some(Address::from([1u8; 20])),
            )
            .unwrap();
        cache
            .add_signature(
                &deposit_id,
                SignatureData {
                    signer_address: Address::from([2u8; 20]),
                    signature: vec![7, 8, 9],
                    proposal_hash,
                    is_mine: false,
                },
                None,
                |_, _| Some(Address::from([2u8; 20])),
            )
            .unwrap();

        // Get signatures
        let sigs = cache.get_signatures_for_posting(&deposit_id).unwrap();
        assert!(sigs.is_some());
        assert_eq!(sigs.unwrap().len(), 3);
    }

    #[test]
    fn test_cache_status_transitions() {
        let cache = ProposalCache::new();
        let proposal = test_proposal();
        let proposal_hash = proposal.compute_proposal_hash();
        let deposit_id = DepositId::from_effect_payload(&proposal);

        // Add proposal
        cache
            .add_signature(
                &deposit_id,
                SignatureData {
                    signer_address: Address::ZERO,
                    signature: vec![1, 2, 3],
                    proposal_hash,
                    is_mine: true,
                },
                Some(proposal),
                mock_verify,
            )
            .unwrap();

        // Mark posting
        cache.mark_posting(&deposit_id).unwrap();
        let state = cache.get_state(&deposit_id).unwrap().unwrap();
        assert_eq!(state.status, ProposalStatus::Posting);

        // Mark confirmed
        cache.mark_confirmed(&deposit_id).unwrap();
        let state = cache.get_state(&deposit_id).unwrap().unwrap();
        assert_eq!(state.status, ProposalStatus::Confirmed);
    }

    #[test]
    fn test_cache_gc() {
        let cache = ProposalCache::new();
        let proposal = test_proposal();
        let proposal_hash = proposal.compute_proposal_hash();
        let deposit_id = DepositId::from_effect_payload(&proposal);

        // Add and confirm a proposal
        cache
            .add_signature(
                &deposit_id,
                SignatureData {
                    signer_address: Address::ZERO,
                    signature: vec![1, 2, 3],
                    proposal_hash,
                    is_mine: true,
                },
                Some(proposal),
                mock_verify,
            )
            .unwrap();
        cache.mark_confirmed(&deposit_id).unwrap();

        // GC with 0 max age should remove it
        let removed = cache.gc(0).unwrap();
        assert_eq!(removed, 1);
        assert!(cache.get_state(&deposit_id).unwrap().is_none());
    }

    #[test]
    fn test_cache_status_counts() {
        let cache = ProposalCache::new();

        // Add multiple proposals in different states
        // Each needs a unique nonce to avoid eviction (same nonce + different hash triggers eviction)
        for i in 0..3 {
            let mut proposal = test_proposal();
            proposal.as_of = Tip5Hash([Belt(i as u64); 5]);
            proposal.nonce = i as u64 + 1;
            let proposal_hash = proposal.compute_proposal_hash();
            let deposit_id = DepositId::from_effect_payload(&proposal);
            cache
                .add_signature(
                    &deposit_id,
                    SignatureData {
                        signer_address: Address::ZERO,
                        signature: vec![1, 2, 3],
                        proposal_hash,
                        is_mine: true,
                    },
                    Some(proposal),
                    mock_verify,
                )
                .unwrap();
        }

        let counts = cache.status_counts().unwrap();
        assert_eq!(*counts.get(&ProposalStatus::Collecting).unwrap(), 3);
    }

    #[test]
    fn test_has_signature() {
        let cache = ProposalCache::new();
        let proposal = test_proposal();
        let proposal_hash = proposal.compute_proposal_hash();
        let deposit_id = DepositId::from_effect_payload(&proposal);

        let addr1 = Address::from([1u8; 20]);
        let addr2 = Address::from([2u8; 20]);

        // No signatures yet
        assert!(!cache.has_signature(&deposit_id, addr1));
        assert!(!cache.has_signature(&deposit_id, addr2));

        // Add signature from addr1
        cache
            .add_signature(
                &deposit_id,
                SignatureData {
                    signer_address: addr1,
                    signature: vec![1, 2, 3],
                    proposal_hash,
                    is_mine: false,
                },
                Some(proposal),
                |_, _| Some(addr1),
            )
            .unwrap();

        // Now addr1 should be present
        assert!(cache.has_signature(&deposit_id, addr1));
        assert!(!cache.has_signature(&deposit_id, addr2));

        // Add signature from addr2
        cache
            .add_signature(
                &deposit_id,
                SignatureData {
                    signer_address: addr2,
                    signature: vec![4, 5, 6],
                    proposal_hash,
                    is_mine: false,
                },
                None,
                |_, _| Some(addr2),
            )
            .unwrap();

        // Both should be present
        assert!(cache.has_signature(&deposit_id, addr1));
        assert!(cache.has_signature(&deposit_id, addr2));
    }

    #[test]
    fn test_is_known() {
        let cache = ProposalCache::new();
        let proposal = test_proposal();
        let proposal_hash = proposal.compute_proposal_hash();
        let deposit_id = DepositId::from_effect_payload(&proposal);

        // Not known initially
        assert!(!cache.is_known(&deposit_id));

        // Add signature
        cache
            .add_signature(
                &deposit_id,
                SignatureData {
                    signer_address: Address::ZERO,
                    signature: vec![1, 2, 3],
                    proposal_hash,
                    is_mine: true,
                },
                Some(proposal),
                mock_verify,
            )
            .unwrap();

        // Now known
        assert!(cache.is_known(&deposit_id));
    }

    #[test]
    fn test_is_confirmed() {
        let cache = ProposalCache::new();
        let proposal = test_proposal();
        let proposal_hash = proposal.compute_proposal_hash();
        let deposit_id = DepositId::from_effect_payload(&proposal);

        // Not confirmed initially
        assert!(!cache.is_confirmed(&deposit_id));

        // Add and confirm
        cache
            .add_signature(
                &deposit_id,
                SignatureData {
                    signer_address: Address::ZERO,
                    signature: vec![1, 2, 3],
                    proposal_hash,
                    is_mine: true,
                },
                Some(proposal),
                mock_verify,
            )
            .unwrap();
        cache.mark_confirmed(&deposit_id).unwrap();

        // Now confirmed
        assert!(cache.is_confirmed(&deposit_id));
    }

    #[test]
    fn test_reject_signature_when_confirmed() {
        let cache = ProposalCache::new();
        let proposal = test_proposal();
        let proposal_hash = proposal.compute_proposal_hash();
        let deposit_id = DepositId::from_effect_payload(&proposal);

        // Add first signature
        cache
            .add_signature(
                &deposit_id,
                SignatureData {
                    signer_address: Address::ZERO,
                    signature: vec![1, 2, 3],
                    proposal_hash,
                    is_mine: true,
                },
                Some(proposal),
                mock_verify,
            )
            .unwrap();

        // Mark as confirmed
        cache.mark_confirmed(&deposit_id).unwrap();

        // Try to add another signature - should be rejected
        let result = cache
            .add_signature(
                &deposit_id,
                SignatureData {
                    signer_address: Address::from([1u8; 20]),
                    signature: vec![4, 5, 6],
                    proposal_hash,
                    is_mine: false,
                },
                None,
                |_, _| Some(Address::from([1u8; 20])),
            )
            .unwrap();

        assert_eq!(result, SignatureAddResult::Stale);
    }
}

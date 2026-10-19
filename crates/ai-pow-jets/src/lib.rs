//! Consensus verifier jet for the AI-PoW (`%ai-pow`) puzzle — Branch (b): a full
//! Rust verify jet with a stubbed Hoon arm.
//!
//! nockchain's existing consensus verify (`check-pow` → `verify:nv`) is a Hoon
//! STARK verifier with jetted primitives. AI-PoW's compact **recursive**-STARK
//! verify is Rust-only, so — per the chosen architecture — the Hoon arm
//! `++ai-pow-verify` is a stub and this jet is the real implementation.
//!
//! **Transparency:** the jet's sample carries a kernel-selected rule tag, the
//! structured `[%ai-pow nonce certificate]` artifact, the structured block
//! commitment, and a target atom; the result is a loobean. Only the opaque `nonce` (the
//! Pearl statement bytes) and the recursive certificate body are byte-atoms —
//! everything Hoon reasons about stays inspectable.
//!
//! **Soundness:** the matrices are miner-chosen (arbitrary model, Pearl
//! parity — no synthetic-matrix pin). The no-grind binding is the
//! block-committed `H_A`/`H_B`: the compact node verifiers (dense + MoE)
//! prove the opened tile in-circuit against those committed roots under the
//! canonical-program pin, and the verifier never needs the model itself.
//! The trusted compact verifier setup (`context` + `verifier_key_digest`) is
//! deterministic from the production params and **proof-independent**
//! (validated in `ai-pow-miner`), so it is built once at boot and injected
//! via [`init_ai_pow_verifier_setup`].

#![cfg_attr(test, allow(clippy::unwrap_used))]
// The consensus verifier requires unwinding so `catch_unwind` can convert decode
// and recursion-verifier panics into deterministic invalid-block `NO` results.
// `panic = "abort"` does not support this rejection contract.
#[cfg(panic = "abort")]
compile_error!(
    "ai-pow-jets requires panic=unwind: the consensus verify jet relies on \
     catch_unwind to turn crafted-block panics into deterministic invalid-block \
     rejections; under panic=abort a crafted %ai-pow block would crash the node."
);

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

#[cfg(test)]
use ai_pow_miner::certificate_noun::decode_ai_pow_pearl_merge_artifact_noun;
use ai_pow_miner::certificate_noun::{
    ai_pow_compact_recursive_certificate_from_node,
    decode_ai_pow_pearl_merge_artifact_noun_with_rules, AiPowBlockVerifyOutcome,
    CertificateNounLimits, PearlMergeAiPowArtifactShape,
};

// Tests use jemalloc (the production allocator) so RSS-reclaim probes measure the real
// behavior — the system allocator retains freed page-outs and would give misleading
// numbers. Matches the required `#[global_allocator]` in the nockchain binary.
#[cfg(test)]
#[global_allocator]
static TEST_ALLOC: tikv_jemallocator::Jemalloc = tikv_jemallocator::Jemalloc;

use ai_pow_zk::proof_rules::{ProofRules, VerificationRules};
use ai_pow_zk::recursion::AiPowCompactBatchVerifierContext;
use nockvm::interpreter::Context;
use nockvm::jets::util::{slot, BAIL_FAIL};
use nockvm::jets::JetErr;
use nockvm::noun::{Noun, NounSpace, D};
use nockvm_macros::tas;
use once_cell::sync::OnceCell;

pub mod setup;
pub mod table_digest;

/// Pattern-length bound the verifier enforces (protocol constant; matches the
/// production admission envelope).
pub const AI_POW_VERIFY_MAX_PATTERN_LEN: usize = 4096;

/// Trusted compact verifier setup for one consensus-reachable verifier shape.
#[derive(serde::Serialize, serde::Deserialize)]
pub struct AiPowVerifierSetup {
    pub trace_height: usize,
    pub sx_bound: bool,
    pub context: AiPowCompactBatchVerifierContext,
    /// Canonical 40-byte verifier-key/setup digest.
    pub digest_bytes: Vec<u8>,
}

/// Verifier setup identity. `sx_bound` is load-bearing because it selects a
/// different Layer-0 AIR layout for stripe-major R-b schedules.
#[derive(
    Clone, Copy, Debug, Eq, PartialEq, Hash, Ord, PartialOrd, serde::Serialize, serde::Deserialize,
)]
pub struct VerifierSetupShapeKey {
    pub rules: ProofRules,
    pub trace_height: usize,
    pub sx_bound: bool,
}

impl VerifierSetupShapeKey {
    pub const fn new(trace_height: usize, sx_bound: bool) -> Self {
        Self::with_rules(trace_height, sx_bound, ProofRules::Hardened)
    }

    pub const fn with_rules(trace_height: usize, sx_bound: bool, rules: ProofRules) -> Self {
        Self {
            rules,
            trace_height,
            sx_bound,
        }
    }

    pub const fn for_rules(mut self, rules: ProofRules) -> Self {
        self.rules = rules;
        self
    }

    pub fn from_zk_params(zk_params: &ai_pow_zk::ZkParams, trace_height: usize) -> Option<Self> {
        if zk_params.noise_rank == 0 {
            return None;
        }
        let num_stripes = zk_params.k / zk_params.noise_rank;
        Some(Self::new(
            trace_height,
            (num_stripes as usize) <= ai_pow::params::STRIPE_MAX,
        ))
    }
}

impl AiPowVerifierSetup {
    pub const fn shape_key(&self) -> VerifierSetupShapeKey {
        VerifierSetupShapeKey::with_rules(
            self.trace_height,
            self.sx_bound,
            self.context.proof_rules(),
        )
    }
}

/// A per-bucket verifier context living ON DISK: the path of its serialized context
/// file plus its committed verifier-key digest. The context contains verifier
/// metadata and commitments, with prover trees and raw columns removed. It is
/// deserialized on demand (see [`ai_pow_verifier_setup_for`]), never rebuilt
/// during block verification.
pub struct DiskBucket {
    shape_key: VerifierSetupShapeKey,
    /// The canonical 40-byte verifier-key digest this context must match (the
    /// consensus parameter). Checked at boot and on every page-in.
    committed_digest: Vec<u8>,
    context_path: std::path::PathBuf,
    /// BLAKE3 of the serialized context bytes, checked on every page-in.
    /// A corrupt native context is a load failure, not a proof rejection.
    context_file_blake3: [u8; 32],
}

impl DiskBucket {
    /// Assemble a disk bucket from its shape key, committed verifier-key digest,
    /// the path of its serialized context file, and the BLAKE3 of that file's bytes.
    /// The boot installer builds these.
    pub fn new(
        shape_key: VerifierSetupShapeKey,
        committed_digest: Vec<u8>,
        context_path: std::path::PathBuf,
        context_file_blake3: [u8; 32],
    ) -> Self {
        Self {
            shape_key,
            committed_digest,
            context_path,
            context_file_blake3,
        }
    }
}

/// The outcome of resolving a verifier setup for a trace height. Distinguishes a
/// DETERMINISTIC "this block is invalid" (no committed bucket for the height ⇒ the
/// jet returns `NO`, which flows to the normal liar-block handling) from a
/// NON-DETERMINISTIC local fault (a real bucket whose context could not be loaded ⇒
/// the jet `%fail`s, so a broken node halts instead of wrongly rejecting valid
/// blocks that honest nodes accept).
pub(crate) enum VerifierSetupLookup {
    Found(Arc<AiPowVerifierSetup>),
    NoSuchBucket,
    LoadFailed,
}

/// A bounded LRU keyed by verifier setup shape. MRU is the back of `order`.
/// Generic over the value so its eviction/dedup logic is unit-testable without
/// real contexts.
struct Lru<V> {
    map: HashMap<VerifierSetupShapeKey, V>,
    order: Vec<VerifierSetupShapeKey>,
}

impl<V: Clone> Lru<V> {
    fn empty() -> Self {
        Self {
            map: HashMap::new(),
            order: Vec::new(),
        }
    }

    fn touch(&mut self, key: VerifierSetupShapeKey) {
        if let Some(pos) = self.order.iter().position(|&x| x == key) {
            self.order.remove(pos);
        }
        self.order.push(key);
    }

    /// Return the resident value for `key`, bumping it to MRU.
    fn get_touch(&mut self, key: VerifierSetupShapeKey) -> Option<V> {
        let v = self.map.get(&key)?.clone();
        self.touch(key);
        Some(v)
    }

    /// Insert `key` (deduping if it is already present, e.g. another thread filled it
    /// while we rebuilt) and evict the least-recently-used entries beyond `cap`.
    /// Returns the resident value for `key`.
    fn insert_capped(&mut self, key: VerifierSetupShapeKey, value: V, cap: usize) -> V {
        if let Some(existing) = self.map.get(&key).cloned() {
            self.touch(key);
            return existing;
        }
        self.map.insert(key, value.clone());
        self.order.push(key); // MRU
                              // Evict the least-recently-used beyond `cap`. Guard `remove(0)` on a non-empty
                              // `order` so this can never panic even if a prior panic (recovered via a
                              // poisoned lock) left `order`/`map` momentarily inconsistent.
        while self.map.len() > cap.max(1) && !self.order.is_empty() {
            let lru = self.order.remove(0); // front = LRU; `key` is at the back, so safe
            self.map.remove(&lru);
        }
        value
    }
}

/// The process-global verifier-setup table: per-bucket on-disk contexts (built once
/// at boot) plus a bounded LRU of the contexts currently paged into memory. A verify
/// pays at most a fast disk read + deserialize (~0.6 s worst case), NEVER a circuit
/// rebuild. See the disk-paged residency doc.
struct DiskPagedSetup {
    /// On-disk buckets, keyed by verifier setup shape. Empty for an eager-injected
    /// table (tests): those contexts are pinned in `resident` and never paged.
    disk: HashMap<VerifierSetupShapeKey, DiskBucket>,
    /// Max contexts paged into memory at once. Production disk-paged setup keeps the
    /// full committed table resident after first use.
    cap: usize,
    resident: Mutex<Lru<Arc<AiPowVerifierSetup>>>,
}

static SETUP: OnceCell<DiskPagedSetup> = OnceCell::new();

/// Resolve the setup for a given verifier shape, paging its context in from disk if
/// it is not already resident. See [`VerifierSetupLookup`] for how the jet treats
/// each outcome.
///
/// - `NoSuchBucket`: the table is missing a bucket for this shape. The
///   committed table is derived from the consensus admission predicates
///   (`validate_prod_envelope` + `envelope_check_dims`) — covering both MoE
///   and dense reachable shapes — so a missing bucket means the block's
///   certificate declares a shape consensus does not admit. The jet returns
///   `NO`, and consensus marks the block a liar so it cannot be re-spammed.
/// - `LoadFailed`: the table is uninjected, or a bucket EXISTS but its context could
///   not be loaded (missing / corrupt / bit-rotten file, disk error). That is a
///   NON-DETERMINISTIC per-node fault — the jet `%fail`s rather than voting.
///
/// The returned `Arc` keeps the context alive for the caller's verify even if the LRU
/// evicts it concurrently.
pub(crate) fn ai_pow_verifier_setup_for(key: VerifierSetupShapeKey) -> VerifierSetupLookup {
    // Uninjected setup is a per-node boot state (this node cannot verify anything yet),
    // not a property of the block ⇒ non-deterministic fault.
    let Some(s) = SETUP.get() else {
        return VerifierSetupLookup::LoadFailed;
    };
    // Fast path: already paged into memory.
    {
        let mut lru = s.resident.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(arc) = lru.get_touch(key) {
            return VerifierSetupLookup::Found(arc);
        }
    }
    // No committed bucket for this shape ⇒ the block is invalid (deterministic).
    let Some(bucket) = s.disk.get(&key) else {
        return VerifierSetupLookup::NoSuchBucket;
    };
    // A real bucket: page its context in from disk OUTSIDE the lock (~0.6 s worst
    // case). A load failure here is a non-deterministic local fault, NOT an
    // invalid-block signal.
    let setup = match page_in_bucket(bucket) {
        Some(s) => s,
        None => return VerifierSetupLookup::LoadFailed,
    };
    let arc = Arc::new(setup);
    let mut lru = s.resident.lock().unwrap_or_else(|e| e.into_inner());
    VerifierSetupLookup::Found(lru.insert_capped(key, arc, s.cap))
}

/// Page one bucket's verifier context IN from its on-disk file, verifying its file
/// checksum and its committed verifier-key digest. `None` on any read / checksum /
/// deserialize / digest failure — a missing, corrupt, or divergent context is a
/// NON-DETERMINISTIC local fault (the caller turns `None` into a jet `%fail`), never a
/// silent reject. This reads + deserializes a PREBUILT context (fast); it never
/// rebuilds.
fn page_in_bucket(bucket: &DiskBucket) -> Option<AiPowVerifierSetup> {
    let raw = match std::fs::read(&bucket.context_path) {
        Ok(b) => b,
        Err(e) => {
            tracing::error!(
                "ai-pow: reading verifier context {} ({:?}) failed: {e}",
                bucket.context_path.display(),
                bucket.shape_key,
            );
            return None;
        }
    };
    // Integrity: the file bytes must hash to the checksum recorded when it was written.
    // This catches on-disk bit-rot in the multi-GB preprocessed tree BEFORE it can
    // reach verification (where it would otherwise cause a valid block to be rejected).
    let file_blake3 = *blake3::hash(&raw).as_bytes();
    if file_blake3 != bucket.context_file_blake3 {
        tracing::error!(
            "ai-pow: verifier context file for {:?} failed its checksum (on-disk corruption) — \
             refusing to verify against it",
            bucket.shape_key,
        );
        return None;
    }
    // Deserialize under catch_unwind: a corrupt-but-checksum-colliding or otherwise
    // pathological file must not panic the node (this is a local, non-deterministic
    // fault ⇒ `None` ⇒ the caller `%fail`s).
    let decoded = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        bincode::serde::decode_from_slice::<AiPowVerifierSetup, _>(
            &raw,
            bincode::config::standard(),
        )
    }));
    let (setup, consumed) = match decoded {
        Ok(Ok(v)) => v,
        Ok(Err(e)) => {
            tracing::error!(
                "ai-pow: deserializing verifier context for {:?} failed: {e:?}", bucket.shape_key,
            );
            return None;
        }
        Err(_panic) => {
            tracing::error!(
                "ai-pow: deserializing verifier context for {:?} panicked (corrupt file)",
                bucket.shape_key,
            );
            return None;
        }
    };
    if consumed != raw.len() {
        tracing::error!("ai-pow: verifier context for {:?} has trailing bytes", bucket.shape_key);
        return None;
    }
    // Consensus + integrity gate: the deserialized context must internally bind its
    // verifier-key digest to its metadata, FRI shape, and common data, then match the
    // committed bucket digest.
    let recomputed_digest = match setup.context.validate_setup_binding() {
        Ok(digest) => digest,
        Err(e) => {
            tracing::error!(
                "ai-pow: on-disk verifier context for {:?} failed setup binding validation: {e:?}",
                bucket.shape_key,
            );
            return None;
        }
    };
    let digest =
        ai_pow_zk::recursion::compact_batch_verifier_key_digest_to_bytes(&recomputed_digest);
    if setup.shape_key() != bucket.shape_key
        || digest.as_slice() != bucket.committed_digest.as_slice()
        || setup.digest_bytes != bucket.committed_digest
    {
        tracing::error!(
            "ai-pow: on-disk verifier context for {:?} does not match its committed digest — \
             refusing to verify against a divergent/corrupt context",
            bucket.shape_key,
        );
        return None;
    }
    Some(setup)
}

/// Whether the boot verifier-setup table has already been injected. Boot uses this
/// to stay idempotent — a second boot in the same process (e.g. a test harness)
/// must not treat "already installed" as a fatal generation failure.
pub fn ai_pow_verifier_setup_initialized() -> bool {
    SETUP.get().is_some()
}

/// EAGER injection — inject a fully-built verifier-setup TABLE (one entry per Pearl
/// trace-height bucket) and PIN it resident. Used by tests / harnesses that already
/// hold rebuilt contexts. The contexts are never evicted (cap `>= len`) and never
/// paged (no disk buckets), so this reproduces the all-resident behavior.
///
/// Returns `Err` if already initialized (boot should call this exactly once) or if
/// the table is empty / has duplicate buckets.
// `Err(())` is a deliberate succeeded/failed marker: every caller adds its own
// boot-context error via `.map_err(|()| ..)`, so a richer error type would be
// discarded at the call site.
#[allow(clippy::result_unit_err)]
pub fn init_ai_pow_verifier_setup(setups: Vec<AiPowVerifierSetup>) -> Result<(), ()> {
    setup::prepare_verifier_resources().map_err(|_| ())?;
    let keys: Vec<VerifierSetupShapeKey> =
        setups.iter().map(AiPowVerifierSetup::shape_key).collect();
    if !setup_table_keys_valid(&keys) {
        return Err(());
    }
    let cap = setups.len().max(1);
    let mut lru = Lru::empty();
    for s in setups {
        let key = s.shape_key();
        lru.map.insert(key, Arc::new(s));
        lru.order.push(key);
    }
    let setup = DiskPagedSetup {
        disk: HashMap::new(),
        cap,
        resident: Mutex::new(lru),
    };
    SETUP.set(setup).map_err(|_| ())
}

/// DISK-PAGED injection — inject the per-bucket ON-DISK contexts (built at boot),
/// once. Each bucket's context is paged into memory on demand by
/// [`ai_pow_verifier_setup_for`] and held in a bounded LRU of `cap` contexts, so
/// standing RSS can be tuned without changing the committed setup table.
///
/// The caller (the boot installer) is responsible for having BUILT each context,
/// validated it against the committed consensus digest, and serialized it to
/// `context_path`; the recorded `committed_digest` is re-checked on every page-in.
/// Rejects an empty table or duplicate trace-height buckets; `Err` if already
/// initialized.
// `Err(())` marker: callers add boot context via `.map_err(|()| ..)`.
#[allow(clippy::result_unit_err)]
pub fn init_ai_pow_verifier_setup_disk(buckets: Vec<DiskBucket>, cap: usize) -> Result<(), ()> {
    setup::prepare_verifier_resources().map_err(|_| ())?;
    let keys: Vec<VerifierSetupShapeKey> = buckets.iter().map(|b| b.shape_key).collect();
    if !setup_table_keys_valid(&keys) {
        return Err(());
    }
    let disk: HashMap<VerifierSetupShapeKey, DiskBucket> =
        buckets.into_iter().map(|b| (b.shape_key, b)).collect();
    let setup = DiskPagedSetup {
        disk,
        cap: cap.max(1),
        resident: Mutex::new(Lru::empty()),
    };
    SETUP.set(setup).map_err(|_| ())
}

/// A verifier-setup table is well-formed iff it is non-empty and has no duplicate
/// shape keys. Pure so the admission rule is unit-testable without constructing
/// real setups.
fn setup_table_keys_valid(keys: &[VerifierSetupShapeKey]) -> bool {
    if keys.is_empty() {
        return false;
    }
    for (i, &key) in keys.iter().enumerate() {
        if keys[..i].contains(&key) {
            return false;
        }
    }
    true
}

/// Loobean helpers (`&`/yes = 0 = verified, `|`/no = 1 = rejected).
const YES: Noun = D(0);
const NO: Noun = D(1);

/// Convert a PoW **target** atom to a 32-byte little-endian value, SATURATING to
/// `[0xff; 32]` (2^256 − 1) when the atom exceeds 32 bytes.
///
/// The Nockchain block target is a tip5-atom-sized value (`merge:bignum` of
/// `max_tip5_atom / 2^bex`, up to ~2^320 — see `blockchain_constants::DEFAULT_MAX_TIP5_ATOM`),
/// so a real target routinely needs ~40 bytes and would fail the strict 32-byte
/// [`atom_to_32`]. The AI-PoW jackpot digest is a 256-bit value (`< 2^256`), so any
/// target `>= 2^256` is trivially satisfied by every valid jackpot. Clamping such a
/// target to `2^256 − 1` is EXACTLY equivalent for a 256-bit jackpot (`jackpot <=
/// 2^256 − 1 <= real_target`), and the downstream difficulty adjustment
/// (`u256_le_mul_u128_saturating`) already saturates in the same 256-bit domain.
/// Targets `< 2^256` still parse exactly. This never widens acceptance beyond the
/// real target, so it is sound; it only stops the jet from rejecting every real
/// block on the target-parse step.
fn target_atom_to_32_saturating(noun: Noun, space: &NounSpace) -> Option<[u8; 32]> {
    let atom = noun.in_space(space).as_atom().ok()?.atom();
    let handle = atom.in_space(space);
    let bytes = handle.as_ne_bytes();
    if bytes.len() > 32 {
        return Some([0xffu8; 32]);
    }
    let mut out = [0u8; 32];
    out[..bytes.len()].copy_from_slice(bytes);
    Some(out)
}

/// Derive the 32-byte Nockchain block commitment from the kernel's
/// `block-commitment:page:t` **noun** exactly as the miner does
/// (`ai-pow-miner::derive_job_inputs`): `BLAKE3(jam(commitment-noun))`.
///
/// This is the soundness-critical representation binding: the kernel's commitment
/// is a tip5 5-`belt` digest (a structured noun), NOT a 32-byte atom, so the jet
/// canonicalizes it the same way the prover did. `nockvm::serialization::jam`
/// (here) and `NounSlab::jam` (the miner) are the same canonical jam, so the
/// BLAKE3 inputs — and thus the commitments — match.
pub(crate) fn commit_from_noun(stack: &mut nockvm::mem::NockStack, noun: Noun) -> [u8; 32] {
    let jammed = nockvm::serialization::jam(stack, noun);
    let space = stack.noun_space();
    let handle = jammed.in_space(&space);
    let full = handle.as_ne_bytes();
    // `as_ne_bytes` is word-padded; the miner hashes `NounSlab::jam()` which is the
    // canonical (trailing-zero-trimmed) jam. Trim to the same significant length so
    // BLAKE3 matches — a padding mismatch here would reject every valid block.
    let sig_len = full.iter().rposition(|&b| b != 0).map_or(0, |i| i + 1);
    *blake3::hash(&full[..sig_len]).as_bytes()
}

/// Verify an already-decoded `%ai-pow` block artifact given the resolved 32-byte
/// block commitment + target and an explicit setup. Factored out so it is
/// unit-testable without the boot cache. Returns `Ok(true)` iff the block verifies,
/// `Ok(false)` if it is well-formed but invalid.
#[cfg(test)]
pub(crate) fn ai_pow_verify_core(
    artifact: &PearlMergeAiPowArtifactShape,
    commit: [u8; 32],
    target: [u8; 32],
    setup: &AiPowVerifierSetup,
) -> Result<bool, JetErr> {
    ai_pow_verify_core_with_rules(artifact, commit, target, setup, VerificationRules::Hardened)
}

fn ai_pow_verify_core_with_rules(
    artifact: &PearlMergeAiPowArtifactShape,
    commit: [u8; 32],
    target: [u8; 32],
    setup: &AiPowVerifierSetup,
    rules: VerificationRules,
) -> Result<bool, JetErr> {
    let limits = CertificateNounLimits::default();
    match ai_pow_miner::certificate_noun::verify_ai_pow_block_artifact_with_verification_rules(
        artifact, limits, &commit, &target, AI_POW_VERIFY_MAX_PATTERN_LEN, &setup.context,
        &setup.digest_bytes, rules,
    ) {
        Ok(AiPowBlockVerifyOutcome::Dense(_)) | Ok(AiPowBlockVerifyOutcome::Moe(_)) => Ok(true),
        Err(_) => Ok(false),
    }
}

/// The AI-PoW verify jet. Sample:
/// `[rules=?(%legacy %hardened %consrule) artifact=ai-pow-artifact commit=block-commitment:page:t target=@]`
/// Hoon derives `rules` from the block height. An unknown rule is a kernel/jet
/// interface error and halts with `%fail`; it must not become a block rejection.
/// — `commit` is the STRUCTURED commitment noun (canonicalized here via
/// `commit_from_noun`), `target` the `merge:bignum` LE atom the Hoon arm passes.
/// Result: loobean.
///
/// Artifact decoding and certificate verification run under `catch_unwind`;
/// a panic in either step yields the deterministic invalid-block result `NO`.
/// Neither wrapped step mutates the interpreter stack. A missing setup bucket
/// also yields `NO`, while a local setup fault (missing/corrupt context file or
/// uninjected table) yields `%fail` via `BAIL_FAIL` rather than rejecting a valid
/// block.
pub fn ai_pow_verify_jet(context: &mut Context, subject: Noun) -> Result<Noun, JetErr> {
    // Resource failure is local, including for direct library callers. Keep
    // preflight outside both invalid-artifact panic boundaries.
    setup::prepare_verifier_resources().map_err(|_| BAIL_FAIL)?;
    let space = context.stack.noun_space();
    // sample = [rules artifact commit target], selected by the kernel.
    let sample = slot(subject, 6, &space)?;
    let rules = verification_rules_from_noun(slot(sample, 2, &space)?)?;
    let artifact_noun = slot(sample, 6, &space)?;
    let commit_noun = slot(sample, 14, &space)?;
    let target_noun = slot(sample, 15, &space)?;

    // Decode, target parsing, and cap checks run under `catch_unwind`.
    // Decode/shape failures, invalid targets, over-cap heights, and panics yield NO.
    // This closure does not mutate `context.stack`.
    let decoded = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let limits = CertificateNounLimits::default();
        let artifact = decode_ai_pow_pearl_merge_artifact_noun_with_rules(
            artifact_noun,
            &space,
            limits,
            rules.proof_rules(),
        )
        .ok()?;
        ai_pow_miner::certificate_noun::validate_ai_pow_block_artifact_admission(&artifact, rules)
            .ok()?;
        // Dense and MoE artifacts share this compact certificate envelope.
        // Reject noncanonical values before paging in a verifier setup. Never
        // normalize here: Hoon already hashes the original artifact bytes.
        if rules.proof_rules() == ProofRules::Hardened
            && !ai_pow_compact_recursive_certificate_from_node(&artifact.certificate.certificate)
                .ok()?
                .has_canonical_pow_witnesses()
        {
            return None;
        }
        let target = target_atom_to_32_saturating(target_noun, &space)?;
        let mut setup_key = VerifierSetupShapeKey::from_zk_params(
            &artifact.certificate.zk_params, artifact.certificate.trace_height,
        )?;
        setup_key.rules = rules.proof_rules();
        Some((artifact, target, setup_key))
    }));
    let (artifact, target, setup_key) = match decoded {
        Ok(Some(v)) => v,
        Ok(None) | Err(_) => return Ok(NO),
    };

    // Resolve the setup for THIS cert's verifier shape.
    let setup = match ai_pow_verifier_setup_for(setup_key) {
        VerifierSetupLookup::Found(s) => s,
        // Deterministic: no committed bucket ⇒ the block is invalid on every honest
        // node ⇒ NO (and consensus marks it a liar).
        VerifierSetupLookup::NoSuchBucket => return Ok(NO),
        // Non-deterministic per-node fault (missing/corrupt file, uninjected table) ⇒
        // %fail; this node halts rather than wrongly voting.
        VerifierSetupLookup::LoadFailed => return Err(BAIL_FAIL),
    };

    // Canonicalize the structured commitment noun (mutates the stack via jam). Kept
    // OUTSIDE the catch_unwind below (which must not wrap stack mutation); the noun is
    // Hoon-constructed (the candidate's commitment), not attacker-shaped.
    let commit = commit_from_noun(&mut context.stack, commit_noun);

    // Verify the certificate against the trusted setup, under catch_unwind. A panic in
    // the recursion verifier on a crafted cert is a deterministic reject → NO, never a
    // node crash. `ai_pow_verify_core` touches only owned/borrowed data (no stack).
    let verified = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        ai_pow_verify_core_with_rules(&artifact, commit, target, &setup, rules)
    }));
    match verified {
        Ok(Ok(true)) => Ok(YES),
        Ok(Ok(false)) | Ok(Err(_)) | Err(_) => Ok(NO),
    }
}

fn verification_rules_from_noun(noun: Noun) -> Result<VerificationRules, JetErr> {
    match noun.as_direct().map_err(|_| BAIL_FAIL)?.data() {
        tas!(b"legacy") => Ok(VerificationRules::Legacy),
        tas!(b"hardened") => Ok(VerificationRules::Hardened),
        tas!(b"consrule") => Ok(VerificationRules::ConsensusBatch),
        _ => Err(BAIL_FAIL),
    }
}

/// Hot-state entry set for the AI-PoW verify jet. Appended to the nockchain kernel
/// hot state alongside `zkvm-jetpack`'s prover jets.
///
/// The Hoon `++ai-pow-verify` (`~/ %ai-pow-verify-v2`) lives in the shared
/// `/common/pow` lib under a `~% %pow-lib ..ut ~` root (it cannot be a kernel
/// door arm — the `fort` mold fixes %dumb-inner to load/peek/poke — nor a
/// `|^`-nested arm, which fails cold registration). `..ut` resolves to the
/// hoon.hoon std-library prefix `[one two tri qua pen]` (confirmed by the
/// `%zeke`-anchored jets, e.g. cheetah `ser-a-pt`, which sit at
/// `[one two tri qua pen zeke ..]`). So `%pow-lib` sits at
/// `[one two tri qua pen pow-lib]` and the jetted arm at
/// `[one two tri qua pen pow-lib ai-pow-verify-v2]`. Axis `1` is the `~/`-gate
/// convention (matches every base58 / ec-point `|=` jet). Runtime-validated by
/// the roswell `test-ai-pow-verify-jet-fires` unit test.
pub fn produce_ai_pow_hot_state() -> Vec<nockvm::jets::hot::HotEntry> {
    use either::Either::Left;
    use nockvm::jets::hot::K_138;
    // The v2 identity is intentionally distinct from both former three-field
    // bindings. A mismatched kernel/binary reaches the Hoon fail-safe stub.
    vec![(
        &[
            K_138,
            Left(b"one"),
            Left(b"two"),
            Left(b"tri"),
            Left(b"qua"),
            Left(b"pen"),
            Left(b"pow-lib"),
            Left(b"ai-pow-verify-v2"),
        ],
        1,
        ai_pow_verify_jet,
    )]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn verifier_rule_tags_reject_unknown_versions() {
        assert_eq!(
            verification_rules_from_noun(D(tas!(b"legacy"))).unwrap(),
            VerificationRules::Legacy
        );
        assert_eq!(
            verification_rules_from_noun(D(tas!(b"hardened"))).unwrap(),
            VerificationRules::Hardened
        );
        assert_eq!(
            verification_rules_from_noun(D(tas!(b"consrule"))).unwrap(),
            VerificationRules::ConsensusBatch
        );
        let mut stack = nockvm::mem::NockStack::new(8 << 20, 0);
        let cell = nockvm::noun::T(&mut stack, &[D(0), D(0)]);
        for unknown in [D(0), D(1), D(tas!(b"future")), cell] {
            assert!(matches!(
                verification_rules_from_noun(unknown),
                Err(JetErr::Fail(nockvm::interpreter::Error::NonDeterministic(
                    nockvm::interpreter::Mote::Fail,
                    _
                )))
            ));
        }
    }

    #[test]
    fn verifier_registers_only_the_versioned_single_jet() {
        let hot = produce_ai_pow_hot_state();
        assert_eq!(hot.len(), 1);
        assert_eq!(
            hot[0].0.last(),
            Some(&either::Either::Left(&b"ai-pow-verify-v2"[..]))
        );
        assert_eq!(hot[0].1, 1);
    }

    /// The bounded LRU underlying lazy residency: MRU touch, cap eviction of the
    /// least-recently-used entry, and dedup insert. Uses cheap `u64` values so the
    /// eviction/recency logic is tested without building real contexts.
    #[test]
    fn lru_evicts_least_recently_used() {
        let k13 = VerifierSetupShapeKey::new(13, true);
        let k14 = VerifierSetupShapeKey::new(14, true);
        let k15 = VerifierSetupShapeKey::new(15, false);
        let mut lru: Lru<u64> = Lru::empty();
        // Fill to cap 2.
        assert_eq!(lru.insert_capped(k13, 130, 2), 130);
        assert_eq!(lru.insert_capped(k14, 140, 2), 140);
        assert_eq!(lru.map.len(), 2);
        // Touch k13 so k14 becomes the LRU.
        assert_eq!(lru.get_touch(k13), Some(130));
        // Insert k15 → evicts k14 (LRU), keeps k13 (just touched) and k15.
        lru.insert_capped(k15, 150, 2);
        assert_eq!(lru.map.len(), 2);
        assert_eq!(
            lru.get_touch(k14),
            None,
            "k14 was evicted as least-recently-used"
        );
        assert_eq!(lru.get_touch(k13), Some(130), "k13 survived (was touched)");
        assert_eq!(lru.get_touch(k15), Some(150), "k15 is resident");
    }

    /// Dedup insert: inserting a key already resident returns the EXISTING value
    /// (a concurrent double-rebuild does not double-store or grow past cap).
    #[test]
    fn lru_insert_dedups_existing() {
        let key = VerifierSetupShapeKey::new(13, true);
        let mut lru: Lru<u64> = Lru::empty();
        assert_eq!(lru.insert_capped(key, 130, 3), 130);
        // Second insert of the same key with a DIFFERENT value returns the first.
        assert_eq!(
            lru.insert_capped(key, 999, 3),
            130,
            "existing value kept on dedup"
        );
        assert_eq!(lru.map.len(), 1, "no duplicate entry");
    }

    /// A cap of 1 keeps only the most-recent bucket (minimum RSS). Monotonic height
    /// progression (the common case) never thrashes: each new height evicts the old.
    #[test]
    fn lru_cap_one_keeps_only_mru() {
        let a = VerifierSetupShapeKey::new(13, true);
        let b = VerifierSetupShapeKey::new(14, false);
        let mut lru: Lru<u64> = Lru::empty();
        lru.insert_capped(a, 130, 1);
        lru.insert_capped(b, 140, 1);
        assert_eq!(lru.map.len(), 1);
        assert_eq!(lru.get_touch(a), None);
        assert_eq!(lru.get_touch(b), Some(140));
    }

    /// The verifier-setup TABLE admission rule: non-empty, one setup per verifier
    /// shape key, no duplicates.
    #[test]
    fn setup_table_admission_rule() {
        let h13_sx = VerifierSetupShapeKey::new(8192, true);
        let h14_sx = VerifierSetupShapeKey::new(16384, true);
        let h14_rb = VerifierSetupShapeKey::new(16384, false);
        assert!(!setup_table_keys_valid(&[]), "empty table rejected");
        assert!(setup_table_keys_valid(&[h13_sx]), "single bucket ok");
        assert!(
            setup_table_keys_valid(&[h13_sx, h14_sx, h14_rb]),
            "distinct keys ok"
        );
        assert!(
            !setup_table_keys_valid(&[h13_sx, h14_sx, h14_sx]),
            "duplicate key rejected (a cert must resolve to exactly one setup)"
        );
    }

    /// The boot installer is generate-or-shutdown: with NO cache file and NO bucket
    /// shapes to generate from, it returns `Err` (fatal — the caller shuts the node
    /// down) rather than silently booting without a verifier setup. It must not
    /// touch the global setup OnceCell in this failure path. (Cheap: no proving.)
    #[test]
    fn boot_installer_no_cache_no_buckets_is_fatal() {
        assert!(
            !crate::ai_pow_verifier_setup_initialized(),
            "precondition: no setup installed in this test process",
        );
        let empty_dir =
            std::env::temp_dir().join(format!("ai-pow-jets-no-cache-{}", std::process::id()));
        // No cache file present AND no bucket shapes ⇒ cannot generate ⇒ fatal.
        let result = crate::setup::install_or_build_verifier_setup(&empty_dir, &[]);
        assert!(
            result.is_err(),
            "no cache + no buckets must be a fatal error (generate-or-shutdown)",
        );
        assert!(
            !crate::ai_pow_verifier_setup_initialized(),
            "failed install must not have injected a table",
        );
    }

    /// Corrupt-cache recovery (cheap, no proving): a present-but-unreadable cache
    /// remains available for diagnosis until a complete replacement can be written.
    /// With no bucket shapes available, the installer fails without injecting a
    /// table or deleting the cache.
    #[test]
    fn corrupt_cache_is_retained_when_regeneration_is_unavailable() {
        assert!(
            !crate::ai_pow_verifier_setup_initialized(),
            "precondition: no setup installed in this test process",
        );
        let dir = std::env::temp_dir().join(format!("ai-pow-jets-corrupt-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let path = crate::setup::verifier_setup_descriptor_cache_path(&dir);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, b"this is not a valid verifier-setup cache").unwrap();
        assert!(path.exists(), "precondition: corrupt cache present");

        let result = crate::setup::install_or_build_verifier_setup(&dir, &[]);
        assert!(
            result.is_err(),
            "corrupt cache + no buckets to regenerate must be fatal",
        );
        assert!(
            path.exists(),
            "the corrupt cache remains available until regeneration succeeds",
        );
        assert!(
            !crate::ai_pow_verifier_setup_initialized(),
            "failed install must not have injected a table",
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The production bucket set (what boot generates) must cover every consensus
    /// accept-band verifier shape: at least one key for each height 2^13..2^19, and
    /// both `sx_bound` classes wherever the envelope can reach both layouts.
    /// Cheap: heights computed WITHOUT proving.
    #[test]
    fn production_verifier_setup_buckets_cover_the_capped_band() {
        let buckets = crate::setup::production_verifier_setup_buckets();
        assert!(!buckets.is_empty(), "must return at least one bucket");
        assert_eq!(
            crate::setup::AI_POW_VERIFIER_CACHE_CAP_DEFAULT,
            2 * buckets.len(),
            "the production default must retain both versions of every shape",
        );
        let cap_db = (ai_pow::params::AI_POW_MAX_TRACE_HEIGHT as u32).trailing_zeros();
        let mut keys: Vec<VerifierSetupShapeKey> = Vec::new();
        let mut by_height: std::collections::BTreeMap<u32, std::collections::BTreeSet<bool>> =
            std::collections::BTreeMap::new();
        for b in &buckets {
            let (th, sx_bound) = if b.dense {
                let schedule = ai_pow_zk::canonical::StripIndexSchedule {
                    a_indices: (0..b.params.tile).collect(),
                    b_indices: (0..b.params.tile).collect(),
                };
                let budget = ai_pow::zk_bridge::expected_layer0_rows_for_strip_schedule(
                    &b.params, &schedule,
                )
                .expect("dense bucket trace height");
                let th = budget.required_trace_len();
                (
                    th,
                    (b.params.num_stripes() as usize) <= ai_pow::params::STRIPE_MAX,
                )
            } else {
                let th = crate::setup::reference_moe_trace_height(&b.params, b.hw, b.e, b.top_k)
                    .expect("cheap trace height");
                (
                    th,
                    (b.params.k / b.params.noise_rank) as usize <= ai_pow::params::STRIPE_MAX,
                )
            };
            assert!(
                th.is_power_of_two(),
                "bucket height {th} must be a power of two"
            );
            assert!(
                th >= 1 << 13,
                "bucket height {th} must be >= MIN_STARK_LEN (2^13)"
            );
            assert!(
                th <= ai_pow::params::AI_POW_MAX_TRACE_HEIGHT,
                "bucket height {th} must be <= the consensus cap 2^{cap_db}",
            );
            let key = VerifierSetupShapeKey::new(th, sx_bound);
            keys.push(key);
            by_height
                .entry(th.trailing_zeros())
                .or_default()
                .insert(sx_bound);
        }
        keys.sort_unstable();
        let distinct: std::collections::BTreeSet<VerifierSetupShapeKey> =
            keys.iter().copied().collect();
        eprintln!("production setup keys: {keys:?}");
        assert_eq!(
            distinct.len(),
            buckets.len(),
            "buckets must have distinct shape keys",
        );
        for db in 13u32..=cap_db {
            assert!(
                by_height.contains_key(&db),
                "production buckets must cover 2^{db}; covered = {by_height:?}",
            );
        }
        let both_sx_classes: std::collections::BTreeSet<bool> = [false, true].into_iter().collect();
        for db in 13u32..=cap_db {
            assert_eq!(
                by_height.get(&db),
                Some(&both_sx_classes),
                "production buckets must cover both sx_bound classes at 2^{db}",
            );
        }
    }

    #[test]
    fn peak_dense_profile_uses_an_installed_setup_bucket() {
        let params = ai_pow_miner::PEAK_PRODUCTION_PARAMS;
        let trace_height = ai_pow::zk_bridge::pearl_dense_canonical_trace_height(&params, 0, 0)
            .expect("peak dense trace height");
        let key = VerifierSetupShapeKey::new(
            trace_height,
            (params.k / params.noise_rank) as usize <= ai_pow::params::STRIPE_MAX,
        );

        let installed_keys: std::collections::BTreeSet<_> =
            crate::setup::production_verifier_setup_buckets()
                .into_iter()
                .map(|bucket| {
                    let (height, sx) = if bucket.dense {
                        let schedule = ai_pow_zk::canonical::StripIndexSchedule {
                            a_indices: (0..bucket.params.tile).collect(),
                            b_indices: (0..bucket.params.tile).collect(),
                        };
                        let budget = ai_pow::zk_bridge::expected_layer0_rows_for_strip_schedule(
                            &bucket.params, &schedule,
                        )
                        .expect("dense bucket trace height");
                        (
                            budget.required_trace_len(),
                            (bucket.params.num_stripes() as usize) <= ai_pow::params::STRIPE_MAX,
                        )
                    } else {
                        let h = crate::setup::reference_moe_trace_height(
                            &bucket.params, bucket.hw, bucket.e, bucket.top_k,
                        )
                        .expect("production setup bucket trace height");
                        (
                            h,
                            (bucket.params.k / bucket.params.noise_rank) as usize
                                <= ai_pow::params::STRIPE_MAX,
                        )
                    };
                    VerifierSetupShapeKey::new(height, sx)
                })
                .collect();

        assert!(
            installed_keys.contains(&key),
            "missing peak setup key {key:?}"
        );
    }
}

#[cfg(test)]
mod jet_tests {
    use ai_pow::difficulty::{attempt_wins, shape_work_factor_for, AI_POW_MAX_CONSENSUS_TARGET};
    use ai_pow::params::MatmulParams;
    use ai_pow_miner::certificate_noun::build_ai_pow_pearl_merge_moe_artifact_noun_from_node;
    use ai_pow_miner::reference::evaluate_reference_moe_jackpot;
    use nockapp::noun::slab::NounSlab;
    use nockvm::noun::NounAllocator;

    use super::*;
    use crate::setup::{
        build_verifier_setup_descriptor, prove_reference_moe_block, ReferenceBlock,
        REFERENCE_SETUP_COMMIT,
    };

    fn reference_block_descriptor(
        block: &ReferenceBlock,
    ) -> ai_pow_zk::recursion::AiPowCompactVerifierSetupDescriptor {
        ai_pow_zk::recursion::AiPowCompactVerifierSetupDescriptor::from_context(
            block.run.zk_params, block.run.trace_height, &block.run.verifier_context,
        )
        .expect("descriptor from genuine proof context")
    }

    /// Cue a jammed artifact into a fresh slab and return `(slab, root)`.
    fn cue_artifact(jammed: nockapp::Bytes) -> NounSlab {
        let mut slab: NounSlab = NounSlab::new();
        let root = slab.cue_into(jammed).expect("cue artifact");
        slab.set_root(root);
        slab
    }

    /// Unwrap a `VerifierSetupLookup::Found`, panicking (in test) otherwise.
    fn expect_found(l: crate::VerifierSetupLookup) -> std::sync::Arc<crate::AiPowVerifierSetup> {
        match l {
            crate::VerifierSetupLookup::Found(a) => a,
            crate::VerifierSetupLookup::NoSuchBucket => panic!("expected Found, got NoSuchBucket"),
            crate::VerifierSetupLookup::LoadFailed => panic!("expected Found, got LoadFailed"),
        }
    }

    /// Return a canonical artifact that clears the largest consensus-minable target.
    ///
    /// The target is scaled by the shape work factor, so `[0xff; 32]` overflows
    /// and is deliberately unminable. A fixed commitment search makes the
    /// acceptance KAT exercise a real winning ticket.
    fn target_hitting_reference_moe_block() -> (ReferenceBlock, [u8; 32]) {
        let params = MatmulParams {
            m: 64,
            k: 1024,
            n: 64,
            noise_rank: 64,
            tile: 8,
            spot_checks: 1,
            difficulty_bits: 0,
        };
        let target = AI_POW_MAX_CONSENSUS_TARGET;
        let work_factor =
            shape_work_factor_for(8, 8, params.k, params.noise_rank).expect("valid shape");
        let commit = (0u64..4096)
            .find_map(|attempt| {
                let mut commit = [0u8; 32];
                commit[..8].copy_from_slice(&attempt.to_le_bytes());
                let jackpot = evaluate_reference_moe_jackpot(&params, 8, 2, 1, commit, 0)
                    .expect("evaluate canonical MoE ticket");
                attempt_wins(&jackpot, &target, work_factor)
                    .expect("max consensus target is minable")
                    .then_some(commit)
            })
            .expect("canonical commitment search must find a target-winning ticket");
        (
            prove_reference_moe_block(&params, 8, 2, 1, commit)
                .expect("prove target-winning canonical MoE block"),
            target,
        )
    }

    /// **Soundness KAT (fast, no proving): the commit representation binding.**
    /// The jet derives the 32-byte block commitment as `BLAKE3(jam(commit-noun))`
    /// via `nockvm::serialization::jam`; the miner (`derive_job_inputs`) uses
    /// `BLAKE3(NounSlab::jam(..))`. These must be byte-identical — including the
    /// trailing-zero trimming — or every valid block is rejected. This pins that.
    #[test]
    fn commit_from_noun_matches_miner_derivation() {
        use nockvm::mem::NockStack;
        use nockvm::noun::{D, T};
        for payload in [D(0), D(1), D(0xdead_beef_u64), D(0xff00_u64)] {
            // Miner path: build the noun in a NounSlab, hash its canonical jam.
            let mut slab: NounSlab = NounSlab::new();
            let s = T(&mut slab, &[D(1), D(2), D(3), payload]);
            slab.set_root(s);
            let miner = *blake3::hash(&slab.jam()).as_bytes();

            // Jet path: the same logical noun in a NockStack, via commit_from_noun.
            let mut stack = NockStack::new(8 << 20, 0);
            let k = T(&mut stack, &[D(1), D(2), D(3), payload]);
            let jet = commit_from_noun(&mut stack, k);

            assert_eq!(
                jet, miner,
                "jet BLAKE3(nockvm jam) must equal miner BLAKE3(NounSlab::jam)",
            );
        }
    }

    /// **Robustness (fast, no proving): a MALFORMED `%ai-pow` artifact is a clean
    /// reject, never a verifier crash or a false accept.** The jet decodes the
    /// artifact BEFORE it requires the boot setup and turns any decode `Err` into
    /// `Ok(NO)` (see `ai_pow_verify_jet` / `ai_pow_verify_core`). This pins the three
    /// malformed shapes a hostile miner can submit to the live `+do-pow` path — a
    /// non-tuple, a wrong artifact tag, and a well-tagged artifact with an
    /// undecodable nonce/certificate tail — each rejecting at the decode boundary.
    #[test]
    fn malformed_ai_pow_artifact_is_rejected_at_decode() {
        use nockvm::noun::{D, T};
        use nockvm_macros::tas;
        let limits = CertificateNounLimits::default();

        // Build a root noun in a fresh slab and assert it does NOT decode.
        let assert_rejected = |build: &dyn Fn(&mut NounSlab) -> nockvm::noun::Noun, why: &str| {
            let mut slab: NounSlab = NounSlab::new();
            let root = build(&mut slab);
            slab.set_root(root);
            let space = slab.noun_space();
            let root = unsafe { *slab.root() };
            assert!(
                decode_ai_pow_pearl_merge_artifact_noun(root, &space, limits).is_err(),
                "{why}",
            );
        };

        // 1) Not a 3-tuple — a bare atom.
        assert_rejected(&|_s| D(0), "a bare atom is not an %ai-pow artifact");
        // 2) A 3-tuple with the WRONG head tag.
        assert_rejected(
            &|s| T(s, &[D(0xdead_beef), D(0), D(0)]),
            "a non-%ai-pow artifact tag must be rejected",
        );
        // 3) Correct %ai-pow tag but a garbage (undecodable) nonce + certificate tail.
        assert_rejected(
            &|s| T(s, &[D(tas!(b"ai-pow")), D(0), D(0)]),
            "a well-tagged artifact with an undecodable nonce/cert must be rejected",
        );
    }

    /// Target atoms below `2^256` parse exactly; larger atoms saturate to the
    /// largest 256-bit target, which is equivalent for a 256-bit jackpot hash.
    #[test]
    fn target_atom_to_32_saturates_only_oversized_targets() {
        use nockvm::noun::{IndirectAtom, D};

        fn indirect_target(bytes: &[u8]) -> [u8; 32] {
            let mut slab: NounSlab = NounSlab::new();
            let atom = <IndirectAtom as nockapp::IndirectAtomExt>::from_bytes(&mut slab, bytes);
            target_atom_to_32_saturating(atom.as_noun(), &slab.noun_space()).expect("target atom")
        }

        let slab: NounSlab = NounSlab::new();
        let small = target_atom_to_32_saturating(D(0x0102), &slab.noun_space()).expect("atom");
        assert_eq!(&small[..2], &[0x02, 0x01]);
        assert!(small[2..].iter().all(|&b| b == 0));

        let mut exact = [0u8; 32];
        exact[0] = 0x34;
        exact[31] = 0x80;
        assert_eq!(indirect_target(&exact), exact);
        assert_eq!(indirect_target(&[0xff; 32]), [0xff; 32]);

        let mut over = [0u8; 33];
        over[32] = 1;
        assert_eq!(indirect_target(&over), [0xff; 32]);
    }

    /// KAT (real proving, ~25s): a real MoE `%ai-pow` block artifact verifies
    /// through the jet CORE; a wrong commitment, forged found index, and unmet
    /// difficulty are rejected (`Ok(false)`, not a jet error). Validates the artifact
    /// decode-from-noun + verify dispatch over `verify_ai_pow_block_artifact`.
    #[test]
    #[ignore = "real MoE compact proof (~25s); opt-in"]
    fn ai_pow_verify_jet_core_accepts_real_block_and_rejects_tampering() {
        let (block, target) = target_hitting_reference_moe_block();

        let jammed = build_ai_pow_pearl_merge_moe_artifact_noun_from_node(
            &block.statement, &block.aux_inclusion, &block.moe_art, &block.certificate.zk_params,
            block.certificate.found_idx, block.certificate.trace_height,
            &block.certificate.commitments, &block.certificate.public_inputs,
            &block.certificate.certificate,
        )
        .expect("build MoE artifact noun")
        .jam();

        let digest_bytes = ai_pow_zk::recursion::compact_batch_verifier_key_digest_to_bytes(
            &block.run.verifier_key_digest(),
        )
        .to_vec();
        let setup_key = VerifierSetupShapeKey::from_zk_params(
            &block.certificate.zk_params, block.run.trace_height,
        )
        .expect("valid setup key");
        let setup = AiPowVerifierSetup {
            trace_height: setup_key.trace_height,
            sx_bound: setup_key.sx_bound,
            context: block.run.verifier_context,
            digest_bytes,
        };
        let commit = block.commit;
        // Decode the artifact noun to the shape (what the jet does before verify).
        let slab = cue_artifact(jammed);
        let space = slab.noun_space();
        let root = unsafe { *slab.root() };
        let artifact =
            decode_ai_pow_pearl_merge_artifact_noun(root, &space, CertificateNounLimits::default())
                .expect("decode artifact noun");

        assert!(
            ai_pow_compact_recursive_certificate_from_node(&artifact.certificate.certificate)
                .expect("decode honest MoE compact certificate")
                .has_canonical_pow_witnesses(),
            "honest MoE prover must satisfy post-activation admission",
        );
        assert!(
            matches!(
                ai_pow_verify_core(&artifact, commit, target, &setup),
                Ok(true)
            ),
            "real MoE block must verify through the jet core",
        );
        assert!(
            matches!(
                ai_pow_verify_core(&artifact, [0x99u8; 32], target, &setup),
                Ok(false)
            ),
            "wrong block commitment must be rejected",
        );
        let mut forged_found_idx = artifact.clone();
        forged_found_idx.certificate.found_idx = 1;
        assert!(
            matches!(
                ai_pow_verify_core(&forged_found_idx, commit, target, &setup),
                Ok(false)
            ),
            "MoE certificate metadata must bind the fixed found index",
        );
        assert!(
            matches!(
                ai_pow_verify_core(&artifact, commit, [0u8; 32], &setup),
                Ok(false)
            ),
            "unmet difficulty must be rejected",
        );
    }

    /// Genuine proof generation and shape-only setup produce byte-identical native
    /// verifier contexts and keys. Both accept an honest artifact after a context
    /// serialization round trip and reject a changed block commitment.
    #[test]
    #[ignore = "real MoE compact proof (~25s); opt-in"]
    fn genuine_and_shape_verifier_contexts_are_equivalent() {
        let (block, target) = target_hitting_reference_moe_block();
        let commit = block.commit;

        let jammed = build_ai_pow_pearl_merge_moe_artifact_noun_from_node(
            &block.statement, &block.aux_inclusion, &block.moe_art, &block.certificate.zk_params,
            block.certificate.found_idx, block.certificate.trace_height,
            &block.certificate.commitments, &block.certificate.public_inputs,
            &block.certificate.certificate,
        )
        .expect("build MoE artifact noun")
        .jam();

        // The genuine proof run's native verifier context.
        let full_digest = ai_pow_zk::recursion::compact_batch_verifier_key_digest_to_bytes(
            &block.run.verifier_key_digest(),
        )
        .to_vec();
        let setup_key = VerifierSetupShapeKey::from_zk_params(
            &block.certificate.zk_params, block.run.trace_height,
        )
        .expect("valid setup key");
        let descriptor = reference_block_descriptor(&block);
        let full_setup = AiPowVerifierSetup {
            trace_height: setup_key.trace_height,
            sx_bound: setup_key.sx_bound,
            context: block.run.verifier_context,
            digest_bytes: full_digest.clone(),
        };
        // Independently derive the native context from canonical geometry.
        let slim_setup = crate::setup::rebuild_verifier_setup_from_descriptor(descriptor)
            .expect("derive slimmed setup from shape descriptor");

        assert_eq!(
            slim_setup.shape_key(),
            full_setup.shape_key(),
            "same setup key"
        );
        assert_eq!(
            slim_setup.digest_bytes, full_digest,
            "shape-only setup must preserve the genuine verifier key",
        );

        let full_ser =
            bincode::serde::encode_to_vec(&full_setup.context, bincode::config::standard())
                .expect("serialize genuine context");
        let slim_ser =
            bincode::serde::encode_to_vec(&slim_setup.context, bincode::config::standard())
                .expect("serialize shape-derived context");
        assert_eq!(full_ser, slim_ser, "complete native verifier context bytes");
        assert!(
            slim_ser.len() < 64 * 1024,
            "native verifier metadata must not retain a prover tree",
        );
        let encoded = bincode::serde::encode_to_vec(&slim_setup, bincode::config::standard())
            .expect("serialize compact setup");
        let (slim_setup, used): (AiPowVerifierSetup, usize) =
            bincode::serde::decode_from_slice(&encoded, bincode::config::standard())
                .expect("deserialize compact setup");
        assert_eq!(used, encoded.len());

        let slab = cue_artifact(jammed);
        let space = slab.noun_space();
        let root = unsafe { *slab.root() };
        let artifact =
            decode_ai_pow_pearl_merge_artifact_noun(root, &space, CertificateNounLimits::default())
                .expect("decode artifact noun");

        // ACCEPT: the real block verifies against BOTH contexts.
        assert!(
            matches!(
                ai_pow_verify_core(&artifact, commit, target, &full_setup),
                Ok(true)
            ),
            "real block verifies against the FULL context",
        );
        assert!(
            matches!(
                ai_pow_verify_core(&artifact, commit, target, &slim_setup),
                Ok(true)
            ),
            "real block verifies IDENTICALLY against the SLIMMED context",
        );
        // REJECT: a wrong commitment is rejected by BOTH contexts.
        assert!(
            matches!(
                ai_pow_verify_core(&artifact, [0x99u8; 32], target, &full_setup),
                Ok(false)
            ),
            "wrong commitment rejected by the FULL context",
        );
        assert!(
            matches!(
                ai_pow_verify_core(&artifact, [0x99u8; 32], target, &slim_setup),
                Ok(false)
            ),
            "wrong commitment rejected IDENTICALLY by the SLIMMED context",
        );
    }

    /// DISK-PAGED RESIDENCY KAT (real proof, ~25s): a context built at boot and
    /// serialized to disk is PAGED IN at the first lookup (read + deserialize, no
    /// rebuild), verifies a real block identically to the eager path, caches it (a
    /// second lookup returns the SAME `Arc`), and rejects a wrong commitment.
    /// Exercises the full disk page-in + per-bucket digest-validation path used in
    /// production.
    #[test]
    #[ignore = "real MoE compact proof (~25s); opt-in; sets the process-global setup"]
    fn disk_paged_setup_pages_in_and_verifies() {
        assert!(
            !crate::ai_pow_verifier_setup_initialized(),
            "run in a fresh process (installs the process-global setup)",
        );
        let (block, target) = target_hitting_reference_moe_block();
        let commit = block.commit;
        let setup_key = VerifierSetupShapeKey::from_zk_params(
            &block.certificate.zk_params, block.certificate.trace_height,
        )
        .expect("valid setup key");
        let jammed = build_ai_pow_pearl_merge_moe_artifact_noun_from_node(
            &block.statement, &block.aux_inclusion, &block.moe_art, &block.certificate.zk_params,
            block.certificate.found_idx, block.certificate.trace_height,
            &block.certificate.commitments, &block.certificate.public_inputs,
            &block.certificate.certificate,
        )
        .expect("build MoE artifact noun")
        .jam();

        // Inject the setup DISK-PAGED (production path): serializes the context, but leaves
        // nothing heavy resident yet.
        let tmp = tempfile::TempDir::new().unwrap();
        let setup_built = crate::setup::rebuild_verifier_setup_from_descriptor(
            reference_block_descriptor(&block),
        )
        .expect("build context");
        crate::setup::install_verifier_setup_disk_from_setups(vec![setup_built], tmp.path(), 2)
            .expect("disk-paged init");

        // First lookup pages the context in from disk (and validates its digest).
        let setup = expect_found(crate::ai_pow_verifier_setup_for(setup_key));

        let slab = cue_artifact(jammed);
        let space = slab.noun_space();
        let root = unsafe { *slab.root() };
        let artifact =
            decode_ai_pow_pearl_merge_artifact_noun(root, &space, CertificateNounLimits::default())
                .expect("decode artifact noun");

        assert!(
            matches!(
                ai_pow_verify_core(&artifact, commit, target, &setup),
                Ok(true)
            ),
            "a lazily-built setup verifies a real block",
        );
        assert!(
            matches!(
                ai_pow_verify_core(&artifact, [0x99u8; 32], target, &setup),
                Ok(false)
            ),
            "wrong commitment rejected",
        );

        // A second lookup returns the SAME resident context (no page-in).
        let setup2 = expect_found(crate::ai_pow_verifier_setup_for(setup_key));
        assert!(
            std::sync::Arc::ptr_eq(&setup, &setup2),
            "the second lookup must return the cached (resident) context, not a fresh page-in",
        );

        // An unknown in-band shape key has no bucket ⇒ NoSuchBucket (jet returns NO, a
        // deterministic invalid-block reject — NOT a %fail).
        let missing_key =
            VerifierSetupShapeKey::new(setup_key.trace_height + 1, setup_key.sx_bound);
        assert!(
            matches!(
                crate::ai_pow_verifier_setup_for(missing_key),
                crate::VerifierSetupLookup::NoSuchBucket
            ),
            "a shape with no disk bucket resolves to NoSuchBucket",
        );
    }

    /// A context file with an invalid checksum resolves to `LoadFailed` at page-in.
    /// This is a local setup fault (`%fail`), not a setup match or invalid-block
    /// rejection. Sets the process-global setup; run alone.
    #[test]
    #[ignore = "real MoE compact proof (~25s); corrupts a context file; run alone"]
    fn corrupt_context_file_pages_in_as_load_failed() {
        if crate::ai_pow_verifier_setup_initialized() {
            eprintln!("skip: verifier setup already initialized in this process");
            return;
        }
        let params = MatmulParams {
            m: 64,
            k: 1024,
            n: 64,
            noise_rank: 64,
            tile: 8,
            spot_checks: 1,
            difficulty_bits: 0,
        };
        let block =
            prove_reference_moe_block(&params, 8, 2, 1, REFERENCE_SETUP_COMMIT).expect("prove");
        let setup = crate::setup::rebuild_verifier_setup_from_descriptor(
            reference_block_descriptor(&block),
        )
        .expect("build");
        let setup_key = setup.shape_key();
        let digest = setup.digest_bytes.clone();
        let tmp = tempfile::TempDir::new().unwrap();
        crate::setup::install_verifier_setup_disk_from_setups(vec![setup], tmp.path(), 2)
            .expect("disk-paged init");

        // Corrupt the context file (flip a byte in the middle — lands in the tree),
        // leaving the sidecar checksum intact.
        let ctx_path = crate::setup::verifier_context_file_path(tmp.path(), setup_key, &digest);
        let mut bytes = std::fs::read(&ctx_path).unwrap();
        let mid = bytes.len() / 2;
        bytes[mid] ^= 0xff;
        std::fs::write(&ctx_path, &bytes).unwrap();

        // First page-in reads the corrupt file → checksum mismatch → LoadFailed.
        assert!(
            matches!(
                crate::ai_pow_verifier_setup_for(setup_key),
                crate::VerifierSetupLookup::LoadFailed
            ),
            "a corrupt context file must page in as LoadFailed (→ %fail), never Found or a \
             silent reject",
        );
    }

    /// Page-in rejects changed key bytes, geometry, rules, or trailing payload,
    /// even when the checksum matches the file that was written.
    #[test]
    fn divergent_serialized_setup_pages_in_as_load_failed() {
        let params = MatmulParams {
            m: 64,
            k: 1024,
            n: 64,
            noise_rank: 64,
            tile: 8,
            spot_checks: 1,
            difficulty_bits: 0,
        };
        for rules in [ProofRules::Legacy, ProofRules::Hardened] {
            let mut setup = crate::setup::build_verifier_setup_with_rules(&params, 8, 2, 1, rules)
                .expect("canonical shape context");
            let key = setup.shape_key();
            let committed = setup.digest_bytes.clone();
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("context.bin");
            let page = |setup: &AiPowVerifierSetup, trailing: bool| {
                let mut bytes = bincode::serde::encode_to_vec(setup, bincode::config::standard())
                    .expect("serialize setup");
                if trailing {
                    bytes.push(0x42);
                }
                let checksum = *blake3::hash(&bytes).as_bytes();
                std::fs::write(&path, bytes).unwrap();
                crate::page_in_bucket(&crate::DiskBucket::new(
                    key,
                    committed.clone(),
                    path.clone(),
                    checksum,
                ))
            };
            assert!(page(&setup, false).is_some());
            assert!(page(&setup, true).is_none(), "reject trailing bytes");
            setup.digest_bytes[0] ^= 0xff;
            assert!(page(&setup, false).is_none(), "reject changed key");
            setup.digest_bytes[0] ^= 0xff;
            setup.trace_height *= 2;
            assert!(page(&setup, false).is_none(), "reject changed geometry");
            setup.trace_height /= 2;
            setup.sx_bound = !setup.sx_bound;
            assert!(
                page(&setup, false).is_none(),
                "reject changed stripe binding"
            );
            setup.sx_bound = !setup.sx_bound;
            let swapped = VerifierSetupShapeKey::with_rules(
                key.trace_height,
                key.sx_bound,
                if rules == ProofRules::Legacy {
                    ProofRules::Hardened
                } else {
                    ProofRules::Legacy
                },
            );
            let bytes = bincode::serde::encode_to_vec(&setup, bincode::config::standard()).unwrap();
            let checksum = *blake3::hash(&bytes).as_bytes();
            std::fs::write(&path, bytes).unwrap();
            assert!(
                crate::page_in_bucket(&crate::DiskBucket::new(swapped, committed, path, checksum,))
                    .is_none(),
                "reject changed rules"
            );
        }
    }

    /// Resolve every committed key with bounded residency using a complete cache.
    #[test]
    #[ignore = "builds all production contexts to disk + measures paged RSS; run alone"]
    fn install_or_build_disk_paged_boot_and_rss() {
        if crate::ai_pow_verifier_setup_initialized() {
            eprintln!("skip: verifier setup already initialized in this process");
            return;
        }
        fn rss_mb() -> u64 {
            let pid = std::process::id().to_string();
            let out = std::process::Command::new("ps")
                .args(["-o", "rss=", "-p", &pid])
                .output()
                .expect("ps");
            String::from_utf8_lossy(&out.stdout)
                .trim()
                .parse::<u64>()
                .unwrap_or(0)
                / 1024
        }
        // Missing context files are deterministically reconstructed from shape.
        let dir = std::env::var_os("AI_POW_SETUP_GENERATION_DIR")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| std::env::temp_dir().join("aipow-rss-cache"));
        let src_cache = crate::setup::verifier_setup_descriptor_cache_path(&dir);
        if !src_cache.exists() {
            eprintln!("skip: no shape cache at {}", src_cache.display());
            return;
        }
        // Detect whether this is a build-boot or a reuse-boot (context files present).
        let descriptors =
            crate::setup::load_verifier_setup_descriptors(&src_cache).expect("load descriptors");
        let reuse = descriptors.iter().all(|s| {
            let key = VerifierSetupShapeKey::from_zk_params(&s.zk_params, s.trace_height())
                .expect("valid descriptor key")
                .for_rules(s.rules);
            crate::setup::verifier_context_file_path(&dir, key, &s.verifier_key_digest_bytes)
                .exists()
        });
        let keys: Vec<_> = descriptors
            .iter()
            .map(|s| {
                VerifierSetupShapeKey::from_zk_params(&s.zk_params, s.trace_height())
                    .expect("valid descriptor key")
                    .for_rules(s.rules)
            })
            .collect();
        let expected_cap = 2usize;
        drop(descriptors);
        std::env::set_var(
            crate::setup::AI_POW_VERIFIER_CACHE_CAP_ENV,
            expected_cap.to_string(),
        );
        let base = rss_mb();
        // Production boot: build every production context to disk (first run) or reuse
        // them (later runs) + inject disk-paged.
        let n = crate::setup::install_or_build_verifier_setup(&dir, &[])
            .expect("install_or_build (disk-paged)");
        let buckets = crate::setup::production_verifier_setup_buckets();
        assert_eq!(n, 2 * buckets.len(), "both rule versions installed");
        assert_eq!(n, keys.len());
        let after_boot = rss_mb();
        eprintln!(
            "boot mode: {}; after boot (contexts on disk, none paged in): RSS {after_boot} MB (base {base})",
            if reuse {
                "REUSE (files existed)"
            } else {
                "BUILD (first run)"
            },
        );

        // Page every key in, with cap=2 — RSS must stay ~2 contexts, not the full table.
        for key in keys {
            let setup = expect_found(crate::ai_pow_verifier_setup_for(key));
            assert_eq!(setup.shape_key(), key);
        }
        let after_paging = rss_mb();
        eprintln!(
            "after paging all contexts (cap={expected_cap}): RSS {after_paging} MB — bounded to \
             ~{expected_cap} resident contexts",
        );
        assert!(
            after_paging < 6000,
            "disk-paged RSS with cap=2 must stay well under the all-resident table \
             (got {after_paging} MB)",
        );
    }

    /// Diagnostic: record resident memory during repeated context page-in and
    /// eviction under the active allocator.
    /// Requires a built 2^19 context file; skips otherwise.
    #[test]
    #[ignore = "pages a big context in/out repeatedly to check that eviction reclaims RSS; opt-in"]
    fn eviction_reclaims_rss_probe() {
        fn rss_mb() -> u64 {
            let pid = std::process::id().to_string();
            let out = std::process::Command::new("ps")
                .args(["-o", "rss=", "-p", &pid])
                .output()
                .expect("ps");
            String::from_utf8_lossy(&out.stdout)
                .trim()
                .parse::<u64>()
                .unwrap_or(0)
                / 1024
        }
        let dir = std::env::var_os("AI_POW_SETUP_GENERATION_DIR")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| std::env::temp_dir().join("aipow-rss-cache"));
        let cache = crate::setup::verifier_setup_descriptor_cache_path(&dir);
        if !cache.exists() {
            eprintln!("skip: no stable shape cache");
            return;
        }
        let mut descriptors =
            crate::setup::load_verifier_setup_descriptors(&cache).expect("load descriptors");
        descriptors.sort_by_key(|s| s.trace_height());
        let big = descriptors.pop().expect("a descriptor");
        let big_key = VerifierSetupShapeKey::from_zk_params(&big.zk_params, big.trace_height())
            .expect("valid descriptor key")
            .for_rules(big.rules);
        let ctx_path =
            crate::setup::verifier_context_file_path(&dir, big_key, &big.verifier_key_digest_bytes);
        if !ctx_path.exists() {
            eprintln!(
                "skip: no built 2^19 context file at {} (run the boot+RSS test first)",
                ctx_path.display()
            );
            return;
        }
        let base = rss_mb();
        eprintln!("base RSS {base} MB; paging 2^19 in and out 4x:");
        let mut peak_held = 0u64;
        for i in 0..4 {
            let held = {
                let raw = std::fs::read(&ctx_path).expect("read ctx");
                let (setup, _): (AiPowVerifierSetup, _) =
                    bincode::serde::decode_from_slice(&raw, bincode::config::standard())
                        .expect("decode ctx");
                let h = rss_mb();
                std::hint::black_box(&setup);
                h
                // setup + raw dropped here (eviction)
            };
            peak_held = peak_held.max(held);
            let after_drop = rss_mb();
            eprintln!(
                "  cycle {i}: held {held} MB (+{} over base), after drop {after_drop} MB",
                held.saturating_sub(base)
            );
        }
        let end = rss_mb();
        eprintln!(
            "END RSS {end} MB. If ~base ({base}) the drop reclaims; if ~peak_held ({peak_held}) the \
             allocator retains (needs jemalloc decay). context ~{} MB.",
            peak_held.saturating_sub(base),
        );
    }

    /// A complete shape cache must match both committed table fingerprints.
    #[test]
    #[ignore = "requires a generated production shape cache"]
    fn stable_shape_cache_matches_committed_table_fingerprints() {
        let dir = std::env::var_os("AI_POW_SETUP_GENERATION_DIR")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| std::env::temp_dir().join("aipow-rss-cache"));
        let path = crate::setup::verifier_setup_descriptor_cache_path(&dir);
        if !path.exists() {
            eprintln!("skip: no stable shape cache at {}", path.display());
            return;
        }
        let descriptors =
            crate::setup::load_verifier_setup_descriptors(&path).expect("load descriptors");
        assert_eq!(
            setup_descriptor_keys(&descriptors),
            production_setup_descriptor_keys(),
            "full production versioned shape-key cache",
        );
        crate::table_digest::verify_verifier_setup_descriptor_table_digest(&descriptors)
            .expect("stable shape cache must match both committed fingerprints");
        eprintln!("stable shape cache matches both committed table fingerprints");
    }

    /// KAT (real proving, ~25s): the ACCEPTANCE path with the block commitment
    /// derived exactly as the jet derives it in consensus — `commit_from_noun`
    /// (BLAKE3 of the nockvm jam) of a realistic block-commitment noun (a tip5
    /// 5-belt digest), NOT an arbitrary 32-byte constant. We prove a real cert
    /// against that noun-derived commit and confirm the jet-core ACCEPTS it, then
    /// confirm a different commitment noun (⇒ different commit) is rejected. This
    /// closes the `block-commitment noun → commit_from_noun → prove → verify=%.y`
    /// loop that the live +check-pow path exercises, with real proving — the
    /// acceptance-direction analog of `commit_from_noun_matches_miner_derivation`.
    #[test]
    #[ignore = "real MoE compact proof (~25s); opt-in"]
    fn jet_commit_from_noun_seeds_a_cert_the_core_accepts() {
        use nockvm::mem::NockStack;
        use nockvm::noun::{D, T};

        let params = MatmulParams {
            m: 64,
            k: 1024,
            n: 64,
            noise_rank: 64,
            tile: 8,
            spot_checks: 1,
            difficulty_bits: 0,
        };
        let target = AI_POW_MAX_CONSENSUS_TARGET;
        let work_factor =
            shape_work_factor_for(8, 8, params.k, params.noise_rank).expect("valid shape");

        // A realistic block-commitment noun: a tip5 noun-digest is 5 belts. The
        // first belt is a deterministic counter so this KAT proves a ticket that
        // clears the consensus-minable target.
        let mut stack = NockStack::new(8 << 20, 0);
        let commit = (0u64..4096)
            .find_map(|counter| {
                let commit_noun = T(
                    &mut stack,
                    &[
                        D(counter),
                        D(0x1122_3344_5566_7788),
                        D(0x2233_4455_6677_8899),
                        D(0x3344_5566_7788_99aa),
                        D(0x4455_6677_8899_aabb),
                    ],
                );
                let commit = commit_from_noun(&mut stack, commit_noun);
                let jackpot = evaluate_reference_moe_jackpot(&params, 8, 2, 1, commit, 0)
                    .expect("evaluate noun-derived canonical ticket");
                attempt_wins(&jackpot, &target, work_factor)
                    .expect("max consensus target is minable")
                    .then_some(commit)
            })
            .expect("noun-derived commitment search must find a target-winning ticket");

        // Prove a real cert bound to that noun-derived commit (the miner's job).
        let block = prove_reference_moe_block(&params, 8, 2, 1, commit)
            .expect("prove canonical MoE block for the noun-derived commit");
        assert_eq!(
            block.commit, commit,
            "the proved cert must commit to the jet-derived commitment",
        );

        let jammed = build_ai_pow_pearl_merge_moe_artifact_noun_from_node(
            &block.statement, &block.aux_inclusion, &block.moe_art, &block.certificate.zk_params,
            block.certificate.found_idx, block.certificate.trace_height,
            &block.certificate.commitments, &block.certificate.public_inputs,
            &block.certificate.certificate,
        )
        .expect("build MoE artifact noun")
        .jam();

        let digest_bytes = ai_pow_zk::recursion::compact_batch_verifier_key_digest_to_bytes(
            &block.run.verifier_key_digest(),
        )
        .to_vec();
        let setup_key = VerifierSetupShapeKey::from_zk_params(
            &block.certificate.zk_params, block.run.trace_height,
        )
        .expect("valid setup key");
        let setup = AiPowVerifierSetup {
            trace_height: setup_key.trace_height,
            sx_bound: setup_key.sx_bound,
            context: block.run.verifier_context,
            digest_bytes,
        };

        let slab = cue_artifact(jammed);
        let space = slab.noun_space();
        let root = unsafe { *slab.root() };
        let artifact =
            decode_ai_pow_pearl_merge_artifact_noun(root, &space, CertificateNounLimits::default())
                .expect("decode artifact noun");

        // ACCEPT: the jet-derived commit matches the cert's commitment.
        assert!(
            matches!(
                ai_pow_verify_core(&artifact, commit, target, &setup),
                Ok(true)
            ),
            "real block must verify when the commit is derived from its commitment noun",
        );
        // REJECT: a different commitment noun yields a different commit.
        let mut stack2 = NockStack::new(8 << 20, 0);
        let other_noun = T(&mut stack2, &[D(1), D(2), D(3), D(4), D(5)]);
        let other_commit = commit_from_noun(&mut stack2, other_noun);
        assert_ne!(other_commit, commit, "distinct nouns ⇒ distinct commits");
        assert!(
            matches!(
                ai_pow_verify_core(&artifact, other_commit, target, &setup),
                Ok(false)
            ),
            "a block committed to a different noun must be rejected",
        );
    }

    /// A shape-only cache reconstructs the key used by a genuine certificate.
    #[test]
    #[ignore = "real MoE compact proof + rebuild (~30s); opt-in"]
    fn moe_verifier_setup_descriptor_roundtrip_rebuilds_working_setup() {
        use crate::setup::rebuild_verifier_setup_from_descriptor;

        let (block, target) = target_hitting_reference_moe_block();
        let commit = block.commit;

        let descriptor = reference_block_descriptor(&block);
        let descriptor_bytes =
            bincode::serde::encode_to_vec(&descriptor, bincode::config::standard())
                .expect("serialize shape descriptor");
        assert!(
            descriptor_bytes.len() < 256,
            "cache must contain geometry, not proofs"
        );

        // Build the block artifact noun (uses the freshly-proved cert; unchanged).
        let jammed = build_ai_pow_pearl_merge_moe_artifact_noun_from_node(
            &block.statement, &block.aux_inclusion, &block.moe_art, &block.certificate.zk_params,
            block.certificate.found_idx, block.certificate.trace_height,
            &block.certificate.commitments, &block.certificate.public_inputs,
            &block.certificate.certificate,
        )
        .expect("build MoE artifact noun")
        .jam();

        let (descriptor2, consumed): (
            ai_pow_zk::recursion::AiPowCompactVerifierSetupDescriptor,
            _,
        ) = bincode::serde::decode_from_slice(&descriptor_bytes, bincode::config::standard())
            .expect("deserialize shape descriptor");
        assert_eq!(consumed, descriptor_bytes.len());
        let setup = rebuild_verifier_setup_from_descriptor(descriptor2)
            .expect("derive context from shape descriptor");
        assert_eq!(
            setup.trace_height, block.run.trace_height,
            "rebuilt setup trace height matches the proved cert",
        );
        let proved_digest = ai_pow_zk::recursion::compact_batch_verifier_key_digest_to_bytes(
            &block.run.verifier_key_digest(),
        )
        .to_vec();
        assert_eq!(
            setup.digest_bytes, proved_digest,
            "rebuilt setup digest matches the proved cert digest",
        );

        let slab = cue_artifact(jammed);
        let space = slab.noun_space();
        let root = unsafe { *slab.root() };
        let mut artifact =
            decode_ai_pow_pearl_merge_artifact_noun(root, &space, CertificateNounLimits::default())
                .expect("decode artifact noun");

        assert!(
            matches!(
                ai_pow_verify_core(&artifact, commit, target, &setup),
                Ok(true)
            ),
            "real MoE block must verify against the shape-derived setup",
        );
        assert!(
            matches!(
                ai_pow_verify_core(&artifact, [0x99u8; 32], target, &setup),
                Ok(false)
            ),
            "wrong block commitment must still be rejected against the rebuilt setup",
        );

        // Round-trip the same shape descriptor through the disk envelope.
        let tmp_data_dir =
            std::env::temp_dir().join(format!("ai-pow-jets-shapecache-{}", std::process::id()));
        let cache_path = crate::setup::verifier_setup_descriptor_cache_path(&tmp_data_dir);
        crate::setup::save_verifier_setup_descriptors(
            &cache_path,
            std::slice::from_ref(&descriptor),
        )
        .expect("save descriptor cache");
        let table = crate::setup::load_verifier_setup_table(&cache_path)
            .expect("load and derive shape table");
        let _ = std::fs::remove_dir_all(&tmp_data_dir);
        assert_eq!(table.len(), 1, "one-bucket table loaded from disk");
        assert_eq!(
            table[0].trace_height, block.run.trace_height,
            "disk-loaded setup trace height matches the proved cert",
        );
        assert!(
            matches!(
                ai_pow_verify_core(&artifact, commit, target, &table[0]),
                Ok(true)
            ),
            "real MoE block must verify against the DISK-loaded rebuilt setup",
        );

        // Consensus cap: bumping the claimed Layer-0 trace height above
        // AI_POW_MAX_TRACE_HEIGHT (2^19) makes even this otherwise-valid block
        // reject — the accept-band is capped and the top (2^20) setup is not built.
        artifact.certificate.trace_height = ai_pow::params::AI_POW_MAX_TRACE_HEIGHT + 1;
        assert!(
            matches!(
                ai_pow_verify_core(&artifact, commit, target, &setup),
                Ok(false)
            ),
            "a block claiming trace_height above the consensus cap must be rejected",
        );
    }

    /// Derivation and shape-cache round-trip preserve each rule's native geometry.
    #[test]
    fn boot_generate_and_cache_one_bucket_roundtrips() {
        let buckets = crate::setup::production_verifier_setup_buckets();
        let shape = *buckets.first().expect("at least one production bucket");
        let expected_h = if shape.dense {
            let schedule = ai_pow_zk::canonical::StripIndexSchedule {
                a_indices: (0..shape.params.tile).collect(),
                b_indices: (0..shape.params.tile).collect(),
            };
            ai_pow::zk_bridge::expected_layer0_rows_for_strip_schedule(&shape.params, &schedule)
                .expect("dense bucket trace height")
                .required_trace_len()
        } else {
            crate::setup::reference_moe_trace_height(&shape.params, shape.hw, shape.e, shape.top_k)
                .expect("cheap predicted height")
        };

        let tmp = std::env::temp_dir().join(format!("ai-pow-genboot-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        let path = crate::setup::verifier_setup_descriptor_cache_path(&tmp);

        // Derive both rule versions of one canonical geometry.
        crate::setup::build_and_cache_verifier_setup_descriptors(&path, &[shape])
            .expect("generate + cache one bucket");
        assert!(
            path.exists(),
            "cache file must be written under the data dir"
        );

        let cached_bytes = std::fs::read(&path).expect("read checksummed cache");
        let mut corrupt_bytes = cached_bytes.clone();
        *corrupt_bytes.last_mut().expect("nonempty cache") ^= 0x01;
        std::fs::write(&path, corrupt_bytes).expect("corrupt cache");
        assert!(
            crate::setup::load_verifier_setup_descriptors(&path).is_err(),
            "the envelope must reject a changed payload before descriptor decoding",
        );
        let mut incompatible = cached_bytes.clone();
        incompatible[..8].copy_from_slice(b"NCVPSEED");
        std::fs::write(&path, incompatible).unwrap();
        assert!(crate::setup::load_verifier_setup_descriptors(&path).is_err());
        let mut incompatible = cached_bytes.clone();
        incompatible[8..12].copy_from_slice(&2u32.to_le_bytes());
        std::fs::write(&path, incompatible).unwrap();
        assert!(crate::setup::load_verifier_setup_descriptors(&path).is_err());
        std::fs::write(&path, &cached_bytes).expect("restore checksummed cache");
        let mut descriptors = crate::setup::load_verifier_setup_descriptors(&path).unwrap();
        descriptors[0].verifier_key_digest_bytes[0] ^= 1;
        let changed = descriptors.remove(0);
        assert!(
            changed.derive_context().is_err(),
            "reject a noncanonical cached key"
        );
        let original_limb: [u8; 8] = descriptors[0].verifier_key_digest_bytes[..8]
            .try_into()
            .unwrap();
        descriptors[0].verifier_key_digest_bytes[..8].copy_from_slice(&u64::MAX.to_le_bytes());
        crate::setup::save_verifier_setup_descriptors(&path, &descriptors).unwrap();
        assert!(
            crate::setup::load_verifier_setup_descriptors(&path).is_err(),
            "reject noncanonical field limbs"
        );
        descriptors[0].verifier_key_digest_bytes[..8].copy_from_slice(&original_limb);
        let original_height = descriptors[0].trace_height;
        descriptors[0].trace_height = 1 << 20;
        crate::setup::save_verifier_setup_descriptors(&path, &descriptors).unwrap();
        assert!(
            crate::setup::load_verifier_setup_descriptors(&path).is_err(),
            "reject unsupported trace degrees"
        );
        descriptors[0].trace_height = original_height;
        let original_params = descriptors[0].zk_params;
        descriptors[0].zk_params.m = 1 << 30;
        descriptors[0].zk_params.n = 1 << 30;
        descriptors[0].zk_params.tile = 1 << 30;
        crate::setup::save_verifier_setup_descriptors(&path, &descriptors).unwrap();
        assert!(
            crate::setup::load_verifier_setup_descriptors(&path).is_err(),
            "reject overflowing schedule geometry"
        );
        descriptors[0].zk_params = original_params;
        descriptors[0].zk_params.noise_rank = 0;
        crate::setup::save_verifier_setup_descriptors(&path, &descriptors).unwrap();
        assert!(
            crate::setup::load_verifier_setup_descriptors(&path).is_err(),
            "reject invalid cached geometry"
        );
        std::fs::write(&path, cached_bytes).expect("restore valid descriptors");

        // Reconstruct from the decoded shape descriptors.
        let table = crate::setup::load_verifier_setup_table(&path).expect("load + rebuild table");
        let _ = std::fs::remove_dir_all(&tmp);
        assert_eq!(table.len(), 2, "both rule versions of one shape");
        assert_eq!(
            table[0].trace_height, expected_h,
            "generated+rebuilt bucket lands at the matrix-free predicted height",
        );
    }

    /// Repeated canonical derivation yields identical shape-cache bytes.
    #[test]
    fn verifier_setup_descriptor_bytes_are_deterministic() {
        let buckets = crate::setup::production_verifier_setup_buckets();
        let shape = *buckets.first().expect("at least one production bucket");
        let build_descriptor = || {
            if shape.dense {
                crate::setup::build_verifier_setup_descriptor_dense(&shape.params)
            } else {
                build_verifier_setup_descriptor(&shape.params, shape.hw, shape.e, shape.top_k)
            }
        };
        let first = build_descriptor().expect("first canonical descriptor");
        let second = build_descriptor().expect("second canonical descriptor");
        let first_bytes = bincode::serde::encode_to_vec(&first, bincode::config::standard())
            .expect("serialize first descriptor");
        let second_bytes = bincode::serde::encode_to_vec(&second, bincode::config::standard())
            .expect("serialize second descriptor");
        assert_eq!(
            first_bytes, second_bytes,
            "canonical shape descriptor bytes"
        );
    }

    /// Diagnostic: print every production bucket shape (cheap, no proving).
    #[test]
    fn print_production_bucket_shapes() {
        for b in crate::setup::production_verifier_setup_buckets() {
            let th = if b.dense {
                let schedule = ai_pow_zk::canonical::StripIndexSchedule {
                    a_indices: (0..b.params.tile).collect(),
                    b_indices: (0..b.params.tile).collect(),
                };
                ai_pow::zk_bridge::expected_layer0_rows_for_strip_schedule(&b.params, &schedule)
                    .unwrap()
                    .required_trace_len()
            } else {
                crate::setup::reference_moe_trace_height(&b.params, b.hw, b.e, b.top_k).unwrap()
            };
            eprintln!(
                "2^{}: m={} k={} n={} r={} tile={} | dense={} hw={} e={} top_k={} ns={}",
                th.trailing_zeros(),
                b.params.m,
                b.params.k,
                b.params.n,
                b.params.noise_rank,
                b.params.tile,
                b.dense,
                b.hw,
                b.e,
                b.top_k,
                b.params.k / b.params.noise_rank,
            );
        }
    }

    /// Production setup derives all 28 native keys and preserves both fingerprints.
    #[test]
    fn boot_generate_full_production_table() {
        use crate::setup::*;
        use crate::table_digest::*;
        let buckets = production_verifier_setup_buckets();
        assert_eq!(buckets.len(), 14);
        let dir = std::env::var_os("AI_POW_SETUP_GENERATION_DIR")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| {
                std::env::temp_dir().join(format!("ai-pow-fulltable-{}", std::process::id()))
            });
        let path = verifier_setup_descriptor_cache_path(&dir);
        build_and_cache_verifier_setup_descriptors(&path, &buckets).expect("production setup");
        let mut descriptors =
            load_verifier_setup_descriptors(&path).expect("round-trip shape table");
        assert_eq!(descriptors.len(), 28);
        assert_eq!(
            legacy_verifier_setup_descriptor_table_digest(&descriptors)
                .expect("historical fingerprint"),
            AI_POW_V0_VERIFIER_SETUP_TABLE_DIGEST,
        );
        assert_eq!(
            verify_verifier_setup_descriptor_table_digest(&descriptors).expect("both fingerprints"),
            AI_POW_V1_VERIFIER_SETUP_TABLE_DIGEST,
        );
        let original_m = descriptors[0].zk_params.m;
        descriptors[0].zk_params.m += 1;
        assert!(
            verify_verifier_setup_descriptor_table_digest(&descriptors).is_err(),
            "cached key digests cannot authorize noncanonical production parameters",
        );
        descriptors[0].zk_params.m = original_m;
        let mut keys = std::collections::BTreeSet::new();
        let mut context_bytes = 0usize;
        let mut native_fingerprints = Vec::with_capacity(28);
        for descriptor in descriptors {
            let key = VerifierSetupShapeKey::from_zk_params(
                &descriptor.zk_params,
                descriptor.trace_height(),
            )
            .unwrap()
            .for_rules(descriptor.rules);
            let context_path =
                verifier_context_file_path(&dir, key, &descriptor.verifier_key_digest_bytes);
            let checksum = context_file_checksum(&context_path).expect("native file checksum");
            let bucket = DiskBucket::new(
                key, descriptor.verifier_key_digest_bytes, context_path, checksum,
            );
            let setup = crate::page_in_bucket(&bucket).expect("native page-in");
            let binding = setup
                .context
                .validate_setup_binding()
                .expect("native context binding");
            assert_eq!(
                bucket.committed_digest,
                ai_pow_zk::recursion::compact_batch_verifier_key_digest_to_bytes(&binding),
            );
            native_fingerprints.push((
                setup.shape_key(),
                ai_pow_zk::recursion::compact_batch_verifier_key_digest_to_bytes(&binding),
            ));
            assert!(keys.insert(setup.shape_key()), "duplicate versioned shape");
            let encoded = bincode::serde::encode_to_vec(&setup, bincode::config::standard())
                .expect("serialize native setup");
            assert!(
                encoded.len() < 64 * 1024,
                "prover tree must not be retained"
            );
            context_bytes += encoded.len();
            let (decoded, used): (AiPowVerifierSetup, usize) =
                bincode::serde::decode_from_slice(&encoded, bincode::config::standard())
                    .expect("deserialize native setup");
            assert_eq!(used, encoded.len());
            assert_eq!(decoded.shape_key(), key);
            assert_eq!(decoded.context.validate_setup_binding().unwrap(), binding);
            eprintln!("NATIVE KEY {key:?}: {:?}", setup.digest_bytes);
        }
        assert_eq!(
            verify_production_fingerprints(&native_fingerprints).expect("all native fingerprints"),
            AI_POW_V1_VERIFIER_SETUP_TABLE_DIGEST,
        );
        for rules in [ProofRules::Legacy, ProofRules::Hardened] {
            for log2 in 13..=19 {
                for sx in [false, true] {
                    assert!(keys.contains(&VerifierSetupShapeKey::with_rules(
                        1 << log2,
                        sx,
                        rules
                    )));
                }
            }
        }
        eprintln!("ALL 28 VERIFIER CONTEXTS: {context_bytes} serialized bytes");
    }

    fn production_setup_descriptor_keys() -> std::collections::BTreeSet<VerifierSetupShapeKey> {
        crate::setup::production_verifier_setup_buckets()
            .iter()
            .flat_map(|bucket| {
                let (trace_height, sx_bound) = if bucket.dense {
                    let schedule = ai_pow_zk::canonical::StripIndexSchedule {
                        a_indices: (0..bucket.params.tile).collect(),
                        b_indices: (0..bucket.params.tile).collect(),
                    };
                    let budget = ai_pow::zk_bridge::expected_layer0_rows_for_strip_schedule(
                        &bucket.params, &schedule,
                    )
                    .expect("dense bucket trace height");
                    (
                        budget.required_trace_len(),
                        (bucket.params.num_stripes() as usize) <= ai_pow::params::STRIPE_MAX,
                    )
                } else {
                    let height = crate::setup::reference_moe_trace_height(
                        &bucket.params, bucket.hw, bucket.e, bucket.top_k,
                    )
                    .expect("production setup bucket trace height");
                    (
                        height,
                        (bucket.params.k / bucket.params.noise_rank) as usize
                            <= ai_pow::params::STRIPE_MAX,
                    )
                };
                [ProofRules::Legacy, ProofRules::Hardened]
                    .map(|rules| VerifierSetupShapeKey::with_rules(trace_height, sx_bound, rules))
            })
            .collect()
    }

    fn setup_descriptor_keys(
        descriptors: &[ai_pow_zk::recursion::AiPowCompactVerifierSetupDescriptor],
    ) -> std::collections::BTreeSet<VerifierSetupShapeKey> {
        descriptors
            .iter()
            .map(|descriptor| {
                VerifierSetupShapeKey::from_zk_params(
                    &descriptor.zk_params,
                    descriptor.trace_height(),
                )
                .expect("cached setup descriptor has a valid shape key")
                .for_rules(descriptor.rules)
            })
            .collect()
    }

    fn load_stable_production_setup_descriptors(
        path: &std::path::Path,
    ) -> Vec<ai_pow_zk::recursion::AiPowCompactVerifierSetupDescriptor> {
        let expected = production_setup_descriptor_keys();
        let buckets = crate::setup::production_verifier_setup_buckets();
        if path.exists() {
            match crate::setup::load_verifier_setup_descriptors(path) {
                Ok(descriptors) => {
                    let actual = setup_descriptor_keys(&descriptors);
                    if actual == expected {
                        match crate::table_digest::verify_verifier_setup_descriptor_table_digest(
                            &descriptors,
                        ) {
                            Ok(_) => return descriptors,
                            Err(e) => eprintln!(
                                "shape cache digest does not match production ({e}); \
                                 regenerating {}",
                                path.display(),
                            ),
                        }
                    } else {
                        eprintln!(
                            "shape cache keys do not match production; \
                             cache={actual:?} production={expected:?}; regenerating {}",
                            path.display(),
                        );
                    }
                }
                Err(e) => {
                    eprintln!(
                        "shape cache at {} is unreadable ({e}); regenerating",
                        path.display(),
                    );
                }
            }
        }
        eprintln!(
            "deriving production shape cache at {} ({} buckets)",
            path.display(),
            buckets.len(),
        );
        crate::setup::build_and_cache_verifier_setup_descriptors(path, &buckets)
            .expect("derive production shape cache");
        let descriptors =
            crate::setup::load_verifier_setup_descriptors(path).expect("load descriptors");
        assert_eq!(
            setup_descriptor_keys(&descriptors),
            expected,
            "generated stable setup cache must match current production shape keys",
        );
        descriptors
    }

    /// RSS + rebuild-latency MEASUREMENT for the boot verifier-setup table.
    ///
    /// Reports (a) the resident memory each rebuilt bucket context costs and the
    /// total standing RSS of the production shape-key table, and (b) the wall-clock
    /// to rebuild each bucket from its cached descriptor — the per-height latency an
    /// on-demand (rebuild-per-verify) design would pay. The stable measurement cache
    /// is regenerated when its shape-key set is not the current production set. Run with:
    ///   cargo test -p ai-pow-jets --lib measure_verifier_setup_table_rss -- --ignored --nocapture
    /// Wrap the test binary in `/usr/bin/time -l` to also capture PEAK RSS (the
    /// rebuild transient, which exceeds the steady resident set).
    #[test]
    #[ignore = "generates the current production table if the stable cache is stale; opt-in"]
    fn measure_verifier_setup_table_rss() {
        use std::time::Instant;
        // Current RSS in MiB via `ps` (macOS/Linux report rss in KiB).
        fn rss_mb() -> u64 {
            let pid = std::process::id().to_string();
            let out = std::process::Command::new("ps")
                .args(["-o", "rss=", "-p", &pid])
                .output()
                .expect("ps");
            String::from_utf8_lossy(&out.stdout)
                .trim()
                .parse::<u64>()
                .unwrap_or(0)
                / 1024
        }
        let dir = std::env::var_os("AI_POW_SETUP_GENERATION_DIR")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| std::env::temp_dir().join("aipow-rss-cache"));
        let path = crate::setup::verifier_setup_descriptor_cache_path(&dir);
        let descriptors = load_stable_production_setup_descriptors(&path);
        let cache_mb =
            std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0) as f64 / (1024.0 * 1024.0);
        let base = rss_mb();
        eprintln!(
            "cache {cache_mb:.1} MiB on disk; baseline RSS after loading descriptors = {base} MB; \
             rebuilding {} buckets:",
            descriptors.len(),
        );
        let mut table = Vec::new();
        let mut prev = base;
        let mut total_rebuild_ms = 0u128;
        for descriptor in descriptors {
            let h = (descriptor.trace_height() as u64).trailing_zeros();
            let t = Instant::now();
            let setup =
                crate::setup::rebuild_verifier_setup_from_descriptor(descriptor).expect("rebuild");
            let ms = t.elapsed().as_millis();
            total_rebuild_ms += ms;
            table.push(setup);
            let now = rss_mb();
            eprintln!(
                "  2^{h:>2}: +{:>5} MB resident  ({now:>6} MB total)  rebuilt in {ms:>6} ms",
                now.saturating_sub(prev),
            );
            prev = now;
        }
        let total = rss_mb();
        eprintln!(
            "TOTAL: {} buckets, standing table RSS = {total} MB (delta {} MB over baseline); \
             full-table rebuild = {total_rebuild_ms} ms",
            table.len(),
            total.saturating_sub(base),
        );
        // CONSENSUS INVARIANCE: dropping the prove-only raw columns must NOT change the
        // committed v1 table digest (the digest is over the verifier-key, not the
        // columns). Re-verify against the pinned constant on the slimmed table.
        let table_keys: std::collections::BTreeSet<_> =
            table.iter().map(|setup| setup.shape_key()).collect();
        if table_keys == production_setup_descriptor_keys()
            && crate::table_digest::v1_digest_is_pinned()
        {
            let digest =
                crate::table_digest::verifier_setup_table_digest(&table).expect("table digest");
            assert_eq!(
                digest,
                crate::table_digest::AI_POW_V1_VERIFIER_SETUP_TABLE_DIGEST,
                "slimmed (verifier-only) table digest must equal the pinned v1 constant",
            );
            eprintln!("v1 table digest UNCHANGED by slimming ✓");
        }
        std::hint::black_box(&table);
    }

    /// DE-RISK for disk-paged residency: how fast is "page in from disk" — a
    /// read+deserialize of a PREBUILT context — versus the ~10 s circuit rebuild?
    /// Rebuild the largest production bucket (worst case), serialize it to a file,
    /// drop it, then time reading + deserializing it back. The stable measurement
    /// cache is regenerated when its shape-key set is not the current production set.
    #[test]
    #[ignore = "rebuilds the largest bucket, then times serialize/read/deserialize; opt-in"]
    fn measure_context_page_in_latency() {
        use std::time::Instant;
        let dir = std::env::var_os("AI_POW_SETUP_GENERATION_DIR")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| std::env::temp_dir().join("aipow-rss-cache"));
        let path = crate::setup::verifier_setup_descriptor_cache_path(&dir);
        let mut descriptors = load_stable_production_setup_descriptors(&path);
        descriptors.sort_by_key(|s| s.trace_height());
        let biggest = descriptors.pop().expect("at least one seed");
        let h = (biggest.trace_height() as u64).trailing_zeros();

        let t = Instant::now();
        let setup = crate::setup::rebuild_verifier_setup_from_descriptor(biggest).expect("rebuild");
        eprintln!(
            "REBUILD 2^{h}: {} ms (the latency we must AVOID on the verify path)",
            t.elapsed().as_millis()
        );

        let t = Instant::now();
        let bytes = bincode::serde::encode_to_vec(&setup, bincode::config::standard())
            .expect("serialize context");
        let mb = bytes.len() as f64 / (1024.0 * 1024.0);
        eprintln!(
            "serialize 2^{h}: {mb:.0} MB in {} ms",
            t.elapsed().as_millis()
        );
        let file = dir.join("ctx-page-in-probe.bin");
        std::fs::write(&file, &bytes).expect("write context file");
        drop(setup);
        drop(bytes);

        // Page-in from disk: read + deserialize the prebuilt context.
        let t = Instant::now();
        let raw = std::fs::read(&file).expect("read context file");
        let read_ms = t.elapsed().as_millis();
        let t = Instant::now();
        let (setup2, _): (AiPowVerifierSetup, _) =
            bincode::serde::decode_from_slice(&raw, bincode::config::standard())
                .expect("deserialize context");
        let de_ms = t.elapsed().as_millis();
        eprintln!(
            "PAGE-IN 2^{h}: read {read_ms} ms + deserialize {de_ms} ms = {} ms total (vs the ~10 s rebuild)",
            read_ms + de_ms,
        );
        std::hint::black_box(&setup2);
        let _ = std::fs::remove_file(&file);
    }

    /// **DE-RISK — is the compact verifier setup shape-DEPENDENT?**
    /// Pearl admits a BAND of puzzle shapes; nockchain must verify all of them.
    /// A single embedded boot setup only suffices if the verifier-key digest is
    /// INVARIANT across shapes. This builds setups at several distinct shapes
    /// (varying k / hw / m,n — the axes that drive L0 trace height) and prints
    /// each digest, then asserts nothing (observational) so the run always shows
    /// the full table. If every digest is equal ⇒ one setup covers the band; if
    /// they differ ⇒ we need a per-shape setup table (or fixed-height padding).
    #[test]
    #[ignore = "builds several real compact proofs (~2-4 min); opt-in diagnostic"]
    fn digest_shape_dependence_probe() {
        use crate::setup::build_verifier_setup;
        // MoE routing needs m/e >= hw and n/e >= hw (each expert must have >= hw
        // rows/cols to fill the opened tile), so base at m=n=16, e=2, hw=8.
        let base = MatmulParams {
            m: 16,
            k: 1024,
            n: 16,
            noise_rank: 64,
            tile: 8,
            spot_checks: 1,
            difficulty_bits: 0,
        };
        // (label, params, hw, e, top_k). num_stripes = k/noise_rank; the pinned AIR
        // caps it at STRIPE_MAX=64. Span the band from num_stripes=8 to the max 64,
        // across k / rank / hw / m,n, to confirm ONE digest covers nockchain's whole
        // accept-band.
        let shapes: [(&str, MatmulParams, u32, usize, usize); 7] = [
            ("stripes16 base m16 k1024 r64 hw8", base, 8, 2, 1),
            (
                "stripes8 k512 r64",
                MatmulParams { k: 512, ..base },
                8,
                2,
                1,
            ),
            (
                "stripes16 k512 r32",
                MatmulParams {
                    k: 512,
                    noise_rank: 32,
                    ..base
                },
                8,
                2,
                1,
            ),
            (
                "stripes32 k2048 r64",
                MatmulParams { k: 2048, ..base },
                8,
                2,
                1,
            ),
            (
                "stripes64 k2048 r32 (MAX)",
                MatmulParams {
                    k: 2048,
                    noise_rank: 32,
                    ..base
                },
                8,
                2,
                1,
            ),
            (
                "stripes64 k4096 r64 (MAX)",
                MatmulParams { k: 4096, ..base },
                8,
                2,
                1,
            ),
            (
                "hw16 m32 n32",
                MatmulParams {
                    m: 32,
                    n: 32,
                    ..base
                },
                16,
                2,
                1,
            ),
        ];
        let mut digests = Vec::new();
        for (label, params, hw, e, top_k) in shapes {
            match build_verifier_setup(&params, hw, e, top_k) {
                Ok(setup) => {
                    let hex: String = setup
                        .digest_bytes
                        .iter()
                        .map(|b| format!("{b:02x}"))
                        .collect();
                    eprintln!("SHAPE-DIGEST [{label}] = {hex}");
                    digests.push((label, Some(hex)));
                }
                Err(e) => {
                    eprintln!("SHAPE-DIGEST [{label}] = BUILD-ERROR: {e}");
                    digests.push((label, None));
                }
            }
        }
        let distinct: std::collections::BTreeSet<_> =
            digests.iter().filter_map(|(_, d)| d.clone()).collect();
        eprintln!(
            "SHAPE-DIGEST SUMMARY: {} shapes built, {} DISTINCT digest(s) ⇒ {}",
            digests.iter().filter(|(_, d)| d.is_some()).count(),
            distinct.len(),
            if distinct.len() <= 1 {
                "SHAPE-INDEPENDENT (one setup covers all)"
            } else {
                "SHAPE-DEPENDENT (need a per-shape setup table)"
            },
        );
    }
}

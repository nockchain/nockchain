//! Consensus verifier jet for the AI-PoW (`%ai-pow`) puzzle — Branch (b): a full
//! Rust verify jet with a stubbed Hoon arm.
//!
//! nockchain's existing consensus verify (`check-pow` → `verify:nv`) is a Hoon
//! STARK verifier with jetted primitives. AI-PoW's compact **recursive**-STARK
//! verify is Rust-only, so — per the chosen architecture — the Hoon arm
//! `++ai-pow-verify` is a stub and this jet is the real implementation.
//!
//! **Transparency:** the jet's sample is the *structured* `ai-pow-artifact` noun
//! (`[nonce certificate]`, the same shape Hoon builds) plus the block commitment
//! and target as atoms; the result is a loobean. Only the opaque `nonce` (the
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
// SOUNDNESS (consensus DoS): the verify jet's guarantee that a crafted `%ai-pow`
// block can never crash the node relies on `std::panic::catch_unwind` converting an
// attacker-induced panic (in decode or the recursion verifier) into a
// deterministic invalid-block `NO`. Under `panic = "abort"` catch_unwind is a no-op
// and a panic aborts the whole process — turning one crafted block into a
// network-wide crash. Refuse to build the consensus verifier that way.
#[cfg(panic = "abort")]
compile_error!(
    "ai-pow-jets requires panic=unwind: the consensus verify jet relies on \
     catch_unwind to turn crafted-block panics into deterministic invalid-block \
     rejections; under panic=abort a crafted %ai-pow block would crash the node."
);

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use ai_pow_miner::certificate_noun::{
    decode_ai_pow_pearl_merge_artifact_noun, verify_ai_pow_block_artifact, AiPowBlockVerifyOutcome,
    CertificateNounLimits, PearlMergeAiPowArtifactShape,
};

// Tests use jemalloc (the production allocator) so RSS-reclaim probes measure the real
// behavior — the system allocator retains freed page-outs and would give misleading
// numbers. Matches the required `#[global_allocator]` in the nockchain binary.
#[cfg(test)]
#[global_allocator]
static TEST_ALLOC: tikv_jemallocator::Jemalloc = tikv_jemallocator::Jemalloc;

use ai_pow_zk::recursion::AiPowCompactBatchVerifierContext;
use nockvm::interpreter::Context;
use nockvm::jets::util::{slot, BAIL_FAIL};
use nockvm::jets::JetErr;
use nockvm::noun::{Noun, NounSpace, D};
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
    pub trace_height: usize,
    pub sx_bound: bool,
}

impl VerifierSetupShapeKey {
    pub const fn new(trace_height: usize, sx_bound: bool) -> Self {
        Self {
            trace_height,
            sx_bound,
        }
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
        VerifierSetupShapeKey::new(self.trace_height, self.sx_bound)
    }
}

/// A per-bucket verifier context living ON DISK: the path of its serialized context
/// file plus its committed verifier-key digest. Only this small metadata is resident;
/// the heavy (~0.9–2.7 GB) context is read from disk and deserialized on demand (see
/// [`ai_pow_verifier_setup_for`]) — a fast page-in (~0.6 s worst case), NEVER a
/// circuit rebuild. Built once at boot (see the disk-paged residency doc).
pub struct DiskBucket {
    shape_key: VerifierSetupShapeKey,
    /// The canonical 40-byte verifier-key digest this context must match (the
    /// consensus parameter). Checked at boot and on every page-in.
    committed_digest: Vec<u8>,
    context_path: std::path::PathBuf,
    /// BLAKE3 of the serialized context FILE BYTES, computed when the file was
    /// written. Re-checked on every page-in so on-disk bit-rot (which overwhelmingly
    /// lands in the multi-GB preprocessed tree, leaving the small stored digest fields
    /// intact) is caught as a NON-DETERMINISTIC load fault (→ the jet `%fail`s) rather
    /// than silently corrupting the tree and later rejecting a valid block.
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
    let (setup, _) = match decoded {
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
pub(crate) fn ai_pow_verify_core(
    artifact: &PearlMergeAiPowArtifactShape,
    commit: [u8; 32],
    target: [u8; 32],
    setup: &AiPowVerifierSetup,
) -> Result<bool, JetErr> {
    let limits = CertificateNounLimits::default();
    match verify_ai_pow_block_artifact(
        artifact, limits, &commit, &target, AI_POW_VERIFY_MAX_PATTERN_LEN, &setup.context,
        &setup.digest_bytes,
    ) {
        Ok(AiPowBlockVerifyOutcome::Dense(_)) | Ok(AiPowBlockVerifyOutcome::Moe(_)) => Ok(true),
        Err(_) => Ok(false),
    }
}

/// The AI-PoW verify jet. Sample:
/// `[artifact=ai-pow-artifact commit=block-commitment:page:t target=@]`
/// — `commit` is the STRUCTURED commitment noun (canonicalized here via
/// `commit_from_noun`), `target` the `merge:bignum` LE atom the Hoon arm passes.
/// Result: loobean.
///
/// **It is impossible to panic the node from this jet.** The two attacker-controlled
/// steps — decoding the artifact and running the (large, vendored) recursion verifier
/// on the certificate — are wrapped in `catch_unwind`; a panic on crafted input is a
/// DETERMINISTIC invalid-block signal → `NO` (which consensus turns into a
/// `%liar-block-id`, so the block cannot be re-spammed). Neither wrapped step mutates
/// the interpreter stack, so catching is safe. The setup lookup distinguishes a
/// DETERMINISTIC "no such bucket ⇒ invalid block" (`NO`) from a NON-DETERMINISTIC
/// local fault (missing/corrupt context file, uninjected table ⇒ `%fail` via
/// `BAIL_FAIL`, so a broken node halts rather than wrongly rejecting valid blocks).
pub fn ai_pow_verify_jet(context: &mut Context, subject: Noun) -> Result<Noun, JetErr> {
    let space = context.stack.noun_space();
    // sample = [artifact commit target]  ⇒  head=2, commit=6, target=7
    let sample = slot(subject, 6, &space)?;
    let artifact_noun = slot(sample, 2, &space)?;
    let commit_noun = slot(sample, 6, &space)?;
    let target_noun = slot(sample, 7, &space)?;

    // Decode + target-parse + cap-check the ATTACKER-CONTROLLED artifact under
    // catch_unwind. A decode/shape failure, a bad target, an over-cap height, OR a
    // panic on crafted input are all the same DETERMINISTIC invalid-block signal → NO.
    // None of this mutates `context.stack`, so catching a panic here is safe.
    let decoded = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let limits = CertificateNounLimits::default();
        let artifact =
            decode_ai_pow_pearl_merge_artifact_noun(artifact_noun, &space, limits).ok()?;
        let target = target_atom_to_32_saturating(target_noun, &space)?;
        let setup_key = VerifierSetupShapeKey::from_zk_params(
            &artifact.certificate.zk_params, artifact.certificate.trace_height,
        )?;
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
        ai_pow_verify_core(&artifact, commit, target, &setup)
    }));
    match verified {
        Ok(Ok(true)) => Ok(YES),
        Ok(Ok(false)) | Ok(Err(_)) | Err(_) => Ok(NO),
    }
}

/// Hot-state entry set for the AI-PoW verify jet. Appended to the nockchain kernel
/// hot state alongside `zkvm-jetpack`'s prover jets.
///
/// The Hoon `++ai-pow-verify` (`~/ %ai-pow-verify`) lives in the shared
/// `/common/pow` lib under a `~% %pow-lib ..ut ~` root (it cannot be a kernel
/// door arm — the `fort` mold fixes %dumb-inner to load/peek/poke — nor a
/// `|^`-nested arm, which fails cold registration). `..ut` resolves to the
/// hoon.hoon std-library prefix `[one two tri qua pen]` (confirmed by the
/// `%zeke`-anchored jets, e.g. cheetah `ser-a-pt`, which sit at
/// `[one two tri qua pen zeke ..]`). So `%pow-lib` sits at
/// `[one two tri qua pen pow-lib]` and the jetted arm at
/// `[one two tri qua pen pow-lib ai-pow-verify]`. Axis `1` is the `~/`-gate
/// convention (matches every base58 / ec-point `|=` jet). Runtime-validated by
/// the roswell `test-ai-pow-verify-jet-fires` unit test.
pub fn produce_ai_pow_hot_state() -> Vec<nockvm::jets::hot::HotEntry> {
    use either::Either::Left;
    use nockvm::jets::hot::K_138;
    vec![(
        &[
            K_138,
            Left(b"one"),
            Left(b"two"),
            Left(b"tri"),
            Left(b"qua"),
            Left(b"pen"),
            Left(b"pow-lib"),
            Left(b"ai-pow-verify"),
        ],
        1,
        ai_pow_verify_jet,
    )]
}

#[cfg(test)]
mod tests {
    use super::*;

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
        let path = crate::setup::verifier_setup_seed_cache_path(&dir);
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
            buckets.len(),
            "the production default must retain all 14 attacker-selectable shape keys",
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
                let th = crate::setup::canonical_moe_trace_height(&b.params, b.hw, b.e, b.top_k)
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
    fn dense_production_profiles_use_installed_setup_buckets() {
        let expected_keys: Vec<(&str, VerifierSetupShapeKey)> = [
            ("peak", ai_pow_miner::PEAK_PRODUCTION_PARAMS),
            ("fused Gemma", ai_pow_miner::gemma4::GEMMA4_NATIVE_PARAMS),
        ]
        .into_iter()
        .map(|(label, params)| {
            let trace_height = ai_pow::zk_bridge::pearl_dense_canonical_trace_height(&params, 0, 0)
                .expect("dense production trace height");
            (
                label,
                VerifierSetupShapeKey::new(
                    trace_height,
                    (params.k / params.noise_rank) as usize <= ai_pow::params::STRIPE_MAX,
                ),
            )
        })
        .collect();

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
                        let h = crate::setup::canonical_moe_trace_height(
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

        for (label, key) in expected_keys {
            assert!(
                installed_keys.contains(&key),
                "missing {label} setup key {key:?}"
            );
        }
    }
}

#[cfg(test)]
mod jet_tests {
    use ai_pow::difficulty::{attempt_wins, shape_work_factor_for, AI_POW_MAX_CONSENSUS_TARGET};
    use ai_pow::params::MatmulParams;
    use ai_pow_miner::canonical::evaluate_canonical_moe_jackpot;
    use ai_pow_miner::certificate_noun::build_ai_pow_pearl_merge_moe_artifact_noun_from_node;
    use nockapp::noun::slab::NounSlab;
    use nockvm::noun::NounAllocator;

    use super::*;
    use crate::setup::{
        build_verifier_setup_seed, prove_canonical_moe_block, CanonicalBlock,
        CANONICAL_SETUP_COMMIT,
    };

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
    fn target_hitting_canonical_moe_block() -> (CanonicalBlock, [u8; 32]) {
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
                let jackpot = evaluate_canonical_moe_jackpot(&params, 8, 2, 1, commit, 0)
                    .expect("evaluate canonical MoE ticket");
                attempt_wins(&jackpot, &target, work_factor)
                    .expect("max consensus target is minable")
                    .then_some(commit)
            })
            .expect("canonical commitment search must find a target-winning ticket");
        (
            prove_canonical_moe_block(&params, 8, 2, 1, commit)
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
        let (block, target) = target_hitting_canonical_moe_block();

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

    /// SLIM-CONTEXT SOUNDNESS KAT (real proof, ~25s): dropping the prove-only raw
    /// preprocessed columns from a boot verifier setup (via `into_verifier_only` on
    /// the rebuild path) leaves verification BIT-IDENTICAL. Prove one real block, then
    /// verify a real artifact and a wrong-commit artifact against BOTH the full proved
    /// context (retains the raw columns) and the slimmed rebuilt context (raw columns
    /// dropped), asserting identical accept/reject outcomes and an identical
    /// verifier-key digest. Pins that the RSS trim does not change consensus results.
    #[test]
    #[ignore = "real MoE compact proof (~25s); opt-in"]
    fn slimmed_verifier_only_context_verifies_identically() {
        let (block, target) = target_hitting_canonical_moe_block();
        let commit = block.commit;

        let jammed = build_ai_pow_pearl_merge_moe_artifact_noun_from_node(
            &block.statement, &block.aux_inclusion, &block.moe_art, &block.certificate.zk_params,
            block.certificate.found_idx, block.certificate.trace_height,
            &block.certificate.commitments, &block.certificate.public_inputs,
            &block.certificate.certificate,
        )
        .expect("build MoE artifact noun")
        .jam();

        // FULL context: the freshly-proved context, which retains the raw columns.
        let full_digest = ai_pow_zk::recursion::compact_batch_verifier_key_digest_to_bytes(
            &block.run.verifier_key_digest(),
        )
        .to_vec();
        let setup_key = VerifierSetupShapeKey::from_zk_params(
            &block.certificate.zk_params, block.run.trace_height,
        )
        .expect("valid setup key");
        let full_setup = AiPowVerifierSetup {
            trace_height: setup_key.trace_height,
            sx_bound: setup_key.sx_bound,
            context: block.run.verifier_context,
            digest_bytes: full_digest.clone(),
        };
        // SLIM context: rebuilt from the seed — `into_verifier_only` drops the raw
        // columns on this (verify-only) rebuild path.
        let slim_setup = crate::setup::rebuild_verifier_setup_from_seed(block.seed)
            .expect("rebuild slimmed setup from seed");

        assert_eq!(
            slim_setup.shape_key(),
            full_setup.shape_key(),
            "same setup key"
        );
        assert_eq!(
            slim_setup.digest_bytes, full_digest,
            "verifier-key digest must be UNCHANGED by dropping the raw columns",
        );

        // Confirm the slimming ACTUALLY happened (the rebuild's `Arc::try_unwrap`
        // succeeded and dropped the raw columns): the slimmed context serializes
        // strictly smaller than the full proved one. Deterministic (no RSS noise) and
        // reports the exact prove-only-column bytes dropped for this bucket.
        let full_ser =
            bincode::serde::encode_to_vec(&full_setup.context, bincode::config::standard())
                .expect("serialize full context")
                .len();
        let slim_ser =
            bincode::serde::encode_to_vec(&slim_setup.context, bincode::config::standard())
                .expect("serialize slim context")
                .len();
        eprintln!(
            "context serialized: full {full_ser} B, slim {slim_ser} B (dropped {} B of \
             prove-only columns at 2^{})",
            full_ser.saturating_sub(slim_ser),
            (slim_setup.trace_height as u64).trailing_zeros(),
        );
        assert!(
            slim_ser < full_ser,
            "the slimmed verify-only context must drop the prove-only columns (got full={full_ser} \
             slim={slim_ser})",
        );

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
        let (block, target) = target_hitting_canonical_moe_block();
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
        let setup_built =
            crate::setup::rebuild_verifier_setup_from_seed(block.seed).expect("build context");
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

    /// CORRUPT-FILE FAIL-SAFE KAT (real proof, ~25s): a bit-rotten context file is
    /// caught by its checksum at page-in and resolves to `LoadFailed` (⇒ the jet
    /// `%fail`s — a non-deterministic local fault), NOT `Found` (which would verify
    /// against a corrupt tree) and NOT a silent reject of a valid block. Pins the H3
    /// fix. Sets the process-global setup; run alone.
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
            prove_canonical_moe_block(&params, 8, 2, 1, CANONICAL_SETUP_COMMIT).expect("prove");
        let setup = crate::setup::rebuild_verifier_setup_from_seed(block.seed).expect("build");
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

    /// DIVERGENT-SETUP FAIL-SAFE KAT: a bincode-valid context file whose
    /// serialized setup metadata no longer matches the committed bucket is a local
    /// setup fault at page-in. Uses the stable verifier-setup seed cache when present.
    #[test]
    #[ignore = "rebuilds one cached verifier context; installs process-global setup; run alone"]
    fn divergent_serialized_setup_pages_in_as_load_failed() {
        if crate::ai_pow_verifier_setup_initialized() {
            eprintln!("skip: verifier setup already initialized in this process");
            return;
        }
        let cache_dir = std::env::temp_dir().join("aipow-rss-cache");
        let cache_path = crate::setup::verifier_setup_seed_cache_path(&cache_dir);
        if !cache_path.exists() {
            eprintln!(
                "skip: no stable verifier-setup seed cache at {}",
                cache_path.display()
            );
            return;
        }
        let seeds =
            crate::setup::load_verifier_setup_seeds(&cache_path).expect("load stable setup seeds");
        let mut setup = match seeds.into_iter().find_map(|seed| {
            let rebuilt = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                crate::setup::rebuild_verifier_setup_from_seed(seed)
            }));
            match rebuilt {
                Ok(Ok(setup)) => Some(setup),
                Ok(Err(e)) => {
                    eprintln!("skip seed: cached verifier setup is not rebuildable here: {e}");
                    None
                }
                Err(_) => {
                    eprintln!("skip seed: cached verifier setup rebuild panicked");
                    None
                }
            }
        }) {
            Some(setup) => setup,
            None => {
                eprintln!("skip: no cached verifier setup seed rebuilt successfully");
                return;
            }
        };
        let setup_key = setup.shape_key();
        let committed_digest = setup.digest_bytes.clone();
        setup.digest_bytes[0] ^= 0xff;

        let tmp = tempfile::TempDir::new().unwrap();
        let ctx_path =
            crate::setup::verifier_context_file_path(tmp.path(), setup_key, &committed_digest);
        let bytes = bincode::serde::encode_to_vec(&setup, bincode::config::standard())
            .expect("serialize divergent setup");
        std::fs::write(&ctx_path, &bytes).unwrap();
        let checksum = *blake3::hash(&bytes).as_bytes();
        let bucket = crate::DiskBucket::new(setup_key, committed_digest, ctx_path, checksum);
        crate::init_ai_pow_verifier_setup_disk(vec![bucket], 1).expect("inject disk bucket");

        assert!(
            matches!(
                crate::ai_pow_verifier_setup_for(setup_key),
                crate::VerifierSetupLookup::LoadFailed
            ),
            "a divergent context file must page in as LoadFailed (→ %fail), never invalid-block NO",
        );
    }

    /// PRODUCTION BOOT + RSS KAT (~1–2 min + large disk; needs the stable seed cache):
    /// run the real `install_or_build_verifier_setup` — build all production contexts
    /// to disk at boot, inject disk-paged — then page through every key with `cap=2`
    /// (a) each pages in + resolves, and (b) standing RSS stays bounded to ~2 contexts,
    /// NOT the multi-GB all-resident table. Sets the process-global setup; run alone.
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
        // Use the stable dir (has the seed cache) DIRECTLY so the built context files
        // persist across runs: the first run builds every context file, and later runs
        // find the files and reuse them.
        let dir = std::env::temp_dir().join("aipow-rss-cache");
        let src_cache = crate::setup::verifier_setup_seed_cache_path(&dir);
        if !src_cache.exists() {
            eprintln!("skip: no stable seed cache at {}", src_cache.display());
            return;
        }
        // Detect whether this is a build-boot or a reuse-boot (context files present).
        let seeds = crate::setup::load_verifier_setup_seeds(&src_cache).expect("load seeds");
        let reuse = seeds.iter().all(|s| {
            let key = VerifierSetupShapeKey::from_zk_params(&s.zk_params, s.trace_height())
                .expect("valid seed key");
            crate::setup::verifier_context_file_path(&dir, key, &s.verifier_key_digest_bytes)
                .exists()
        });
        let expected_cap = 2usize;
        drop(seeds);
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
        assert_eq!(n, buckets.len(), "all production buckets installed");
        let after_boot = rss_mb();
        eprintln!(
            "boot mode: {}; after boot (contexts on disk, none paged in): RSS {after_boot} MB (base {base})",
            if reuse { "REUSE (files existed)" } else { "BUILD (first run)" },
        );

        // Page every key in, with cap=2 — RSS must stay ~2 contexts, not the full table.
        for b in buckets {
            let h = crate::setup::canonical_moe_trace_height(&b.params, b.hw, b.e, b.top_k)
                .expect("cheap trace height");
            let sx_bound =
                (b.params.k / b.params.noise_rank) as usize <= ai_pow::params::STRIPE_MAX;
            let key = VerifierSetupShapeKey::new(h, sx_bound);
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

    /// AUDIT PROBE: does DROPPING a paged-in context actually return RSS to the OS on
    /// THIS allocator? The "cap bounds RSS" guarantee needs eviction (a `drop`) to
    /// reclaim memory. Page the largest context in and out several times, printing RSS
    /// each cycle: flat ⇒ the drop reclaims (guarantee holds even on the system
    /// allocator); growing ⇒ the allocator retains freed memory and the guarantee
    /// relies on jemalloc's background decay (production) rather than the drop alone.
    /// Needs a built 2^19 context file (from a prior boot/RSS run); skips otherwise.
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
        let dir = std::env::temp_dir().join("aipow-rss-cache");
        let cache = crate::setup::verifier_setup_seed_cache_path(&dir);
        if !cache.exists() {
            eprintln!("skip: no stable seed cache");
            return;
        }
        let mut seeds = crate::setup::load_verifier_setup_seeds(&cache).expect("load seeds");
        seeds.sort_by_key(|s| s.trace_height());
        let big = seeds.pop().expect("a seed");
        let big_key = VerifierSetupShapeKey::from_zk_params(&big.zk_params, big.trace_height())
            .expect("valid seed key");
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

    /// LAZY BOOT DIGEST CHECK (fast, no proving; needs the generated seed cache): the
    /// real production seed cache's cached per-bucket digests hash to the committed v0
    /// constant via the seed-only path — i.e. the lazy boot check ACCEPTS a valid
    /// cache without rebuilding.
    /// mirroring the rebuilt-table digest check. Skips if the cache is absent.
    #[test]
    #[ignore = "needs the generated seed cache (a prior run); validates the lazy boot digest check"]
    fn stable_cache_seeds_pass_lazy_boot_digest_check() {
        let dir = std::env::temp_dir().join("aipow-rss-cache");
        let path = crate::setup::verifier_setup_seed_cache_path(&dir);
        if !path.exists() {
            eprintln!("skip: no stable seed cache at {}", path.display());
            return;
        }
        let seeds = crate::setup::load_verifier_setup_seeds(&path).expect("load seeds");
        assert_eq!(seeds.len(), 13, "full production shape-key cache");
        crate::table_digest::verify_verifier_setup_seed_table_digest(&seeds)
            .expect("stable-cache seeds must hash to the committed v0 digest (no rebuild)");
        eprintln!("stable cache seeds pass the lazy boot digest check ✓");
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
                let jackpot = evaluate_canonical_moe_jackpot(&params, 8, 2, 1, commit, 0)
                    .expect("evaluate noun-derived canonical ticket");
                attempt_wins(&jackpot, &target, work_factor)
                    .expect("max consensus target is minable")
                    .then_some(commit)
            })
            .expect("noun-derived commitment search must find a target-winning ticket");

        // Prove a real cert bound to that noun-derived commit (the miner's job).
        let block = prove_canonical_moe_block(&params, 8, 2, 1, commit)
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

    /// KAT (real proving + rebuild, ~30s): the boot-setup SEED cache path — the
    /// C4 linchpin. Prove a real MoE block, serialize its SMALL rebuild seed,
    /// deserialize it, and rebuild the FULL verifier setup from it WITHOUT proving.
    /// The real block must verify through the jet CORE against the REBUILT
    /// (cached-seed) setup exactly as against the freshly-proved context, and a
    /// wrong commit must still be rejected. Also asserts the serialized seed is
    /// small (< 16 MiB) — the whole point of caching the seed, not the ~866 MB
    /// context. This proves a boot node can cache seeds and rebuild working setups.
    #[test]
    #[ignore = "real MoE compact proof + rebuild (~30s); opt-in"]
    fn moe_verifier_setup_seed_roundtrip_rebuilds_working_setup() {
        use crate::setup::rebuild_verifier_setup_from_seed;

        let (block, target) = target_hitting_canonical_moe_block();
        let commit = block.commit;

        // Serialize the SMALL seed; assert it is small (vs the ~866 MB context).
        let seed_bytes = bincode::serde::encode_to_vec(&block.seed, bincode::config::standard())
            .expect("serialize verifier-setup seed");
        assert!(
            seed_bytes.len() < 16 * 1024 * 1024,
            "cached seed must be small (< 16 MiB); got {} bytes",
            seed_bytes.len(),
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

        // BOOT path: deserialize the seed and REBUILD the setup (no proving).
        let (seed2, _): (ai_pow_zk::recursion::AiPowCompactVerifierSetupSeed, _) =
            bincode::serde::decode_from_slice(&seed_bytes, bincode::config::standard())
                .expect("deserialize verifier-setup seed");
        let setup = rebuild_verifier_setup_from_seed(seed2).expect("rebuild setup from seed");
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
            "real MoE block must verify against the REBUILT (cached-seed) setup",
        );
        assert!(
            matches!(
                ai_pow_verify_core(&artifact, [0x99u8; 32], target, &setup),
                Ok(false)
            ),
            "wrong block commitment must still be rejected against the rebuilt setup",
        );

        // FILE cache path (C4b): save the seed to a data-dir-style cache file, load
        // + rebuild the TABLE from disk, and verify the SAME block against the
        // disk-loaded setup — the exact boot flow (cache in data dir → load →
        // rebuild → verify), end to end through a real file.
        let tmp_data_dir =
            std::env::temp_dir().join(format!("ai-pow-jets-seedcache-{}", std::process::id()));
        let cache_path = crate::setup::verifier_setup_seed_cache_path(&tmp_data_dir);
        crate::setup::save_verifier_setup_seeds(&cache_path, std::slice::from_ref(&block.seed))
            .expect("save seed cache to data-dir file");
        let table = crate::setup::load_verifier_setup_table(&cache_path)
            .expect("load + rebuild seed table");
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

    /// KAT (real proving, ~40s): the boot GENERATION path end to end for one
    /// bucket. Take the cheapest production bucket shape, `build_and_cache` it
    /// (prove + write the seed cache to a data-dir file), then `load_verifier_setup_table`
    /// (load + rebuild, no proving) and confirm the rebuilt setup lands at exactly
    /// the height `production_verifier_setup_buckets` predicted (matrix-free). This
    /// is what a fresh node does on first boot when it has no cache. Does NOT touch
    /// the global setup OnceCell (so it can't perturb the cheap boot-installer tests).
    #[test]
    #[ignore = "real MoE compact proof + generate/cache/load (~40s); opt-in"]
    fn boot_generate_and_cache_one_bucket_roundtrips() {
        let buckets = crate::setup::production_verifier_setup_buckets();
        let shape = *buckets.first().expect("at least one production bucket");
        let expected_h =
            crate::setup::canonical_moe_trace_height(&shape.params, shape.hw, shape.e, shape.top_k)
                .expect("cheap predicted height");

        let tmp = std::env::temp_dir().join(format!("ai-pow-genboot-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        let path = crate::setup::verifier_setup_seed_cache_path(&tmp);

        // Generate (prove) + cache the one-bucket table to the data-dir file.
        crate::setup::build_and_cache_verifier_setup_seeds(&path, &[shape])
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
            crate::setup::load_verifier_setup_seeds(&path).is_err(),
            "the envelope must reject a changed payload before seed decoding",
        );
        std::fs::write(&path, cached_bytes).expect("restore checksummed cache");

        // Load + rebuild (no proving) — the fast subsequent-boot path.
        let table = crate::setup::load_verifier_setup_table(&path).expect("load + rebuild table");
        let _ = std::fs::remove_dir_all(&tmp);
        assert_eq!(table.len(), 1, "one-bucket table");
        assert_eq!(
            table[0].trace_height, expected_h,
            "generated+rebuilt bucket lands at the matrix-free predicted height",
        );
    }

    /// KAT (two real proofs, ~80s): canonical setup generation has a deterministic
    /// serialized seed, not merely a deterministic verifier-key digest.
    #[test]
    #[ignore = "two real MoE compact proofs (~80s); opt-in"]
    fn verifier_setup_seed_bytes_are_deterministic() {
        let buckets = crate::setup::production_verifier_setup_buckets();
        let shape = *buckets.first().expect("at least one production bucket");
        let first = build_verifier_setup_seed(&shape.params, shape.hw, shape.e, shape.top_k)
            .expect("first canonical seed");
        let second = build_verifier_setup_seed(&shape.params, shape.hw, shape.e, shape.top_k)
            .expect("second canonical seed");

        let first_bytes = bincode::serde::encode_to_vec(&first, bincode::config::standard())
            .expect("serialize first canonical seed");
        let second_bytes = bincode::serde::encode_to_vec(&second, bincode::config::standard())
            .expect("serialize second canonical seed");
        assert_eq!(first_bytes, second_bytes, "canonical setup seed bytes");
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
                crate::setup::canonical_moe_trace_height(&b.params, b.hw, b.e, b.top_k).unwrap()
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

    /// C4 CLOSER (generates the FULL production table; ~6 min): consensus caps the
    /// accept-band at 2^19 (`AI_POW_MAX_TRACE_HEIGHT`), so the table is exactly the
    /// production shape keys across the feasible 2^13..2^19 trace-height band. Proves
    /// one canonical block per shape, logs each seed's trace height + serialized size
    /// (the L0 program grows with trace height), round-trips the whole table through
    /// a data-dir cache file, rebuilds them (no proving), and asserts coverage of the
    /// capped acceptance band. This is exactly what a fresh node does on first boot.
    #[test]
    #[ignore = "generates the full production shape-key table 2^13..2^19; opt-in — closes C4"]
    fn boot_generate_full_production_table() {
        use std::collections::{BTreeMap, BTreeSet};
        let cap_db = (ai_pow::params::AI_POW_MAX_TRACE_HEIGHT as u32).trailing_zeros();
        let buckets = crate::setup::production_verifier_setup_buckets();
        assert_eq!(buckets.len(), 14, "expected 14 production shape keys");

        let mut seeds = Vec::new();
        let mut total_bytes = 0usize;
        for (i, shape) in buckets.iter().enumerate() {
            let seed = if shape.dense {
                crate::setup::build_verifier_setup_seed_dense(&shape.params)
            } else {
                crate::setup::build_verifier_setup_seed(
                    &shape.params, shape.hw, shape.e, shape.top_k,
                )
            }
            .unwrap_or_else(|e| panic!("bucket {i} generation failed: {e}"));
            let sz = bincode::serde::encode_to_vec(&seed, bincode::config::standard())
                .expect("serialize seed")
                .len();
            total_bytes += sz;
            eprintln!(
                "bucket {i}: trace_height=2^{} seed={} bytes (cum {:.1} MiB)",
                seed.trace_height().trailing_zeros(),
                sz,
                total_bytes as f64 / (1024.0 * 1024.0),
            );
            seeds.push(seed);
        }
        eprintln!(
            "FULL TABLE: {} buckets, total seed cache = {:.1} MiB",
            seeds.len(),
            total_bytes as f64 / (1024.0 * 1024.0),
        );

        let mut by_height: BTreeMap<u32, BTreeSet<bool>> = BTreeMap::new();
        for s in &seeds {
            let key = VerifierSetupShapeKey::from_zk_params(&s.zk_params, s.trace_height())
                .expect("valid seed key");
            by_height
                .entry(s.trace_height().trailing_zeros())
                .or_default()
                .insert(key.sx_bound);
        }
        let both_sx_classes: BTreeSet<bool> = [false, true].into_iter().collect();
        for db in 13u32..=cap_db {
            assert_eq!(
                by_height.get(&db),
                Some(&both_sx_classes),
                "2^{db} must cover both sx-bound classes"
            );
        }

        // Round-trip the whole set through a data-dir cache file + rebuild.
        let tmp = std::env::temp_dir().join(format!("ai-pow-fulltable-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        let path = crate::setup::verifier_setup_seed_cache_path(&tmp);
        crate::setup::save_verifier_setup_seeds(&path, &seeds).expect("save table");
        let cache_bytes = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
        eprintln!(
            "cache file on disk = {:.1} MiB",
            cache_bytes as f64 / (1024.0 * 1024.0)
        );
        let table = crate::setup::load_verifier_setup_table(&path).expect("load+rebuild table");
        let _ = std::fs::remove_dir_all(&tmp);
        assert_eq!(table.len(), 14, "rebuilt table has 14 shape keys");
        let table_keys: BTreeSet<VerifierSetupShapeKey> =
            table.iter().map(|s| s.shape_key()).collect();
        let seed_keys: BTreeSet<VerifierSetupShapeKey> = seeds
            .iter()
            .map(|s| {
                VerifierSetupShapeKey::from_zk_params(&s.zk_params, s.trace_height())
                    .expect("valid seed key")
            })
            .collect();
        assert_eq!(
            table_keys, seed_keys,
            "rebuilt table covers the same shape keys"
        );

        // CONSENSUS FINGERPRINT (v0): compute the table digest over the rebuilt table
        // — the exact generate -> cache -> load path a fresh node runs at boot. Print
        // it so the constant can be pinned; once pinned, re-running this test (which
        // re-generates from scratch) also RE-VALIDATES run-to-run determinism, since
        // an independent generation must reproduce the pinned digest.
        let table_digest =
            crate::table_digest::verifier_setup_table_digest(&table).expect("v0 table digest");
        eprintln!(
            "V0 VERIFIER-SETUP TABLE DIGEST = {}",
            crate::table_digest::hex32(&table_digest),
        );
        eprintln!(
            "  pin: AI_POW_V0_VERIFIER_SETUP_TABLE_DIGEST = [{}];",
            table_digest
                .iter()
                .map(|b| format!("0x{b:02x}"))
                .collect::<Vec<_>>()
                .join(", "),
        );
        if crate::table_digest::v0_digest_is_pinned() {
            assert_eq!(
                table_digest,
                crate::table_digest::AI_POW_V0_VERIFIER_SETUP_TABLE_DIGEST,
                "independently-generated table digest must match the pinned v0 consensus constant \
                 (determinism / consensus-parameter check)",
            );
        }
    }

    fn production_setup_seed_keys() -> std::collections::BTreeSet<VerifierSetupShapeKey> {
        crate::setup::production_verifier_setup_buckets()
            .iter()
            .map(|bucket| {
                let trace_height = crate::setup::canonical_moe_trace_height(
                    &bucket.params, bucket.hw, bucket.e, bucket.top_k,
                )
                .expect("production setup bucket has a trace height");
                let sx_bound = (bucket.params.k / bucket.params.noise_rank) as usize
                    <= ai_pow::params::STRIPE_MAX;
                VerifierSetupShapeKey::new(trace_height, sx_bound)
            })
            .collect()
    }

    fn setup_seed_keys(
        seeds: &[ai_pow_zk::recursion::AiPowCompactVerifierSetupSeed],
    ) -> std::collections::BTreeSet<VerifierSetupShapeKey> {
        seeds
            .iter()
            .map(|seed| {
                VerifierSetupShapeKey::from_zk_params(&seed.zk_params, seed.trace_height())
                    .expect("cached setup seed has a valid shape key")
            })
            .collect()
    }

    fn load_stable_production_setup_seeds(
        path: &std::path::Path,
    ) -> Vec<ai_pow_zk::recursion::AiPowCompactVerifierSetupSeed> {
        let expected = production_setup_seed_keys();
        let buckets = crate::setup::production_verifier_setup_buckets();
        if path.exists() {
            match crate::setup::load_verifier_setup_seeds(path) {
                Ok(seeds) => {
                    let actual = setup_seed_keys(&seeds);
                    if actual == expected {
                        match crate::table_digest::verify_verifier_setup_seed_table_digest(&seeds) {
                            Ok(_) => return seeds,
                            Err(e) => eprintln!(
                                "stable setup seed cache digest does not match production ({e}); \
                                 regenerating {}",
                                path.display(),
                            ),
                        }
                    } else {
                        eprintln!(
                            "stable setup seed cache shape keys do not match production; \
                             cache={actual:?} production={expected:?}; regenerating {}",
                            path.display(),
                        );
                    }
                }
                Err(e) => {
                    eprintln!(
                        "stable setup seed cache at {} is unreadable ({e}); regenerating",
                        path.display(),
                    );
                }
            }
            let _ = std::fs::remove_file(path);
        }
        eprintln!(
            "generating current production setup seed cache at {} ({} buckets)",
            path.display(),
            buckets.len(),
        );
        crate::setup::build_and_cache_verifier_setup_seeds(path, &buckets)
            .expect("generate current production setup seed cache");
        let seeds = crate::setup::load_verifier_setup_seeds(path).expect("load generated seeds");
        assert_eq!(
            setup_seed_keys(&seeds),
            expected,
            "generated stable setup cache must match current production shape keys",
        );
        seeds
    }

    /// RSS + rebuild-latency MEASUREMENT for the boot verifier-setup table.
    ///
    /// Reports (a) the resident memory each rebuilt bucket context costs and the
    /// total standing RSS of the production shape-key table, and (b) the wall-clock
    /// to rebuild each bucket from its cached seed — i.e. the per-height latency an
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
        let dir = std::env::temp_dir().join("aipow-rss-cache");
        let path = crate::setup::verifier_setup_seed_cache_path(&dir);
        let seeds = load_stable_production_setup_seeds(&path);
        let cache_mb =
            std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0) as f64 / (1024.0 * 1024.0);
        let base = rss_mb();
        eprintln!(
            "cache {cache_mb:.1} MiB on disk; baseline RSS after loading seeds = {base} MB; \
             rebuilding {} buckets:",
            seeds.len(),
        );
        let mut table = Vec::new();
        let mut prev = base;
        let mut total_rebuild_ms = 0u128;
        for seed in seeds {
            let h = (seed.trace_height() as u64).trailing_zeros();
            let t = Instant::now();
            let setup = crate::setup::rebuild_verifier_setup_from_seed(seed).expect("rebuild");
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
        // committed v0 table digest (the digest is over the verifier-key, not the
        // columns). Re-verify against the pinned constant on the slimmed table.
        let table_keys: std::collections::BTreeSet<_> =
            table.iter().map(|setup| setup.shape_key()).collect();
        if table_keys == production_setup_seed_keys() && crate::table_digest::v0_digest_is_pinned()
        {
            let digest =
                crate::table_digest::verifier_setup_table_digest(&table).expect("table digest");
            assert_eq!(
                digest,
                crate::table_digest::AI_POW_V0_VERIFIER_SETUP_TABLE_DIGEST,
                "slimmed (verifier-only) table digest must equal the pinned v0 constant",
            );
            eprintln!("v0 table digest UNCHANGED by slimming ✓");
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
        let dir = std::env::temp_dir().join("aipow-rss-cache");
        let path = crate::setup::verifier_setup_seed_cache_path(&dir);
        let mut seeds = load_stable_production_setup_seeds(&path);
        seeds.sort_by_key(|s| s.trace_height());
        let biggest = seeds.pop().expect("at least one seed");
        let h = (biggest.trace_height() as u64).trailing_zeros();

        let t = Instant::now();
        let setup = crate::setup::rebuild_verifier_setup_from_seed(biggest).expect("rebuild");
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

//! Consensus fingerprint for the AI-PoW **version 0** verifier-setup table.
//!
//! The verifier-setup table is a CONSENSUS PARAMETER: every node must verify
//! `%ai-pow` blocks against byte-identical verifier keys, or two nodes could
//! disagree on a block's validity and fork. This module pins that parameter with
//! a committed BLAKE3 digest ([`AI_POW_V0_VERIFIER_SETUP_TABLE_DIGEST`]) computed
//! over the per-bucket compact verifier-KEY digests, checked at boot against the
//! table the node actually built (see
//! [`crate::setup::install_or_build_verifier_setup`]).
//!
//! **Why the verifier-key digest is the right fingerprint.** The per-bucket
//! fingerprint is the compact verifier-KEY digest — the cryptographic commitment
//! to the preprocessed verifier data that consensus verification binds to. It is
//! *proof-independent* (two independent canonical proves of the same shape yield
//! the same digest — validated by
//! `ai-pow-miner::moe_compact_verifier_setup_is_proof_independent`) and
//! *canonically encoded* (little-endian Goldilocks limbs via
//! [`compact_batch_verifier_key_digest_to_bytes`]). So the table digest is
//! deterministic across runs and machines: it does not depend on the randomized
//! proof bytes, only on the fixed puzzle shapes the boot generator enumerates.
//!
//! **What the check buys us.** A node whose *generated* table digest differs from
//! the committed constant is producing a DIVERGENT verifier and must not run. The
//! boot check converts what would otherwise be a silent consensus split (or a
//! silently-corrupt cache) into a loud, immediate shutdown.

use ai_pow_zk::recursion::{
    compact_batch_verifier_key_digest_to_bytes, AI_POW_COMPACT_BATCH_VERIFIER_KEY_DIGEST_BYTES,
};

use crate::setup::SetupError;
use crate::AiPowVerifierSetup;

/// One bucket's canonical 40-byte verifier-key digest.
type BucketDigest = [u8; AI_POW_COMPACT_BATCH_VERIFIER_KEY_DIGEST_BYTES];

/// Domain-separation tag + VERSION for the v0 AI-PoW verifier-setup table digest.
///
/// The trailing `v0` is the consensus version the user asked for: bump it (and
/// re-measure [`AI_POW_V0_VERIFIER_SETUP_TABLE_DIGEST`]) whenever the verifier-setup
/// format, the bucket set, or the consensus verifier changes, so an old committed
/// digest can never be silently accepted under new semantics.
const TABLE_DIGEST_DOMAIN_V0: &[u8] = b"ai-pow-v0-verifier-setup-table-digest\0";

/// The committed BLAKE3 digest of the canonical **v0** AI-PoW verifier-setup table
/// (the seven trace-height buckets `2^13..2^19` produced by
/// [`crate::setup::production_verifier_setup_buckets`]).
///
/// This is a CONSENSUS CONSTANT. It is measured once on a reference build by
/// generating the full table and hashing it with [`verifier_setup_table_digest`]
/// (see the ignored test `measure_v0_verifier_setup_table_digest` in
/// `crate::setup`'s test module), then pinned here. Boot recomputes the digest of
/// the table it built and refuses to run on a mismatch.
pub const AI_POW_V0_VERIFIER_SETUP_TABLE_DIGEST: [u8; 32] = [
    // Measured by `boot_generate_full_production_table` on the reference build
    // (generate the 7 buckets 2^13..2^19 -> cache -> rebuild -> hash). Re-running
    // that ignored test regenerates the table from scratch and asserts this exact
    // value, so it doubles as a cross-run reproducibility gate.
    0xe7, 0xee, 0xf3, 0xf4, 0x80, 0x5c, 0xee, 0x01, 0xcf, 0x45, 0xd4, 0xb0, 0x3e, 0xd0, 0x3b, 0x9e,
    0xe3, 0x6b, 0x4a, 0x57, 0x4f, 0x8c, 0xd8, 0xcb, 0x8a, 0xac, 0x79, 0xe2, 0xb6, 0x82, 0x1a, 0xab,
];

/// Whether the committed digest has been pinned to a real measured value (i.e. is
/// not the all-zero placeholder). Used by boot to give an actionable error if a
/// build ships without the constant filled in.
pub fn v0_digest_is_pinned() -> bool {
    AI_POW_V0_VERIFIER_SETUP_TABLE_DIGEST != [0u8; 32]
}

/// Pure fingerprint hash over `(trace_height, verifier_key_digest)` pairs.
///
/// Buckets are sorted by trace height first, so the result is independent of table
/// ordering (however the buckets were enumerated or loaded) and of duplicate-free
/// ordering choices. The length is bound into the hash so a truncated table cannot
/// collide with a full one. Pure and cheap → unit-testable without real contexts.
fn hash_table_fingerprints(fps: &[(u64, BucketDigest)]) -> [u8; 32] {
    let mut sorted: Vec<(u64, BucketDigest)> = fps.to_vec();
    sorted.sort_by_key(|(h, _)| *h);
    let mut hasher = blake3::Hasher::new();
    hasher.update(TABLE_DIGEST_DOMAIN_V0);
    hasher.update(&(sorted.len() as u64).to_le_bytes());
    for (h, d) in &sorted {
        hasher.update(&h.to_le_bytes());
        hasher.update(d);
    }
    *hasher.finalize().as_bytes()
}

/// Compute the consensus fingerprint of a built verifier-setup table.
///
/// For each bucket the fingerprint is the verifier-key digest **recomputed from the
/// rebuilt `context`** (the value consensus verification actually binds to), and it
/// is cross-checked against the cached `digest_bytes` the jet also consumes — so a
/// cache whose cached bytes and rebuilt context disagree is rejected here rather
/// than passed to consensus. Rejects an empty table or duplicate trace-height
/// buckets (each cert must resolve to exactly one setup).
pub fn verifier_setup_table_digest(table: &[AiPowVerifierSetup]) -> Result<[u8; 32], SetupError> {
    if table.is_empty() {
        return Err(SetupError(
            "cannot fingerprint an empty verifier-setup table".to_string(),
        ));
    }
    let mut fps: Vec<(u64, BucketDigest)> = Vec::with_capacity(table.len());
    let mut seen: Vec<usize> = Vec::with_capacity(table.len());
    for s in table {
        let ctx_digest =
            compact_batch_verifier_key_digest_to_bytes(s.context.verifier_key_digest());
        if s.digest_bytes.as_slice() != ctx_digest.as_slice() {
            return Err(SetupError(format!(
                "verifier-setup bucket at trace_height {}: cached digest disagrees with the \
                 rebuilt context digest (corrupt or format-incompatible cache)",
                s.trace_height,
            )));
        }
        if seen.contains(&s.trace_height) {
            return Err(SetupError(format!(
                "verifier-setup table has duplicate trace-height bucket {}",
                s.trace_height,
            )));
        }
        seen.push(s.trace_height);
        fps.push((s.trace_height as u64, ctx_digest));
    }
    Ok(hash_table_fingerprints(&fps))
}

/// Verify a built table matches the committed **v0** consensus digest.
///
/// Returns the (matching) digest on success. On mismatch returns a `SetupError` the
/// boot installer treats as fatal — the node must not run a divergent verifier.
pub fn verify_verifier_setup_table_digest(
    table: &[AiPowVerifierSetup],
) -> Result<[u8; 32], SetupError> {
    if !v0_digest_is_pinned() {
        return Err(SetupError(
            "AI_POW_V0_VERIFIER_SETUP_TABLE_DIGEST is the all-zero placeholder — this build was \
             shipped without pinning the consensus verifier-setup digest"
                .to_string(),
        ));
    }
    let got = verifier_setup_table_digest(table)?;
    if got != AI_POW_V0_VERIFIER_SETUP_TABLE_DIGEST {
        return Err(SetupError(format!(
            "verifier-setup table digest mismatch: built {} but consensus v0 pins {} — refusing to \
             run a divergent verifier",
            hex32(&got),
            hex32(&AI_POW_V0_VERIFIER_SETUP_TABLE_DIGEST),
        )));
    }
    Ok(got)
}

/// Lowercase-hex a 32-byte digest for error messages.
pub(crate) fn hex32(b: &[u8; 32]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn d(byte: u8) -> BucketDigest {
        [byte; AI_POW_COMPACT_BATCH_VERIFIER_KEY_DIGEST_BYTES]
    }

    /// The fingerprint hash is order-independent: the same buckets in any order
    /// produce the same digest (buckets are sorted by trace height first).
    #[test]
    fn fingerprint_hash_is_order_independent() {
        let a = vec![(1u64 << 13, d(0x11)), (1u64 << 15, d(0x22)), (1u64 << 19, d(0x33))];
        let mut b = a.clone();
        b.reverse();
        let c = vec![a[1], a[2], a[0]];
        assert_eq!(hash_table_fingerprints(&a), hash_table_fingerprints(&b));
        assert_eq!(hash_table_fingerprints(&a), hash_table_fingerprints(&c));
    }

    /// The hash is sensitive to every field: a changed height, a changed digest,
    /// or a dropped bucket all change the result (no silent collisions).
    #[test]
    fn fingerprint_hash_is_sensitive() {
        let base = vec![(1u64 << 13, d(0x11)), (1u64 << 15, d(0x22))];
        let h0 = hash_table_fingerprints(&base);

        // changed digest
        let mut v = base.clone();
        v[0].1 = d(0x12);
        assert_ne!(h0, hash_table_fingerprints(&v));

        // changed height
        let mut v = base.clone();
        v[0].0 = 1 << 14;
        assert_ne!(h0, hash_table_fingerprints(&v));

        // dropped bucket (length is bound into the hash)
        assert_ne!(h0, hash_table_fingerprints(&base[..1]));

        // added bucket
        let mut v = base.clone();
        v.push((1u64 << 19, d(0x33)));
        assert_ne!(h0, hash_table_fingerprints(&v));
    }

    /// A single-bucket table has a stable, reproducible digest (determinism of the
    /// pure hash itself).
    #[test]
    fn fingerprint_hash_is_reproducible() {
        let v = vec![(1u64 << 16, d(0xab))];
        assert_eq!(hash_table_fingerprints(&v), hash_table_fingerprints(&v));
    }

    /// A shipping build MUST have the consensus digest pinned to a real (non-zero)
    /// value — the boot check refuses to run against the all-zero placeholder, so a
    /// build that forgot to pin it would never validate a real table.
    #[test]
    fn v0_consensus_digest_is_pinned() {
        assert!(
            v0_digest_is_pinned(),
            "AI_POW_V0_VERIFIER_SETUP_TABLE_DIGEST must be pinned to the measured value, \
             not the all-zero placeholder",
        );
    }
}

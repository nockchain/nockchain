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
use crate::{AiPowVerifierSetup, VerifierSetupShapeKey};

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
/// (the reachable `(trace_height, sx_bound)` keys produced by
/// [`crate::setup::production_verifier_setup_buckets`]).
///
/// This is a CONSENSUS CONSTANT. It is measured once on a reference build by
/// generating the full table and hashing it with [`verifier_setup_table_digest`]
/// (see the ignored test `boot_generate_full_production_table` in
/// `crate`'s test module), then pinned here. Boot recomputes the digest of
/// the table it built and refuses to run on a mismatch.
pub const AI_POW_V0_VERIFIER_SETUP_TABLE_DIGEST: [u8; 32] = [
    // Measured by `boot_generate_full_production_table` on the reference build
    // (generate the production shape-key table -> cache -> rebuild -> hash). Re-running
    // that ignored test regenerates the table from scratch and asserts this exact
    // value, so it doubles as a cross-run reproducibility gate.
    0xd7, 0xdf, 0x5d, 0x38, 0x0f, 0x55, 0xb1, 0x4e, 0x06, 0xb1, 0xf9, 0xf8, 0x39, 0xaf, 0x8b, 0x73,
    0x0a, 0x04, 0x3d, 0x79, 0xa6, 0x60, 0x8e, 0x8d, 0xc7, 0xe1, 0x55, 0x3c, 0x72, 0x77, 0x65, 0x8d,
];

/// Whether the committed digest has been pinned to a real measured value (i.e. is
/// not the all-zero placeholder). Used by boot to give an actionable error if a
/// build ships without the constant filled in.
pub fn v0_digest_is_pinned() -> bool {
    AI_POW_V0_VERIFIER_SETUP_TABLE_DIGEST != [0u8; 32]
}

/// Pure fingerprint hash over `(shape_key, verifier_key_digest)` pairs.
///
/// Buckets are sorted by shape key first, so the result is independent of table
/// ordering (however the buckets were enumerated or loaded) and of duplicate-free
/// ordering choices. The length is bound into the hash so a truncated table cannot
/// collide with a full one. Pure and cheap → unit-testable without real contexts.
fn hash_table_fingerprints(fps: &[(VerifierSetupShapeKey, BucketDigest)]) -> [u8; 32] {
    let mut sorted: Vec<(VerifierSetupShapeKey, BucketDigest)> = fps.to_vec();
    sorted.sort_by_key(|(key, _)| *key);
    let mut hasher = blake3::Hasher::new();
    hasher.update(TABLE_DIGEST_DOMAIN_V0);
    hasher.update(&(sorted.len() as u64).to_le_bytes());
    for (key, d) in &sorted {
        hasher.update(&(key.trace_height as u64).to_le_bytes());
        hasher.update(&[u8::from(key.sx_bound)]);
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
/// than passed to consensus. Rejects an empty table or duplicate shape keys.
pub fn verifier_setup_table_digest(table: &[AiPowVerifierSetup]) -> Result<[u8; 32], SetupError> {
    if table.is_empty() {
        return Err(SetupError(
            "cannot fingerprint an empty verifier-setup table".to_string(),
        ));
    }
    let mut fps: Vec<(VerifierSetupShapeKey, BucketDigest)> = Vec::with_capacity(table.len());
    let mut seen: Vec<VerifierSetupShapeKey> = Vec::with_capacity(table.len());
    for s in table {
        let key = s.shape_key();
        let ctx_digest =
            compact_batch_verifier_key_digest_to_bytes(s.context.verifier_key_digest());
        if s.digest_bytes.as_slice() != ctx_digest.as_slice() {
            return Err(SetupError(format!(
                "verifier-setup bucket {:?}: cached digest disagrees with the rebuilt context \
                 digest (corrupt or format-incompatible cache)",
                key,
            )));
        }
        if seen.contains(&key) {
            return Err(SetupError(format!(
                "verifier-setup table has duplicate shape key {:?}",
                key,
            )));
        }
        seen.push(key);
        fps.push((key, ctx_digest));
    }
    Ok(hash_table_fingerprints(&fps))
}

/// Compute the consensus fingerprint of a SEED table WITHOUT rebuilding — using each
/// seed's cached verifier-key digest. Because a seed's cached digest equals the digest
/// its context rebuilds to (checked on every rebuild), this equals
/// [`verifier_setup_table_digest`] of the rebuilt table. This is the boot-time check
/// for lazy residency: it validates the whole seed set against the pinned constant
/// with no proving and no rebuild. blake3 collision-resistance means a cache whose
/// cached digests hash to the committed value cannot carry forged digests.
pub fn verifier_setup_seed_table_digest(
    seeds: &[ai_pow_zk::recursion::AiPowCompactVerifierSetupSeed],
) -> Result<[u8; 32], SetupError> {
    if seeds.is_empty() {
        return Err(SetupError(
            "cannot fingerprint an empty verifier-setup seed table".to_string(),
        ));
    }
    let mut fps: Vec<(VerifierSetupShapeKey, BucketDigest)> = Vec::with_capacity(seeds.len());
    let mut seen: Vec<VerifierSetupShapeKey> = Vec::with_capacity(seeds.len());
    for s in seeds {
        let key = VerifierSetupShapeKey::from_zk_params(&s.zk_params, s.trace_height())
            .ok_or_else(|| SetupError("seed table has zero noise_rank".to_string()))?;
        if seen.contains(&key) {
            return Err(SetupError(format!(
                "seed table has duplicate verifier-setup shape key {:?}",
                key,
            )));
        }
        seen.push(key);
        let d: BucketDigest = s
            .verifier_key_digest_bytes
            .as_slice()
            .try_into()
            .map_err(|_| {
                SetupError(format!(
                    "seed at {:?}: cached verifier-key digest is not {} bytes",
                    key, AI_POW_COMPACT_BATCH_VERIFIER_KEY_DIGEST_BYTES,
                ))
            })?;
        fps.push((key, d));
    }
    Ok(hash_table_fingerprints(&fps))
}

/// Verify a SEED table matches the committed **v0** consensus digest (boot-time,
/// no rebuild). Same fatal semantics as [`verify_verifier_setup_table_digest`].
pub fn verify_verifier_setup_seed_table_digest(
    seeds: &[ai_pow_zk::recursion::AiPowCompactVerifierSetupSeed],
) -> Result<[u8; 32], SetupError> {
    if !v0_digest_is_pinned() {
        return Err(SetupError(
            "AI_POW_V0_VERIFIER_SETUP_TABLE_DIGEST is the all-zero placeholder".to_string(),
        ));
    }
    let got = verifier_setup_seed_table_digest(seeds)?;
    if got != AI_POW_V0_VERIFIER_SETUP_TABLE_DIGEST {
        return Err(SetupError(format!(
            "verifier-setup SEED table digest mismatch: got {} but consensus v0 pins {} — corrupt \
             or format-incompatible cache",
            hex32(&got),
            hex32(&AI_POW_V0_VERIFIER_SETUP_TABLE_DIGEST),
        )));
    }
    Ok(got)
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

    /// The fingerprint hash is order-independent: the same shape keys in any order
    /// produce the same digest (buckets are sorted by shape key first).
    #[test]
    fn fingerprint_hash_is_order_independent() {
        let a = vec![
            (VerifierSetupShapeKey::new(1usize << 13, true), d(0x11)),
            (VerifierSetupShapeKey::new(1usize << 15, false), d(0x22)),
            (VerifierSetupShapeKey::new(1usize << 19, true), d(0x33)),
        ];
        let mut b = a.clone();
        b.reverse();
        let c = vec![a[1], a[2], a[0]];
        assert_eq!(hash_table_fingerprints(&a), hash_table_fingerprints(&b));
        assert_eq!(hash_table_fingerprints(&a), hash_table_fingerprints(&c));
    }

    /// The hash is sensitive to every field: a changed height, sx-bound bit,
    /// digest, or dropped bucket all change the result (no silent collisions).
    #[test]
    fn fingerprint_hash_is_sensitive() {
        let base = vec![
            (VerifierSetupShapeKey::new(1usize << 13, true), d(0x11)),
            (VerifierSetupShapeKey::new(1usize << 15, false), d(0x22)),
        ];
        let h0 = hash_table_fingerprints(&base);

        let mut v = base.clone();
        v[0].1 = d(0x12);
        assert_ne!(h0, hash_table_fingerprints(&v));

        let mut v = base.clone();
        v[0].0.trace_height = 1 << 14;
        assert_ne!(h0, hash_table_fingerprints(&v));

        let mut v = base.clone();
        v[0].0.sx_bound = !v[0].0.sx_bound;
        assert_ne!(h0, hash_table_fingerprints(&v));

        assert_ne!(h0, hash_table_fingerprints(&base[..1]));

        let mut v = base.clone();
        v.push((VerifierSetupShapeKey::new(1usize << 19, true), d(0x33)));
        assert_ne!(h0, hash_table_fingerprints(&v));
    }

    /// A single-bucket table has a stable, reproducible digest (determinism of the
    /// pure hash itself).
    #[test]
    fn fingerprint_hash_is_reproducible() {
        let v = vec![(VerifierSetupShapeKey::new(1usize << 16, false), d(0xab))];
        assert_eq!(hash_table_fingerprints(&v), hash_table_fingerprints(&v));
    }

    #[test]
    fn setup_table_digest_rejects_empty_tables() {
        let empty_setup: Vec<AiPowVerifierSetup> = Vec::new();
        let err = verifier_setup_table_digest(&empty_setup).expect_err("empty setup table rejects");
        assert!(err.0.contains("empty verifier-setup table"));

        let empty_seeds: Vec<ai_pow_zk::recursion::AiPowCompactVerifierSetupSeed> = Vec::new();
        let err =
            verifier_setup_seed_table_digest(&empty_seeds).expect_err("empty seed table rejects");
        assert!(err.0.contains("empty verifier-setup seed table"));
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

//! Consensus fingerprint for the versioned AI-PoW verifier-setup table.
//!
//! The verifier-setup table is a CONSENSUS PARAMETER: every node must verify
//! `%ai-pow` blocks against byte-identical verifier keys, or two nodes could
//! disagree on a block's validity and fork. This module pins that parameter with
//! a committed BLAKE3 digest ([`AI_POW_V1_VERIFIER_SETUP_TABLE_DIGEST`]) computed
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

use ai_pow_zk::proof_rules::ProofRules;
use ai_pow_zk::recursion::{
    compact_batch_verifier_key_digest_to_bytes, AI_POW_COMPACT_BATCH_VERIFIER_KEY_DIGEST_BYTES,
};

use crate::setup::SetupError;
use crate::{AiPowVerifierSetup, VerifierSetupShapeKey};

/// One bucket's canonical 40-byte verifier-key digest.
type BucketDigest = [u8; AI_POW_COMPACT_BATCH_VERIFIER_KEY_DIGEST_BYTES];

/// Original framing retained to verify that historical keys do not change.
const TABLE_DIGEST_DOMAIN_V0: &[u8] = b"ai-pow-v0-verifier-setup-table-digest\0";

/// The committed BLAKE3 digest of the canonical **v0** AI-PoW verifier-setup table
/// (the reachable `(trace_height, sx_bound)` keys produced by
/// [`crate::setup::production_verifier_setup_buckets`]).
///
/// The Legacy subset must match this original framing at startup and cache load,
/// independently of the combined versioned table fingerprint below.
pub const AI_POW_V0_VERIFIER_SETUP_TABLE_DIGEST: [u8; 32] = [
    // Measured by `boot_generate_full_production_table` on the reference build
    // (generate the production shape-key table -> cache -> rebuild -> hash). Re-running
    // that ignored test regenerates the table from scratch and asserts this exact
    // value, so it doubles as a cross-run reproducibility gate.
    0x57, 0xfb, 0x17, 0x3a, 0xd5, 0xc7, 0x0c, 0x63, 0x82, 0xaa, 0xb7, 0xdd, 0x84, 0xdd, 0x0b, 0xf0,
    0xf6, 0x69, 0x12, 0xe8, 0x47, 0x2b, 0xa4, 0x29, 0xb2, 0xd3, 0x24, 0x39, 0x81, 0xce, 0x46, 0xd7,
];

/// Version 1 pins the verifier keys for both supported rule versions.
/// Measured by the full-table test.
pub const AI_POW_V1_VERIFIER_SETUP_TABLE_DIGEST: [u8; 32] = [
    0x86, 0xb3, 0x9d, 0x69, 0xb0, 0x36, 0xbe, 0x80, 0xf2, 0x28, 0x32, 0x5b, 0xc0, 0xba, 0x4c, 0xd7,
    0x3e, 0xa3, 0xa3, 0xcc, 0xc6, 0x6e, 0xa8, 0x0f, 0xbc, 0x25, 0x86, 0x14, 0xe1, 0x41, 0xba, 0xb6,
];
const TABLE_DIGEST_DOMAIN_V1: &[u8] = b"ai-pow-v1-verifier-setup-table-digest\0";

pub fn v1_digest_is_pinned() -> bool {
    AI_POW_V1_VERIFIER_SETUP_TABLE_DIGEST != [0u8; 32]
}

/// Whether the historical reference digest has been pinned.
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
    hasher.update(TABLE_DIGEST_DOMAIN_V1);
    hasher.update(&(sorted.len() as u64).to_le_bytes());
    for (key, d) in &sorted {
        hasher.update(&[match key.rules {
            ProofRules::Legacy => 0,
            ProofRules::Hardened => 1,
        }]);
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
        let binding = s.context.validate_setup_binding().map_err(|error| {
            SetupError(format!(
                "verifier setup at {key:?} failed binding: {error:?}"
            ))
        })?;
        let ctx_digest = compact_batch_verifier_key_digest_to_bytes(&binding);
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

fn descriptor_fingerprints(
    descriptors: &[ai_pow_zk::recursion::AiPowCompactVerifierSetupDescriptor],
) -> Result<Vec<(VerifierSetupShapeKey, BucketDigest)>, SetupError> {
    if descriptors.is_empty() {
        return Err(SetupError(
            "cannot fingerprint an empty verifier-setup descriptor table".to_string(),
        ));
    }
    let mut fps = Vec::with_capacity(descriptors.len());
    for descriptor in descriptors {
        descriptor
            .validate_shape()
            .map_err(|error| SetupError(format!("invalid verifier setup shape: {error:?}")))?;
        let mut key =
            VerifierSetupShapeKey::from_zk_params(&descriptor.zk_params, descriptor.trace_height())
                .ok_or_else(|| SetupError("descriptor has zero noise rank".to_string()))?;
        key.rules = descriptor.rules;
        if fps.iter().any(|(prior, _)| *prior == key) {
            return Err(SetupError(format!(
                "duplicate verifier setup shape {key:?}"
            )));
        }
        let digest = descriptor
            .verifier_key_digest_bytes
            .as_slice()
            .try_into()
            .map_err(|_| SetupError(format!("verifier digest at {key:?} must be 40 bytes")))?;
        fps.push((key, digest));
    }
    Ok(fps)
}

/// Fingerprint shape descriptors. A context load or rebuild must independently
/// recompute and match each descriptor's digest before using it.
pub fn verifier_setup_descriptor_table_digest(
    descriptors: &[ai_pow_zk::recursion::AiPowCompactVerifierSetupDescriptor],
) -> Result<[u8; 32], SetupError> {
    Ok(hash_table_fingerprints(&descriptor_fingerprints(
        descriptors,
    )?))
}

fn hash_legacy_fingerprints(fps: &[(VerifierSetupShapeKey, BucketDigest)]) -> [u8; 32] {
    let mut legacy = fps
        .iter()
        .filter(|(key, _)| key.rules == ProofRules::Legacy)
        .collect::<Vec<_>>();
    legacy.sort_by_key(|(key, _)| *key);
    let mut hasher = blake3::Hasher::new();
    hasher.update(TABLE_DIGEST_DOMAIN_V0);
    hasher.update(&(legacy.len() as u64).to_le_bytes());
    for (key, digest) in legacy {
        hasher.update(&(key.trace_height as u64).to_le_bytes());
        hasher.update(&[u8::from(key.sx_bound)]);
        hasher.update(digest);
    }
    *hasher.finalize().as_bytes()
}

pub fn legacy_verifier_setup_descriptor_table_digest(
    descriptors: &[ai_pow_zk::recursion::AiPowCompactVerifierSetupDescriptor],
) -> Result<[u8; 32], SetupError> {
    Ok(hash_legacy_fingerprints(&descriptor_fingerprints(
        descriptors,
    )?))
}

pub(crate) fn verify_production_fingerprints(
    fps: &[(VerifierSetupShapeKey, BucketDigest)],
) -> Result<[u8; 32], SetupError> {
    if fps.len() != 28
        || [ProofRules::Legacy, ProofRules::Hardened]
            .into_iter()
            .any(|rules| {
                (13..=19).any(|log2| {
                    [false, true].into_iter().any(|sx| {
                        !fps.iter().any(|(key, _)| {
                            *key == VerifierSetupShapeKey::with_rules(1 << log2, sx, rules)
                        })
                    })
                })
            })
    {
        return Err(SetupError(
            "verifier table must contain all 28 canonical rule/shape keys".to_string(),
        ));
    }
    let historical = hash_legacy_fingerprints(fps);
    if historical != AI_POW_V0_VERIFIER_SETUP_TABLE_DIGEST {
        return Err(SetupError(format!(
            "historical verifier table digest mismatch: {} != {}",
            hex32(&historical),
            hex32(&AI_POW_V0_VERIFIER_SETUP_TABLE_DIGEST),
        )));
    }
    let current = hash_table_fingerprints(fps);
    if current != AI_POW_V1_VERIFIER_SETUP_TABLE_DIGEST {
        return Err(SetupError(format!(
            "versioned verifier table digest mismatch: {} != {}",
            hex32(&current),
            hex32(&AI_POW_V1_VERIFIER_SETUP_TABLE_DIGEST),
        )));
    }
    Ok(current)
}

/// Require complete canonical coverage and both committed fingerprints.
pub fn verify_verifier_setup_descriptor_table_digest(
    descriptors: &[ai_pow_zk::recursion::AiPowCompactVerifierSetupDescriptor],
) -> Result<[u8; 32], SetupError> {
    let representatives = crate::setup::production_verifier_setup_geometries()?;
    if descriptors.iter().any(|descriptor| {
        !representatives.contains(&(descriptor.zk_params, descriptor.trace_height()))
    }) {
        return Err(SetupError(
            "cache contains noncanonical production geometry".to_string(),
        ));
    }
    verify_production_fingerprints(&descriptor_fingerprints(descriptors)?)
}

/// Validate every native binding, complete canonical coverage, and both
/// committed fingerprints before installing a generated table.
pub fn verify_verifier_setup_table_digest(
    table: &[AiPowVerifierSetup],
) -> Result<[u8; 32], SetupError> {
    verifier_setup_table_digest(table)?;
    let fps = table
        .iter()
        .map(|setup| {
            (
                setup.shape_key(),
                compact_batch_verifier_key_digest_to_bytes(setup.context.verifier_key_digest()),
            )
        })
        .collect::<Vec<_>>();
    verify_production_fingerprints(&fps)
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

        let mut v = base.clone();
        v[0].0.rules = ProofRules::Legacy;
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

        let empty_descriptors: Vec<ai_pow_zk::recursion::AiPowCompactVerifierSetupDescriptor> =
            Vec::new();
        let err = verifier_setup_descriptor_table_digest(&empty_descriptors)
            .expect_err("empty descriptor table rejects");
        assert!(err.0.contains("empty verifier-setup descriptor table"));
    }

    /// A shipping build MUST have the consensus digest pinned to a real (non-zero)
    /// value — the boot check refuses to run against the all-zero placeholder, so a
    /// build that forgot to pin it would never validate a real table.
    #[test]
    fn consensus_digests_are_pinned() {
        assert!(
            v1_digest_is_pinned(),
            "v1 table must be measured and pinned before release"
        );
        assert!(
            v0_digest_is_pinned(),
            "AI_POW_V0_VERIFIER_SETUP_TABLE_DIGEST must be pinned to the measured value, \
             not the all-zero placeholder",
        );
    }
}

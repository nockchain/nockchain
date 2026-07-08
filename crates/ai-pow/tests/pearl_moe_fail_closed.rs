//! Track A (A1/A2) — Pearl V2 MoE-aware `MiningConfiguration` trailer + public
//! data decode.
//!
//! The MoE hard fork repurposed the 32-byte `MiningConfiguration` trailer as
//! `e(2 LE) | top_k(2 LE) | zero-padding(28)` (Pearl
//! `zk-pow/src/api/proof_utils.rs::MiningConfiguration::from_bytes`). `e == 0`
//! is a standard (dense) job — byte-identical to the pre-MoE all-zero trailer —
//! and `e > 0` selects GROUPED_GEMM (MoE), which Nockchain does not support yet.
//!
//! These tests pin two things:
//!   * **Dense byte-parity** — a dense (all-zero-trailer) config round-trips and
//!     serializes exactly as before (trailer stays 32 zero bytes).
//!   * **MoE fail-closed** — every non-dense trailer / MoE public-data shape is
//!     rejected with a precise error, before any proving.

use ai_pow::params::MatmulParams;
use ai_pow::pearl_compat::{
    validate_pearl_merge_config_for_recursive_prover, verify_pearl_compatible_work,
    PearlCompatError, PearlIncompleteBlockHeader, PearlMiningConfig, PearlMoeConfig,
    PearlPeriodicPattern, PearlPublicProofParams, PEARL_MINING_CONFIG_RESERVED_SIZE,
    PEARL_MINING_CONFIG_SIZE, PEARL_MMA_INT7XINT7_TO_INT32, PEARL_PUBLIC_PROOF_PARAMS_SIZE,
};

fn dense_config() -> PearlMiningConfig {
    PearlMiningConfig {
        common_dim: 1024,
        rank: 64,
        mma_type: PEARL_MMA_INT7XINT7_TO_INT32,
        rows_pattern: PearlPeriodicPattern {
            shape: [(1, 16), (16, 1), (16, 1)],
        },
        cols_pattern: PearlPeriodicPattern {
            shape: [(1, 16), (16, 1), (16, 1)],
        },
        reserved: [0u8; PEARL_MINING_CONFIG_RESERVED_SIZE],
    }
}

fn dense_header() -> PearlIncompleteBlockHeader {
    PearlIncompleteBlockHeader {
        version: 0x2000_0000,
        prev_block: [1u8; 32],
        merkle_root: [2u8; 32],
        timestamp: 1_700_000_000,
        nbits: 0x207f_ffff,
    }
}

fn dense_public_params() -> PearlPublicProofParams {
    PearlPublicProofParams {
        block_header: dense_header(),
        mining_config: dense_config(),
        hash_a: [3u8; 32],
        hash_b: [4u8; 32],
        hash_jackpot: [5u8; 32],
        m: 128,
        n: 128,
        t_rows: 0,
        t_cols: 0,
    }
}

/// A1 — a dense config round-trips, and its serialized trailer is 32 zero bytes.
/// This is the standing dense-byte-parity guarantee: the MoE-aware trailer parse
/// must not perturb the pre-MoE encoding of a standard job.
#[test]
fn dense_config_round_trips_with_all_zero_trailer() {
    let config = dense_config();
    let bytes = config.to_bytes().expect("dense config serializes");
    assert_eq!(bytes.len(), PEARL_MINING_CONFIG_SIZE);
    assert_eq!(
        &bytes[20..52],
        &[0u8; PEARL_MINING_CONFIG_RESERVED_SIZE],
        "dense trailer must remain 32 zero bytes"
    );
    let restored = PearlMiningConfig::from_bytes(&bytes).expect("dense config decodes");
    assert_eq!(restored, config);
}

/// B3a — an `e > 0` trailer now *parses* as GROUPED_GEMM (Pearl-faithful decode),
/// round-trips byte-for-byte, and is exposed via `moe()` — but block-acceptance
/// stays fail-closed (the recursive prover refuses it until the circuit lands).
#[test]
fn moe_config_parses_and_acceptance_fails_closed() {
    for (e, top_k) in [(1u16, 0u16), (8, 2), (256, 4), (u16::MAX, u16::MAX)] {
        let mut bytes = dense_config().to_bytes().unwrap();
        bytes[20..22].copy_from_slice(&e.to_le_bytes());
        bytes[22..24].copy_from_slice(&top_k.to_le_bytes());

        // Decode now succeeds and exposes the MoE config.
        let config = PearlMiningConfig::from_bytes(&bytes).expect("MoE config decodes");
        assert_eq!(config.moe(), Some(PearlMoeConfig { e, top_k }));
        // Round-trips byte-for-byte.
        assert_eq!(config.to_bytes().unwrap(), bytes);

        // Block-acceptance (recursive prover) refuses MoE.
        assert_eq!(
            validate_pearl_merge_config_for_recursive_prover(
                &config,
                &MatmulParams::TEST_SMALL,
                4096
            ),
            Err(PearlCompatError::UnsupportedRecursivePearlParams(
                "MoE (GROUPED_GEMM) recursive proving is not implemented"
            )),
            "e={e} top_k={top_k} recursive acceptance must fail closed"
        );
    }

    // A dense config still exposes `moe() == None`.
    assert_eq!(dense_config().moe(), None);
}

/// A1 — `top_k != 0` while `e == 0` is malformed (mirrors Pearl's
/// `ensure!(e != 0 || top_k == 0)`).
#[test]
fn nonzero_top_k_without_experts_is_rejected() {
    let mut bytes = dense_config().to_bytes().unwrap();
    bytes[22..24].copy_from_slice(&7u16.to_le_bytes());
    assert_eq!(
        PearlMiningConfig::from_bytes(&bytes),
        Err(PearlCompatError::MoeTopKWithoutExperts(7)),
    );
}

/// A1 — nonzero padding in the true reserved region (bytes 4..32 of the trailer)
/// is still rejected as `NonzeroReserved`.
#[test]
fn nonzero_reserved_padding_is_rejected() {
    for pad_idx in [4usize, 5, 16, 31] {
        let mut bytes = dense_config().to_bytes().unwrap();
        bytes[20 + pad_idx] = 0xAB;
        assert_eq!(
            PearlMiningConfig::from_bytes(&bytes),
            Err(PearlCompatError::NonzeroReserved),
            "nonzero trailer pad at {pad_idx} must be NonzeroReserved"
        );
    }
}

/// B3a — `to_bytes` round-trips an MoE trailer (via `moe_trailer`), but still
/// rejects a malformed trailer with nonzero padding.
#[test]
fn to_bytes_round_trips_moe_and_rejects_bad_pad() {
    let mut config = dense_config();
    config.reserved = PearlMiningConfig::moe_trailer(4, 2);
    let bytes = config.to_bytes().expect("MoE trailer serializes");
    assert_eq!(&bytes[20..22], &4u16.to_le_bytes());
    assert_eq!(&bytes[22..24], &2u16.to_le_bytes());
    assert_eq!(&bytes[24..52], &[0u8; 28]);
    assert_eq!(PearlMiningConfig::from_bytes(&bytes).unwrap(), config);

    let mut padded = dense_config();
    padded.reserved[8] = 1;
    assert_eq!(padded.to_bytes(), Err(PearlCompatError::NonzeroReserved));
}

/// A2 — dense 164-byte public data decodes and round-trips (V2 dense core is
/// byte-identical to V1).
#[test]
fn dense_public_data_round_trips() {
    let params = dense_public_params();
    let bytes = params.to_public_data().expect("serialize dense public data");
    assert_eq!(bytes.len(), PEARL_PUBLIC_PROOF_PARAMS_SIZE);
    let restored = PearlPublicProofParams::from_public_data(dense_header(), &bytes)
        .expect("decode dense public data");
    assert_eq!(restored, params);
}

/// A2 — MoE public data fails closed with `UnsupportedMoeConfig`, not a
/// misleading length error, whether it is exactly the 164-byte core with an MoE
/// trailer or carries the variable-length MoE tail.
#[test]
fn moe_public_data_is_rejected_fail_closed() {
    // (a) 164-byte core, but the mining-config trailer selects MoE.
    let mut core = dense_public_params().to_public_data().unwrap().to_vec();
    core[20..22].copy_from_slice(&6u16.to_le_bytes());
    core[22..24].copy_from_slice(&2u16.to_le_bytes());
    assert_eq!(
        PearlPublicProofParams::from_public_data(dense_header(), &core),
        Err(PearlCompatError::UnsupportedMoeConfig { e: 6, top_k: 2 }),
    );

    // (b) MoE tail appended (len > 164) — must surface the MoE error, not
    // `BadPublicParamsLen`.
    let mut with_tail = core.clone();
    with_tail.extend_from_slice(&[0u8; 40]);
    assert_eq!(
        PearlPublicProofParams::from_public_data(dense_header(), &with_tail),
        Err(PearlCompatError::UnsupportedMoeConfig { e: 6, top_k: 2 }),
    );
}

/// B5-gate (soundness) — an MoE statement is fail-closed end-to-end through the
/// shared-work verify entrypoint (`sanity_check` guard fires before any ticket
/// compute or recursive acceptance). No MoE proof can be accepted until the
/// recursive circuit (Track B5) binds the routing commitment + grouped matmul.
#[test]
fn moe_statement_is_fail_closed_through_verify() {
    let mut public = dense_public_params();
    public.mining_config.reserved = PearlMiningConfig::moe_trailer(4, 2);
    // Empty matrices: the MoE guard rejects before any matrix access.
    let target = [0xffu8; 32];
    assert_eq!(
        verify_pearl_compatible_work(&public, &[], &[], &target, 4096),
        Err(PearlCompatError::UnsupportedMoeConfig { e: 4, top_k: 2 }),
    );
}

/// A2 — a genuinely-wrong length on a dense (e==0) statement still reports
/// `BadPublicParamsLen` (the MoE peek must not swallow ordinary length errors).
#[test]
fn dense_wrong_length_still_reports_bad_len() {
    let mut short = dense_public_params().to_public_data().unwrap().to_vec();
    short.truncate(160);
    assert_eq!(
        PearlPublicProofParams::from_public_data(dense_header(), &short),
        Err(PearlCompatError::BadPublicParamsLen(160)),
    );
}

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

use ai_pow::pearl_compat::{
    PearlCompatError, PearlIncompleteBlockHeader, PearlMiningConfig, PearlPeriodicPattern,
    PearlPublicProofParams, PEARL_MINING_CONFIG_RESERVED_SIZE, PEARL_MINING_CONFIG_SIZE,
    PEARL_MMA_INT7XINT7_TO_INT32, PEARL_PUBLIC_PROOF_PARAMS_SIZE,
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

/// A1 — `e > 0` in the trailer is GROUPED_GEMM and must fail closed with the
/// precise `UnsupportedMoeConfig`, carrying the decoded `e` / `top_k`.
#[test]
fn moe_config_trailer_is_rejected_fail_closed() {
    for (e, top_k) in [(1u16, 0u16), (8, 2), (256, 4), (u16::MAX, u16::MAX)] {
        let mut bytes = dense_config().to_bytes().unwrap();
        bytes[20..22].copy_from_slice(&e.to_le_bytes());
        bytes[22..24].copy_from_slice(&top_k.to_le_bytes());
        assert_eq!(
            PearlMiningConfig::from_bytes(&bytes),
            Err(PearlCompatError::UnsupportedMoeConfig { e, top_k }),
            "e={e} top_k={top_k} must fail closed as UnsupportedMoeConfig"
        );
    }
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

/// A1 — `to_bytes` is symmetric: a struct whose `reserved` encodes an MoE
/// trailer fails closed the same way as decode, so we can never emit an
/// unsupported MoE mining config.
#[test]
fn to_bytes_fails_closed_on_moe_reserved() {
    let mut config = dense_config();
    config.reserved[0..2].copy_from_slice(&4u16.to_le_bytes());
    config.reserved[2..4].copy_from_slice(&2u16.to_le_bytes());
    assert_eq!(
        config.to_bytes(),
        Err(PearlCompatError::UnsupportedMoeConfig { e: 4, top_k: 2 }),
    );

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

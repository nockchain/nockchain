//! Track B3b — Pearl V2 MoE `public_data` wire codec.
//!
//! Byte layout (Pearl `PublicProofParams::to_wire_bytes` / `from_wire_bytes`):
//! ```text
//! core(164) | expert_idx(2) | routing_offsets[e]·(4) | hash_routing(32)
//!           | outer_count(1) | outer_indices[oc]·(4)
//! ```
//! These tests pin the exact layout, bounds, and validation. The codec decodes /
//! encodes MoE public data byte-compatibly; it does **not** accept an MoE proof
//! (recursive acceptance stays fail-closed until Track B5).

use ai_pow::pearl_compat::{
    PearlCompatError, PearlIncompleteBlockHeader, PearlMiningConfig, PearlMoeParams,
    PearlPeriodicPattern, PearlPublicProofParams, PEARL_MINING_CONFIG_RESERVED_SIZE,
    PEARL_MMA_INT7XINT7_TO_INT32, PEARL_MOE_MAX_NUM_EXPERTS, PEARL_MOE_MAX_OUTER_INDICES,
    PEARL_MOE_MAX_WIRE_SIZE, PEARL_MOE_MIN_WIRE_SIZE, PEARL_PUBLIC_PROOF_PARAMS_SIZE,
};

fn pat() -> PearlPeriodicPattern {
    PearlPeriodicPattern {
        shape: [(1, 16), (16, 1), (16, 1)],
    }
}

fn header() -> PearlIncompleteBlockHeader {
    PearlIncompleteBlockHeader {
        version: 0x2000_0000,
        prev_block: [1u8; 32],
        merkle_root: [2u8; 32],
        timestamp: 1_700_000_000,
        nbits: 0x207f_ffff,
    }
}

fn moe_core(e: u16, top_k: u16) -> PearlPublicProofParams {
    PearlPublicProofParams {
        block_header: header(),
        mining_config: PearlMiningConfig {
            common_dim: 1024,
            rank: 64,
            mma_type: PEARL_MMA_INT7XINT7_TO_INT32,
            rows_pattern: pat(),
            cols_pattern: pat(),
            reserved: PearlMiningConfig::moe_trailer(e, top_k),
        },
        hash_a: [3u8; 32],
        hash_b: [4u8; 32],
        hash_jackpot: [5u8; 32],
        m: 128,
        n: 128,
        t_rows: 0,
        t_cols: 0,
    }
}

fn moe_params(e: usize, oc: usize, expert_idx: u16) -> PearlMoeParams {
    PearlMoeParams {
        expert_idx,
        routing_offsets: (0..e).map(|i| (i as u32 + 1) * 10).collect(),
        hash_routing: [6u8; 32],
        outer_indices: (0..oc).map(|i| i as u32 * 7 + 3).collect(),
    }
}

#[test]
fn wire_constants_match_pearl() {
    assert_eq!(PEARL_MOE_MIN_WIRE_SIZE, 199); // 164 + 2 + 32 + 1
    assert_eq!(PEARL_MOE_MAX_NUM_EXPERTS, 1024);
    assert_eq!(PEARL_MOE_MAX_OUTER_INDICES, 128);
    assert_eq!(PEARL_MOE_MAX_WIRE_SIZE, 199 + 128 * 4 + 1024 * 4); // 4807
}

#[test]
fn round_trips_across_shapes() {
    for &(e, oc) in &[(1usize, 0usize), (1, 3), (4, 8), (8, 128), (256, 1)] {
        let core = moe_core(e as u16, 2);
        let moe = moe_params(e, oc, (e as u16) - 1);
        let wire = core.to_wire_bytes_moe(&moe).expect("encode");
        assert_eq!(wire.len(), PEARL_MOE_MIN_WIRE_SIZE + e * 4 + oc * 4);
        let (dec_core, dec_moe) =
            PearlPublicProofParams::from_wire_bytes_moe(header(), &wire).expect("decode");
        assert_eq!(dec_core, core, "core round-trips (e={e}, oc={oc})");
        assert_eq!(dec_moe, moe, "moe params round-trip (e={e}, oc={oc})");
        // Re-encode is byte-identical (injective).
        assert_eq!(dec_core.to_wire_bytes_moe(&dec_moe).unwrap(), wire);
    }
}

#[test]
fn exact_byte_layout() {
    let core = moe_core(3, 2);
    let moe = PearlMoeParams {
        expert_idx: 1,
        routing_offsets: vec![10, 20, 30],
        hash_routing: [0xABu8; 32],
        outer_indices: vec![7, 42],
    };
    let wire = core.to_wire_bytes_moe(&moe).unwrap();

    // Core (164) is exactly the dense public-data serialization (with MoE trailer).
    assert_eq!(&wire[..PEARL_PUBLIC_PROOF_PARAMS_SIZE], &core.to_public_data().unwrap());
    // Trailer discriminant e in the core.
    assert_eq!(&wire[20..22], &3u16.to_le_bytes());

    let mut off = PEARL_PUBLIC_PROOF_PARAMS_SIZE;
    assert_eq!(&wire[off..off + 2], &1u16.to_le_bytes()); // expert_idx
    off += 2;
    for v in [10u32, 20, 30] {
        assert_eq!(&wire[off..off + 4], &v.to_le_bytes());
        off += 4;
    }
    assert_eq!(&wire[off..off + 32], &[0xABu8; 32]); // hash_routing
    off += 32;
    assert_eq!(wire[off], 2); // outer_count
    off += 1;
    for v in [7u32, 42] {
        assert_eq!(&wire[off..off + 4], &v.to_le_bytes());
        off += 4;
    }
    assert_eq!(off, wire.len());
}

#[test]
fn encode_rejects_mismatched_expert_count() {
    let core = moe_core(4, 2);
    let mut moe = moe_params(4, 2, 0);
    moe.routing_offsets.pop(); // now len 3 != e 4
    assert_eq!(
        core.to_wire_bytes_moe(&moe),
        Err(PearlCompatError::MoeExpertCountMismatch {
            expected: 4,
            actual: 3
        })
    );
}

#[test]
fn encode_rejects_dense_config() {
    let mut core = moe_core(2, 1);
    core.mining_config.reserved = [0u8; PEARL_MINING_CONFIG_RESERVED_SIZE]; // dense
    let moe = moe_params(2, 1, 0);
    assert_eq!(
        core.to_wire_bytes_moe(&moe),
        Err(PearlCompatError::MoePublicMissingConfig)
    );
}

#[test]
fn encode_rejects_expert_idx_and_outer_bounds() {
    let core = moe_core(4, 2);
    // expert_idx out of range.
    let moe = moe_params(4, 2, 4);
    assert_eq!(
        core.to_wire_bytes_moe(&moe),
        Err(PearlCompatError::MoeExpertIdxOutOfRange {
            expert_idx: 4,
            e: 4
        })
    );
    // outer_indices too long.
    let moe = moe_params(4, PEARL_MOE_MAX_OUTER_INDICES + 1, 0);
    assert_eq!(
        core.to_wire_bytes_moe(&moe),
        Err(PearlCompatError::MoeOuterIndicesExceedMax(
            PEARL_MOE_MAX_OUTER_INDICES + 1
        ))
    );
}

#[test]
fn decode_rejects_truncated_and_trailing() {
    let core = moe_core(4, 2);
    let moe = moe_params(4, 3, 0);
    let wire = core.to_wire_bytes_moe(&moe).unwrap();

    // Too short (below the fixed minimum).
    let short = &wire[..PEARL_MOE_MIN_WIRE_SIZE - 1];
    assert!(matches!(
        PearlPublicProofParams::from_wire_bytes_moe(header(), short),
        Err(PearlCompatError::MoeWireTooShort { .. })
    ));

    // Trailing byte (length mismatch).
    let mut extra = wire.clone();
    extra.push(0);
    assert!(matches!(
        PearlPublicProofParams::from_wire_bytes_moe(header(), &extra),
        Err(PearlCompatError::MoeWireLengthMismatch { .. })
    ));

    // Truncated in the middle of the offsets/tail.
    let mid = &wire[..PEARL_MOE_MIN_WIRE_SIZE + 4]; // only one offset present of four
    assert!(matches!(
        PearlPublicProofParams::from_wire_bytes_moe(header(), mid),
        Err(PearlCompatError::MoeWireTooShort { .. })
            | Err(PearlCompatError::MoeWireLengthMismatch { .. })
    ));
}

#[test]
fn decode_rejects_dense_core() {
    // A 199+ byte blob whose core trailer is dense (e==0) is not an MoE wire.
    let mut core = moe_core(2, 1);
    let moe = moe_params(2, 0, 0);
    let mut wire = core.to_wire_bytes_moe(&moe).unwrap();
    // Zero the trailer discriminant → dense core.
    wire[20..24].copy_from_slice(&[0u8; 4]);
    core.mining_config.reserved = [0u8; PEARL_MINING_CONFIG_RESERVED_SIZE];
    let _ = core; // silence unused
    assert_eq!(
        PearlPublicProofParams::from_wire_bytes_moe(header(), &wire),
        Err(PearlCompatError::MoePublicMissingConfig)
    );
}

/// B4 — MoE difficulty pricing is identical to dense: Pearl
/// `extract_difficulty_bound` prices by `h·w·dot_product_length`, none of which
/// depend on the MoE config, so the MoE trailer must not perturb the target.
#[test]
fn moe_difficulty_pricing_equals_dense() {
    let dense = {
        let mut c = moe_core(4, 2);
        c.mining_config.reserved = [0u8; PEARL_MINING_CONFIG_RESERVED_SIZE];
        c
    };
    let moe = moe_core(4, 2);
    assert_eq!(
        dense.difficulty_adjustment_factor().unwrap(),
        moe.difficulty_adjustment_factor().unwrap(),
        "h*w*dot adjustment must not depend on the MoE trailer"
    );
    assert_eq!(
        dense.pearl_adjusted_target().unwrap(),
        moe.pearl_adjusted_target().unwrap()
    );
    let nockchain_target = [0x11u8; 32];
    assert_eq!(
        dense.nockchain_adjusted_target(&nockchain_target).unwrap(),
        moe.nockchain_adjusted_target(&nockchain_target).unwrap()
    );
}

#[test]
fn decode_rejects_out_of_matrix_offsets() {
    let mut core = moe_core(2, 1);
    core.t_rows = 200; // >= m (128)
    let moe = moe_params(2, 1, 0);
    // Encoding does not check t_rows<m, but decoding does (Pearl from_wire_bytes).
    let wire = core.to_wire_bytes_moe(&moe).unwrap();
    assert!(matches!(
        PearlPublicProofParams::from_wire_bytes_moe(header(), &wire),
        Err(PearlCompatError::InvalidPatternOffset) | Err(PearlCompatError::PatternOutOfMatrix)
    ));
}

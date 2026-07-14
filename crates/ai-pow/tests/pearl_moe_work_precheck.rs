//! M3 (node-side) — `verify_pearl_moe_compatible_work`: the node's cheap,
//! pre-proof MoE (GROUPED_GEMM) work verification, and specifically its
//! **jackpot/difficulty binding** (the one genuinely-new soundness gate on the
//! MoE compact path).
//!
//! The recursive verify (`verify_pearl_moe_compact_recursive_certificate`) binds
//! matmul correctness + opened schedule + routing-spliced seeds, but — like the
//! dense recursive verify — does NOT check difficulty. That gate is the node
//! precheck's job: recompute the opened tile from the PUBLIC schedule + the
//! spliced seeds and require `jackpot == hash_jackpot`, then `jackpot ≤ target`.
//!
//! These tests validate that binding WITHOUT a recursive proof by cross-checking
//! the precheck's recomputed jackpot/seeds against `compute_pearl_moe_ticket`
//! (the same tile the miner commits to, validated independently in
//! `pearl_moe_tile.rs`). The recursive-proof half is covered by
//! `zk_bridge::real_moe_compact_recursive_certificate_proves_and_verifies`.

use ai_pow::pearl_compat::{
    compute_pearl_moe_ticket, derive_pearl_work_commitments, verify_pearl_moe_compatible_work,
    PearlCompatError, PearlIncompleteBlockHeader, PearlMiningConfig, PearlMoeParams,
    PearlPeriodicPattern, PearlPublicProofParams, PEARL_MINING_CONFIG_RESERVED_SIZE,
    PEARL_MMA_INT7XINT7_TO_INT32,
};
use ai_pow::pearl_moe_routing::build_routing_data;

// Envelope-valid MoE dims: k=1024 (≥1024, mult of 64), r=64 (pow2, 32..1024, mult
// of PEARL_TILE_D=16), h=w=16 (mult of PEARL_TILE_H=2, h·w=256 = PEARL_HW_MAX),
// m=n=128, e=2 experts (n_e=64), top_k=1.
const M: usize = 128;
const K: usize = 1024;
const N_E: usize = 64;
const E: usize = 2;
const R: usize = 64;
const TOP_K: usize = 1;
const HW: usize = 16; // opened tile is 16×16
const MAX_PATTERN_LEN: usize = 4096;
const EXPERT_IDX: usize = 0;

/// Deterministic int7-range matrices (no RNG — resume-safe, reproducible).
fn synth_matrix(seed: u64, len: usize) -> Vec<i8> {
    let mut s = seed | 1;
    (0..len)
        .map(|_| {
            // xorshift-ish LCG, folded into [-8, 7]
            s ^= s << 13;
            s ^= s >> 7;
            s ^= s << 17;
            ((s % 16) as i8) - 8
        })
        .collect()
}

fn header() -> PearlIncompleteBlockHeader {
    PearlIncompleteBlockHeader {
        version: 0x2000_0000,
        prev_block: [7u8; 32],
        merkle_root: [9u8; 32],
        timestamp: 1_700_000_123,
        // very easy target so the jackpot passes with a loose nockchain target
        nbits: 0x207f_ffff,
    }
}

fn moe_config() -> PearlMiningConfig {
    let pat: Vec<u32> = (0..HW as u32).collect(); // [0,1,...,15] ⇒ size()=16
    PearlMiningConfig {
        common_dim: K as u32,
        rank: R as u16,
        mma_type: PEARL_MMA_INT7XINT7_TO_INT32,
        rows_pattern: PearlPeriodicPattern::from_list(&pat).unwrap(),
        cols_pattern: PearlPeriodicPattern::from_list(&pat).unwrap(),
        reserved: PearlMiningConfig::moe_trailer(E as u16, TOP_K as u16),
    }
}

/// Build the full consistent fixture: matrices, routing, derived commitments, the
/// committed MoE ticket, the public statement, and the MoE artifact params.
/// Returns everything a caller needs to exercise `verify_pearl_moe_compatible_work`.
struct Fixture {
    a: Vec<i8>,
    b: Vec<i8>,
    routing_data: Vec<u32>,
    public_params: PearlPublicProofParams,
    moe: PearlMoeParams,
    ticket_jackpot: [u8; 32],
    ticket_s_a: [u8; 32],
}

fn build_fixture() -> Fixture {
    let n = N_E * E;
    let a = synth_matrix(0xA11CE, M * K);
    let b = synth_matrix(0xB0B, n * K);

    // top_k=1: token t → expert (t % E). Expert 0 owns even tokens.
    let topk: Vec<u32> = (0..M).map(|t| (t % E) as u32).collect();
    let routing = build_routing_data(&topk, M, TOP_K, E).unwrap();

    let config = moe_config();
    let sigma = header().to_bytes();
    let mu = config.to_bytes().unwrap();
    let commitments = derive_pearl_work_commitments(&sigma, &mu, &a, &b);

    // The opened schedule the PUBLIC patterns select (t_rows = t_cols = 0).
    let inner: Vec<u32> = config
        .rows_pattern
        .indices_with_offset_bounded(0, MAX_PATTERN_LEN)
        .unwrap();
    let local_b: Vec<u32> = config
        .cols_pattern
        .indices_with_offset_bounded(0, MAX_PATTERN_LEN)
        .unwrap();

    let ticket = compute_pearl_moe_ticket(
        &commitments.kappa,
        &commitments.h_a,
        &commitments.h_b,
        &a,
        &b,
        &routing,
        EXPERT_IDX,
        &inner,
        &local_b,
        N_E,
        K,
        R,
        K, // dot_product_length == common_dim here (rank | k)
    )
    .expect("compute MoE ticket");

    let public_params = PearlPublicProofParams {
        block_header: header(),
        mining_config: config,
        hash_a: commitments.h_a,
        hash_b: commitments.h_b,
        hash_jackpot: ticket.jackpot_hash,
        m: M as u32,
        n: n as u32,
        t_rows: 0,
        t_cols: 0,
    };

    let moe = PearlMoeParams {
        expert_idx: EXPERT_IDX as u16,
        routing_offsets: routing.routing_offsets.clone(),
        hash_routing: ticket.commitment.routing_root,
        outer_indices: ticket.outer_indices.clone(),
    };

    Fixture {
        a,
        b,
        routing_data: routing.routing_data.clone(),
        public_params,
        moe,
        ticket_jackpot: ticket.jackpot_hash,
        ticket_s_a: ticket.s_a,
    }
}

const LOOSE_TARGET: [u8; 32] = [0xffu8; 32];

/// Happy path: the node precheck recomputes exactly the miner's committed jackpot
/// and routing-spliced `s_A`, binds them to the public statement, and passes the
/// (loose) difficulty target.
#[test]
fn moe_work_precheck_recomputes_and_binds_jackpot() {
    let f = build_fixture();
    let pre = verify_pearl_moe_compatible_work(
        &f.public_params,
        &f.a,
        &f.b,
        &f.moe,
        &f.routing_data,
        &LOOSE_TARGET,
        MAX_PATTERN_LEN,
    )
    .expect("valid MoE work must verify");

    assert_eq!(
        pre.jackpot_hash, f.ticket_jackpot,
        "node-recomputed jackpot must equal the miner's committed tile jackpot"
    );
    assert_eq!(
        pre.s_a, f.ticket_s_a,
        "node-recomputed routing-spliced s_A must equal the ticket's s_A"
    );
    assert_eq!(pre.jackpot_hash, f.public_params.hash_jackpot);
}

/// A tampered `hash_jackpot` (statement claims a jackpot the tile does not produce)
/// is rejected — the node binds the recomputed tile, not the prover's claim.
#[test]
fn moe_work_precheck_rejects_wrong_hash_jackpot() {
    let mut f = build_fixture();
    f.public_params.hash_jackpot[0] ^= 0x01;
    assert_eq!(
        verify_pearl_moe_compatible_work(
            &f.public_params,
            &f.a,
            &f.b,
            &f.moe,
            &f.routing_data,
            &LOOSE_TARGET,
            MAX_PATTERN_LEN,
        ),
        Err(PearlCompatError::JackpotHashMismatch),
    );
}

/// A jackpot that does not meet difficulty (target = 0 ⇒ adjusted target 0) is
/// rejected with `NockchainTargetNotMet`.
#[test]
fn moe_work_precheck_rejects_unmet_difficulty() {
    let f = build_fixture();
    let zero_target = [0u8; 32];
    assert_eq!(
        verify_pearl_moe_compatible_work(
            &f.public_params,
            &f.a,
            &f.b,
            &f.moe,
            &f.routing_data,
            &zero_target,
            MAX_PATTERN_LEN,
        ),
        Err(PearlCompatError::NockchainTargetNotMet),
    );
}

/// Forged routing data (valid tokens, but no longer committing to `hash_routing`)
/// is rejected at the routing-consistency binding, before the tile recompute.
#[test]
fn moe_work_precheck_rejects_forged_routing() {
    let f = build_fixture();
    let mut bad = f.routing_data.clone();
    bad[0] ^= 1;
    assert!(
        verify_pearl_moe_compatible_work(
            &f.public_params,
            &f.a,
            &f.b,
            &f.moe,
            &bad,
            &LOOSE_TARGET,
            MAX_PATTERN_LEN,
        )
        .is_err(),
        "forged routing must be rejected by the routing-consistency binding",
    );
}

/// A dense (`e == 0`) statement must NOT be accepted by the MoE work path — the
/// MoE config lookup fails closed.
#[test]
fn moe_work_precheck_rejects_dense_config() {
    let mut f = build_fixture();
    f.public_params.mining_config.reserved = [0u8; PEARL_MINING_CONFIG_RESERVED_SIZE];
    assert!(
        verify_pearl_moe_compatible_work(
            &f.public_params,
            &f.a,
            &f.b,
            &f.moe,
            &f.routing_data,
            &LOOSE_TARGET,
            MAX_PATTERN_LEN,
        )
        .is_err(),
        "a dense config must not verify through the MoE work path",
    );
}

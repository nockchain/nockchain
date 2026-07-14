//! M3 (node-side) — `verify_pearl_moe_compatible_work`: the node's cheap,
//! pre-proof MoE (GROUPED_GEMM) work verification, over ARBITRARY miner-chosen
//! matrices (option (a), Pearl parity — no synth pin).
//!
//! The precheck is model-agnostic: it takes the miner's COMMITTED matrix roots
//! (`public_params.hash_a`/`hash_b`) — never re-deriving matrices — validates the
//! MoE envelope + the routing-consistency binding, recomputes the routing-spliced
//! `s_A`/`s_B` from those commitments, and gates difficulty on the authenticated
//! `hash_jackpot`. It does NOT recompute the tile: the recursive certificate
//! (`verify_pearl_moe_compact_recursive_certificate`) proves `pis.hash_jackpot` is
//! the opened tile's real output over the committed matrices, and the node caller
//! binds `pis.hash_jackpot == public_params.hash_jackpot`. The commitment-keyed
//! noise (Pearl `ffi/mine.rs`) is the anti-grind, so arbitrary/degenerate matrices
//! are safe.
//!
//! These tests use matrices from a non-production seed (an arbitrary model) and
//! verify the precheck binds their COMMITMENTS and the routing splice. The
//! proof-half + the forged-`hash_jackpot` rejection are covered by
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
    routing_data: Vec<u32>,
    public_params: PearlPublicProofParams,
    moe: PearlMoeParams,
    ticket_jackpot: [u8; 32],
    ticket_s_a: [u8; 32],
    committed_h_a: [u8; 32],
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
        routing_data: routing.routing_data.clone(),
        public_params,
        moe,
        ticket_jackpot: ticket.jackpot_hash,
        ticket_s_a: ticket.s_a,
        committed_h_a: commitments.h_a,
    }
}

const LOOSE_TARGET: [u8; 32] = [0xffu8; 32];

/// Happy path (arbitrary model): the precheck binds the miner's COMMITTED matrix
/// roots (never a synth re-derivation), recomputes the routing-spliced `s_A` from
/// them, surfaces the authenticated jackpot, and passes the (loose) difficulty
/// target. `committed_h_a` is the commitment of the non-production-seed matrices.
#[test]
fn moe_work_precheck_binds_committed_commitments_and_splice() {
    let f = build_fixture();
    let pre = verify_pearl_moe_compatible_work(
        &f.public_params,
        &f.moe,
        &f.routing_data,
        &LOOSE_TARGET,
        MAX_PATTERN_LEN,
    )
    .expect("valid MoE work must verify");

    assert_eq!(
        pre.commitments.h_a, f.committed_h_a,
        "precheck must bind the miner's COMMITTED H_A (model-agnostic), not a synth one"
    );
    assert_eq!(
        pre.commitments.h_a, f.public_params.hash_a,
        "committed H_A comes straight from the authenticated statement"
    );
    assert_eq!(
        pre.s_a, f.ticket_s_a,
        "routing-spliced s_A (from the committed H_A + routing) must equal the ticket's"
    );
    assert_eq!(
        pre.jackpot_hash, f.public_params.hash_jackpot,
        "precheck surfaces the authenticated statement jackpot (caller binds it to the proof)"
    );
    assert_eq!(pre.jackpot_hash, f.ticket_jackpot);
}

// NOTE: a tampered `hash_jackpot` is no longer a precheck concern (there is no tile
// recompute over a fixed matrix set). It is caught by (i) the recursive certificate,
// which proves `pis.hash_jackpot` (zk_bridge de-risk: a forged value rejects), and
// (ii) the node caller, which binds `pis.hash_jackpot == public_params.hash_jackpot`.
// A statement jackpot ABOVE target is still rejected here — see
// `moe_work_precheck_rejects_unmet_difficulty`.

/// A jackpot that does not meet difficulty (target = 0 ⇒ adjusted target 0) is
/// rejected with `NockchainTargetNotMet`.
#[test]
fn moe_work_precheck_rejects_unmet_difficulty() {
    let f = build_fixture();
    let zero_target = [0u8; 32];
    assert_eq!(
        verify_pearl_moe_compatible_work(
            &f.public_params,
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
            &f.moe,
            &f.routing_data,
            &LOOSE_TARGET,
            MAX_PATTERN_LEN,
        )
        .is_err(),
        "a dense config must not verify through the MoE work path",
    );
}

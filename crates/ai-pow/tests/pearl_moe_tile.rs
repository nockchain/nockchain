//! Track B3d — off-circuit MoE grouped-tile reference (`compute_moe_tile`).
//!
//! Validation strategy (no live Pearl vector required — see residual note in
//! `docs/2026-07-07_PEARL_MOE_TRACK_B_SPEC.md`):
//!   * **Dense-equivalence** — the grouped-tile compute is byte-identical to the
//!     already-validated dense per-tile compute (`compute_pearl_pattern_ticket`)
//!     when given the same opened indices and seeds. This pins the compute path.
//!   * **MoE self-consistency** — routing → `outer_indices` gather feeds the tile;
//!     the MoE routing-commitment splice (`s_a`) actually changes the tile, and
//!     every opened-index / seed perturbation binds.

#![allow(clippy::unwrap_used)] // integration test: unwrap is acceptable
use ai_pow::commit::matrix_commitment;
use ai_pow::fiat_shamir::{
    canonical_noise_seeds_moe, moe_hash_activations, moe_hash_routing, noise_seed_a, noise_seed_b,
};
use ai_pow::params::MatmulParams;
use ai_pow::pearl_compat::{
    compute_moe_tile, compute_pearl_moe_ticket, compute_pearl_pattern_ticket,
    derive_pearl_work_commitments, PearlIncompleteBlockHeader, PearlMiningConfig,
    PearlPeriodicPattern, PearlPublicProofParams, PEARL_MINING_CONFIG_RESERVED_SIZE,
    PEARL_MMA_INT7XINT7_TO_INT32,
};
use ai_pow::pearl_moe_routing::build_routing_data;
use ai_pow::synth::synth_matrices;

fn header() -> PearlIncompleteBlockHeader {
    PearlIncompleteBlockHeader {
        version: 0x2000_0000,
        prev_block: [1u8; 32],
        merkle_root: [2u8; 32],
        timestamp: 1_700_000_000,
        nbits: 0x207f_ffff,
    }
}

/// The grouped-tile compute reduces to the validated dense per-tile compute when
/// given the dense ticket's opened rows/cols and the dense seeds.
#[test]
fn compute_moe_tile_matches_dense_ticket_compute() {
    let params = MatmulParams {
        m: 128,
        k: 1024,
        n: 128,
        noise_rank: 64,
        tile: 8,
        spot_checks: 1,
        difficulty_bits: 0,
    };
    let (a, b) = synth_matrices(b"pearl-moe-tile-dense-equiv", &params);
    let config = PearlMiningConfig {
        common_dim: params.k,
        rank: params.noise_rank as u16,
        mma_type: PEARL_MMA_INT7XINT7_TO_INT32,
        rows_pattern: PearlPeriodicPattern::from_list(&[0, 1, 8, 9, 64, 65, 72, 73]).unwrap(),
        cols_pattern: PearlPeriodicPattern::from_list(&[0, 1, 8, 9, 64, 65, 72, 73]).unwrap(),
        reserved: [0u8; PEARL_MINING_CONFIG_RESERVED_SIZE],
    };
    let commitments =
        derive_pearl_work_commitments(&header().to_bytes(), &config.to_bytes().unwrap(), &a, &b);
    let public = PearlPublicProofParams {
        block_header: header(),
        mining_config: config,
        hash_a: commitments.h_a,
        hash_b: commitments.h_b,
        hash_jackpot: [0u8; 32],
        m: params.m,
        n: params.n,
        t_rows: 0,
        t_cols: 0,
    };
    let ticket = compute_pearl_pattern_ticket(&public, &a, &b, &commitments, 32).unwrap();

    let k = params.k as usize;
    let r = params.noise_rank as usize;
    let dot = config.dot_product_length().unwrap();
    let (tile, jackpot) = compute_moe_tile(
        &a, &b, &ticket.a_rows, &ticket.b_cols, &commitments.s_a, &commitments.s_b, k, r, dot,
    );
    assert_eq!(
        tile, ticket.tile_state,
        "grouped compute == dense tile state"
    );
    assert_eq!(
        jackpot, ticket.jackpot_hash,
        "grouped compute == dense jackpot"
    );
}

// ── MoE self-consistency (small, fast) ──────────────────────────────────

#[test]
fn moe_tile_uses_routing_and_splice_end_to_end() {
    let (m, k, n_e, e, r) = (8usize, 64usize, 4usize, 2usize, 4usize);
    // top_k = 1 so experts get disjoint token sets (expert 0 = even tokens,
    // expert 1 = odd), making opened-row perturbations observable.
    let top_k = 1usize;
    // High-entropy Pearl-valid matrices: A is m×k, B is (n_e·e)×k column-major —
    // i.e. the full k×(n_e·e) weight matrix with one column per expert-column.
    let moe_params = MatmulParams {
        m: m as u32,
        k: k as u32,
        n: (n_e * e) as u32,
        noise_rank: r as u32,
        tile: 2,
        spot_checks: 1,
        difficulty_bits: 0,
    };
    let (a, b) = synth_matrices(b"pearl-moe-tile-self-consistency", &moe_params);

    // Routing: token t routed to expert t%e (top_k=1).
    let topk: Vec<u32> = (0..m).map(|t| (t % e) as u32).collect();
    let routing = build_routing_data(&topk, m, top_k, e).unwrap();

    let kappa = [0x42u8; 32];
    let a_bytes: Vec<u8> = a.iter().map(|&v| v as u8).collect();
    let b_bytes: Vec<u8> = b.iter().map(|&v| v as u8).collect();
    let h_a = matrix_commitment(&a_bytes, &kappa);
    let h_b = matrix_commitment(&b_bytes, &kappa);
    let (s_a, s_b, _c) = canonical_noise_seeds_moe(
        &kappa,
        &h_a,
        &h_b,
        &routing.routing_data_le_bytes(),
        &routing.routing_offsets_le_bytes(),
    );

    // Open expert 1: inner rows 0,1 → global tokens; columns local 0,2 → global
    // (offset by expert_idx * n_e).
    let expert_idx = 1usize;
    let outer = routing.outer_indices(expert_idx, &[0, 1]).unwrap();
    let b_cols: Vec<u32> = [0u32, 2]
        .iter()
        .map(|c| c + (expert_idx * n_e) as u32)
        .collect();
    assert!(b_cols.iter().all(|&c| (c as usize) < n_e * e));

    let (tile, jackpot) = compute_moe_tile(&a, &b, &outer, &b_cols, &s_a, &s_b, k, r, k);

    // Deterministic.
    let (tile2, jackpot2) = compute_moe_tile(&a, &b, &outer, &b_cols, &s_a, &s_b, k, r, k);
    assert_eq!((tile, jackpot), (tile2, jackpot2));

    // The MoE splice binds: using the *dense* s_a (keyed off h_a, not
    // hash_activations) yields a different tile/jackpot.
    let dense_s_b = ai_pow::fiat_shamir::noise_seed_b(&kappa, &h_b);
    let dense_s_a = ai_pow::fiat_shamir::noise_seed_a(&dense_s_b, &h_a);
    let (dense_tile, dense_jackpot) =
        compute_moe_tile(&a, &b, &outer, &b_cols, &dense_s_a, &dense_s_b, k, r, k);
    assert_ne!(
        jackpot, dense_jackpot,
        "routing splice must change the jackpot"
    );
    assert_ne!(tile, dense_tile);

    // Opened-index perturbations bind.
    let other_outer = routing.outer_indices(0, &[0, 1]).unwrap();
    let (t_rows, _) = compute_moe_tile(&a, &b, &other_outer, &b_cols, &s_a, &s_b, k, r, k);
    assert_ne!(tile, t_rows, "different opened rows must change the tile");
    let other_cols = [1u32, 3]
        .iter()
        .map(|c| c + (expert_idx * n_e) as u32)
        .collect::<Vec<_>>();
    let (t_cols, _) = compute_moe_tile(&a, &b, &outer, &other_cols, &s_a, &s_b, k, r, k);
    assert_ne!(tile, t_cols, "different opened cols must change the tile");
}

/// End-to-end Rust MoE work ticket (B1 routing → B2 splice → B3d gather+tile),
/// plus the soundness-relevant property that the **verifier recomputes `s_a`
/// from public data only** (`kappa`, `h_a`, `moe.hash_routing` = routing_root,
/// `routing_offsets`) — no private `routing_data` — so the `hash_activations`
/// reroute needs no circuit change. (The remaining in-circuit requirement is the
/// `outer_indices`↔routing CTL; MoE stays fail-closed until it lands.)
#[test]
fn moe_ticket_end_to_end_and_verifier_recomputes_s_a_from_public_data() {
    use ai_pow::commit::matrix_commitment;
    use ai_pow::params::MatmulParams;
    use ai_pow::pearl_moe_routing::build_routing_data;

    let (m, k, n_e, e, r) = (8usize, 64usize, 4usize, 2usize, 4usize);
    let top_k = 1usize;
    let moe_params = MatmulParams {
        m: m as u32,
        k: k as u32,
        n: (n_e * e) as u32,
        noise_rank: r as u32,
        tile: 2,
        spot_checks: 1,
        difficulty_bits: 0,
    };
    let (a, b) = synth_matrices(b"pearl-moe-ticket-e2e", &moe_params);
    let topk: Vec<u32> = (0..m).map(|t| (t % e) as u32).collect();
    let routing = build_routing_data(&topk, m, top_k, e).unwrap();

    let kappa = [0x42u8; 32];
    let a_bytes: Vec<u8> = a.iter().map(|&v| v as u8).collect();
    let b_bytes: Vec<u8> = b.iter().map(|&v| v as u8).collect();
    let h_a = matrix_commitment(&a_bytes, &kappa);
    let h_b = matrix_commitment(&b_bytes, &kappa);

    let expert_idx = 1usize;
    let inner = [0u32, 1];
    let local_b = [0u32, 2];
    let ticket = compute_pearl_moe_ticket(
        &kappa, &h_a, &h_b, &a, &b, &routing, expert_idx, &inner, &local_b, n_e, k, r, k,
    )
    .unwrap();

    // Ticket internally consistent: gather + expert column offset + splice.
    assert_eq!(
        ticket.outer_indices,
        routing.outer_indices(expert_idx, &inner).unwrap()
    );
    assert_eq!(
        ticket.b_cols_global,
        vec![(expert_idx * n_e) as u32, 2 + (expert_idx * n_e) as u32]
    );
    assert_eq!(
        ticket.s_a,
        noise_seed_a(&ticket.s_b, &ticket.commitment.hash_activations)
    );

    // Verifier recomputes s_a from PUBLIC data only (no private routing_data):
    // routing_root is public (moe.hash_routing); routing_offsets is public.
    let hash_offsets = matrix_commitment(&routing.routing_offsets_le_bytes(), &kappa);
    let hash_routing = moe_hash_routing(&ticket.commitment.routing_root, &hash_offsets);
    let hash_activations = moe_hash_activations(&h_a, &hash_routing);
    let s_b_pub = noise_seed_b(&kappa, &h_b);
    let s_a_pub = noise_seed_a(&s_b_pub, &hash_activations);
    assert_eq!(
        s_a_pub, ticket.s_a,
        "verifier recomputes s_a from public MoE params"
    );

    // Routing binds the jackpot: a different routing yields a different result.
    let topk2: Vec<u32> = (0..m).map(|t| ((t + 1) % e) as u32).collect();
    let routing2 = build_routing_data(&topk2, m, top_k, e).unwrap();
    let ticket2 = compute_pearl_moe_ticket(
        &kappa, &h_a, &h_b, &a, &b, &routing2, expert_idx, &inner, &local_b, n_e, k, r, k,
    )
    .unwrap();
    assert_ne!(
        ticket.jackpot_hash, ticket2.jackpot_hash,
        "routing binds the jackpot"
    );
}

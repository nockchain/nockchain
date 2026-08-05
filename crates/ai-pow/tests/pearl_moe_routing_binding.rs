//! Adversarial tests for the MoE routing-consistency binding
//! (`verify_pearl_moe_routing_binding`), the soundness gate that prevents a
//! prover from opening arbitrary A-rows and claiming they are an expert's routed
//! tokens. Every forgery path must be rejected.

#![allow(clippy::unwrap_used)] // integration test: unwrap is acceptable
use ai_pow::commit::matrix_commitment;
use ai_pow::pearl_compat::{
    moe_expert_b_cols_global, verify_pearl_moe_routing_binding, PearlCompatError,
    PearlIncompleteBlockHeader, PearlMiningConfig, PearlMoeParams, PearlPeriodicPattern,
    PearlPublicProofParams, PEARL_MMA_INT7XINT7_TO_INT32, PEARL_MOE_MAX_ROUTING_ENTRIES,
};
use ai_pow::pearl_moe_routing::{build_routing_data, RoutingData};

const KAPPA: [u8; 32] = [0x11u8; 32];
const M: u32 = 8;
const TOP_K: usize = 1;
const E: usize = 2;

/// Valid setup: 8 tokens, top_k=1, 2 experts (token t → expert t%2).
/// expert 0 tokens = [0,2,4,6]; the row pattern [0,1] opens the first two.
fn valid() -> (PearlMiningConfig, RoutingData, PearlMoeParams) {
    let topk: Vec<u32> = (0..M).map(|t| t % E as u32).collect();
    let routing = build_routing_data(&topk, M as usize, TOP_K, E).unwrap();
    // routing_data = [0,2,4,6, 1,3,5,7]; routing_offsets = [4,8].
    assert_eq!(routing.routing_data, vec![0, 2, 4, 6, 1, 3, 5, 7]);
    assert_eq!(routing.routing_offsets, vec![4, 8]);

    let hash_routing = matrix_commitment(&routing.routing_data_le_bytes(), &KAPPA);
    let config = PearlMiningConfig {
        common_dim: 1024,
        rank: 64,
        mma_type: PEARL_MMA_INT7XINT7_TO_INT32,
        rows_pattern: PearlPeriodicPattern::from_list(&[0, 1]).unwrap(),
        cols_pattern: PearlPeriodicPattern::from_list(&[0, 1]).unwrap(),
        reserved: PearlMiningConfig::moe_trailer(E as u16, TOP_K as u16),
    };
    // expert 0, pattern [0,1] → outer_indices = routing_data[0..2] = [0,2].
    let moe = PearlMoeParams {
        expert_idx: 0,
        routing_offsets: routing.routing_offsets.clone(),
        hash_routing,
        outer_indices: vec![0, 2],
    };
    (config, routing, moe)
}

fn check(
    config: &PearlMiningConfig,
    routing: &RoutingData,
    moe: &PearlMoeParams,
) -> Result<(), PearlCompatError> {
    verify_pearl_moe_routing_binding(&KAPPA, config, moe, M, 0, &routing.routing_data, 4096)
}

#[test]
fn valid_routing_binding_accepts() {
    let (config, routing, moe) = valid();
    check(&config, &routing, &moe).expect("valid MoE routing binding accepts");
    // Expert 1 (tokens [1,3,5,7]) with the same pattern → outer [1,3].
    let mut moe1 = moe.clone();
    moe1.expert_idx = 1;
    moe1.outer_indices = vec![1, 3];
    check(&config, &routing, &moe1).expect("expert 1 binding accepts");
}

#[test]
fn forged_outer_indices_rejected() {
    let (config, routing, mut moe) = valid();
    // Prover opens A-rows [4,6] (real A rows, but NOT expert 0's first two tokens).
    moe.outer_indices = vec![4, 6];
    assert_eq!(
        check(&config, &routing, &moe),
        Err(PearlCompatError::MoeOuterIndicesMismatch)
    );
    // Even one wrong entry is caught.
    let (config, routing, mut moe) = valid();
    moe.outer_indices = vec![0, 4];
    assert_eq!(
        check(&config, &routing, &moe),
        Err(PearlCompatError::MoeOuterIndicesMismatch)
    );
}

#[test]
fn cross_expert_forgery_rejected() {
    // expert_idx=0 but claiming expert 1's tokens [1,3] must fail.
    let (config, routing, mut moe) = valid();
    moe.outer_indices = vec![1, 3];
    assert_eq!(
        check(&config, &routing, &moe),
        Err(PearlCompatError::MoeOuterIndicesMismatch)
    );
}

/// Pearl acceptance-set parity. `top_k >= e` (each token routed to at
/// least as many experts as exist) is rejected by Pearl `sanity_checks.rs`; we
/// must reject it too or we accept a routing Pearl rejects (merge-mining divergence).
#[test]
fn top_k_not_less_than_experts_rejected() {
    let (e, top_k, m) = (2usize, 2usize, 4u32); // top_k == e (invalid)
                                                // Each token → both experts; grouped: expert 0 = [0,1,2,3], expert 1 = [0,1,2,3].
    let routing_data: Vec<u32> = vec![0, 1, 2, 3, 0, 1, 2, 3];
    let routing_data_le: Vec<u8> = routing_data.iter().flat_map(|v| v.to_le_bytes()).collect();
    let config = PearlMiningConfig {
        common_dim: 1024,
        rank: 64,
        mma_type: PEARL_MMA_INT7XINT7_TO_INT32,
        rows_pattern: PearlPeriodicPattern::from_list(&[0, 1]).unwrap(),
        cols_pattern: PearlPeriodicPattern::from_list(&[0, 1]).unwrap(),
        reserved: PearlMiningConfig::moe_trailer(e as u16, top_k as u16),
    };
    let moe = PearlMoeParams {
        expert_idx: 0,
        routing_offsets: vec![4, 8],
        hash_routing: matrix_commitment(&routing_data_le, &KAPPA),
        outer_indices: vec![0, 1],
    };
    assert_eq!(
        verify_pearl_moe_routing_binding(&KAPPA, &config, &moe, m, 0, &routing_data, 4096),
        Err(PearlCompatError::MoeTopKNotLessThanExperts { top_k, e })
    );
}

/// A token routed to the SAME expert twice makes that expert's span
/// exceed `m`. Pearl caps each expert at `m` tokens (`w[1]-w[0] <= m`); we must
/// reject the over-routing too. Here expert 0 spans 3 slots for m=2.
#[test]
fn expert_span_exceeding_m_rejected() {
    let (e, top_k, m) = (3usize, 2usize, 2u32); // top_k < e, so the span check is reached
                                                // expert 0 = [0,0,1] (token 0 twice → span 3 > m), expert 1 = [1], expert 2 = [].
    let routing_data: Vec<u32> = vec![0, 0, 1, 1];
    let routing_data_le: Vec<u8> = routing_data.iter().flat_map(|v| v.to_le_bytes()).collect();
    let config = PearlMiningConfig {
        common_dim: 1024,
        rank: 64,
        mma_type: PEARL_MMA_INT7XINT7_TO_INT32,
        rows_pattern: PearlPeriodicPattern::from_list(&[0, 1]).unwrap(),
        cols_pattern: PearlPeriodicPattern::from_list(&[0, 1]).unwrap(),
        reserved: PearlMiningConfig::moe_trailer(e as u16, top_k as u16),
    };
    let moe = PearlMoeParams {
        expert_idx: 0,
        routing_offsets: vec![3, 4, 4],
        hash_routing: matrix_commitment(&routing_data_le, &KAPPA),
        outer_indices: vec![0, 0],
    };
    assert_eq!(
        verify_pearl_moe_routing_binding(&KAPPA, &config, &moe, m, 0, &routing_data, 4096),
        Err(PearlCompatError::MoeExpertSpanExceedsTokens {
            expert: 0,
            span: 3,
            m: 2
        })
    );
}

/// A short expert span can still repeat tokens. The pattern offsets 0 and 4
/// open strictly increasing tiles `[0,1,2,3]` and `[1,2,3,4]`, but reuse rows
/// 1, 2, and 3. Every expert span must therefore be strictly increasing.
#[test]
fn duplicate_expert_tokens_across_tiles_rejected() {
    let (m, h, e, top_k) = (8u32, 4u32, 2usize, 1usize);
    let routing_data: Vec<u32> = (0..m).map(|p| p / h + p % h).collect();
    let first_tile = &routing_data[..h as usize];
    let second_tile = routing_data[h as usize..].to_vec();
    assert_eq!(first_tile, &[0, 1, 2, 3]);
    assert_eq!(second_tile, vec![1, 2, 3, 4]);

    let rows_pattern = PearlPeriodicPattern::from_list(&[0, 1, 2, 3]).unwrap();
    assert!(rows_pattern.offset_is_valid(h));
    let config = PearlMiningConfig {
        common_dim: 1024,
        rank: 64,
        mma_type: PEARL_MMA_INT7XINT7_TO_INT32,
        rows_pattern,
        cols_pattern: PearlPeriodicPattern::from_list(&[0, 1]).unwrap(),
        reserved: PearlMiningConfig::moe_trailer(e as u16, top_k as u16),
    };
    let moe = PearlMoeParams {
        expert_idx: 0,
        routing_offsets: vec![m, m],
        hash_routing: matrix_commitment(
            &routing_data
                .iter()
                .flat_map(|v| v.to_le_bytes())
                .collect::<Vec<_>>(),
            &KAPPA,
        ),
        outer_indices: second_tile,
    };

    assert_eq!(
        verify_pearl_moe_routing_binding(&KAPPA, &config, &moe, m, h, &routing_data, 4096),
        Err(PearlCompatError::MoeRoutingNotStrictlyIncreasing)
    );
}

/// Column-within-expert bleed. The opened B-columns for expert
/// `expert_idx` must stay inside that expert's `[expert_idx·n_e, (expert_idx+1)·n_e)`
/// block. A `cols_pattern`/`t_cols` reaching `local ≥ n_e` bleeds into a
/// neighbouring expert's weights (a Pearl fork + a column-grinding lever) — the
/// downstream global `< n` check does not catch it.
fn cols_config(pattern: &[u32]) -> PearlMiningConfig {
    PearlMiningConfig {
        common_dim: 1024,
        rank: 64,
        mma_type: PEARL_MMA_INT7XINT7_TO_INT32,
        rows_pattern: PearlPeriodicPattern::from_list(&[0, 1]).unwrap(),
        cols_pattern: PearlPeriodicPattern::from_list(pattern).unwrap(),
        reserved: PearlMiningConfig::moe_trailer(2, 1),
    }
}

#[test]
fn moe_columns_use_pearl_per_expert_n_semantics() {
    // Pearl PublicProofParams::n is n_e, the width of one expert's B block.
    // With e=2 and n_e=4, expert 1 starts at global column 4.
    let cfg = cols_config(&[0, 1]);
    assert_eq!(
        moe_expert_b_cols_global(&cfg, 2, 4, 0, 0, 4096).unwrap(),
        vec![0, 1]
    );
    assert_eq!(
        moe_expert_b_cols_global(&cfg, 2, 4, 1, 0, 4096).unwrap(),
        vec![4, 5]
    );
    assert_eq!(
        moe_expert_b_cols_global(&cfg, 2, 4, 1, 2, 4096).unwrap(),
        vec![6, 7]
    );
}

#[test]
fn moe_column_bleed_via_t_cols_rejected() {
    // Public n is n_e=4. t_cols=4 reaches the next expert's block.
    let cfg = cols_config(&[0, 1]);
    assert_eq!(
        moe_expert_b_cols_global(&cfg, 2, 4, 0, 4, 4096),
        Err(PearlCompatError::MoeColumnOutsideExpert {
            local: 4,
            n_e: 4,
            expert_idx: 0
        })
    );
}

#[test]
fn moe_column_bleed_via_wide_pattern_rejected() {
    // A pattern whose local index reaches n_e (=4) opens the next expert's
    // first weight column and must be rejected.
    let cfg = cols_config(&[0, 4]);
    assert_eq!(
        moe_expert_b_cols_global(&cfg, 2, 4, 0, 0, 4096),
        Err(PearlCompatError::MoeColumnOutsideExpert {
            local: 4,
            n_e: 4,
            expert_idx: 0
        })
    );
}

#[test]
fn moe_per_expert_n_need_not_be_divisible_by_e() {
    // Pearl stores n_e directly; there is no n/e operation or divisibility
    // requirement. Three experts of width eight occupy 24 total columns.
    let cfg = cols_config(&[0, 1]);
    assert_eq!(
        moe_expert_b_cols_global(&cfg, 3, 8, 2, 0, 4096).unwrap(),
        vec![16, 17]
    );
}

#[test]
fn moe_public_wire_serializes_per_expert_n() {
    let mut config = cols_config(&[0, 1]);
    config.reserved = PearlMiningConfig::moe_trailer(3, 1);
    let public = PearlPublicProofParams {
        block_header: PearlIncompleteBlockHeader {
            version: 0,
            prev_block: [0; 32],
            merkle_root: [0; 32],
            timestamp: 0,
            nbits: 0x1e7f_ffff,
        },
        mining_config: config,
        hash_a: [1; 32],
        hash_b: [2; 32],
        hash_jackpot: [3; 32],
        m: 8,
        n: 8,
        t_rows: 0,
        t_cols: 0,
    };
    assert_eq!(public.total_b_cols().unwrap(), 24);
    assert_eq!(
        &public.to_public_data().unwrap()[152..156],
        &8u32.to_le_bytes()
    );
}

#[test]
fn tampered_routing_data_root_mismatch() {
    let (config, mut routing, moe) = valid();
    // Preserve expert-span ordering so the committed-root check is the first
    // rejection for this unauthenticated routing value.
    routing.routing_data[1] = 1;
    assert_eq!(
        check(&config, &routing, &moe),
        Err(PearlCompatError::MoeRoutingRootMismatch)
    );
}

#[test]
fn out_of_range_token_rejected() {
    let (config, mut routing, mut moe) = valid();
    routing.routing_data[0] = 100; // >= m
                                   // Recommit so the root check would pass; the token-range check fires first.
    moe.hash_routing = matrix_commitment(&routing.routing_data_le_bytes(), &KAPPA);
    assert!(matches!(
        check(&config, &routing, &moe),
        Err(PearlCompatError::MoeRoutingTokenOutOfRange {
            slot: 0,
            token: 100,
            m: 8
        })
    ));
}

#[test]
fn wrong_routing_data_length_rejected() {
    let (config, mut routing, mut moe) = valid();
    routing.routing_data.pop(); // now 7 != m*top_k=8
    moe.hash_routing = matrix_commitment(&routing.routing_data_le_bytes(), &KAPPA);
    assert!(matches!(
        check(&config, &routing, &moe),
        Err(PearlCompatError::MoeRoutingDataLenMismatch {
            expected: 8,
            actual: 7
        })
    ));
}

#[test]
fn inconsistent_offsets_rejected() {
    // Non-monotone offsets.
    let (config, routing, mut moe) = valid();
    moe.routing_offsets = vec![8, 4];
    assert_eq!(
        check(&config, &routing, &moe),
        Err(PearlCompatError::MoeOffsetsInconsistent)
    );
    // Last offset != m*top_k.
    let (config, routing, mut moe) = valid();
    moe.routing_offsets = vec![4, 7];
    assert_eq!(
        check(&config, &routing, &moe),
        Err(PearlCompatError::MoeOffsetsInconsistent)
    );
}

#[test]
fn wrong_expert_count_or_idx_rejected() {
    let (config, routing, mut moe) = valid();
    moe.routing_offsets = vec![8]; // len 1 != e=2
    assert!(matches!(
        check(&config, &routing, &moe),
        Err(PearlCompatError::MoeExpertCountMismatch {
            expected: 2,
            actual: 1
        })
    ));
    let (config, routing, mut moe) = valid();
    moe.expert_idx = 5; // >= e
    assert!(matches!(
        check(&config, &routing, &moe),
        Err(PearlCompatError::MoeExpertIdxOutOfRange {
            expert_idx: 5,
            e: 2
        })
    ));
}

#[test]
fn outer_indices_length_must_match_pattern() {
    let (config, routing, mut moe) = valid();
    moe.outer_indices = vec![0, 2, 4]; // pattern size is 2
    assert!(matches!(
        check(&config, &routing, &moe),
        Err(PearlCompatError::MoeOuterIndicesLenMismatch {
            expected: 2,
            actual: 3
        })
    ));
}

#[test]
fn pattern_position_beyond_expert_tokens_rejected() {
    // A pattern selecting position 5, but expert 0 only has 4 tokens (positions
    // 0..4) — position 5 would read into expert 1 / padding.
    let topk: Vec<u32> = (0..M).map(|t| t % E as u32).collect();
    let routing = build_routing_data(&topk, M as usize, TOP_K, E).unwrap();
    let hash_routing = matrix_commitment(&routing.routing_data_le_bytes(), &KAPPA);
    let config = PearlMiningConfig {
        common_dim: 1024,
        rank: 64,
        mma_type: PEARL_MMA_INT7XINT7_TO_INT32,
        rows_pattern: PearlPeriodicPattern::from_list(&[0, 5]).unwrap(),
        cols_pattern: PearlPeriodicPattern::from_list(&[0, 1]).unwrap(),
        reserved: PearlMiningConfig::moe_trailer(E as u16, TOP_K as u16),
    };
    // Even if the prover supplies the "matching" cross-expert token, it must be
    // rejected because position 5 is outside expert 0's [0,4) span.
    let moe = PearlMoeParams {
        expert_idx: 0,
        routing_offsets: routing.routing_offsets.clone(),
        hash_routing,
        outer_indices: vec![routing.routing_data[0], routing.routing_data[5]],
    };
    assert!(matches!(
        verify_pearl_moe_routing_binding(&KAPPA, &config, &moe, M, 0, &routing.routing_data, 4096),
        Err(PearlCompatError::MoeOuterIndexOutsideExpert {
            expert_idx: 0,
            pos: 5
        })
    ));
}

/// A config with `m·top_k` over [`PEARL_MOE_MAX_ROUTING_ENTRIES`] is rejected by
/// the DoS cap *before* the O(m·top_k) token loop / routing hash. The cap fires
/// ahead of the length / partition / root checks, so an empty `routing_data` and
/// placeholder offsets suffice — we are asserting the guard, not the data.
#[test]
fn routing_entries_exceeding_cap_rejected() {
    let m: u32 = (PEARL_MOE_MAX_ROUTING_ENTRIES as u32) + 1; // numel = m*1 > cap
    let config = PearlMiningConfig {
        common_dim: 1024,
        rank: 64,
        mma_type: PEARL_MMA_INT7XINT7_TO_INT32,
        rows_pattern: PearlPeriodicPattern::from_list(&[0, 1]).unwrap(),
        cols_pattern: PearlPeriodicPattern::from_list(&[0, 1]).unwrap(),
        reserved: PearlMiningConfig::moe_trailer(2, 1), // e=2, top_k=1
    };
    let moe = PearlMoeParams {
        expert_idx: 0,
        routing_offsets: vec![0, 0], // len == e; values irrelevant (cap fires first)
        hash_routing: [0u8; 32],
        outer_indices: vec![],
    };
    assert!(matches!(
        verify_pearl_moe_routing_binding(&KAPPA, &config, &moe, m, 0, &[], 4096),
        Err(PearlCompatError::MoeRoutingEntriesExceedMax { numel, max })
            if numel == m as u64 && max == PEARL_MOE_MAX_ROUTING_ENTRIES
    ));
}

/// At exactly the cap the entries-guard passes (it is `>`, not `>=`), so the
/// function proceeds to the length check — proving the boundary is admitted and
/// the cap is not off-by-one.
#[test]
fn routing_entries_at_cap_passes_entries_guard() {
    let m: u32 = PEARL_MOE_MAX_ROUTING_ENTRIES as u32; // numel = cap exactly
    let config = PearlMiningConfig {
        common_dim: 1024,
        rank: 64,
        mma_type: PEARL_MMA_INT7XINT7_TO_INT32,
        rows_pattern: PearlPeriodicPattern::from_list(&[0, 1]).unwrap(),
        cols_pattern: PearlPeriodicPattern::from_list(&[0, 1]).unwrap(),
        reserved: PearlMiningConfig::moe_trailer(2, 1),
    };
    let moe = PearlMoeParams {
        expert_idx: 0,
        routing_offsets: vec![0, 0],
        hash_routing: [0u8; 32],
        outer_indices: vec![],
    };
    // Empty routing_data at the cap fails the *length* check, not the entries cap.
    let err = verify_pearl_moe_routing_binding(&KAPPA, &config, &moe, m, 0, &[], 4096)
        .expect_err("empty routing_data at cap must still fail the length check");
    assert!(matches!(
        err,
        PearlCompatError::MoeRoutingDataLenMismatch { .. }
    ));
}

/// Pearl acceptance parity (`sanity_checks.rs:132`) — `outer_indices` must be
/// strictly increasing. An unsorted or duplicated opened set is Pearl-invalid;
/// accepting it would be a merge-mining divergence. The check fires before the
/// gather, so a Pearl-rejected order is caught even when it would otherwise
/// gather-match.
#[test]
fn unsorted_or_duplicate_outer_indices_rejected() {
    let (config, routing, mut moe) = valid();
    moe.outer_indices = vec![2, 0]; // valid is [0,2]; reversed is unsorted
    assert_eq!(
        check(&config, &routing, &moe),
        Err(PearlCompatError::MoeOuterIndicesNotSortedUnique)
    );
    let (config, routing, mut moe) = valid();
    moe.outer_indices = vec![0, 0]; // duplicate
    assert_eq!(
        check(&config, &routing, &moe),
        Err(PearlCompatError::MoeOuterIndicesNotSortedUnique)
    );
}

/// Pearl acceptance parity (`sanity_checks.rs:69`) — `top_k > 0`. The trailer
/// parse permits `top_k == 0` for `e > 0`, so the routing binding must reject the
/// degenerate no-routing config explicitly, matching Pearl.
#[test]
fn top_k_zero_rejected() {
    let (e, top_k, m) = (2usize, 0usize, 4u32);
    let config = PearlMiningConfig {
        common_dim: 1024,
        rank: 64,
        mma_type: PEARL_MMA_INT7XINT7_TO_INT32,
        rows_pattern: PearlPeriodicPattern::from_list(&[0, 1]).unwrap(),
        cols_pattern: PearlPeriodicPattern::from_list(&[0, 1]).unwrap(),
        reserved: PearlMiningConfig::moe_trailer(e as u16, top_k as u16),
    };
    // top_k=0 ⇒ m*top_k=0 ⇒ empty routing; reach the top_k guard with matching
    // empty routing_data + zero offsets.
    let moe = PearlMoeParams {
        expert_idx: 0,
        routing_offsets: vec![0, 0],
        hash_routing: [0u8; 32],
        outer_indices: vec![],
    };
    assert_eq!(
        verify_pearl_moe_routing_binding(&KAPPA, &config, &moe, m, 0, &[], 4096),
        Err(PearlCompatError::MoeTopKZero { e })
    );
}

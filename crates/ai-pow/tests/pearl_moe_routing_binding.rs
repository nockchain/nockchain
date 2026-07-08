//! Track B5b — adversarial tests for the MoE routing-consistency binding
//! (`verify_pearl_moe_routing_binding`), the soundness gate that prevents a
//! prover from opening arbitrary A-rows and claiming they are an expert's routed
//! tokens. Every forgery path must be rejected.

use ai_pow::commit::matrix_commitment;
use ai_pow::pearl_compat::{
    verify_pearl_moe_routing_binding, PearlCompatError, PearlMiningConfig, PearlMoeParams,
    PearlPeriodicPattern, PEARL_MINING_CONFIG_RESERVED_SIZE, PEARL_MMA_INT7XINT7_TO_INT32,
};
use ai_pow::pearl_moe_routing::{build_routing_data, RoutingData};

const KAPPA: [u8; 32] = [0x11u8; 32];
const M: u32 = 8;
const TOP_K: usize = 1;
const E: usize = 2;

/// Valid setup: 8 tokens, top_k=1, 2 experts (token t → expert t%2).
/// expert 0 tokens = [0,2,4,6]; the row pattern [0,1] opens the first two.
fn valid() -> (PearlMiningConfig, RoutingData, PearlMoeParams) {
    let topk: Vec<u32> = (0..M).map(|t| (t % E as u32)).collect();
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

fn check(config: &PearlMiningConfig, routing: &RoutingData, moe: &PearlMoeParams) -> Result<(), PearlCompatError> {
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
    assert_eq!(check(&config, &routing, &moe), Err(PearlCompatError::MoeOuterIndicesMismatch));
    // Even one wrong entry is caught.
    let (config, routing, mut moe) = valid();
    moe.outer_indices = vec![0, 4];
    assert_eq!(check(&config, &routing, &moe), Err(PearlCompatError::MoeOuterIndicesMismatch));
}

#[test]
fn cross_expert_forgery_rejected() {
    // expert_idx=0 but claiming expert 1's tokens [1,3] must fail.
    let (config, routing, mut moe) = valid();
    moe.outer_indices = vec![1, 3];
    assert_eq!(check(&config, &routing, &moe), Err(PearlCompatError::MoeOuterIndicesMismatch));
}

/// §D audit — Pearl acceptance-set parity. `top_k >= e` (each token routed to at
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

/// §D audit — a token routed to the SAME expert twice makes that expert's span
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
        Err(PearlCompatError::MoeExpertSpanExceedsTokens { expert: 0, span: 3, m: 2 })
    );
}

#[test]
fn tampered_routing_data_root_mismatch() {
    let (config, mut routing, moe) = valid();
    // Change routing_data so outer_indices' claimed source differs; hash_routing
    // (committed) no longer matches.
    routing.routing_data[1] = 0; // was 2
    assert_eq!(check(&config, &routing, &moe), Err(PearlCompatError::MoeRoutingRootMismatch));
}

#[test]
fn out_of_range_token_rejected() {
    let (config, mut routing, mut moe) = valid();
    routing.routing_data[0] = 100; // >= m
    // Recommit so the root check would pass; the token-range check fires first.
    moe.hash_routing = matrix_commitment(&routing.routing_data_le_bytes(), &KAPPA);
    assert!(matches!(
        check(&config, &routing, &moe),
        Err(PearlCompatError::MoeRoutingTokenOutOfRange { slot: 0, token: 100, m: 8 })
    ));
}

#[test]
fn wrong_routing_data_length_rejected() {
    let (config, mut routing, mut moe) = valid();
    routing.routing_data.pop(); // now 7 != m*top_k=8
    moe.hash_routing = matrix_commitment(&routing.routing_data_le_bytes(), &KAPPA);
    assert!(matches!(
        check(&config, &routing, &moe),
        Err(PearlCompatError::MoeRoutingDataLenMismatch { expected: 8, actual: 7 })
    ));
}

#[test]
fn inconsistent_offsets_rejected() {
    // Non-monotone offsets.
    let (config, routing, mut moe) = valid();
    moe.routing_offsets = vec![8, 4];
    assert_eq!(check(&config, &routing, &moe), Err(PearlCompatError::MoeOffsetsInconsistent));
    // Last offset != m*top_k.
    let (config, routing, mut moe) = valid();
    moe.routing_offsets = vec![4, 7];
    assert_eq!(check(&config, &routing, &moe), Err(PearlCompatError::MoeOffsetsInconsistent));
}

#[test]
fn wrong_expert_count_or_idx_rejected() {
    let (config, routing, mut moe) = valid();
    moe.routing_offsets = vec![8]; // len 1 != e=2
    assert!(matches!(
        check(&config, &routing, &moe),
        Err(PearlCompatError::MoeExpertCountMismatch { expected: 2, actual: 1 })
    ));
    let (config, routing, mut moe) = valid();
    moe.expert_idx = 5; // >= e
    assert!(matches!(
        check(&config, &routing, &moe),
        Err(PearlCompatError::MoeExpertIdxOutOfRange { expert_idx: 5, e: 2 })
    ));
}

#[test]
fn outer_indices_length_must_match_pattern() {
    let (config, routing, mut moe) = valid();
    moe.outer_indices = vec![0, 2, 4]; // pattern size is 2
    assert!(matches!(
        check(&config, &routing, &moe),
        Err(PearlCompatError::MoeOuterIndicesLenMismatch { expected: 2, actual: 3 })
    ));
}

#[test]
fn pattern_position_beyond_expert_tokens_rejected() {
    // A pattern selecting position 5, but expert 0 only has 4 tokens (positions
    // 0..4) — position 5 would read into expert 1 / padding.
    let topk: Vec<u32> = (0..M).map(|t| (t % E as u32)).collect();
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
        Err(PearlCompatError::MoeOuterIndexOutsideExpert { expert_idx: 0, pos: 5 })
    ));
}

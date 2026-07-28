//! Adversarial audit — degree-adaptive FRI profile selection.
//!
//! The prover and verifier both derive the Layer-0/recursion profile from the
//! (proof-bound) `trace_height` via `CircuitConfig::for_layer0_trace`. This pins:
//! (1) both degree classes hold the SAME 60-bit operational FRI floor — there is
//! no weaker profile to grind toward; (2) the crossover is exactly at degree 15;
//! (3) non-power-of-2 inputs round up identically on both sides (no boundary
//! desync). `trace_height` itself is bound by the node precheck
//! (`metadata.trace_height != expected_layer0_rows → reject`), so a prover cannot
//! choose a class.

use ai_pow_zk::CircuitConfig;

fn operational_bits(c: &CircuitConfig) -> u32 {
    c.operational_fri_bits()
}

#[test]
fn for_layer0_trace_boundary_floor_and_rounding() {
    // Both production profiles hold the identical 60-bit floor.
    assert_eq!(operational_bits(&CircuitConfig::PROD), 60);
    assert_eq!(operational_bits(&CircuitConfig::PROD_LB2_NQ30), 60);

    // Crossover at degree 15: ≤14 → lb=4 (PROD), ≥15 → lb=2 — and EVERY class is
    // 60-bit, so there is no soundness advantage to any degree label.
    for bits in 1u32..=14 {
        let c = CircuitConfig::for_layer0_trace(1usize << bits);
        assert_eq!(c.log_blowup, 4, "degree {bits} must select lb=4");
        assert_eq!(operational_bits(&c), 60, "degree {bits} floor");
    }
    for bits in 15u32..=20 {
        let c = CircuitConfig::for_layer0_trace(1usize << bits);
        assert_eq!(c.log_blowup, 2, "degree {bits} must select lb=2");
        assert_eq!(operational_bits(&c), 60, "degree {bits} floor");
    }

    // Non-power-of-2 `trace_len` rounds UP to the next degree, identically on both
    // sides (they call `for_layer0_trace` on the same bound value → no desync).
    // The real STARK trace is always a power of two, so this is purely defensive.
    assert_eq!(CircuitConfig::for_layer0_trace(1 << 14).log_blowup, 4); // exact 2^14
    assert_eq!(CircuitConfig::for_layer0_trace((1 << 14) + 1).log_blowup, 2); // → 2^15
    assert_eq!(CircuitConfig::for_layer0_trace((1 << 15) - 1).log_blowup, 2); // → 2^15
}

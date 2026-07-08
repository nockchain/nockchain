//! Phase 14b — LogUp-aware composite proof.
//!
//! Wires the M10.1c stack to `p3-batch-stark`, which is Plonky3's
//! standard wrapper that natively supports lookups via the
//! `InteractionBuilder` trait. Each AIR's `eval` declares the
//! lookups it participates in (sends / receives on named buses)
//! and `prove_batch` / `verify_batch` reify them as LogUp
//! constraints at proof time.
//!
//! ## Scope
//!
//! Phase 14b lands in two slices:
//!
//! 1. **Minimal POC (this module's tests).** A standalone AIR
//!    with a `URange8` table column + a single query cell
//!    demonstrates that:
//!      * Each AIR can emit interactions via `push_interaction`.
//!      * `prove_batch` / `verify_batch` accept the lookups.
//!      * Tampering a queried value with one out of range causes
//!        verification to fail.
//! 2. **Full composite wiring (follow-on).** Each existing chip
//!    gains an `eval_with_lookups` companion to its
//!    `eval_composite`, emitting interactions on the appropriate
//!    [`composite_lookups`](crate::composite_lookups) bus. The
//!    `CompositeFullAirWithLookups` AIR collects them all.

use p3_air::{Air, AirBuilder, BaseAir, WindowAccess};
use p3_field::PrimeCharacteristicRing;
use p3_lookup::InteractionBuilder;
use p3_matrix::dense::RowMajorMatrix;

use crate::Val;

/// Minimal lookup-bus demo AIR. Layout per row:
///
/// ```text
///   col 0: URANGE8_TABLE — enumerates 0, 1, ..., 255 then replays 255.
///   col 1: URANGE8_FREQ  — multiplicity each entry is consumed (the prover's claim).
///   col 2: QUERY         — a value the AIR queries against the table.
///   col 3: QUERY_FLAG    — 1 iff this row should query (acts as multiplicity).
/// ```
///
/// The AIR emits two interactions on the bus `"urange8"`:
///   * Table side: `(URANGE8_TABLE, −FREQ)` — send the entry with
///     count equal to the negative multiplicity (provider).
///   * Query side: `(QUERY, +QUERY_FLAG)` — receive the value with
///     positive count (consumer).
///
/// LogUp soundness: the global sum on bus `"urange8"` is zero iff
/// every query value appears in the table with the right
/// multiplicity. The prover commits to URANGE8_FREQ; if any
/// QUERY value is outside `[0, 256)`, no FREQ assignment can
/// balance the sum.
#[derive(Copy, Clone, Debug, Default)]
pub struct UrangeBusDemoAir;

const BUS_URANGE8: &str = "urange8";

const COL_TABLE: usize = 0;
const COL_FREQ: usize = 1;
const COL_QUERY: usize = 2;
const COL_QUERY_FLAG: usize = 3;
const WIDTH: usize = 4;

impl<F> BaseAir<F> for UrangeBusDemoAir {
    fn width(&self) -> usize {
        WIDTH
    }
}

impl<AB> Air<AB> for UrangeBusDemoAir
where
    AB: AirBuilder + InteractionBuilder,
{
    fn eval(&self, builder: &mut AB) {
        let main = builder.main();
        let cur = main.current_slice();
        let nxt = main.next_slice();

        // Table integrity: first row TABLE = 0, transition delta
        // is {0, 1}. Matches the existing `RangeTableChip`.
        builder.when_first_row().assert_zero(cur[COL_TABLE]);
        {
            let delta: AB::Expr = nxt[COL_TABLE].into() - cur[COL_TABLE].into();
            let one: AB::Expr = <AB::Expr as PrimeCharacteristicRing>::ONE;
            builder
                .when_transition()
                .assert_zero(delta.clone() * (delta - one));
        }

        // Booleanity on QUERY_FLAG so the multiplicity stays
        // sensible.
        builder.assert_bool(cur[COL_QUERY_FLAG]);

        // Table interaction: each row provides one table entry
        // with multiplicity = URANGE8_FREQ.
        builder.push_interaction(
            BUS_URANGE8,
            [<AB::Var as Into<AB::Expr>>::into(cur[COL_TABLE])],
            -<AB::Var as Into<AB::Expr>>::into(cur[COL_FREQ]),
            0, // count_weight = 0 for table entries
        );

        // Query interaction: each row queries QUERY with
        // multiplicity = QUERY_FLAG.
        builder.push_interaction(
            BUS_URANGE8,
            [<AB::Var as Into<AB::Expr>>::into(cur[COL_QUERY])],
            <AB::Var as Into<AB::Expr>>::into(cur[COL_QUERY_FLAG]),
            1, // count_weight = 1 for queries
        );
    }
}

/// Build a `2^log_n × WIDTH` trace where:
///   * Column 0 (TABLE) enumerates `0..256` then replays `255`.
///   * Column 1 (FREQ) is `0` everywhere except on rows
///     `query_values[i]`, where it counts the number of times
///     value `i` is queried.
///   * Column 2 (QUERY) holds one query per "active" row; non-
///     active rows hold `query_padding`.
///   * Column 3 (QUERY_FLAG) is `1` on active rows, `0` otherwise.
pub fn build_trace(log_n: usize, queries: &[u32], query_padding: u32) -> RowMajorMatrix<Val> {
    use p3_field::integers::QuotientMap;

    let n = 1usize << log_n;
    let mut flat = vec![Val::default(); n * WIDTH];

    // (1) Fill TABLE.
    for r in 0..n {
        let v = r.min(255) as u32;
        flat[r * WIDTH + COL_TABLE] = <Val as QuotientMap<u64>>::from_int(v as u64);
    }

    // (2) Compute FREQ from queries.
    let mut freq = [0u64; 256];
    for &q in queries {
        if (q as usize) < 256 {
            freq[q as usize] += 1;
        }
        // Out-of-range queries don't contribute to FREQ — the
        // unbalance shows up at LogUp verification time.
    }
    for v in 0..n {
        let row_freq = if v < 256 { freq[v] } else { 0 };
        flat[v * WIDTH + COL_FREQ] = <Val as QuotientMap<u64>>::from_int(row_freq);
    }

    // (3) Fill QUERY / QUERY_FLAG.
    for r in 0..n {
        if r < queries.len() {
            flat[r * WIDTH + COL_QUERY] = <Val as QuotientMap<u64>>::from_int(queries[r] as u64);
            flat[r * WIDTH + COL_QUERY_FLAG] = <Val as QuotientMap<u64>>::from_int(1);
        } else {
            flat[r * WIDTH + COL_QUERY] = <Val as QuotientMap<u64>>::from_int(query_padding as u64);
            flat[r * WIDTH + COL_QUERY_FLAG] = Val::default();
        }
    }

    RowMajorMatrix::new(flat, WIDTH)
}

#[cfg(test)]
mod tests {
    //! End-to-end LogUp tests: `prove_batch` + `verify_batch` over
    //! the `UrangeBusDemoAir`. Valid traces verify; tampering a
    //! query to an out-of-range value rejects.

    use p3_batch_stark::{prove_batch, verify_batch, ProverData, StarkInstance};
    use p3_field::integers::QuotientMap;

    use super::*;
    use crate::circuit::{build_stark_config, AiPowStarkConfig, CircuitConfig};
    use crate::params::ZkParams;

    fn test_zk_params() -> ZkParams {
        ZkParams {
            m: 8,
            k: 16,
            n: 8,
            noise_rank: 2,
            tile: 2,
            difficulty_bits: 0,
        }
    }

    /// Helper: run prove_batch / verify_batch on a single instance.
    fn run_batch_proof(
        config: &AiPowStarkConfig,
        air: &UrangeBusDemoAir,
        trace: &RowMajorMatrix<Val>,
    ) -> Result<(), String> {
        let instances = vec![StarkInstance {
            air,
            trace,
            public_values: vec![],
        }];
        let prover_data = ProverData::from_instances(config, &instances);
        let proof = prove_batch(config, &instances, &prover_data);
        verify_batch(config, &[*air], &proof, &[vec![]], &prover_data.common)
            .map_err(|e| format!("{:?}", e))
    }

    /// Baseline: 256-row trace with the table filled and no
    /// queries. LogUp balances trivially.
    #[test]
    fn no_queries_balances_via_batch_stark() {
        let cfg = build_stark_config(&test_zk_params(), &CircuitConfig::TEST_PEARL);
        let trace = build_trace(8, &[], 0);
        run_batch_proof(&cfg, &UrangeBusDemoAir, &trace).expect("empty-queries trace must verify");
    }

    /// A trace with 5 in-range queries verifies (the FREQ column
    /// balances the queries).
    #[test]
    fn in_range_queries_balance() {
        let cfg = build_stark_config(&test_zk_params(), &CircuitConfig::TEST_PEARL);
        let queries = [0, 1, 42, 100, 255];
        let trace = build_trace(8, &queries, 0);
        run_batch_proof(&cfg, &UrangeBusDemoAir, &trace).expect("in-range queries must verify");
    }

    /// Tamper a query to an out-of-range value (300, not in [0,
    /// 256)). The LogUp argument detects the unbalance:
    /// query side contributes (300, +1), but the table has no
    /// entry with key 300.
    #[test]
    fn out_of_range_query_rejected_by_logup() {
        let cfg = build_stark_config(&test_zk_params(), &CircuitConfig::TEST_PEARL);
        let queries = [0, 1, 42, 100, 255];
        let mut trace = build_trace(8, &queries, 0);
        // Tamper row 2's QUERY to 300.
        let target = 2 * WIDTH + COL_QUERY;
        trace.values[target] = <Val as QuotientMap<u64>>::from_int(300);
        let res = run_batch_proof(&cfg, &UrangeBusDemoAir, &trace);
        assert!(
            res.is_err(),
            "out-of-range query must reject via LogUp; got {:?}",
            res
        );
    }

    /// Tamper FREQ — over-claim a multiplicity. The query side
    /// doesn't actually consume that many; LogUp catches it.
    #[test]
    fn over_claimed_freq_rejected_by_logup() {
        let cfg = build_stark_config(&test_zk_params(), &CircuitConfig::TEST_PEARL);
        let queries = [42];
        let mut trace = build_trace(8, &queries, 0);
        // Inflate FREQ[42] from 1 to 5.
        let target = 42 * WIDTH + COL_FREQ;
        trace.values[target] = <Val as QuotientMap<u64>>::from_int(5);
        let res = run_batch_proof(&cfg, &UrangeBusDemoAir, &trace);
        assert!(
            res.is_err(),
            "over-claimed FREQ must reject via LogUp; got {:?}",
            res
        );
    }

    /// Many queries hitting the same value: the LogUp argument
    /// handles multiplicities > 1 correctly.
    #[test]
    fn multi_query_same_value_balances() {
        let cfg = build_stark_config(&test_zk_params(), &CircuitConfig::TEST_PEARL);
        let queries = [42, 42, 42, 100, 100, 0];
        let trace = build_trace(8, &queries, 0);
        run_batch_proof(&cfg, &UrangeBusDemoAir, &trace).expect("repeated queries must verify");
    }

    /// Tamper QUERY_FLAG — drop a real query so the FREQ becomes
    /// inconsistent.
    #[test]
    fn dropped_query_flag_rejected_by_logup() {
        let cfg = build_stark_config(&test_zk_params(), &CircuitConfig::TEST_PEARL);
        let queries = [42, 100];
        let mut trace = build_trace(8, &queries, 0);
        // Set QUERY_FLAG on row 0 to 0 — claims "row 0 doesn't
        // query", but FREQ[42] still says one query happened.
        let target = 0 * WIDTH + COL_QUERY_FLAG;
        trace.values[target] = Val::default();
        let res = run_batch_proof(&cfg, &UrangeBusDemoAir, &trace);
        assert!(
            res.is_err(),
            "inconsistent QUERY_FLAG must reject; got {:?}",
            res
        );
    }
}

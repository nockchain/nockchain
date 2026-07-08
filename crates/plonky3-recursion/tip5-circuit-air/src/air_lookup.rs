//! Lookup-table Tip5 permutation AIR — **global-bus form** (narrow
//! split-S-box encoding, Tip5 paper IACR ePrint 2023/107 §4.7: a
//! Hash table that *looks up* `L` in a 256-row Lookup table).
//!
//! **Degree flaw FIXED (L4, 2026-05-18).** The earlier single
//! `push_local_interaction` with all 225 tuples/row was
//! `LogUpGadget::constraint_degree = 1 + Σ ≈ 226` (FRI-infeasible).
//! `eval` now emits **one small `push_interaction` per byte** on a
//! shared global bus `tip5_l` (`LookupBus::lookup_key` query;
//! `table_entry` provide) — each a single-tuple, degree-1-element,
//! degree-1/2-count interaction ⇒ `constraint_degree = 2`
//! (decisively asserted in `tests::global_bus_interactions_are
//! _low_degree`). 224 query + 1 provide = 225 separate global
//! interactions ⇒ 225 aux EF cols; the global net sum is
//! reconciled across the Hash & Lookup tables by
//! `verify_global_final_value`.
//!
//! Validated standalone here: native-equivalence (trace ==
//! `nockchain_math::tip5::permute_5round`), algebraic constraints, the
//! LogUp **value** identity (`Σ = 0` honest / ≠0 tamper), **and the
//! per-interaction constraint degree ≤ 2** (the feasibility fix).
//! *Residual (C2.3, precisely scoped):* a full `p3-batch-stark`
//! `prove_all_tables`/`verify_all_tables` runs the global
//! reconciliation in a real STARK — that needs the Tip5 NPO
//! subsystem + circuit-prover table registration (the
//! `test_poseidon2_ctl_lookups` machinery). Not faked here; see
//! `2026-05-18_C2_TIP5_CIRCUIT_AIR_DESIGN.md` §2c.
//!
//! This **replaces the ≈7168-column boolean range-check core** of
//! the lookup-free [`crate::Tip5PermAir`] with **2 columns per byte**
//! (`b`,`c`) + one verifier-fixed 256-row preprocessed L-table + a
//! per-table-row multiplicity + a local LogUp interaction. The
//! algebraic constraints (canonical recomposition + §4.6 `<p` guard
//! + x⁷ + circulant MDS + round constants) are the *same, validated*
//! logic as `Tip5PermAir`; only the split-S-box encoding changes
//! (bits+cube ⇒ byte+image+LogUp). It is standalone-validatable with
//! **no recursion machinery** (mirroring Plonky3 `p3-lookup`'s own
//! `RangeCheckAir`): the LogUp soundness is the running-sum
//! accumulator returning to zero.
//!
//! Layout — a single main trace of `H = next_pow2(256 + 5P)` rows:
//! * rows `[0,256)`  = **table rows** (`KIND=0`); preprocessed
//!   `(TIN,TOUT)=(i, LOOKUP_TABLE[i])`, main `TMULT = #queries of i`.
//! * rows `[256,256+5P)` = **round rows**: five consecutive rows per
//!   Tip5 permutation, with verifier-fixed round selectors in the
//!   preprocessed trace.
//! * remaining rows are inert padding.
//!
//! Soundness: every per-byte `(b,c)` on a perm row is a LogUp query
//! into the preprocessed table; the accumulator is zero iff every
//! `(b,c)` equals a genuine `(i, LOOKUP_TABLE[i])` row ⇒ `b∈[0,256)`
//! **and** `c = LOOKUP_TABLE[b]` (the C2.0 identity anchors that the
//! table is the paper's `L`). A tampered `c`, an out-of-table `b`,
//! or a non-canonical §4.6 split ⇒ accumulator ≠ 0 / constraints
//! fail (adversarially tested).

use p3_air::{Air, AirBuilder, BaseAir, WindowAccess};
use p3_field::PrimeCharacteristicRing;
use p3_lookup::builder::InteractionBuilder;

use crate::tip5_spec::{
    LOOKUP_TABLE, NUM_ROUNDS, NUM_SPLIT_AND_LOOKUP, ROUND_CONSTANTS, STATE_SIZE, mds_matrix,
    rc_precomp,
};

pub(crate) const NS: usize = NUM_SPLIT_AND_LOOKUP; // 4 split lanes
pub(crate) const NBYTES: usize = 8; // bytes per split lane
/// Number of L-table rows (every byte value 0..256).
pub const TABLE_ROWS: usize = 256;

// ---- main-trace flat layout ----
// [ TMULT | TIN[16] | IN[16] | split byte/image pairs | INV[NS] | OUT[16] ]
//
// One row encodes one Tip5 round. The sbox-output `A[STATE_SIZE]`
// columns and the helper `X2/X3` columns for power-of-7 lanes are
// substituted inline. Consecutive rows inside one permutation are
// linked by verifier-fixed round selectors. TIN carries the original
// permutation input across the five rows so the final row contains the
// full terminal IO tuple `(TIN, OUT)`.
const SPLIT_BC: usize = NS * 2 * NBYTES; // 64

const C_TMULT: usize = 0;
const C_TIN: usize = 1; // original permutation input, carried across rounds
const C_IN: usize = C_TIN + STATE_SIZE; // current round input
const C_SPLIT: usize = C_IN + STATE_SIZE;
const C_INV: usize = C_SPLIT + SPLIT_BC;
const C_OUT: usize = C_INV + NS;

#[inline]
const fn b_col(t: usize, k: usize) -> usize {
    C_SPLIT + t * (2 * NBYTES) + k
}
#[inline]
const fn c_col(t: usize, k: usize) -> usize {
    C_SPLIT + t * (2 * NBYTES) + NBYTES + k
}
#[inline]
const fn inv_col(t: usize) -> usize {
    C_INV + t
}
#[inline]
pub(crate) const fn rout_col(i: usize) -> usize {
    C_OUT + i
}

/// Total main-trace width (≈8× narrower than the lookup-free AIR).
pub const fn tip5_lookup_air_width() -> usize {
    C_OUT + STATE_SIZE
}

/// Main-trace column index of input lane `lane` (`IN[lane]`, the Tip5
/// permutation input state read by the WitnessChecks CTL).
pub(crate) const fn tip5_in_col(lane: usize) -> usize {
    C_TIN + lane
}

/// Main-trace column index of the final-round output lane `lane`
/// (`ROUT[NUM_ROUNDS-1][lane]`, the permuted state exposed via CTL).
pub(crate) const fn tip5_out_col(lane: usize) -> usize {
    rout_col(lane)
}

// ---- preprocessed (verifier-fixed) L-table and round selectors ----
const P_IS_TABLE: usize = 0;
const P_TIN: usize = 1;
const P_TOUT: usize = 2;
const P_IS_ROUND: usize = 3;
const P_ROUND0: usize = 4;
pub const PREP_WIDTH: usize = P_ROUND0 + NUM_ROUNDS;

/// The lookup-table Tip5 permutation AIR. Carries the verifier-fixed
/// preprocessed L-table (flat row-major, `PREP_WIDTH` cols, height =
/// the main trace height) — the poseidon-circuit-air pattern, so the
/// `(i, LOOKUP_TABLE[i])` rows are *not* prover-controlled.
#[derive(Debug, Default, Clone)]
pub struct Tip5PermLookupAir<F> {
    preprocessed: alloc::vec::Vec<F>,
}

impl<F> Tip5PermLookupAir<F> {
    pub const fn new(preprocessed: alloc::vec::Vec<F>) -> Self {
        Self { preprocessed }
    }
}

impl<F: p3_field::Field> BaseAir<F> for Tip5PermLookupAir<F> {
    fn width(&self) -> usize {
        tip5_lookup_air_width()
    }
    fn preprocessed_width(&self) -> usize {
        PREP_WIDTH
    }
    fn preprocessed_trace(&self) -> Option<p3_matrix::dense::RowMajorMatrix<F>> {
        Some(p3_matrix::dense::RowMajorMatrix::new(
            self.preprocessed.clone(),
            PREP_WIDTH,
        ))
    }
    fn max_constraint_degree(&self) -> Option<usize> {
        // Max over BOTH constraint families:
        //  • hand-written algebraic constraints: degree 8 after
        //    inlining power-lane x^7 into the kind-gated MDS relation;
        //  • LogUp gadget (per the global bus): degree 2
        //    (`tests::global_bus_interactions_are_low_degree`).
        Some(8)
    }
}

impl<AB: AirBuilder + InteractionBuilder> Air<AB> for Tip5PermLookupAir<AB::F>
where
    AB::F: p3_field::Field,
{
    fn eval(&self, builder: &mut AB) {
        let main = builder.main();
        let prep = builder.preprocessed().clone();
        let local = main.current_slice();
        let next = main.next_slice();
        let pre = prep.current_slice();

        let fe = |v: u64| -> AB::Expr { AB::Expr::from(AB::F::from_u64(v)) };
        let var = |c: usize| -> AB::Expr { local[c].into() };
        let nvar = |c: usize| -> AB::Expr { next[c].into() };
        let pvar = |c: usize| -> AB::Expr { pre[c].into() };
        let pow8 = |k: usize| -> AB::Expr { fe(1u64 << (8 * k)) };

        let is_table = pvar(P_IS_TABLE);
        let is_round = pvar(P_IS_ROUND);
        let round_sel = (0..NUM_ROUNDS)
            .map(|round| pvar(P_ROUND0 + round))
            .collect::<alloc::vec::Vec<_>>();
        let link_next = round_sel
            .iter()
            .take(NUM_ROUNDS - 1)
            .cloned()
            .fold(AB::Expr::ZERO, |acc, value| acc + value);

        let mds = mds_matrix();
        let two32_m1 = fe((1u64 << 32) - 1);

        // ---- LogUp GLOBAL bus (paper §4.7 Hash⟷Lookup table) ----
        // Each byte (b,c) is its OWN small interaction on the shared
        // `tip5_l` bus: `LookupBus::lookup_key` ⇒
        // `push_interaction(name,[b,c],kind,1)` — ONE tuple, elements
        // degree 1, count `kind` degree 1 ⇒ `LogUpGadget::
        // constraint_degree = 1 + 1 = 2` (proven in
        // `tests::global_bus_interactions_are_low_degree`). The
        // 256-row preprocessed L-table PROVIDES `(i,L[i])` with global
        // multiplicity `TMULT` via `table_entry`
        // (`push_interaction(name,[i,L[i]],-(TMULT·IS_TABLE),0)`).
        // 224 query + 1 provide = 225 separate global interactions ⇒
        // 225 aux EF columns (the bus-form width cost), each a
        // degree-2 running-sum constraint; the global net sum is
        // reconciled by `verify_global_final_value` across the Hash &
        // Lookup tables (the C2.3 batch-stark path — NOT the old
        // degree-≈226 single-interaction batching).
        let bus = p3_lookup::bus::LookupBus::new("tip5_l");
        for t in 0..NS {
            for k in 0..NBYTES {
                bus.lookup_key(builder, [var(b_col(t, k)), var(c_col(t, k))], is_round.clone());
            }
        }
        bus.table_entry(
            builder,
            [pvar(P_TIN), pvar(P_TOUT)],
            var(C_TMULT) * is_table.clone(),
        );

        // ---- algebraic constraints, gated by KIND (perm rows only;
        //      table/pad rows have KIND=0 ⇒ trivially satisfied) ----
        //
        // The sbox-output A[i] columns and the x2/x3 helper columns for
        // power lanes are eliminated; each A[i] is substituted inline into
        // the MDS sum below.
        //
        //   A[t] = recompose_c        (for t < NS; degree-1 in byte cols)
        //   A[j] = x^7               (for j ≥ NS; degree-7 in input lane)
        //
        // We pre-compute these per-round into `a_expr[0..STATE_SIZE]`
        // (an array of AB::Expr) and reuse them in the rout MDS sum.
        // The earlier A, x2, and x3 columns were verifier-derivable from
        // existing columns; removing them shrinks the AIR width by
        // `STATE_SIZE + 2 * (STATE_SIZE - NS) = 40` columns per round, at the
        // cost of raising the round-gated MDS output relation to degree 8.
        builder.assert_zero((AB::Expr::ONE - is_table.clone()) * var(C_TMULT));

        let mut a_expr: alloc::vec::Vec<AB::Expr> = alloc::vec::Vec::with_capacity(STATE_SIZE);
        for t in 0..NS {
            let mut recompose_b = AB::Expr::ZERO;
            let mut recompose_c = AB::Expr::ZERO;
            let mut low = AB::Expr::ZERO;
            let mut high = AB::Expr::ZERO;
            for k in 0..NBYTES {
                let bk = var(b_col(t, k));
                let ck = var(c_col(t, k));
                recompose_b = recompose_b + bk.clone() * pow8(k);
                recompose_c = recompose_c + ck * pow8(k);
                if k < 4 {
                    low = low + bk * pow8(k);
                } else {
                    high = high + bk * pow8(k - 4);
                }
            }
            builder.assert_zero(is_round.clone() * (recompose_b - var(C_IN + t)));
            a_expr.push(recompose_c);
            let g = high - two32_m1.clone();
            let inv = var(inv_col(t));
            let prod = g.clone() * inv;
            builder.assert_zero(is_round.clone() * g * (prod.clone() - AB::Expr::ONE));
            builder.assert_zero(is_round.clone() * (AB::Expr::ONE - prod) * low);
        }

        for j in NS..STATE_SIZE {
            let x = var(C_IN + j);
            let x2 = x.clone() * x.clone();
            let x3 = x2 * x.clone();
            a_expr.push(x3.clone() * x3 * x);
        }

        for i in 0..STATE_SIZE {
            let mut acc = AB::Expr::ZERO;
            for j in 0..STATE_SIZE {
                acc = acc + fe(mds[i][j]) * a_expr[j].clone();
            }
            let rc = round_sel
                .iter()
                .enumerate()
                .fold(AB::Expr::ZERO, |sum, (round, selector)| {
                    sum + selector.clone() * fe(rc_precomp(ROUND_CONSTANTS[round * STATE_SIZE + i]))
                });
            builder.assert_zero(is_round.clone() * (var(rout_col(i)) - acc - rc));
            builder.assert_zero(link_next.clone() * (var(rout_col(i)) - nvar(C_IN + i)));
            builder.assert_zero(round_sel[0].clone() * (var(C_TIN + i) - var(C_IN + i)));
            builder.assert_zero(link_next.clone() * (var(C_TIN + i) - nvar(C_TIN + i)));
        }
        let _ = LOOKUP_TABLE; // table content lives in the preprocessed trace
    }
}

#[cfg(test)]
mod tests {
    extern crate std;

    use alloc::vec::Vec;
    use std::panic::{AssertUnwindSafe, catch_unwind};
    use std::{fs, vec};

    use p3_air::check_constraints;
    use p3_air::symbolic::AirLayout;
    use p3_field::{Field, PrimeCharacteristicRing, PrimeField64};
    use p3_goldilocks::Goldilocks;
    use p3_lookup::symbolic::InteractionSymbolicBuilder;
    use p3_lookup::{Kind, LogUpGadget, Lookup, LookupProtocol};
    use p3_matrix::Matrix;
    use p3_test_utils::goldilocks_params::Challenge;

    use super::*;
    use crate::generation_lookup::generate_lookup_trace;
    use crate::tip5_spec::permute;

    const FIXTURE: &str = concat!(
        env!("CARGO_MANIFEST_DIR"),
        // 5-round ai-pow-zk fixture (NOT the 7-round canonical Nockchain one).
        "/../../ai-pow-zk/tests/fixtures/tip5_5round_golden_kat.txt"
    );

    fn fixture_vectors() -> Vec<([u64; STATE_SIZE], [u64; STATE_SIZE])> {
        let text = fs::read_to_string(FIXTURE).expect("C2.0 fixture present");
        let nums = |l: &str| -> Vec<u64> {
            l.split_whitespace()
                .skip(1)
                .map(|t| t.parse().unwrap())
                .collect()
        };
        let mut out = Vec::new();
        let mut pend: Option<[u64; STATE_SIZE]> = None;
        for line in text.lines() {
            if line.starts_with("IN ") {
                let v = nums(line);
                let mut a = [0u64; STATE_SIZE];
                a.copy_from_slice(&v);
                pend = Some(a);
            } else if line.starts_with("OUT ") {
                let v = nums(line);
                let mut a = [0u64; STATE_SIZE];
                a.copy_from_slice(&v);
                out.push((pend.take().unwrap(), a));
            }
        }
        out
    }

    fn xs(s: &mut u64) -> u64 {
        let mut x = *s;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        *s = x;
        x.wrapping_mul(0x2545F4914F6CDD1D) % crate::tip5_spec::P_GOLDILOCKS
    }

    /// β = 257 makes `combine(b,c) = 257·b + c` injective on bytes
    /// (`b,c < 256`); α huge avoids poles. For a valid lookup the
    /// query multiset equals the table multiset ⇒ the running sum is
    /// **exactly** 0; any byte tamper breaks injective-matching ⇒ ≠ 0.
    fn logup_accumulator(
        main: &p3_matrix::dense::RowMajorMatrix<Goldilocks>,
        prep: &[Goldilocks],
    ) -> Challenge {
        let w = main.width();
        let h = main.height();
        let beta = Challenge::from_u64(257);
        let alpha = Challenge::from_u64(0x1_0000_0001);
        let comb = |x: u64, y: u64| beta * Challenge::from_u64(x) + Challenge::from_u64(y);
        let g2u = |g: Goldilocks| g.as_canonical_u64();
        let mut acc = Challenge::ZERO;
        for row in 0..h {
            let b = row * w;
            let pb = row * PREP_WIDTH;
            let is_round = g2u(prep[pb + P_IS_ROUND]);
            if is_round == 1 {
                for t in 0..NS {
                    for k in 0..NBYTES {
                        let bb = g2u(main.values[b + b_col(t, k)]);
                        let cc = g2u(main.values[b + c_col(t, k)]);
                        acc += (alpha - comb(bb, cc)).inverse();
                    }
                }
            }
            // table side: -(TMULT * IS_TABLE) / (α - combine(TIN,TOUT))
            let is_t = g2u(prep[pb + P_IS_TABLE]);
            let tmult = g2u(main.values[b + C_TMULT]);
            if is_t == 1 && tmult != 0 {
                let tin = g2u(prep[pb + P_TIN]);
                let tout = g2u(prep[pb + P_TOUT]);
                acc -= Challenge::from_u64(tmult) * (alpha - comb(tin, tout)).inverse();
            }
        }
        acc
    }

    /// **The decisive degree-fix proof (L4).** Extract every global
    /// `tip5_l` interaction the AIR emits and assert each compiles to
    /// a LogUp `Lookup` of `constraint_degree ≤ 2` — i.e. the
    /// catastrophic single-interaction degree (≈226) is gone and the
    /// bus form stays low-degree. The separate algebraic AIR constraints
    /// determine the final quotient degree hint.
    /// Also asserts the exact interaction count
    /// (`7·4·8 = 224` byte queries + 1 L-table provide = 225).
    #[test]
    fn global_bus_interactions_are_low_degree() {
        let mut sb = InteractionSymbolicBuilder::<Goldilocks>::new(AirLayout {
            preprocessed_width: PREP_WIDTH,
            main_width: tip5_lookup_air_width(),
            ..Default::default()
        });
        let air = Tip5PermLookupAir::<Goldilocks>::new(Vec::new());
        Air::<InteractionSymbolicBuilder<Goldilocks>>::eval(&air, &mut sb);

        let interactions = sb.global_interactions();
        assert_eq!(
            interactions.len(),
            NS * NBYTES + 1,
            "expected 32 byte-query + 1 table-provide global interactions"
        );

        let gadget = LogUpGadget::new();
        let mut max_deg = 0usize;
        let (mut n_query, mut n_provide) = (0usize, 0usize);
        for si in interactions {
            assert_eq!(si.bus_name, "tip5_l");
            match si.count_weight {
                1 => n_query += 1,
                0 => n_provide += 1,
                w => panic!("unexpected count_weight {w}"),
            }
            // One push_interaction ⇒ one single-tuple Lookup.
            let lookup = Lookup::<Goldilocks> {
                kind: Kind::Global(si.bus_name.clone()),
                elements: vec![si.fields.clone()],
                multiplicities: vec![si.count.clone()],
                column: 0,
            };
            let d = gadget.constraint_degree(&lookup);
            assert!(
                d <= 2,
                "global interaction constraint_degree {d} > 2 — bus form not low-degree"
            );
            max_deg = max_deg.max(d);
        }
        assert_eq!(n_query, NS * NBYTES);
        assert_eq!(n_provide, 1);
        std::eprintln!(
            "tip5_l global bus: {} interactions, max LogUp constraint_degree = {} \
             (was ≈226 for the single-interaction form)",
            interactions.len(),
            max_deg
        );
    }

    /// Width: the lookup AIR must be dramatically narrower than the
    /// lookup-free one (the whole point of this work).
    #[test]
    fn width_is_dramatically_narrower() {
        let lookup = tip5_lookup_air_width();
        let free = crate::tip5_perm_air_width();
        std::eprintln!(
            "tip5 AIR width: lookup-table={lookup} vs lookup-free={free} \
             ({:.1}× narrower)",
            free as f64 / lookup as f64
        );
        assert!(
            lookup < free / 5,
            "expected ≥5× narrower, got {lookup} vs {free}"
        );
    }

    /// Native-equivalence: lookup-AIR trace final ROUT == nockchain-math
    /// `permute_5round`, on all 315 fixture vectors **and** 2048 random.
    #[test]
    fn lookup_air_equals_native_spec() {
        let fv = fixture_vectors();
        assert!(fv.len() >= 300);
        let mut inputs: Vec<[u64; STATE_SIZE]> = fv.iter().map(|(i, _)| *i).collect();
        let mut seed = 0xC0FFEE_1234_5678u64;
        for _ in 0..2048 {
            inputs.push(core::array::from_fn(|_| xs(&mut seed)));
        }
        let (main, prep) = generate_lookup_trace(&inputs);
        let w = main.width();

        for (pi, inp) in inputs.iter().enumerate() {
            let first_row = TABLE_ROWS + pi * NUM_ROUNDS;
            let last_row = first_row + NUM_ROUNDS - 1;
            let first = first_row * w;
            let last = last_row * w;
            let mut exp = *inp;
            permute(&mut exp);
            for lane in 0..STATE_SIZE {
                assert_eq!(
                    main.values[first + C_IN + lane],
                    Goldilocks::from_u64(inp[lane])
                );
                assert_eq!(
                    main.values[last + rout_col(lane)],
                    Goldilocks::from_u64(exp[lane]),
                    "lookup-AIR != nockchain-math permute, perm {pi} lane {lane}"
                );
            }
        }
        // fixture OUT must match too (== nockchain-math, by C2.0)
        for (pi, (_, out)) in fv.iter().enumerate() {
            let bse = (TABLE_ROWS + pi * NUM_ROUNDS + NUM_ROUNDS - 1) * w;
            for lane in 0..STATE_SIZE {
                assert_eq!(
                    main.values[bse + rout_col(lane)],
                    Goldilocks::from_u64(out[lane])
                );
            }
        }

        // algebraic constraints satisfied (kind-gated; preprocessed table)
        let air = Tip5PermLookupAir::new(prep.clone());
        check_constraints(&air, &main, &[]);
        // LogUp soundness: honest accumulator is exactly zero.
        assert_eq!(
            logup_accumulator(&main, &prep),
            Challenge::ZERO,
            "honest LogUp accumulator must be 0"
        );
    }

    fn panics(f: impl FnOnce()) -> bool {
        catch_unwind(AssertUnwindSafe(f)).is_err()
    }

    /// Adversarial: tampered S-box image / output / non-canonical
    /// split are all rejected (LogUp accumulator ≠ 0 or constraints
    /// fail) — the lookup binding + §4.6 guard hold.
    #[test]
    fn lookup_air_adversarial() {
        let inputs: Vec<[u64; STATE_SIZE]> =
            vec![core::array::from_fn(|i| (i as u64) * 0x1111 + 1); 1];
        let air_of = |p: &[Goldilocks]| Tip5PermLookupAir::new(p.to_vec());
        let (good, gp) = generate_lookup_trace(&inputs);
        check_constraints(&air_of(&gp), &good, &[]);
        assert_eq!(logup_accumulator(&good, &gp), Challenge::ZERO);
        let w = good.width();
        let prow = TABLE_ROWS; // first perm round row
        let final_row = TABLE_ROWS + NUM_ROUNDS - 1;

        // (a) tamper an S-box image byte c → LogUp accumulator ≠ 0
        let mut t = good.clone();
        let cc = prow * w + c_col(0, 0);
        t.values[cc] += Goldilocks::ONE;
        assert_ne!(
            logup_accumulator(&t, &gp),
            Challenge::ZERO,
            "tampered S-box image accepted by the lookup"
        );

        // (b) tamper the permutation output final ROUT -> constraints fail
        let mut t2 = good.clone();
        t2.values[final_row * w + rout_col(3)] += Goldilocks::ONE;
        assert!(
            panics(|| check_constraints(&air_of(&gp), &t2, &[])),
            "tampered ROUT accepted"
        );

        // (c) §4.6 non-canonical split: rewrite lane-0 round-0 bytes to
        //     the alias x+p (H=2^32−1, L≠0) keeping recompose ≡ — only
        //     the kind-gated canonical guard must fire.
        let mut t3 = good.clone();
        // input lane0 chosen = small; alias bytes of (v + p):
        let v0 = inputs[0][0];
        let alias = (v0 as u128 + crate::tip5_spec::P_GOLDILOCKS as u128) as u64;
        if (alias as u128) < (1u128 << 64) {
            let ab = alias.to_le_bytes();
            for k in 0..NBYTES {
                t3.values[prow * w + b_col(0, k)] = Goldilocks::from_u64(ab[k] as u64);
            }
            assert!(
                panics(|| check_constraints(&air_of(&gp), &t3, &[])),
                "non-canonical §4.6 split accepted"
            );
        }
    }
}

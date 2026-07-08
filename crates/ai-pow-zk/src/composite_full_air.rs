//! M10.1c composite AIR — Phase 12 integration layer.
//!
//! **Pearl ISC.** This file is derived from Pearl source code
//! (Copyright (c) 2025-2026 Pearl Research Labs; 2015-2016 The Decred
//! developers); see `crates/ai-pow-zk/LICENSE-PEARL` for the full
//! permission notice.
//!
//! Port of `Pearl zk-pow pearl_air.rs:46-89` — the
//! top-level `eval` that wires every chip's constraints into a
//! single AIR over [`composite_layout`]'s `TOTAL_TRACE_WIDTH`
//! columns.
//!
//! ## Phase scope (12a — what's wired here)
//!
//! Phase 12 lands in two slices so the integration is incremental:
//!
//! * **12a (this commit)** — Phase 3-6 chips that already read by
//!   `composite_layout` offsets. These slot in directly:
//!     * [`stark_row`](crate::chips::stark_row::StarkRowChip)
//!     * [`range_table`](crate::chips::range_table) — `URange8`,
//!       `URange13`, `IRange7P1`, `IRange8`
//!     * [`i8u8`](crate::chips::i8u8::I8U8Chip)
//!     * [`control`](crate::chips::control::ControlChip)
//!     * [`input`](crate::chips::input::InputChip)
//! * **12b (pending)** — Phase 7-10 chips that currently use a
//!   chip-local layout. Wiring them needs a refactor pass: each
//!   chip's eval lifts to a free function taking column offsets
//!   so `CompositeFullAir` can pass `composite_layout`'s offsets.
//!     * [`blake3`](crate::chips::blake3)
//!     * [`matmul`](crate::chips::matmul)
//!     * [`jackpot`](crate::chips::jackpot)
//!
//! ## Per-row dispatch
//!
//! Every chip's constraint is **always on** at this layer. Per-row
//! activity selection (via CONTROL_PREP unpacking, IS_NEW_BLAKE,
//! etc.) is what makes individual chip constraints "fire" or
//! silence on a given row. The composite AIR's job is just to
//! collect them all.
//!
//! ## Trace shape
//!
//! `TOTAL_TRACE_WIDTH × N` where `N >= MIN_STARK_LEN = 8192`.
//! Padding rows that aren't filled by any chip are all-zero; the
//! all-zero pattern satisfies every wired-in chip's constraints
//! (range-table boundaries are filled by `fill_row` past `span`,
//! all selectors are 0, all data columns are 0, control_prep = 0,
//! mat_id = 0, etc.).

use p3_air::{Air, AirBuilder, BaseAir, WindowAccess};
use p3_field::PrimeCharacteristicRing;
use p3_matrix::Matrix;
use thiserror::Error;

use crate::chips::blake3::chip::Blake3Chip;
use crate::chips::control::ControlChip;
use crate::chips::i8u8::I8U8Chip;
use crate::chips::input::InputChip;
use crate::chips::jackpot::chip::JackpotChip;
use crate::chips::matmul::chip::MatmulCumsumChip;
use crate::chips::range_table::{IRange7P1Chip, IRange8Chip, URange13Chip, URange8Chip};
use crate::chips::stark_row::StarkRowChip;
use crate::composite_layout::{
    BLAKE3_MSG_START, CUMSUM_TILE_START, CV_IN_LEN, CV_IN_START, CV_OUT_LEN, CV_OUT_START,
    IS_HASH_A, IS_HASH_B, IS_HASH_JACKPOT, IS_MSG_MAT, IS_NEW_BLAKE, IS_USE_COMMITMENT_HASH,
    IS_USE_JOB_KEY, JACKPOT_MSG_START, JACKPOT_SIZE, MSG_PAIR_SEL_LEN, MSG_PAIR_SEL_START,
    TOTAL_TRACE_WIDTH, UINT8_DATA_LEN, UINT8_DATA_START,
};
use crate::composite_public::{
    NUM_PUBLIC_VALUES, PI_COMMITMENT_HASH_OFFSET, PI_CUMSUM_LEN, PI_CUMSUM_OFFSET,
    PI_HASH_A_OFFSET, PI_HASH_B_OFFSET, PI_HASH_JACKPOT_OFFSET, PI_JACKPOT_OFFSET,
    PI_JOB_KEY_OFFSET,
};

/// The M10.1c composite AIR (Phase 12a slice).
///
/// Trace width: [`TOTAL_TRACE_WIDTH`]. The constraint-bearing
/// chips wired here are Phase 3-6's. Phase 12b adds Phase 7-10's
/// chips (BLAKE3, matmul, jackpot).
///
/// Public inputs ([`NUM_PUBLIC_VALUES`] field elements) bind the
/// trace's last-row CUMSUM_TILE and JACKPOT_MSG cells, threaded
/// through the trace via `fill_*_passthrough` helpers. See
/// [`crate::composite_public::CompositePublicInputs`].
#[derive(Copy, Clone, Debug, Default)]
pub struct CompositeFullAir;

impl<F> BaseAir<F> for CompositeFullAir {
    fn width(&self) -> usize {
        TOTAL_TRACE_WIDTH
    }

    fn num_public_values(&self) -> usize {
        NUM_PUBLIC_VALUES
    }
}

/// The verifier-fixed "program / setup" columns. Pinning these to a
/// verifier-committed preprocessed trace makes the entire
/// instruction schedule + noise verifier-fixed:
/// `CONTROL_PREP` pins all 21 selectors **and** `MAT_ID` (the
/// control chip already enforces `CONTROL_PREP == pack(selectors,
/// mat_id)`), so a malicious prover can no longer zero selectors
/// to vacate the C1/C3/C4 bindings (ZKP_SECURITY_REPORT CRIT-1).
// HIGH-2.2 §4.C Route C (naive form) — REVERTED. Extending this
// to pin `A_NOISED_UNPACK`/`B_NOISED_UNPACK` is mechanically the
// least-invasive binding, but it widens the *preprocessed* trace
// 5 → 69 columns, committed + FRI'd at full trace height every
// `composite_setup`. Empirically pathological: the composite_proof
// suite went from fast to ~22 min CPU (a 10x+ prover blow-up),
// and the binding is *vacuous* in the shipping path anyway
// (`zk_bridge` places no matmul rows until §4.A). The
// cost-aware redesign (pin only verifier-derived row metadata rather
// than all matmul bytes) is in `2026-05-15_HIGH2_2_DESIGN.md` §4.C.8.
// §4.C.2 c-exact (cx.2-pcols/X1): `NOISE_PACKED_PREP` widened
// 1→8 (one `polyval(noise_subslice,129)` per co-located leaf
// block sub-slice), and positioned A/B chunk IDs add 8 more cols.
// Order MUST match the
// preprocessed column order (`extract_program` iterates this;
// the pin asserts `main[PROGRAM_COLS[k]] == preproc[k]`;
// `build_preprocessed_columns` emits in this order).
pub const PROGRAM_COLS: [usize; 20] = [
    crate::composite_layout::CONTROL_PREP,
    crate::composite_layout::NOISE_PACKED_PREP,
    crate::composite_layout::NOISE_PACKED_PREP + 1,
    crate::composite_layout::NOISE_PACKED_PREP + 2,
    crate::composite_layout::NOISE_PACKED_PREP + 3,
    crate::composite_layout::NOISE_PACKED_PREP + 4,
    crate::composite_layout::NOISE_PACKED_PREP + 5,
    crate::composite_layout::NOISE_PACKED_PREP + 6,
    crate::composite_layout::NOISE_PACKED_PREP + 7,
    crate::composite_layout::CV_OR_TWEAK_PREP,
    crate::composite_layout::AB_ID_PREP,
    crate::composite_layout::A_ID,
    crate::composite_layout::A_ID + 1,
    crate::composite_layout::A_ID + 2,
    crate::composite_layout::A_ID + 3,
    crate::composite_layout::B_ID,
    crate::composite_layout::B_ID + 1,
    crate::composite_layout::B_ID + 2,
    crate::composite_layout::B_ID + 3,
    crate::composite_layout::STARK_ROW_IDX,
];

#[derive(Debug, Error, PartialEq, Eq)]
pub enum ProgramShapeError {
    #[error("program width mismatch (expected {expected}, got {actual})")]
    WidthMismatch { expected: usize, actual: usize },
    #[error("program height must be a non-zero power of two, got {height}")]
    HeightNotPowerOfTwo { height: usize },
}

pub fn program_degree_bits(
    program: &p3_matrix::dense::RowMajorMatrix<crate::Val>,
) -> Result<usize, ProgramShapeError> {
    let expected = PROGRAM_COLS.len();
    let actual = program.width();
    if actual != expected {
        return Err(ProgramShapeError::WidthMismatch { expected, actual });
    }

    let height = program.height();
    if !height.is_power_of_two() {
        return Err(ProgramShapeError::HeightNotPowerOfTwo { height });
    }

    Ok(height.trailing_zeros() as usize)
}

/// Extract the [`PROGRAM_COLS`] from a full trace into a
/// `len(PROGRAM_COLS)`-wide row-major matrix — the canonical
/// "program" for that shape. The honest prover and the verifier
/// each build this from the *canonical* trace for the agreed
/// `ZkParams` (never from an untrusted proof); the
/// `CompositeFullAirPinned` constraints then force the prover's
/// in-trace `*_PREP` columns to equal it.
pub fn extract_program(
    trace: &p3_matrix::dense::RowMajorMatrix<crate::Val>,
) -> p3_matrix::dense::RowMajorMatrix<crate::Val> {
    let n = trace.values.len() / TOTAL_TRACE_WIDTH;
    let w = PROGRAM_COLS.len();
    let mut v = Vec::with_capacity(n * w);
    for r in 0..n {
        let base = r * TOTAL_TRACE_WIDTH;
        for &c in &PROGRAM_COLS {
            v.push(trace.values[base + c]);
        }
    }
    p3_matrix::dense::RowMajorMatrix::new(v, w)
}

/// Program-pinned composite AIR (ZKP_SECURITY_REPORT CRIT-1 fix).
///
/// Same constraints as [`CompositeFullAir`] **plus** an
/// unconditional per-row equality tying each [`PROGRAM_COLS`]
/// main-trace cell to the corresponding column of a
/// verifier-committed *preprocessed* trace. With the preprocessed
/// commitment fixed in the verifying key (independently rebuilt
/// by the verifier from `ZkParams`), the prover cannot choose the
/// selector schedule, so the selector-gated C1/C3/C4 bindings are
/// forced live. This is the production AIR (used by
/// `ai-pow::zk_bridge` / the `mine()` gate); the unit
/// [`CompositeFullAir`] remains a constraint-logic test harness.
#[derive(Clone)]
pub struct CompositeFullAirPinned {
    preprocessed: std::sync::Arc<p3_matrix::dense::RowMajorMatrix<crate::Val>>,
    /// HIGH-2.2 §6(b) — emit the `FOLD_XSTEP == SX_XR[stripe]`
    /// keystone. **Verifier-set from the trusted block params**
    /// (`num_stripes ≤ StripeXor STATE_LEN = 16`), never from the
    /// proof, so it is as sound as CRIT-1: a malicious prover
    /// cannot turn the binding off for a params set the verifier
    /// runs it for. `false` only for the `num_stripes > 16` legacy
    /// path (rectangular / PROD), where the §6(b) malicious-prover
    /// binding is the documented residual (wider-register
    /// generalization) and the StripeXor sweep is not placed.
    sx_bound: bool,
}

impl CompositeFullAirPinned {
    /// Build from the canonical program matrix (see
    /// [`extract_program`]) with the §6(b) keystone **enabled**
    /// (the production / `num_stripes ≤ 16` path). `program.width()`
    /// must equal `PROGRAM_COLS.len()`.
    pub fn new(program: p3_matrix::dense::RowMajorMatrix<crate::Val>) -> Self {
        Self::new_with(program, true)
    }

    /// Fallible version of [`Self::new`] for verifier-facing code
    /// that may be handed malformed block/proof artifacts.
    pub fn try_new(
        program: p3_matrix::dense::RowMajorMatrix<crate::Val>,
    ) -> Result<Self, ProgramShapeError> {
        Self::try_new_with(program, true)
    }

    /// Build with an explicit §6(b)-keystone flag. `sx_bound`
    /// MUST be derived by the verifier from the trusted block
    /// params (`num_stripes ≤ 16`), never from the proof. This
    /// constructor is for trusted setup paths; use
    /// [`Self::try_new_with`] when malformed input must be rejected
    /// as an ordinary verifier error.
    pub fn new_with(program: p3_matrix::dense::RowMajorMatrix<crate::Val>, sx_bound: bool) -> Self {
        Self::try_new_with(program, sx_bound).expect("canonical program shape already validated")
    }

    /// Fallible constructor for verifier-facing code. It validates
    /// both width and power-of-two height before the AIR reaches any
    /// panic-on-bad-shape setup path.
    pub fn try_new_with(
        program: p3_matrix::dense::RowMajorMatrix<crate::Val>,
        sx_bound: bool,
    ) -> Result<Self, ProgramShapeError> {
        program_degree_bits(&program)?;
        Ok(Self {
            preprocessed: std::sync::Arc::new(program),
            sx_bound,
        })
    }
}

impl BaseAir<crate::Val> for CompositeFullAirPinned {
    fn width(&self) -> usize {
        TOTAL_TRACE_WIDTH
    }
    fn num_public_values(&self) -> usize {
        NUM_PUBLIC_VALUES
    }
    fn preprocessed_width(&self) -> usize {
        PROGRAM_COLS.len()
    }
    fn preprocessed_trace(&self) -> Option<p3_matrix::dense::RowMajorMatrix<crate::Val>> {
        Some((*self.preprocessed).clone())
    }
}

impl<AB: AirBuilder<F = crate::Val>> Air<AB> for CompositeFullAirPinned {
    fn eval(&self, builder: &mut AB) {
        // All base constraints (chips + selector-gated PI bindings).
        <CompositeFullAir as Air<AB>>::eval(&CompositeFullAir, builder);

        // CRIT-1 program pin: main[PROGRAM_COLS[k]] == preprocessed[k]
        // on every row, unconditionally. Snapshot both rows before
        // opening the mutable assert (can't hold the window borrows
        // across builder.assert_*).
        let main = builder.main();
        let m_cur = main.current_slice();
        // cx.2-pcols plus positioned A/B chunk IDs widen PROGRAM_COLS;
        // collect into Vecs (no fixed [_;N]) so the pin tracks
        // PROGRAM_COLS.len() automatically.
        let m: Vec<AB::Var> = PROGRAM_COLS.iter().map(|&c| m_cur[c]).collect();
        let prep = builder.preprocessed();
        let p_cur = prep.current_slice();
        let p: Vec<AB::Var> = (0..PROGRAM_COLS.len()).map(|k| p_cur[k]).collect();
        for k in 0..PROGRAM_COLS.len() {
            builder.assert_eq(m[k], p[k]);
        }

        // HIGH-2.2 §4.D keystone (generalised): bind the C4-hashed
        // jackpot message to the **full folded TileState M**. On
        // the last trace row force
        //   JACKPOT_MSG[0..16] == FOLD_STATE[0..16]
        // unconditionally (a fixed boundary predicate, not a
        // prover selector). `FoldChip` (wired in
        // `CompositeFullAir::eval`) constrains FOLD_STATE to be
        // the Pearl §4.5 rotl13-XOR fold of the per-stripe X_STEP
        // sequence; the last row therefore carries the real
        // `TileState M` (the honest bridge places the real solved
        // tile's fold chain — §4.A). Hence HASH_JACKPOT =
        // BLAKE3(M, key=pow_key_for_nonce(s_a, nonce)) is the genuine PoW digest, not a
        // prover-free constant nor the 2×2 micro-accumulator
        // stop-gap this replaces (which only pinned 4 of 16 words
        // and zeroed the rest — §4.0). Honest traces with no fold
        // activity have JACKPOT_MSG = FOLD_STATE = 0 ⇒ 0 == 0
        // holds; a planted free JACKPOT_MSG (FOLD_STATE = 0) is
        // still rejected. Residual: X_STEP↔matmul-accumulator
        // binding (XStepChip-in-composite + subtile sweep) — the
        // §4.C noised_packed Route-A binds the matmul *inputs*;
        // full step-transcript binding is the precisely-scoped
        // remaining item. Pinned production path only; the unit
        // `CompositeFullAir` keeps independent PIs for the
        // constraint-logic harness.
        let main2 = builder.main();
        let c2 = main2.current_slice();
        let fs: [AB::Var; JACKPOT_SIZE] =
            core::array::from_fn(|i| c2[crate::composite_layout::FOLD_STATE_START + i]);
        let jm: [AB::Var; JACKPOT_SIZE] = core::array::from_fn(|i| c2[JACKPOT_MSG_START + i]);
        let mut last = builder.when_last_row();
        for i in 0..JACKPOT_SIZE {
            last.assert_eq(jm[i], fs[i]);
        }

        // HIGH-2.2 §6(b)/§6(b)-G2 keystone — bind the FoldChip's
        // per-stripe `FOLD_XSTEP` to the StripeXorChip register
        // lane for *that stripe*. The fold row carries a one-hot
        // `FOLD_STRIPE_SEL` (Σ == FOLD_IS_FOLD, enforced by
        // `FoldChip::eval_composite`) whose 6-bit index is pinned
        // into `CONTROL_PREP` by `ControlChip` (CRIT-1) — so the
        // selected lane is **verifier-fixed**, for *any*
        // `num_stripes ≤ STRIPE_MAX` (not just the old
        // `slot = stripe % 16 == stripe` ⇔ `num_stripes ≤ 16`
        // coincidence). Then
        //   Σ_{s<STRIPE_MAX} FOLD_STRIPE_SEL[s]·(FOLD_XSTEP − SX_XR[s]) = 0
        // forces `FOLD_XSTEP == SX_XR[stripe]`. `SX_XR` is the final
        // register (propagated by the StripeXor passthrough through
        // every post-sweep row, incl. the fold rows), constrained
        // by `StripeXorChip` to be the XOR-reduction of the matmul
        // accumulator-after-step (bound via `SX_IN ==
        // nxt.CUMSUM_TILE` to the committed-matrix sweep). Closes
        //   committed A/B → CUMSUM → SX_IN → SX_XR → FOLD_XSTEP →
        //   FoldChip → FOLD_STATE → §4.D keystone → JACKPOT_MSG → C4
        // for every single-Layer-0 params set. Degree 2 (one-hot ·
        // linear); vacuous off fold rows (`FOLD_STRIPE_SEL` all 0).
        // Pinned production path only — the unit `CompositeFullAir`
        // keeps `FOLD_XSTEP` free so the ~300 constraint-logic
        // tests stay untouched (identical scoping to §4.D). With
        // G1+G2 `sx_bound` is `true` for all `num_stripes ≤
        // STRIPE_MAX`; the flag remains for the G3/PROD-segment
        // boundary case.
        if self.sx_bound {
            use crate::composite_layout::STRIPE_MAX;
            let main3 = builder.main();
            let c3 = main3.current_slice();
            let fx: AB::Var = c3[crate::composite_layout::FOLD_XSTEP];
            let sel: [AB::Var; STRIPE_MAX] =
                core::array::from_fn(|s| c3[crate::composite_layout::FOLD_STRIPE_SEL_START + s]);
            let xr: [AB::Var; STRIPE_MAX] =
                core::array::from_fn(|s| c3[crate::composite_layout::SX_XR_START + s]);
            let mut bind: AB::Expr = <AB::Expr as PrimeCharacteristicRing>::ZERO;
            for s in 0..STRIPE_MAX {
                bind = bind + sel[s].into() * (fx.into() - xr[s].into());
            }
            builder.assert_zero(bind);
        }
    }
}

impl<AB: AirBuilder> Air<AB> for CompositeFullAir {
    fn eval(&self, builder: &mut AB) {
        // STARK_ROW_IDX monotonic.
        StarkRowChip.eval(builder);

        // Range tables: enforce table integrity.
        URange8Chip::default().eval(builder);
        URange13Chip::default().eval(builder);
        IRange7P1Chip::default().eval(builder);
        IRange8Chip::default().eval(builder);

        // I8U8 conversion table.
        I8U8Chip.eval(builder);

        // CONTROL_PREP unpacking + MAT_ID limb decomposition.
        ControlChip.eval(builder);

        // AB_ID_PREP unpacking for the first A/B sub-slice ID pair.
        // The remaining per-sub-slice IDs are verifier-fixed by the
        // CRIT-1 program pin directly; this keeps the historical pack
        // meaningful instead of leaving AB_ID_PREP as inert metadata.
        {
            use crate::composite_layout::{
                AB_ID_LIMBS_START, AB_ID_PREP, A_ID, BITS_PER_LIMB, B_ID,
            };
            let main = builder.main();
            let cur = main.current_slice();
            let base = <AB::F as PrimeCharacteristicRing>::from_u32(1u32 << BITS_PER_LIMB);
            let base2 = <AB::F as PrimeCharacteristicRing>::from_u64(1u64 << (2 * BITS_PER_LIMB));
            let a_id: AB::Expr =
                cur[AB_ID_LIMBS_START].into() + cur[AB_ID_LIMBS_START + 1].into() * base.clone();
            let b_id: AB::Expr =
                cur[AB_ID_LIMBS_START + 2].into() + cur[AB_ID_LIMBS_START + 3].into() * base;
            builder.assert_eq(cur[A_ID], a_id.clone());
            builder.assert_eq(cur[B_ID], b_id.clone());
            builder.assert_eq(cur[AB_ID_PREP], a_id + b_id * base2);
        }

        // Input chip: NOISE_PACKED_PREP unpacking + NOISED_PACKED
        // = polyval(MAT, 256) + polyval(NOISE, 256) integrity.
        InputChip.eval(builder);

        // Matmul cumsum-update chip (Phase 12b wiring): reads
        // A_NOISED_UNPACK / B_NOISED_UNPACK / CUMSUM_TILE /
        // IS_RESET_CUMSUM / IS_UPDATE_CUMSUM at composite-layout
        // offsets.
        MatmulCumsumChip::eval_composite(builder);

        // BLAKE3 chip (Phase 12c wiring): reads BLAKE3_ROUND (4
        // state snapshots), BLAKE3_MSG, BLAKE3_CV, CV_OR_TWEAK_PREP,
        // CV_OUT at composite-layout offsets. Dispatch driven by
        // IS_NEW_BLAKE / IS_LAST_ROUND selector bits (unpacked from
        // CONTROL_PREP by ControlChip).
        Blake3Chip::eval_composite(builder);

        // Jackpot chip (Phase 12d wiring): reads JACKPOT_MSG (16
        // u32 slots), BIT_REG (V_BITS), JACKPOT_X_BITS, and
        // JACKPOT_SLOT_SEL. Dispatch driven by IS_HASH_JACKPOT
        // selector. Phase 14b's LogUp wiring will tie the X_BITS
        // bit-decomposition back to CUMSUM_BUFFER.
        JackpotChip::eval_composite(builder);

        // FoldChip (HIGH-2.2 §4.B/§4.A): Pearl §4.5 rotl13-XOR
        // fold over the FOLD_* composite block. A pure function
        // of the per-stripe X_STEP sequence (§4.0, Option B2).
        // All-zero FOLD columns satisfy it vacuously, so traces
        // with no fold activity are unaffected; the §4.D keystone
        // (in CompositeFullAirPinned) binds JACKPOT_MSG to the
        // last-row FOLD_STATE.
        crate::chips::fold::FoldChip::eval_composite(builder);

        // StripeXorChip (HIGH-2.2 §6(b)): cross-row transport that
        // XOR-reduces the sub-block-major matmul sweep's per-row
        // accumulator-after-step into a per-stripe register, and
        // binds SX_IN to the matmul chip's `nxt.CUMSUM_TILE`
        // (`committed A/B → CUMSUM → SX_IN → XR`). All-zero SX
        // columns satisfy it vacuously, so traces with no
        // stripe-xor activity are unaffected; the §6(b) keystone
        // (in CompositeFullAirPinned) binds FOLD_XSTEP to the
        // final XR lane.
        crate::chips::stripe_xor::StripeXorChip::eval_composite(builder);

        // M-S1 (§4.C.11) — matmul-input pack-link. On every matmul
        // row (`IS_RESET_CUMSUM + IS_UPDATE_CUMSUM`) the packed
        // `A_NOISED[c]` / `B_NOISED[c]` cells equal the base-256
        // polyval of the 4 i8 `*_NOISED_UNPACK` lanes they cover
        // (same encoding `InputChip` uses for `NOISED_PACKED`). The
        // `BUS_MATMUL_INPUT` LogUp binds the packed `A_NOISED`/
        // `B_NOISED` cells to the canonical producer store; the
        // `MatmulCumsumChip` dot multiplies the *unpack* lanes.
        // This link makes them provably the same value (§4.C.11
        // finding 2). Degree 2 (gate · linear); vacuous off matmul
        // rows ⇒ zero regression.
        {
            use crate::composite_layout::{
                A_NOISED_LEN, A_NOISED_START, A_NOISED_UNPACK_START, B_NOISED_LEN, B_NOISED_START,
                B_NOISED_UNPACK_START, IS_RESET_CUMSUM, IS_UPDATE_CUMSUM,
            };
            const N: usize = 8;
            debug_assert_eq!(A_NOISED_LEN, N);
            debug_assert_eq!(B_NOISED_LEN, N);
            let (is_reset, is_update): (AB::Var, AB::Var);
            let a_p: [AB::Var; N];
            let b_p: [AB::Var; N];
            let a_u: [AB::Var; 4 * N];
            let b_u: [AB::Var; 4 * N];
            {
                let main = builder.main();
                let cur = main.current_slice();
                is_reset = cur[IS_RESET_CUMSUM];
                is_update = cur[IS_UPDATE_CUMSUM];
                a_p = core::array::from_fn(|c| cur[A_NOISED_START + c]);
                b_p = core::array::from_fn(|c| cur[B_NOISED_START + c]);
                a_u = core::array::from_fn(|i| cur[A_NOISED_UNPACK_START + i]);
                b_u = core::array::from_fn(|i| cur[B_NOISED_UNPACK_START + i]);
            }
            let matmul_active: AB::Expr = is_reset.into() + is_update.into();
            let b256 = <AB::F as PrimeCharacteristicRing>::from_i32(256);
            for (packed, unpack) in [(&a_p, &a_u), (&b_p, &b_u)] {
                for c in 0..N {
                    let mut recon: AB::Expr = <AB::Expr as PrimeCharacteristicRing>::ZERO;
                    let mut pow: AB::F = <AB::F as PrimeCharacteristicRing>::ONE;
                    for d in 0..4 {
                        recon = recon + unpack[c * 4 + d] * pow.clone();
                        pow = pow * b256.clone();
                    }
                    let diff: AB::Expr = packed[c].into() - recon;
                    builder.assert_zero(matmul_active.clone() * diff);
                }
            }
        }

        // Public-input binding.
        //
        // CUMSUM_TILE and JACKPOT_MSG bind on the LAST row via the
        // `fill_*_passthrough` helpers. HASH_A and HASH_B bind on
        // whichever row sets `IS_HASH_A` / `IS_HASH_B` (selector-
        // gated, fires once per matrix when a real `place_matrix_
        // hash_*` block is in the trace; vacuous for baseline
        // traces with no hash activity).
        //
        // Snapshot the PIs and the current-row cells into owned
        // arrays before opening sub-builders (the sub-builder
        // borrows `builder` mutably; can't coexist with the
        // `public_values()` slice borrow).
        let pi_cumsum: [AB::PublicVar; PI_CUMSUM_LEN] =
            core::array::from_fn(|i| builder.public_values()[PI_CUMSUM_OFFSET + i]);
        let pi_jackpot: [AB::PublicVar; JACKPOT_SIZE] =
            core::array::from_fn(|i| builder.public_values()[PI_JACKPOT_OFFSET + i]);
        let pi_hash_a: [AB::PublicVar; CV_OUT_LEN] =
            core::array::from_fn(|i| builder.public_values()[PI_HASH_A_OFFSET + i]);
        let pi_hash_b: [AB::PublicVar; CV_OUT_LEN] =
            core::array::from_fn(|i| builder.public_values()[PI_HASH_B_OFFSET + i]);
        // C1/C4 — Pearl Layer-0 canonical bindings.
        let pi_job_key: [AB::PublicVar; CV_IN_LEN] =
            core::array::from_fn(|i| builder.public_values()[PI_JOB_KEY_OFFSET + i]);
        let pi_commitment_hash: [AB::PublicVar; CV_IN_LEN] =
            core::array::from_fn(|i| builder.public_values()[PI_COMMITMENT_HASH_OFFSET + i]);
        let pi_hash_jackpot: [AB::PublicVar; CV_OUT_LEN] =
            core::array::from_fn(|i| builder.public_values()[PI_HASH_JACKPOT_OFFSET + i]);
        let main = builder.main();
        let cur = main.current_slice();
        let cur_cumsum: [AB::Var; PI_CUMSUM_LEN] =
            core::array::from_fn(|i| cur[CUMSUM_TILE_START + i]);
        let cur_jackpot: [AB::Var; JACKPOT_SIZE] =
            core::array::from_fn(|i| cur[JACKPOT_MSG_START + i]);
        let cur_is_hash_a: AB::Var = cur[IS_HASH_A];
        let cur_is_hash_b: AB::Var = cur[IS_HASH_B];
        let cur_is_hash_jackpot: AB::Var = cur[IS_HASH_JACKPOT];
        let cur_is_use_job_key: AB::Var = cur[IS_USE_JOB_KEY];
        let cur_is_use_commitment_hash: AB::Var = cur[IS_USE_COMMITMENT_HASH];
        let cur_cv_out: [AB::Var; CV_OUT_LEN] = core::array::from_fn(|i| cur[CV_OUT_START + i]);
        let cur_cv_in: [AB::Var; CV_IN_LEN] = core::array::from_fn(|i| cur[CV_IN_START + i]);

        // Selector-gated per-row PI binding (fires on every row but
        // only constrains when the selector = 1).
        //
        // HASH_A / HASH_B / HASH_JACKPOT bind the BLAKE3 `CV_OUT`
        // on their producing rows (Pearl `pearl_circuit.rs:20-22`
        // constraints b + d). JOB_KEY / COMMITMENT_HASH bind the
        // BLAKE3 `CV_IN` (the chain-pinned key) on rows that use
        // them as the compression key — this ties the entire proof
        // to the block-header-derived κ and the `s_a` noise seed,
        // making it a proof *of work for this block* rather than an
        // unanchored "some matmul happened" statement.
        for i in 0..CV_OUT_LEN {
            builder
                .assert_zero(cur_is_hash_a.into() * (cur_cv_out[i].into() - pi_hash_a[i].into()));
            builder
                .assert_zero(cur_is_hash_b.into() * (cur_cv_out[i].into() - pi_hash_b[i].into()));
            // C4: IS_HASH_JACKPOT · (CV_OUT[i] − PI_HASH_JACKPOT[i]) = 0
            builder.assert_zero(
                cur_is_hash_jackpot.into() * (cur_cv_out[i].into() - pi_hash_jackpot[i].into()),
            );
        }
        for i in 0..CV_IN_LEN {
            // C1: IS_USE_JOB_KEY · (CV_IN[i] − PI_JOB_KEY[i]) = 0
            builder.assert_zero(
                cur_is_use_job_key.into() * (cur_cv_in[i].into() - pi_job_key[i].into()),
            );
            // C1: IS_USE_COMMITMENT_HASH · (CV_IN[i] − PI_COMMITMENT_HASH[i]) = 0
            builder.assert_zero(
                cur_is_use_commitment_hash.into()
                    * (cur_cv_in[i].into() - pi_commitment_hash[i].into()),
            );
        }

        // C3 (M52 step 4.3+) — bind MAT_UNPACK to BLAKE3_MSG.
        //
        // Closes the residual matrix-binding soundness gap. The
        // chain is:
        //   canonical store ─(noised_packed bus, M52 4.1)─ MAT_UNPACK
        //   MAT_UNPACK ─(i8u8 bus, IS_MSG_MAT-gated)─ UINT8_DATA
        //   UINT8_DATA ─(THIS constraint)─ BLAKE3_MSG
        //   BLAKE3_MSG → mixing rounds → CV_OUT → HASH_A (M52 step 2)
        // Without this link an adversary could put matrix X in
        // MAT_UNPACK (what the buses bind) and matrix Y in
        // BLAKE3_MSG (what actually gets hashed into HASH_A).
        //
        // Gate: IS_MSG_MAT · IS_NEW_BLAKE. `IS_MSG_MAT` alone is
        // *overloaded* — the i8u8 / urange8 / noised_packed bus
        // emissions reuse it to mean "UINT8_DATA holds matrix
        // bytes for range/conversion checking," on rows that are
        // NOT blake3 compression rows (BLAKE3_MSG = 0 there).
        // Gating C3 on bare IS_MSG_MAT wrongly forces those
        // data-validation rows to also satisfy
        // BLAKE3_MSG = base256(UINT8_DATA). The extra IS_NEW_BLAKE
        // factor restricts C3 to a blake3 compression's round-0
        // row (its unpermuted message), which is exactly where a
        // matrix-leaf message must equal the matrix-byte view —
        // and is never set by the i8u8-bus tests. `place_blake3_hash`
        // sets IS_NEW_BLAKE on row 0 of every block; the F1
        // matrix-leaf path additionally sets IS_MSG_MAT there.
        // Round 0 is the unpermuted message, so word j = LE bytes
        // 4j..4j+4 — the same order `u32::from_le_bytes` uses.
        // Vacuous on every current trace (no row has both set).
        //
        // §4.C.2 c-exact (cx.1b-constraints) — generalize C3 from
        // the FIXED message words {0,1} to a verifier-pinned word-
        // PAIR `p`. cx.0 (`2bbf4cd`) proved each co-located store
        // window lives at leaf message words `(2p, 2p+1)`,
        // `p = word_off/2 ∈ 0..8`, at a witness-free address.
        // `MSG_PAIR_SEL[0..8]` is a per-row one-hot; the C3 gate
        // `g = IS_MSG_MAT · IS_NEW_BLAKE` is unchanged. Constraints:
        //   (i)   MSG_PAIR_SEL[p] boolean,
        //   (ii)  Σ_p MSG_PAIR_SEL[p] == g   (exactly one pair iff
        //         the C3 gate is live; 0 elsewhere),
        //   (iii) Σ_p MSG_PAIR_SEL[p]·(BLAKE3_MSG[2p+j] −
        //         recomposed_j) = 0, j∈{0,1}.
        // (i)+(ii) ⇒ when g=1 exactly one pair selected; cx.1c
        // pins *which* p in CONTROL_PREP so the prover cannot
        // choose it (a forged p ⇒ Σ≠pinned ⇒ reject). All three
        // are degree ≤2 (≤ the prior degree-3 C3). ZERO-BLAST:
        // every current trace has g=0 and MSG_PAIR_SEL=0 (default)
        // ⇒ (i) 0∈{0,1} ✓, (ii) 0==0 ✓, (iii) 0 ✓ — byte-identical
        // to the prior (vacuous) C3. Note (iii) is written
        // `Σ sel·(msg−recomposed)`, NOT `Σ sel·msg − recomposed`:
        // the former is vacuous when all sel=0 (g=0), the latter
        // would wrongly force recomposed==0 on every g=0 row.
        let cur_is_msg_mat: AB::Var = cur[IS_MSG_MAT];
        let cur_is_new_blake: AB::Var = cur[IS_NEW_BLAKE];
        let c3_gate: AB::Expr = cur_is_msg_mat.into() * cur_is_new_blake.into();
        // (i) booleanity + (ii) Σ MSG_PAIR_SEL == g.
        let mut pair_sum: AB::Expr = <AB::Expr as PrimeCharacteristicRing>::ZERO;
        for p in 0..MSG_PAIR_SEL_LEN {
            let sel: AB::Var = cur[MSG_PAIR_SEL_START + p];
            builder.assert_bool(sel);
            pair_sum = pair_sum + sel.into();
        }
        builder.assert_zero(pair_sum - c3_gate.clone());
        // (iii) §4.C.2 c-exact cx.2/X1 — **whole-block** C3.
        // X1 (SEC_4C2 §8.6): the strip-opening leaf round-0 row
        // carries its entire 64-byte committed block in the
        // widened `UINT8_DATA[0..64]`; bind **every** one of the
        // 16 `BLAKE3_MSG` words to it (not a `MSG_PAIR_SEL`-
        // selected pair — that was cx.1b's interim window-bind).
        // ⇒ `UINT8_DATA[0..64]` ≡ the committed block ∈ `HASH_A`
        // (every swept 8-byte sub-slice of the block is therefore
        // covered by this one row — the resolution of the cx.2.0
        // multiplicity blocker). `MSG_PAIR_SEL` / cx.1c's
        // `CONTROL_PREP` pin survive (above + ControlChip) as the
        // verifier-fixed *sub-slice address* the co-located M-S1
        // `noised_packed` producer uses at the activation stage —
        // not the C3 binding.
        //   g·(BLAKE3_MSG[w] − Σ_{b<4} UINT8_DATA[4w+b]·256^b)=0,
        //   w ∈ 0..16  (degree 3 = the pre-cx.1b C3 degree).
        // ZERO-BLAST: g = IS_MSG_MAT·IS_NEW_BLAKE = 0 on every
        // current trace (nothing co-locates yet) ⇒ all 16 terms
        // ×g = 0 ⇒ vacuous, byte-identical. Activation flips g=1
        // on the co-located leaf rows (its own staged landing).
        let base256 = <AB::F as PrimeCharacteristicRing>::from_i32(256);
        for w in 0..(UINT8_DATA_LEN / 4) {
            // recomposed_w = Σ_{b<4} UINT8_DATA[4w+b]·256^b
            // (base-256 LE, the order BLAKE3 from_le_bytes uses).
            let mut recomposed: AB::Expr = <AB::Expr as PrimeCharacteristicRing>::ZERO;
            let mut pow: AB::F = <AB::F as PrimeCharacteristicRing>::ONE;
            for b in 0..4 {
                recomposed = recomposed + cur[UINT8_DATA_START + 4 * w + b] * pow.clone();
                pow = pow * base256.clone();
            }
            let msg_word: AB::Var = cur[BLAKE3_MSG_START + w];
            builder.assert_zero(c3_gate.clone() * (msg_word.into() - recomposed));
        }

        let mut last = builder.when_last_row();
        for i in 0..PI_CUMSUM_LEN {
            last.assert_eq(cur_cumsum[i], pi_cumsum[i]);
        }
        for i in 0..JACKPOT_SIZE {
            last.assert_eq(cur_jackpot[i], pi_jackpot[i]);
        }
    }
}

#[cfg(test)]
mod tests {
    //! End-to-end integration test: build a TOTAL_TRACE_WIDTH ×
    //! MIN_STARK_LEN trace where every wired chip's columns are
    //! filled correctly, then prove + verify.

    use p3_field::integers::QuotientMap;
    use p3_matrix::dense::RowMajorMatrix;
    use p3_uni_stark::{prove, verify};

    use super::*;
    use crate::chips::i8u8::I8U8_TABLE_SIZE;
    use crate::chips::range_table::{IRange7P1Chip, IRange8Chip, URange13Chip, URange8Chip};
    use crate::circuit::{build_stark_config, AiPowStarkConfig, CircuitConfig};
    use crate::composite_layout::{MIN_STARK_LEN, STARK_ROW_IDX, TOTAL_TRACE_WIDTH};
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

    /// Build a baseline trace of `n` rows where the wired chips
    /// are satisfied:
    ///   * STARK_ROW_IDX = 0, 1, 2, ..., n-1.
    ///   * Range tables filled by their fill_row helpers (so the
    ///     last row equals MAX).
    ///   * I8U8 table filled by its fill_row helper.
    ///   * All other columns = 0 (selectors off, data = 0 satisfies
    ///     control's CONTROL_PREP = 0 and input chip's degenerate
    ///     polyval = 0 + 0 = 0).
    fn build_baseline_trace(n: usize) -> RowMajorMatrix<crate::Val> {
        assert!(n.is_power_of_two(), "trace length must be power of 2");
        let mut flat = vec![crate::Val::default(); n * TOTAL_TRACE_WIDTH];

        for r in 0..n {
            let row_start = r * TOTAL_TRACE_WIDTH;
            let row = &mut flat[row_start..row_start + TOTAL_TRACE_WIDTH];

            // STARK_ROW_IDX = r.
            row[STARK_ROW_IDX] = <crate::Val as QuotientMap<u64>>::from_int(r as u64);

            // Range table cells.
            URange8Chip::default().fill_row(r, row);
            URange13Chip::default().fill_row(r, row);
            IRange7P1Chip::default().fill_row(r, row);
            IRange8Chip::default().fill_row(r, row);

            // I8U8 table cells.
            I8U8Chip.fill_row(r, row);

            // CONTROL_PREP / MAT_ID / NOISE_UNPACK / MAT_UNPACK
            // / NOISED_PACKED all left as 0 — control + input
            // chips' constraints all degenerate to 0 = 0 in this
            // case.
        }

        RowMajorMatrix::new(flat, TOTAL_TRACE_WIDTH)
    }

    #[test]
    fn pinned_try_new_rejects_malformed_program_shape() {
        let bad_width = RowMajorMatrix::new(vec![crate::Val::default(); 8], 1);
        assert!(matches!(
            CompositeFullAirPinned::try_new(bad_width),
            Err(ProgramShapeError::WidthMismatch {
                expected,
                actual: 1
            }) if expected == PROGRAM_COLS.len()
        ));

        let bad_height = RowMajorMatrix::new(
            vec![crate::Val::default(); PROGRAM_COLS.len() * 3],
            PROGRAM_COLS.len(),
        );
        assert!(matches!(
            CompositeFullAirPinned::try_new(bad_height),
            Err(ProgramShapeError::HeightNotPowerOfTwo { height: 3 })
        ));

        let valid = RowMajorMatrix::new(
            vec![crate::Val::default(); PROGRAM_COLS.len() * 4],
            PROGRAM_COLS.len(),
        );
        assert!(CompositeFullAirPinned::try_new(valid).is_ok());
    }

    #[test]
    fn composite_full_air_baseline_trace_verifies() {
        let cfg = build_stark_config(&test_zk_params(), &CircuitConfig::TEST_PEARL);
        let trace = build_baseline_trace(MIN_STARK_LEN);
        let pis =
            crate::composite_public::CompositePublicInputs::derive_from_matrix(&trace).to_vec();
        let proof = prove::<AiPowStarkConfig, _>(&cfg, &CompositeFullAir, trace, &pis);
        verify::<AiPowStarkConfig, _>(&cfg, &CompositeFullAir, &proof, &pis)
            .expect("baseline composite trace must verify");
    }

    /// Tamper STARK_ROW_IDX — should reject.
    #[test]
    fn composite_full_air_rejects_bad_row_idx() {
        let cfg = build_stark_config(&test_zk_params(), &CircuitConfig::TEST_PEARL);
        let mut trace = build_baseline_trace(MIN_STARK_LEN);
        // Set row 3's STARK_ROW_IDX to 999 instead of 3.
        let target = 3 * TOTAL_TRACE_WIDTH + STARK_ROW_IDX;
        trace.values[target] = <crate::Val as QuotientMap<u64>>::from_int(999);
        let pis =
            crate::composite_public::CompositePublicInputs::derive_from_matrix(&trace).to_vec();
        let proof = prove::<AiPowStarkConfig, _>(&cfg, &CompositeFullAir, trace, &pis);
        assert!(
            verify::<AiPowStarkConfig, _>(&cfg, &CompositeFullAir, &proof, &pis).is_err(),
            "tampered STARK_ROW_IDX must reject"
        );
    }

    /// Tamper a range table cell (URANGE8_TABLE row 1 — should be
    /// 1, set to 5). The transition delta check `(table[i+1] −
    /// table[i]) ∈ {0, 1}` rejects.
    #[test]
    fn composite_full_air_rejects_bad_range_table() {
        let cfg = build_stark_config(&test_zk_params(), &CircuitConfig::TEST_PEARL);
        let mut trace = build_baseline_trace(MIN_STARK_LEN);
        use crate::composite_layout::URANGE8_TABLE;
        let target = 1 * TOTAL_TRACE_WIDTH + URANGE8_TABLE;
        trace.values[target] = <crate::Val as QuotientMap<u64>>::from_int(5);
        let pis =
            crate::composite_public::CompositePublicInputs::derive_from_matrix(&trace).to_vec();
        let proof = prove::<AiPowStarkConfig, _>(&cfg, &CompositeFullAir, trace, &pis);
        assert!(
            verify::<AiPowStarkConfig, _>(&cfg, &CompositeFullAir, &proof, &pis).is_err(),
            "tampered range table must reject"
        );
    }

    /// Tamper I8U8 AUX. AUX must start at 0, become 1 at the
    /// sign-boundary row, and stay 1. Setting AUX = 1 on row 0
    /// breaks the first-row constraint.
    #[test]
    fn composite_full_air_rejects_bad_i8u8_aux() {
        let cfg = build_stark_config(&test_zk_params(), &CircuitConfig::TEST_PEARL);
        let mut trace = build_baseline_trace(MIN_STARK_LEN);
        use crate::composite_layout::I8U8_AUX;
        let target = 0 * TOTAL_TRACE_WIDTH + I8U8_AUX;
        trace.values[target] = <crate::Val as QuotientMap<u64>>::from_int(1);
        let pis =
            crate::composite_public::CompositePublicInputs::derive_from_matrix(&trace).to_vec();
        let proof = prove::<AiPowStarkConfig, _>(&cfg, &CompositeFullAir, trace, &pis);
        assert!(
            verify::<AiPowStarkConfig, _>(&cfg, &CompositeFullAir, &proof, &pis).is_err(),
            "tampered I8U8_AUX must reject"
        );
    }

    /// Tamper CONTROL_PREP — set a selector bit without updating
    /// CONTROL_PREP. The control chip's constraint
    /// `CONTROL_PREP == polyval(selectors..., mat_id; base=2)`
    /// rejects.
    #[test]
    fn composite_full_air_rejects_inconsistent_control_prep() {
        let cfg = build_stark_config(&test_zk_params(), &CircuitConfig::TEST_PEARL);
        let mut trace = build_baseline_trace(MIN_STARK_LEN);
        use crate::composite_layout::IS_RESET_CUMSUM;
        // Flip IS_RESET_CUMSUM on row 0 without updating CONTROL_PREP.
        let target = 0 * TOTAL_TRACE_WIDTH + IS_RESET_CUMSUM;
        trace.values[target] = <crate::Val as QuotientMap<u64>>::from_int(1);
        let pis =
            crate::composite_public::CompositePublicInputs::derive_from_matrix(&trace).to_vec();
        let proof = prove::<AiPowStarkConfig, _>(&cfg, &CompositeFullAir, trace, &pis);
        assert!(
            verify::<AiPowStarkConfig, _>(&cfg, &CompositeFullAir, &proof, &pis).is_err(),
            "inconsistent CONTROL_PREP must reject"
        );
    }

    /// Tamper NOISED_PACKED without updating MAT_UNPACK / NOISE_UNPACK.
    /// The input chip's constraint forces NOISED_PACKED[i] ==
    /// polyval(MAT[i*4..(i+1)*4], 256) + polyval(NOISE[i*4..(i+1)*4],
    /// 256). Changing NOISED_PACKED but not the unpacks rejects.
    #[test]
    fn composite_full_air_rejects_inconsistent_noised_packed() {
        let cfg = build_stark_config(&test_zk_params(), &CircuitConfig::TEST_PEARL);
        let mut trace = build_baseline_trace(MIN_STARK_LEN);
        use crate::composite_layout::NOISED_PACKED_START;
        let target = 0 * TOTAL_TRACE_WIDTH + NOISED_PACKED_START;
        trace.values[target] = <crate::Val as QuotientMap<u64>>::from_int(42);
        let pis =
            crate::composite_public::CompositePublicInputs::derive_from_matrix(&trace).to_vec();
        let proof = prove::<AiPowStarkConfig, _>(&cfg, &CompositeFullAir, trace, &pis);
        assert!(
            verify::<AiPowStarkConfig, _>(&cfg, &CompositeFullAir, &proof, &pis).is_err(),
            "inconsistent NOISED_PACKED must reject"
        );
    }

    #[test]
    fn composite_full_air_width_matches_total_trace_width() {
        let air = CompositeFullAir;
        let w = <CompositeFullAir as BaseAir<crate::Val>>::width(&air);
        assert_eq!(w, TOTAL_TRACE_WIDTH);
    }

    /// Production-scale anchor: at exactly MIN_STARK_LEN (8192)
    /// rows the trace passes. This is the row count Pearl pins
    /// for its smallest stark proof; bigger sizes are powers of 2
    /// up.
    #[test]
    fn composite_full_air_min_stark_len_anchor() {
        let cfg = build_stark_config(&test_zk_params(), &CircuitConfig::TEST_PEARL);
        let trace = build_baseline_trace(MIN_STARK_LEN);
        assert_eq!(
            trace.values.len(),
            MIN_STARK_LEN * TOTAL_TRACE_WIDTH,
            "trace dimensions"
        );
        let pis =
            crate::composite_public::CompositePublicInputs::derive_from_matrix(&trace).to_vec();
        let proof = prove::<AiPowStarkConfig, _>(&cfg, &CompositeFullAir, trace, &pis);
        verify::<AiPowStarkConfig, _>(&cfg, &CompositeFullAir, &proof, &pis)
            .expect("min-stark-len trace must verify");
    }

    /// Sanity: I8U8 table-size matches Pearl's `1 << 8 = 256`.
    #[test]
    fn i8u8_table_size_pinned() {
        assert_eq!(I8U8_TABLE_SIZE, 256);
    }

    /// Tamper a CUMSUM_TILE cell — the matmul cumsum-update
    /// constraint (gated by IS_RESET_CUMSUM + IS_UPDATE_CUMSUM)
    /// becomes `next = (0 + 0) * dot + (1 - 0) * cur = cur`, so
    /// any cross-row change to CUMSUM_TILE rejects.
    #[test]
    fn composite_full_air_rejects_changed_cumsum_without_selectors() {
        let cfg = build_stark_config(&test_zk_params(), &CircuitConfig::TEST_PEARL);
        let mut trace = build_baseline_trace(MIN_STARK_LEN);
        use crate::composite_layout::CUMSUM_TILE_START;
        // Set CUMSUM_TILE[0] on row 1 to 42 while row 0 is still 0.
        // With both selectors zero (passthrough mode), the matmul
        // constraint forces row 1's CUMSUM = row 0's CUMSUM = 0.
        let target = 1 * TOTAL_TRACE_WIDTH + CUMSUM_TILE_START;
        trace.values[target] = <crate::Val as QuotientMap<u64>>::from_int(42);
        let pis =
            crate::composite_public::CompositePublicInputs::derive_from_matrix(&trace).to_vec();
        let proof = prove::<AiPowStarkConfig, _>(&cfg, &CompositeFullAir, trace, &pis);
        assert!(
            verify::<AiPowStarkConfig, _>(&cfg, &CompositeFullAir, &proof, &pis).is_err(),
            "tampered CUMSUM_TILE in passthrough mode must reject"
        );
    }

    /// Tamper a BLAKE3 state bit (in STATE1.row2) — the BLAKE3
    /// round constraint asserts boolean bits in every state
    /// snapshot via xor_32_shift_if's `assert_bool` calls.
    /// Setting a row2 cell to 2 violates booleanity, regardless of
    /// selectors.
    #[test]
    fn composite_full_air_rejects_non_boolean_blake3_state_bit() {
        let cfg = build_stark_config(&test_zk_params(), &CircuitConfig::TEST_PEARL);
        let mut trace = build_baseline_trace(MIN_STARK_LEN);
        use crate::composite_layout::BLAKE3_ROUND_START;
        // STATE1.row2[0] starts at offset BLAKE3_ROUND_START +
        // STATE_W + 4 (= STATE1 cells 4..36 hold row2[0]'s bits).
        const STATE_W: usize = 264;
        let target = 0 * TOTAL_TRACE_WIDTH + BLAKE3_ROUND_START + STATE_W + 4;
        trace.values[target] = <crate::Val as QuotientMap<u64>>::from_int(2);
        let pis =
            crate::composite_public::CompositePublicInputs::derive_from_matrix(&trace).to_vec();
        let proof = prove::<AiPowStarkConfig, _>(&cfg, &CompositeFullAir, trace, &pis);
        assert!(
            verify::<AiPowStarkConfig, _>(&cfg, &CompositeFullAir, &proof, &pis).is_err(),
            "non-boolean BLAKE3 state bit must reject"
        );
    }

    /// Tamper an A_NOISED_UNPACK cell *without* setting either
    /// matmul selector. Since both selectors are 0, the dot
    /// product term is multiplied by `(is_reset + is_update) = 0`
    /// and the change has no effect. The constraint stays
    /// satisfied — this test is a regression anchor confirming
    /// the gating actually silences correctly.
    #[test]
    fn composite_full_air_accepts_changed_a_unpack_in_passthrough() {
        let cfg = build_stark_config(&test_zk_params(), &CircuitConfig::TEST_PEARL);
        let mut trace = build_baseline_trace(MIN_STARK_LEN);
        use crate::composite_layout::A_NOISED_UNPACK_START;
        let target = 1 * TOTAL_TRACE_WIDTH + A_NOISED_UNPACK_START;
        trace.values[target] = <crate::Val as QuotientMap<i64>>::from_int(100);
        let pis =
            crate::composite_public::CompositePublicInputs::derive_from_matrix(&trace).to_vec();
        let proof = prove::<AiPowStarkConfig, _>(&cfg, &CompositeFullAir, trace, &pis);
        verify::<AiPowStarkConfig, _>(&cfg, &CompositeFullAir, &proof, &pis)
            .expect("change to A_NOISED_UNPACK in passthrough mode must verify");
    }
}

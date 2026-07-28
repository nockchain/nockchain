//! Phase 14b — `CompositeFullAir` extended with LogUp emissions.
//!
//! `CompositeFullAirWithLookups` is a thin wrapper around
//! [`CompositeFullAir`] that requires its builder to implement
//! [`InteractionBuilder`] (the lookup-aware builder from
//! `p3-lookup`). All of `CompositeFullAir`'s constraints fire
//! identically; on top, this AIR pushes the cross-chip lookup
//! interactions that Phase 11 documented.
//!
//! ## Wiring approach
//!
//! Phase 14b-2 wires **one** lookup bus end-to-end as a proof of
//! concept that scales: `urange8` (u8 range check). Subsequent
//! sub-phases add the remaining buses by the same pattern.
//!
//! Each bus has:
//!   * A **table-side** emission of `(table_value, −freq_value)`
//!     per row, with `count_weight = 0`. The range-table chip
//!     provides every value once across `[MIN..=MAX]`; the
//!     `*_FREQ` column carries how many times that value is
//!     consumed.
//!   * A **query-side** emission of `(query_value, +query_flag)`
//!     per row, with `count_weight = 1`. The chip that has u8
//!     cells emits the queries.
//!
//! For matrix-message rows, `UINT8_DATA` byte validity is enforced
//! through the paired `i8u8` bus plus `irange8(MAT_UNPACK)`. The
//! standalone `urange8` bus keeps one byte query as a compatibility
//! anchor for the recursive verifier's lookup shape, but does not pay
//! the 63 redundant extra query interactions.
//!
//! ## What this gives us
//!
//! With this wired, `prove_batch` + `verify_batch` will reject
//! traces where:
//!   * `URANGE8_FREQ` is over- or under-claimed for the anchored
//!     byte query.
//!   * Any `UINT8_DATA[0..64]` cell is inconsistent with its signed
//!     `MAT_UNPACK` view while `IS_MSG_MAT = 1`.
//!
//! Range-table integrity (TABLE column enumerates `[0..256)`)
//! continues to be enforced by `URange8Chip`'s constraints —
//! see [`crate::chips::range_table`].

use p3_air::{Air, AirBuilder, BaseAir, WindowAccess};
use p3_lookup::{Count, InteractionBuilder};

use crate::composite_full_air::{CompositeFullAir, CompositeFullAirPinned, ProgramShapeError};
use crate::composite_layout::{
    AB_ID_LIMBS_LEN, AB_ID_LIMBS_START, A_ID, A_ID_LEN, A_NOISED_START, A_NOISED_UNPACK_LEN,
    A_NOISED_UNPACK_START, B_ID, B_ID_LEN, B_NOISED_START, B_NOISED_UNPACK_LEN,
    B_NOISED_UNPACK_START, CV_IN_LEN, CV_IN_START, CV_OR_TWEAK_PREP, CV_OUT_FREQ, CV_OUT_LEN,
    CV_OUT_START, I8U8_FREQ, I8U8_TABLE, IRANGE7P1_FREQ, IRANGE7P1_TABLE, IRANGE8_FREQ,
    IRANGE8_TABLE, IS_CV_IN, IS_MSG_MAT, IS_RESET_CUMSUM, IS_UPDATE_CUMSUM, MAT_FREQ, MAT_ID,
    MAT_ID_LIMBS_LEN, MAT_ID_LIMBS_START, MAT_UNPACK_START, MAT_UNPACK_WIN, NOISED_PACKED_START,
    NOISE_UNPACK_START, NOISE_UNPACK_WIN, STARK_ROW_IDX, TOTAL_TRACE_WIDTH, UINT8_DATA_START,
    UINT8_DATA_WIN, URANGE13_FREQ, URANGE13_TABLE, URANGE8_FREQ, URANGE8_TABLE,
};
use crate::composite_lookups::{
    BUS_CV_ROUTING, BUS_I8U8, BUS_IRANGE7P1, BUS_IRANGE8, BUS_NOISED_PACKED, BUS_URANGE13,
    BUS_URANGE8,
};

/// Lookup-aware composite AIR.
///
/// Delegates every constraint to `CompositeFullAir` and adds
/// cross-chip interaction emissions on top. Use with
/// `p3-batch-stark`'s `prove_batch` / `verify_batch`.
#[derive(Copy, Clone, Debug, Default)]
pub struct CompositeFullAirWithLookups;

impl<F> BaseAir<F> for CompositeFullAirWithLookups {
    fn width(&self) -> usize {
        TOTAL_TRACE_WIDTH
    }

    fn num_public_values(&self) -> usize {
        crate::composite_public::NUM_PUBLIC_VALUES
    }
}

impl<AB> Air<AB> for CompositeFullAirWithLookups
where
    AB: AirBuilder + InteractionBuilder,
{
    fn eval(&self, builder: &mut AB) {
        // (1) Delegate all chip-level constraints to CompositeFullAir.
        <CompositeFullAir as Air<AB>>::eval(&CompositeFullAir, builder);

        // (2) Emit each LogUp bus's table + query interactions via a
        // per-bus helper. Each helper is self-contained and named
        // for the bus it serves; adding or removing a bus changes
        // exactly one call site here. The helpers are in
        // `bus_emit::*` below.
        bus_emit::urange8::<AB>(builder);
        bus_emit::urange13::<AB>(builder);
        bus_emit::irange7p1::<AB>(builder);
        bus_emit::irange8::<AB>(builder);
        bus_emit::i8u8::<AB>(builder);
        bus_emit::noised_packed::<AB>(builder);
        bus_emit::cv_routing::<AB>(builder);
    }
}

/// **Route A**: canonical-program pin **and** the
/// `noised_packed` LogUp, unified in one batch-stark AIR.
///
/// `CompositeFullAirWithLookups` enforces the bus interactions
/// but is *unpinned* (a malicious prover can pick the program);
/// `CompositeFullAirPinned` pins the program but is uni-stark
/// (no LogUp). This composes both: it delegates to
/// [`CompositeFullAirPinned`] (= `CompositeFullAir` constraints +
/// preprocessed program-pin + the jackpot keystone) and
/// then emits every LogUp bus. Proven via `p3-batch-stark`
/// (`prove_batch`), whose multi-phase prover supports a
/// preprocessed/verifier-fixed trace *and* the permutation
/// argument simultaneously. The `noised_packed` bus binds the
/// matmul `A_NOISED`/`B_NOISED` reads to the canonical
/// `NOISED_PACKED` store (C3-tied to the program-pinned
/// `HASH_A`/`HASH_B`) without widening the preprocessed trace.
///
/// Preprocessed exposure rides the standard
/// `BaseAir::preprocessed_trace()` API that batch-stark's
/// `ProverData::from_instances` reads automatically (same
/// mechanism the uni-stark `CompositeFullAirPinned` uses).
#[derive(Clone)]
pub struct CompositeFullAirWithLookupsPinned {
    inner: CompositeFullAirPinned,
}

impl CompositeFullAirWithLookupsPinned {
    /// Build from the canonical program matrix (see
    /// `composite_full_air::extract_program`) with the
    /// keystone enabled (production / `num_stripes ≤ 16`).
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

    /// Build with an explicit keystone flag (verifier-set
    /// from trusted params; see [`CompositeFullAirPinned::new_with`]).
    pub fn new_with(program: p3_matrix::dense::RowMajorMatrix<crate::Val>, sx_bound: bool) -> Self {
        Self::try_new_with(program, sx_bound).expect("canonical program shape already validated")
    }

    /// Fallible constructor for verifier-facing code.
    pub fn try_new_with(
        program: p3_matrix::dense::RowMajorMatrix<crate::Val>,
        sx_bound: bool,
    ) -> Result<Self, ProgramShapeError> {
        Ok(Self {
            inner: CompositeFullAirPinned::try_new_with(program, sx_bound)?,
        })
    }
}

impl BaseAir<crate::Val> for CompositeFullAirWithLookupsPinned {
    fn width(&self) -> usize {
        TOTAL_TRACE_WIDTH
    }
    fn num_public_values(&self) -> usize {
        crate::composite_public::NUM_PUBLIC_VALUES
    }
    fn preprocessed_width(&self) -> usize {
        BaseAir::<crate::Val>::preprocessed_width(&self.inner)
    }
    fn preprocessed_trace(&self) -> Option<p3_matrix::dense::RowMajorMatrix<crate::Val>> {
        BaseAir::<crate::Val>::preprocessed_trace(&self.inner)
    }
}

impl<AB> Air<AB> for CompositeFullAirWithLookupsPinned
where
    AB: AirBuilder<F = crate::Val> + InteractionBuilder,
{
    fn eval(&self, builder: &mut AB) {
        // (1) CompositeFullAir constraints + canonical-program pin +
        //     jackpot keystone (one CompositeFullAir::eval inside).
        <CompositeFullAirPinned as Air<AB>>::eval(&self.inner, builder);

        // (2) Cross-chip LogUp interactions — identical to
        //     CompositeFullAirWithLookups, now enforced *under the
        //     pinned program* (Route A's whole point).
        bus_emit::urange8::<AB>(builder);
        bus_emit::urange13::<AB>(builder);
        bus_emit::irange7p1::<AB>(builder);
        bus_emit::irange8::<AB>(builder);
        bus_emit::i8u8::<AB>(builder);
        bus_emit::noised_packed::<AB>(builder);
        bus_emit::cv_routing::<AB>(builder);
    }
}

/// Per-bus LogUp emissions. Factored out of
/// [`CompositeFullAirWithLookups::eval`] so each bus's emission is
/// a single named, testable unit. All functions take the same
/// `&mut AB` and call `builder.push_interaction` on the
/// appropriate bus.
mod bus_emit {
    use super::*;

    /// `urange8` bus — u8 range check `[0, 256)`.
    ///
    /// Table: (URANGE8_TABLE, −URANGE8_FREQ).
    /// Queries: `UINT8_DATA[0]` gated by IS_MSG_MAT. The remaining
    /// bytes' u8 validity is implied by `IRANGE8(MAT_UNPACK)` plus
    /// the `i8u8` conversion table on `IS_MSG_MAT` rows, so the other
    /// 63 standalone u8 queries are redundant.
    pub fn urange8<AB: AirBuilder + InteractionBuilder>(builder: &mut AB) {
        let main = builder.main();
        let cur = main.current_slice();
        builder.push_interaction(
            BUS_URANGE8,
            [<AB::Var as Into<AB::Expr>>::into(cur[URANGE8_TABLE])],
            Count::bounded(-<AB::Var as Into<AB::Expr>>::into(cur[URANGE8_FREQ]), 0),
        );
        builder.push_interaction(
            BUS_URANGE8,
            [<AB::Var as Into<AB::Expr>>::into(cur[UINT8_DATA_START])],
            Count::bounded(<AB::Var as Into<AB::Expr>>::into(cur[IS_MSG_MAT]), 1),
        );
    }

    /// `urange13` bus — u13 range check `[0, 8192)`.
    ///
    /// Table: (URANGE13_TABLE, −URANGE13_FREQ).
    /// Queries: MAT_ID_LIMBS[0..2] + AB_ID_LIMBS[0..4] every row.
    pub fn urange13<AB: AirBuilder + InteractionBuilder>(builder: &mut AB) {
        let main = builder.main();
        let cur = main.current_slice();
        builder.push_interaction(
            BUS_URANGE13,
            [<AB::Var as Into<AB::Expr>>::into(cur[URANGE13_TABLE])],
            Count::bounded(-<AB::Var as Into<AB::Expr>>::into(cur[URANGE13_FREQ]), 0),
        );
        for i in 0..MAT_ID_LIMBS_LEN {
            builder.push_interaction(
                BUS_URANGE13,
                [<AB::Var as Into<AB::Expr>>::into(cur[MAT_ID_LIMBS_START + i])],
                Count::bounded(<AB::Expr as p3_field::PrimeCharacteristicRing>::ONE, 1),
            );
        }
        for i in 0..AB_ID_LIMBS_LEN {
            builder.push_interaction(
                BUS_URANGE13,
                [<AB::Var as Into<AB::Expr>>::into(cur[AB_ID_LIMBS_START + i])],
                Count::bounded(<AB::Expr as p3_field::PrimeCharacteristicRing>::ONE, 1),
            );
        }
    }

    /// `irange7p1` bus — i7+1 range check `[-64, 64]`.
    ///
    /// Table: (IRANGE7P1_TABLE, −IRANGE7P1_FREQ).
    /// Queries: NOISE_UNPACK[0..64] every row (Pearl's signed
    /// noise bytes).
    pub fn irange7p1<AB: AirBuilder + InteractionBuilder>(builder: &mut AB) {
        let main = builder.main();
        let cur = main.current_slice();
        builder.push_interaction(
            BUS_IRANGE7P1,
            [<AB::Var as Into<AB::Expr>>::into(cur[IRANGE7P1_TABLE])],
            Count::bounded(-<AB::Var as Into<AB::Expr>>::into(cur[IRANGE7P1_FREQ]), 0),
        );
        for i in 0..NOISE_UNPACK_WIN {
            builder.push_interaction(
                BUS_IRANGE7P1,
                [<AB::Var as Into<AB::Expr>>::into(cur[NOISE_UNPACK_START + i])],
                Count::bounded(<AB::Expr as p3_field::PrimeCharacteristicRing>::ONE, 1),
            );
        }
        // Pearl parity (audit N2): the PLAIN matrix operand `MAT_UNPACK` is
        // int7 `[-64,64]`, range-checked here alongside `NOISE_UNPACK` — exactly
        // Pearl `pearl_stark.rs` (`MAT_UNPACK_RANGE.chain(NOISE_UNPACK_RANGE)` →
        // IRANGE7P1, "// Signal is in [-64, 64]"). The tighter int7 bound (vs a
        // plain i8 `[-128,127]`) is what keeps the accept-set identical to Pearl.
        // `i8u8` bridges these i8 cells to `UINT8_DATA`.
        for i in 0..MAT_UNPACK_WIN {
            builder.push_interaction(
                BUS_IRANGE7P1,
                [<AB::Var as Into<AB::Expr>>::into(cur[MAT_UNPACK_START + i])],
                Count::bounded(<AB::Expr as p3_field::PrimeCharacteristicRing>::ONE, 1),
            );
        }
    }

    /// `irange8` bus — i8 range check `[-128, 127]`.
    ///
    /// Table: (IRANGE8_TABLE, −IRANGE8_FREQ).
    /// Queries: A_NOISED_UNPACK[0..32] + B_NOISED_UNPACK[0..32] every row (the
    /// NOISED operands, genuinely i8). `MAT_UNPACK` moved to IRANGE7P1 `[-64,64]`
    /// for Pearl parity (audit N2) — matches Pearl's IRANGE8 = A_NOISED ⧺ B_NOISED.
    pub fn irange8<AB: AirBuilder + InteractionBuilder>(builder: &mut AB) {
        let main = builder.main();
        let cur = main.current_slice();
        builder.push_interaction(
            BUS_IRANGE8,
            [<AB::Var as Into<AB::Expr>>::into(cur[IRANGE8_TABLE])],
            Count::bounded(-<AB::Var as Into<AB::Expr>>::into(cur[IRANGE8_FREQ]), 0),
        );
        for i in 0..A_NOISED_UNPACK_LEN {
            builder.push_interaction(
                BUS_IRANGE8,
                [<AB::Var as Into<AB::Expr>>::into(cur[A_NOISED_UNPACK_START + i])],
                Count::bounded(<AB::Expr as p3_field::PrimeCharacteristicRing>::ONE, 1),
            );
        }
        for i in 0..B_NOISED_UNPACK_LEN {
            builder.push_interaction(
                BUS_IRANGE8,
                [<AB::Var as Into<AB::Expr>>::into(cur[B_NOISED_UNPACK_START + i])],
                Count::bounded(<AB::Expr as p3_field::PrimeCharacteristicRing>::ONE, 1),
            );
        }
    }

    /// `i8u8` bus — paired i8 ↔ u8 conversion.
    ///
    /// Table: (I8U8_TABLE, −I8U8_FREQ). Each table row holds
    /// `pack = signed*256 + unsigned` where unsigned =
    /// signed.rem_euclid(256).
    ///
    /// Queries: `pack = MAT_UNPACK[i]*256 + UINT8_DATA[i]` gated
    /// by IS_MSG_MAT (the i8 and u8 views of each matrix byte
    /// must agree).
    pub fn i8u8<AB: AirBuilder + InteractionBuilder>(builder: &mut AB) {
        let main = builder.main();
        let cur = main.current_slice();
        let is_msg_mat: AB::Expr = cur[IS_MSG_MAT].into();
        let two_fifty_six: AB::Expr =
            <AB::F as p3_field::PrimeCharacteristicRing>::from_u64(256).into();
        builder.push_interaction(
            BUS_I8U8,
            [<AB::Var as Into<AB::Expr>>::into(cur[I8U8_TABLE])],
            Count::bounded(-<AB::Var as Into<AB::Expr>>::into(cur[I8U8_FREQ]), 0),
        );
        for i in 0..MAT_UNPACK_WIN.min(UINT8_DATA_WIN) {
            let signed: AB::Expr = cur[MAT_UNPACK_START + i].into();
            let unsigned: AB::Expr = cur[UINT8_DATA_START + i].into();
            let pack = signed * two_fifty_six.clone() + unsigned;
            builder.push_interaction(BUS_I8U8, [pack], Count::bounded(is_msg_mat.clone(), 1));
        }
    }

    /// `noised_packed` bus — RAM lookup tying matmul reads to
    /// the canonical NOISED_PACKED matrix store.
    ///
    /// Table key: (MAT_ID, NOISED_PACKED[2s..2s+2]) per row
    /// sub-slice, with multiplicity MAT_FREQ[s].
    /// Queries:
    /// * matmul-side (gated by IS_RESET_CUMSUM + IS_UPDATE_CUMSUM):
    ///   (A_ID, A_NOISED[0..2]) and (B_ID, B_NOISED[0..2]).
    /// * BLAKE3-side (gated by IS_MSG_MAT): all eight row-local
    ///   sub-slice keys. When the BLAKE3 chip reads matrix
    ///   bytes, the row also serves as a "plain"
    ///   table entry: NOISE_UNPACK = 0 ⇒ NOISED_PACKED = polyval(MAT_UNPACK).
    ///   This binds the bytes BLAKE3 absorbs to the canonical
    ///   matrix store. Self-referential — locally balances at
    ///   MAT_FREQ = 1.
    pub fn noised_packed<AB: AirBuilder + InteractionBuilder>(builder: &mut AB) {
        let main = builder.main();
        let cur = main.current_slice();
        // The producer (table) side
        // publishes **8 sub-slice keys** per row — a co-located
        // strip-opening leaf round-0 row is the `noised_packed`
        // producer for every swept 8-byte sub-slice of its 64-byte
        // block: key s = (MAT_ID, NOISED_PACKED[2s], NOISED_PACKED
        // [2s+1]) × −MAT_FREQ[s], s∈0..8. ZERO-BLAST: on every current trace
        // NOISED_PACKED[2..16]=0 and MAT_FREQ[1..8]=0, so s=0 is
        // byte-identical to the pre-co-location single emission and
        // s=1..7 are (MAT_ID,0,0)×−0 = no-ops ⇒ the bus balance is
        // unchanged. The g=1 co-location landing fills the
        // sub-slices live; `populate_lookup_freq` accounts each
        // sub-slice's `MAT_FREQ[s]`.
        for s in 0..crate::composite_layout::MAT_FREQ_LEN {
            let chunk_id: AB::Expr = <AB::Var as Into<AB::Expr>>::into(cur[MAT_ID])
                + <AB::F as p3_field::PrimeCharacteristicRing>::from_u64(s as u64);
            let table_mult: AB::Expr = if s == 0 {
                <AB::Var as Into<AB::Expr>>::into(cur[MAT_FREQ])
            } else {
                <AB::Var as Into<AB::Expr>>::into(cur[MAT_FREQ + s])
                    * <AB::Var as Into<AB::Expr>>::into(cur[IS_MSG_MAT])
            };
            builder.push_interaction(
                BUS_NOISED_PACKED,
                [
                    chunk_id,
                    <AB::Var as Into<AB::Expr>>::into(cur[NOISED_PACKED_START + 2 * s]),
                    <AB::Var as Into<AB::Expr>>::into(cur[NOISED_PACKED_START + 2 * s + 1]),
                ],
                Count::bounded(-table_mult, 0),
            );
        }

        // Bind the WHOLE micro-tile A/B input
        // (all `A_NOISED_LEN` packed cells = 32 i8), not just the
        // first 2-cell chunk. With the pack-link
        // (`A_NOISED[c] == polyval(A_NOISED_UNPACK)`) each 2-cell
        // chunk is provably the dot inputs; emit one query per
        // 2-cell chunk so every consumed chunk must match the exact
        // verifier-fixed chunk position ID and packed value in the
        // canonical producer store. The producer key is
        // `MAT_ID + sub_slice`; matmul queries use the corresponding
        // per-sub-slice `A_ID[j]` / `B_ID[j]`. A value from the wrong
        // position, or a zero substituted against padding, leaves no
        // matching table key ⇒ LogUp rejects.
        let matmul_active: AB::Expr = <AB::Var as Into<AB::Expr>>::into(cur[IS_RESET_CUMSUM])
            + <AB::Var as Into<AB::Expr>>::into(cur[IS_UPDATE_CUMSUM]);
        for j in 0..A_ID_LEN {
            builder.push_interaction(
                BUS_NOISED_PACKED,
                [
                    <AB::Var as Into<AB::Expr>>::into(cur[A_ID + j]),
                    <AB::Var as Into<AB::Expr>>::into(cur[A_NOISED_START + 2 * j]),
                    <AB::Var as Into<AB::Expr>>::into(cur[A_NOISED_START + 2 * j + 1]),
                ],
                Count::bounded(matmul_active.clone(), 1),
            );
        }
        for j in 0..B_ID_LEN {
            builder.push_interaction(
                BUS_NOISED_PACKED,
                [
                    <AB::Var as Into<AB::Expr>>::into(cur[B_ID + j]),
                    <AB::Var as Into<AB::Expr>>::into(cur[B_NOISED_START + 2 * j]),
                    <AB::Var as Into<AB::Expr>>::into(cur[B_NOISED_START + 2 * j + 1]),
                ],
                Count::bounded(matmul_active.clone(), 1),
            );
        }

        // BLAKE3-side self-queries. Gated by
        // IS_MSG_MAT. The row's own eight sub-slice keys are
        // self-referential. Combined with the input-chip packing and
        // full-width i8/u8 conversion, this binds every 8-byte
        // sub-slice BLAKE3 absorbs to the canonical matrix store.
        let blake_msg_mat: AB::Expr = cur[IS_MSG_MAT].into();
        for s in 0..crate::composite_layout::MAT_FREQ_LEN {
            let chunk_id: AB::Expr = <AB::Var as Into<AB::Expr>>::into(cur[MAT_ID])
                + <AB::F as p3_field::PrimeCharacteristicRing>::from_u64(s as u64);
            builder.push_interaction(
                BUS_NOISED_PACKED,
                [
                    chunk_id,
                    <AB::Var as Into<AB::Expr>>::into(cur[NOISED_PACKED_START + 2 * s]),
                    <AB::Var as Into<AB::Expr>>::into(cur[NOISED_PACKED_START + 2 * s + 1]),
                ],
                Count::bounded(blake_msg_mat.clone(), 1),
            );
        }
    }

    /// `cv_routing` bus — BLAKE3 chaining-value routing across
    /// non-adjacent rows.
    ///
    /// Table key: (STARK_ROW_IDX, CV_OUT[0..8]), multiplicity
    /// CV_OUT_FREQ. Every row publishes its CV_OUT.
    /// Queries: (CV_OR_TWEAK_PREP, CV_IN[0..8]) gated by
    /// IS_CV_IN. When IS_CV_IN = 1, CV_OR_TWEAK_PREP holds the
    /// referenced row's STARK_ROW_IDX (the column's dual use as
    /// a BLAKE3 tweak is gated by IS_NEW_BLAKE instead).
    pub fn cv_routing<AB: AirBuilder + InteractionBuilder>(builder: &mut AB) {
        let main = builder.main();
        let cur = main.current_slice();

        let mut table_key: Vec<AB::Expr> = Vec::with_capacity(1 + CV_OUT_LEN);
        table_key.push(cur[STARK_ROW_IDX].into());
        for i in 0..CV_OUT_LEN {
            table_key.push(cur[CV_OUT_START + i].into());
        }
        builder.push_interaction(
            BUS_CV_ROUTING,
            table_key,
            Count::bounded(-<AB::Var as Into<AB::Expr>>::into(cur[CV_OUT_FREQ]), 0),
        );

        let mut query_key: Vec<AB::Expr> = Vec::with_capacity(1 + CV_IN_LEN);
        query_key.push(cur[CV_OR_TWEAK_PREP].into());
        for i in 0..CV_IN_LEN {
            query_key.push(cur[CV_IN_START + i].into());
        }
        builder.push_interaction(
            BUS_CV_ROUTING,
            query_key,
            Count::bounded(<AB::Var as Into<AB::Expr>>::into(cur[IS_CV_IN]), 1),
        );
    }
}

#[cfg(test)]
mod tests {
    //! End-to-end LogUp tests on the full composite trace.
    //! `prove_batch` enforces the `urange8` bus balance at proof
    //! time. Valid traces verify; over-claimed `URANGE8_FREQ` or
    //! out-of-range anchored `UINT8_DATA[0]` queries are rejected.

    use p3_batch_stark::{prove_batch, verify_batch, ProverData, StarkInstance};
    use p3_field::integers::QuotientMap;

    use super::*;
    use crate::chips::control::ControlChip;
    use crate::circuit::{build_stark_config, AiPowStarkConfig, CircuitConfig};
    use crate::composite_layout::{
        MAT_UNPACK_START, NOISED_PACKED_START, NOISE_PACKED_PREP, NOISE_UNPACK_START,
        UINT8_DATA_START, URANGE8_FREQ,
    };
    use crate::composite_trace::CompositeTrace;
    use crate::params::ZkParams;
    use crate::Val;

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

    fn run_batch(
        config: &AiPowStarkConfig,
        trace: &p3_matrix::dense::RowMajorMatrix<Val>,
    ) -> Result<(), String> {
        // Derive PIs via the centralized helper — picks up CUMSUM_TILE
        // and JACKPOT_MSG from the last row plus HASH_A / HASH_B from
        // their selector rows (zero for baseline traces).
        let pis: Vec<Val> =
            crate::composite_public::CompositePublicInputs::derive_from_matrix(trace).to_vec();

        let air = CompositeFullAirWithLookups;
        let instances = vec![StarkInstance {
            air: &air,
            trace,
            public_values: pis.clone(),
        }];
        let prover_data = ProverData::from_instances(config, &instances);
        let proof = prove_batch(config, &instances, &prover_data);
        verify_batch(config, &[air], &proof, &[pis], &prover_data.common)
            .map_err(|e| format!("{:?}", e))
    }

    /// Baseline trace + populate_lookup_freq: all-zero query
    /// cells, populate_lookup_freq accumulates the unconditional
    /// queries (MAT_ID_LIMBS, AB_ID_LIMBS, NOISE_UNPACK,
    /// A/B_NOISED_UNPACK, MAT_UNPACK — all 0 on baseline) into
    /// the corresponding FREQ[0] cells. The lookup balance holds
    /// because every query is at value 0 and the table provides
    /// matching multiplicity.
    #[test]
    fn baseline_balances_via_logup_after_populate() {
        let cfg = build_stark_config(&test_zk_params(), &CircuitConfig::TEST_PEARL);
        let mut trace = CompositeTrace::baseline_min();
        trace.populate_lookup_freq();
        run_batch(&cfg, &trace.matrix).expect("baseline must verify with LogUp");
    }

    /// Place a single "matrix message byte" at row 0: set
    /// MAT_UNPACK[0] / UINT8_DATA[0] to a consistent (i8, u8)
    /// pair, IS_MSG_MAT = 1, NOISED_PACKED[0] consistent with the
    /// input chip's `polyval(MAT, 256) + polyval(NOISE, 256)`
    /// constraint.
    ///
    /// The argument `u8_value` is the unsigned byte. The
    /// corresponding signed i8 view is `u8_value` if `< 128`
    /// else `u8_value - 256` (two's complement). With this
    /// helper, all wired lookup buses balance for the placed row:
    ///   - URange8 query on UINT8_DATA[0] = u8_value
    ///   - IRange8 query on MAT_UNPACK[0] = i8_view
    ///   - I8U8 query on the packed pair (matches valid table entry)
    fn place_urange8_query(trace: &mut CompositeTrace, row_idx: usize, u8_value: u32) {
        assert!(row_idx < trace.height());
        assert!(u8_value < 256);
        let base = row_idx * TOTAL_TRACE_WIDTH;
        let row = &mut trace.matrix.values[base..base + TOTAL_TRACE_WIDTH];

        let mut selectors = [false; 21];
        selectors[10] = true; // IS_MSG_MAT
        ControlChip.fill_row(&selectors, 0, row);

        // i8 view of u8: 0..127 → 0..127; 128..255 → -128..-1.
        let signed: i64 = if u8_value < 128 {
            u8_value as i64
        } else {
            u8_value as i64 - 256
        };

        row[crate::composite_layout::MAT_UNPACK_START] =
            <Val as QuotientMap<i64>>::from_int(signed);
        row[UINT8_DATA_START] = <Val as QuotientMap<u64>>::from_int(u8_value as u64);
        // Input chip: NOISED_PACKED[0] = polyval(MAT_UNPACK[0..4],
        // 256) = MAT_UNPACK[0] (since [1..4] are 0). NOISE_UNPACK
        // is 0 so polyval(NOISE, 256) = 0.
        row[crate::composite_layout::NOISED_PACKED_START] =
            <Val as QuotientMap<i64>>::from_int(signed);
    }

    fn pack4_signed(bytes: &[i8]) -> i64 {
        assert_eq!(bytes.len(), 4);
        bytes
            .iter()
            .enumerate()
            .map(|(j, &b)| (b as i64) * 256i64.pow(j as u32))
            .sum()
    }

    fn place_full_msg_mat_row(trace: &mut CompositeTrace, row_idx: usize, plain: &[i8; 64]) {
        assert!(row_idx < trace.height());
        let base = row_idx * TOTAL_TRACE_WIDTH;
        let row = &mut trace.matrix.values[base..base + TOTAL_TRACE_WIDTH];

        let mut selectors = [false; 21];
        selectors[10] = true; // IS_MSG_MAT
        ControlChip.fill_row(&selectors, 0, row);

        for i in 0..64 {
            row[MAT_UNPACK_START + i] = <Val as QuotientMap<i64>>::from_int(plain[i] as i64);
            row[UINT8_DATA_START + i] =
                <Val as QuotientMap<u64>>::from_int((plain[i] as u8) as u64);
            row[NOISE_UNPACK_START + i] = Val::default();
        }
        for s in 0..8 {
            row[NOISE_PACKED_PREP + s] = Val::default();
        }
        for cell in 0..16 {
            row[NOISED_PACKED_START + cell] =
                <Val as QuotientMap<i64>>::from_int(pack4_signed(&plain[cell * 4..cell * 4 + 4]));
        }
    }

    #[test]
    fn single_u8_query_balances_via_logup() {
        let cfg = build_stark_config(&test_zk_params(), &CircuitConfig::TEST_PEARL);
        let mut trace = CompositeTrace::baseline_min();
        place_urange8_query(&mut trace, 0, 42);
        trace.populate_lookup_freq();
        run_batch(&cfg, &trace.matrix).expect("single in-range query must verify");
    }

    /// Tamper UINT8_DATA[0] on row 0 to 300 (out of u8 range)
    /// without updating any other cell. With IS_MSG_MAT = 0 on
    /// row 0, the query multiplicity is 0, so the lookup
    /// constraint is silenced — this test verifies the gating
    /// works correctly (no rejection just because we wrote junk
    /// in a u8 cell).
    #[test]
    fn out_of_range_uint8_silenced_when_is_msg_mat_zero() {
        let cfg = build_stark_config(&test_zk_params(), &CircuitConfig::TEST_PEARL);
        let mut trace = CompositeTrace::baseline_min();
        let target = 0 * TOTAL_TRACE_WIDTH + UINT8_DATA_START;
        trace.matrix.values[target] = <Val as QuotientMap<u64>>::from_int(300);
        trace.populate_lookup_freq();
        run_batch(&cfg, &trace.matrix)
            .expect("out-of-range u8 with IS_MSG_MAT=0 must still verify");
    }

    /// Place a query at value 300 (out of u8 range). The trace
    /// has no URANGE8_TABLE entry for value 300, so the LogUp
    /// argument can't balance the bus → reject.
    #[test]
    fn out_of_range_uint8_with_active_query_rejected_by_logup() {
        let cfg = build_stark_config(&test_zk_params(), &CircuitConfig::TEST_PEARL);
        let mut trace = CompositeTrace::baseline_min();

        // Set up the query: IS_MSG_MAT = 1, UINT8_DATA[0] = 300.
        let base = 0 * TOTAL_TRACE_WIDTH;
        let row = &mut trace.matrix.values[base..base + TOTAL_TRACE_WIDTH];
        let mut selectors = [false; 21];
        selectors[10] = true;
        ControlChip.fill_row(&selectors, 0, row);
        row[UINT8_DATA_START] = <Val as QuotientMap<u64>>::from_int(300);
        trace.populate_lookup_freq();

        let res = run_batch(&cfg, &trace.matrix);
        assert!(
            res.is_err(),
            "out-of-range query must reject via LogUp; got {:?}",
            res
        );
    }

    /// Tamper URANGE8_FREQ AFTER populate_lookup_freq to claim a
    /// query was made when none was. The table over-provides →
    /// unbalanced → reject.
    #[test]
    fn over_claimed_urange8_freq_rejected_by_logup() {
        let cfg = build_stark_config(&test_zk_params(), &CircuitConfig::TEST_PEARL);
        let mut trace = CompositeTrace::baseline_min();
        trace.populate_lookup_freq();
        let target = 42 * TOTAL_TRACE_WIDTH + URANGE8_FREQ;
        trace.matrix.values[target] = <Val as QuotientMap<u64>>::from_int(1);
        let res = run_batch(&cfg, &trace.matrix);
        assert!(
            res.is_err(),
            "over-claimed URANGE8_FREQ must reject; got {:?}",
            res
        );
    }

    /// Place a query but don't call populate_lookup_freq. The
    /// FREQ column is stale → unbalanced → reject.
    #[test]
    fn under_claimed_urange8_freq_rejected_by_logup() {
        let cfg = build_stark_config(&test_zk_params(), &CircuitConfig::TEST_PEARL);
        let mut trace = CompositeTrace::baseline_min();
        place_urange8_query(&mut trace, 0, 42);
        // Intentionally NOT calling populate_lookup_freq.

        let res = run_batch(&cfg, &trace.matrix);
        assert!(
            res.is_err(),
            "missing URANGE8_FREQ must reject; got {:?}",
            res
        );
    }

    /// Multiple queries at the same u8 value: FREQ correctly
    /// counts the multiplicity.
    #[test]
    fn multi_query_same_value_balances() {
        let cfg = build_stark_config(&test_zk_params(), &CircuitConfig::TEST_PEARL);
        let mut trace = CompositeTrace::baseline_min();
        for r in 0..5 {
            place_urange8_query(&mut trace, r, 42);
        }
        trace.populate_lookup_freq();
        run_batch(&cfg, &trace.matrix).expect("5 queries at value 42 must verify");
    }

    /// Several queries at different values: each value's FREQ
    /// reflects its own count.
    #[test]
    fn queries_at_different_values_balance() {
        let cfg = build_stark_config(&test_zk_params(), &CircuitConfig::TEST_PEARL);
        let mut trace = CompositeTrace::baseline_min();
        place_urange8_query(&mut trace, 0, 1);
        place_urange8_query(&mut trace, 1, 42);
        place_urange8_query(&mut trace, 2, 200);
        trace.populate_lookup_freq();
        run_batch(&cfg, &trace.matrix).expect("3 distinct queries must verify");
    }

    // =================================================================
    //  URange13 / IRange7P1 / IRange8 unconditional-query buses
    // =================================================================

    /// Tamper a MAT_ID_LIMBS cell to an out-of-u13 value (9000 ∉
    /// [0, 8192)). populate_lookup_freq won't increment any FREQ
    /// for 9000 (out of range), so the query side has +1 with no
    /// matching table entry → unbalanced.
    #[test]
    fn out_of_range_mat_id_limb_rejected_by_logup() {
        let cfg = build_stark_config(&test_zk_params(), &CircuitConfig::TEST_PEARL);
        let mut trace = CompositeTrace::baseline_min();
        let target = 0 * TOTAL_TRACE_WIDTH + crate::composite_layout::MAT_ID_LIMBS_START;
        trace.matrix.values[target] = <Val as QuotientMap<u64>>::from_int(9000);
        trace.populate_lookup_freq();
        let res = run_batch(&cfg, &trace.matrix);
        assert!(
            res.is_err(),
            "MAT_ID_LIMBS out of u13 range must reject; got {:?}",
            res
        );
    }

    /// Tamper a NOISE_UNPACK cell to an out-of-i7+1 value (100 ∉
    /// [-64, 64]).
    #[test]
    fn out_of_range_noise_unpack_rejected_by_logup() {
        let cfg = build_stark_config(&test_zk_params(), &CircuitConfig::TEST_PEARL);
        let mut trace = CompositeTrace::baseline_min();
        let target = 0 * TOTAL_TRACE_WIDTH + crate::composite_layout::NOISE_UNPACK_START;
        trace.matrix.values[target] = <Val as QuotientMap<u64>>::from_int(100);
        trace.populate_lookup_freq();
        let res = run_batch(&cfg, &trace.matrix);
        assert!(
            res.is_err(),
            "NOISE_UNPACK out of [-64, 64] must reject; got {:?}",
            res
        );
    }

    /// Tamper an A_NOISED_UNPACK cell to an out-of-i8 value (200
    /// ∉ [-128, 127]).
    #[test]
    fn out_of_range_a_noised_unpack_rejected_by_logup() {
        let cfg = build_stark_config(&test_zk_params(), &CircuitConfig::TEST_PEARL);
        let mut trace = CompositeTrace::baseline_min();
        let target = 0 * TOTAL_TRACE_WIDTH + crate::composite_layout::A_NOISED_UNPACK_START;
        trace.matrix.values[target] = <Val as QuotientMap<u64>>::from_int(200);
        trace.populate_lookup_freq();
        let res = run_batch(&cfg, &trace.matrix);
        assert!(
            res.is_err(),
            "A_NOISED_UNPACK out of i8 range must reject; got {:?}",
            res
        );
    }

    /// Tamper a B_NOISED_UNPACK cell to a NEGATIVE out-of-i8
    /// value (-200, encoded as Goldilocks_p − 200, which falls
    /// outside [-128, 127]).
    #[test]
    fn out_of_range_b_noised_unpack_negative_rejected_by_logup() {
        let cfg = build_stark_config(&test_zk_params(), &CircuitConfig::TEST_PEARL);
        let mut trace = CompositeTrace::baseline_min();
        let target = 0 * TOTAL_TRACE_WIDTH + crate::composite_layout::B_NOISED_UNPACK_START;
        trace.matrix.values[target] = <Val as QuotientMap<i64>>::from_int(-200);
        trace.populate_lookup_freq();
        let res = run_batch(&cfg, &trace.matrix);
        assert!(
            res.is_err(),
            "negative out-of-range B_NOISED_UNPACK must reject; got {:?}",
            res
        );
    }

    /// Tamper a MAT_UNPACK cell to an out-of-i8 value.
    #[test]
    fn out_of_range_mat_unpack_rejected_by_logup() {
        let cfg = build_stark_config(&test_zk_params(), &CircuitConfig::TEST_PEARL);
        let mut trace = CompositeTrace::baseline_min();
        let target = 0 * TOTAL_TRACE_WIDTH + crate::composite_layout::MAT_UNPACK_START;
        trace.matrix.values[target] = <Val as QuotientMap<u64>>::from_int(150);
        trace.populate_lookup_freq();
        let res = run_batch(&cfg, &trace.matrix);
        assert!(
            res.is_err(),
            "MAT_UNPACK out of i8 range must reject; got {:?}",
            res
        );
    }

    // Note: a positive-direction in_range_noise_unpack_balances
    // would also have to update NOISE_PACKED_PREP to satisfy the
    // input chip's `polyval(NOISE_UNPACK, base=129) ==
    // NOISE_PACKED_PREP` constraint. That conflates the LogUp
    // test with a per-chip constraint. The out-of-range
    // rejection test above is sufficient (both constraints fail
    // in the rejection direction).

    /// In-range A_NOISED_UNPACK with a mix of positive and
    /// negative i8 values balances.
    #[test]
    fn in_range_a_noised_unpack_balances() {
        let cfg = build_stark_config(&test_zk_params(), &CircuitConfig::TEST_PEARL);
        let mut trace = CompositeTrace::baseline_min();
        let row_start = 0 * TOTAL_TRACE_WIDTH;
        let test_vals: [(usize, i64); 4] = [(0, -128), (1, 127), (2, 0), (3, -1)];
        for (off, v) in test_vals {
            trace.matrix.values[row_start + crate::composite_layout::A_NOISED_UNPACK_START + off] =
                <Val as QuotientMap<i64>>::from_int(v);
        }
        trace.populate_lookup_freq();
        run_batch(&cfg, &trace.matrix).expect("in-range i8 A_NOISED_UNPACK values must verify");
    }

    // =================================================================
    //  I8U8 paired i8↔u8 conversion bus
    // =================================================================

    /// Place a valid (i8, u8) pair on row 0 with IS_MSG_MAT=1
    /// using place_urange8_query, which sets MAT_UNPACK[0],
    /// UINT8_DATA[0], and NOISED_PACKED[0] consistently. The
    /// I8U8 bus's table row for pack = 42*256+42 is at row
    /// (42 + 128) = 170. populate_lookup_freq increments
    /// I8U8_FREQ[170] to balance.
    #[test]
    fn valid_i8u8_pair_balances_via_logup() {
        let cfg = build_stark_config(&test_zk_params(), &CircuitConfig::TEST_PEARL);
        let mut trace = CompositeTrace::baseline_min();
        place_urange8_query(&mut trace, 0, 42);
        trace.populate_lookup_freq();
        run_batch(&cfg, &trace.matrix).expect("valid (42, 42) pair must verify");
    }

    /// Place a valid NEGATIVE (i8, u8) pair: u8 = 255 → signed = -1.
    /// I8U8 table row = -1 + 128 = 127. populate_lookup_freq
    /// updates I8U8_FREQ[127] to balance.
    #[test]
    fn valid_negative_i8u8_pair_balances() {
        let cfg = build_stark_config(&test_zk_params(), &CircuitConfig::TEST_PEARL);
        let mut trace = CompositeTrace::baseline_min();
        place_urange8_query(&mut trace, 0, 255);
        trace.populate_lookup_freq();
        run_batch(&cfg, &trace.matrix).expect("valid (-1, 255) pair must verify");
    }

    /// **Cryptographically critical test.** Place a valid (42, 42)
    /// pair via place_urange8_query (which sets NOISED_PACKED
    /// consistently), then tamper UINT8_DATA[0] to 43. The packed
    /// value 42*256+43 = 10795 is not in the I8U8 table (which
    /// only contains valid signed/rem_euclid pairs), so LogUp
    /// rejects. This is the constraint that ensures matrix bytes
    /// can't be inconsistently presented as i8 vs. u8 — essential
    /// for the merge-mining byte-equivalence with Pearl.
    #[test]
    fn inconsistent_i8u8_pair_rejected_by_logup() {
        let cfg = build_stark_config(&test_zk_params(), &CircuitConfig::TEST_PEARL);
        let mut trace = CompositeTrace::baseline_min();
        place_urange8_query(&mut trace, 0, 42);
        // Tamper UINT8_DATA[0] from 42 to 43 (inconsistent with
        // MAT_UNPACK[0] which is still 42).
        let target = 0 * TOTAL_TRACE_WIDTH + UINT8_DATA_START;
        trace.matrix.values[target] = <Val as QuotientMap<u64>>::from_int(43);
        trace.populate_lookup_freq();
        let res = run_batch(&cfg, &trace.matrix);
        assert!(
            res.is_err(),
            "inconsistent (i8, u8) pair (42, 43) must reject; got {:?}",
            res
        );
    }

    fn assert_split_view_rejects_at_byte(cfg: &AiPowStarkConfig, byte_idx: usize) {
        assert!(byte_idx < 64);
        let mut trace = CompositeTrace::baseline_min();
        let mut plain = [0i8; 64];
        plain[byte_idx] = 5;
        place_full_msg_mat_row(&mut trace, 0, &plain);

        let base = 0 * TOTAL_TRACE_WIDTH;
        trace.matrix.values[base + MAT_UNPACK_START + byte_idx] =
            <Val as QuotientMap<i64>>::from_int(6);
        let cell = byte_idx / 4;
        let cell_base = cell * 4;
        let mut tampered = [0i8; 4];
        tampered.copy_from_slice(&plain[cell_base..cell_base + 4]);
        tampered[byte_idx - cell_base] = 6;
        trace.matrix.values[base + NOISED_PACKED_START + cell] =
            <Val as QuotientMap<i64>>::from_int(pack4_signed(&tampered));

        trace.populate_lookup_freq();
        let res = run_batch(cfg, &trace.matrix);
        assert!(
            res.is_err(),
            "MAT_UNPACK/UINT8_DATA split view at byte {byte_idx} must reject; got {:?}",
            res
        );
    }

    /// Regression for the split-view gap. The attack leaves the
    /// unsigned BLAKE3-facing view unchanged, mutates the signed
    /// matmul-facing view, and recomputes the matching `NOISED_PACKED`
    /// cell so the input chip still accepts. Full 64-byte i8/u8 lookup
    /// activation must reject it in every later sub-slice, not only at
    /// byte 8.
    #[test]
    fn non_first_subslice_split_view_rejected_by_i8u8_logup() {
        let cfg = build_stark_config(&test_zk_params(), &CircuitConfig::TEST_PEARL);
        for byte_idx in [8usize, 15, 16, 31, 32, 47, 48, 63] {
            assert_split_view_rejects_at_byte(&cfg, byte_idx);
        }
    }

    /// Cheap full-position coverage for the frequency side of the
    /// split-view fix. A single matrix-message row must populate
    /// one I8U8 multiplicity for each of its 64 paired
    /// MAT_UNPACK/UINT8_DATA cells.
    #[test]
    fn full_msg_mat_row_populates_i8u8_freq_for_all_64_bytes() {
        use p3_field::PrimeField64;

        let mut trace = CompositeTrace::baseline_min();
        let plain = core::array::from_fn(|i| i as i8 - 32);
        place_full_msg_mat_row(&mut trace, 0, &plain);
        trace.populate_lookup_freq();

        let freq_sum: u64 = (-32i64..32)
            .map(|signed| {
                let row = (signed + 128) as usize;
                trace.matrix.values[row * TOTAL_TRACE_WIDTH + crate::composite_layout::I8U8_FREQ]
                    .as_canonical_u64()
            })
            .sum();
        assert_eq!(freq_sum, 64, "one IS_MSG_MAT row must emit 64 I8U8 pairs");
        for signed in -32i64..32 {
            let row = (signed + 128) as usize;
            assert_eq!(
                trace.matrix.values[row * TOTAL_TRACE_WIDTH + crate::composite_layout::I8U8_FREQ]
                    .as_canonical_u64(),
                1,
                "signed byte {signed} should have exactly one I8U8 query"
            );
        }
    }

    /// Each BLAKE3 matrix-message row self-queries all eight
    /// `noised_packed` sub-slices. If one later MAT_FREQ slot is
    /// dropped after frequency population, the table side no longer
    /// balances that sub-slice's self-query and the proof must reject.
    #[test]
    fn non_first_subslice_mat_freq_drop_rejected_by_logup() {
        let cfg = build_stark_config(&test_zk_params(), &CircuitConfig::TEST_PEARL);
        let mut trace = CompositeTrace::baseline_min();
        let mut plain = [0i8; 64];
        plain[63] = -7;
        place_full_msg_mat_row(&mut trace, 0, &plain);
        trace.populate_lookup_freq();

        let target = MAT_FREQ + 7;
        trace.matrix.values[target] = Val::default();
        let res = run_batch(&cfg, &trace.matrix);
        assert!(
            res.is_err(),
            "dropping a later noised_packed self-query MAT_FREQ must reject; got {:?}",
            res
        );
    }

    /// Cheap full-sub-slice coverage for the producer side of the
    /// `noised_packed` fix. A co-located BLAKE3 matrix-message
    /// row must publish and self-query all eight sub-slices, not only
    /// the legacy first one.
    #[test]
    fn full_msg_mat_row_populates_mat_freq_for_all_8_subslices() {
        use p3_field::PrimeField64;

        let mut trace = CompositeTrace::baseline_min();
        let plain = core::array::from_fn(|i| i as i8 + 1);
        place_full_msg_mat_row(&mut trace, 0, &plain);
        trace.populate_lookup_freq();

        for s in 0..crate::composite_layout::MAT_FREQ_LEN {
            assert_eq!(
                trace.matrix.values[MAT_FREQ + s].as_canonical_u64(),
                1,
                "IS_MSG_MAT row must self-query noised_packed sub-slice {s}"
            );
        }
    }

    /// Tamper I8U8_FREQ AFTER populate → reject.
    #[test]
    fn tampered_i8u8_freq_rejected_by_logup() {
        let cfg = build_stark_config(&test_zk_params(), &CircuitConfig::TEST_PEARL);
        let mut trace = CompositeTrace::baseline_min();
        trace.populate_lookup_freq();
        // Inflate I8U8_FREQ at row 128 (the table entry for pair
        // (0, 0)) from 0 to 1.
        let target = 128 * TOTAL_TRACE_WIDTH + crate::composite_layout::I8U8_FREQ;
        trace.matrix.values[target] = <Val as QuotientMap<u64>>::from_int(1);
        let res = run_batch(&cfg, &trace.matrix);
        assert!(
            res.is_err(),
            "over-claimed I8U8_FREQ must reject; got {:?}",
            res
        );
    }

    // =================================================================
    //  NOISED_PACKED RAM-lookup bus
    // =================================================================

    /// Baseline trace + matmul activity in a chain. The matmul
    /// rows query (A_ID=0, A_NOISED[0..2]=0) and (B_ID=0,
    /// B_NOISED[0..2]=0) — values that match the all-zero baseline
    /// table entry on row 0. populate_lookup_freq updates
    /// MAT_FREQ to balance.
    #[test]
    fn matmul_chain_with_zero_reads_balances_noised_packed() {
        use crate::composite_layout::TILE_D;

        let cfg = build_stark_config(&test_zk_params(), &CircuitConfig::TEST_PEARL);
        let mut trace = CompositeTrace::baseline_min();

        // Build a 2-step matmul chain at rows 8, 9. The chips'
        // A_NOISED / B_NOISED columns stay 0 (we only set
        // A_NOISED_UNPACK / B_NOISED_UNPACK).
        let a = [[0i8; TILE_D]; crate::composite_layout::TILE_H];
        let b = [[0i8; TILE_D]; crate::composite_layout::TILE_H];
        let cumsum_zero = [0i32; crate::chips::matmul::compute::CUMSUM_LEN];
        let after_reset = trace.place_matmul_step(8, &a, &b, true, false, &cumsum_zero);
        let after_update = trace.place_matmul_step(9, &a, &b, false, true, &after_reset);
        trace.fill_cumsum_passthrough(10, &after_update);

        trace.populate_lookup_freq();
        run_batch(&cfg, &trace.matrix)
            .expect("matmul chain with zero NOISED_PACKED reads must verify");
    }

    /// Tamper A_NOISED so the matmul row queries a triple that
    /// doesn't appear in the table → LogUp rejects. Since
    /// A_NOISED isn't currently constrained against A_NOISED_UNPACK
    /// at the AIR level, the only thing keeping A_NOISED honest
    /// is the LogUp against NOISED_PACKED. Without that, a prover
    /// could feed arbitrary data into the matmul accumulator.
    #[test]
    fn tampered_a_noised_with_no_matching_table_entry_rejects() {
        use crate::composite_layout::TILE_D;
        let cfg = build_stark_config(&test_zk_params(), &CircuitConfig::TEST_PEARL);
        let mut trace = CompositeTrace::baseline_min();

        let a = [[0i8; TILE_D]; crate::composite_layout::TILE_H];
        let b = [[0i8; TILE_D]; crate::composite_layout::TILE_H];
        let cumsum_zero = [0i32; crate::chips::matmul::compute::CUMSUM_LEN];
        let _ = trace.place_matmul_step(8, &a, &b, true, false, &cumsum_zero);
        trace.fill_cumsum_passthrough(9, &cumsum_zero);

        // Tamper A_NOISED[0] on row 8 to a value that's not in
        // any table row (all table rows have NOISED_PACKED = 0).
        let target = 8 * TOTAL_TRACE_WIDTH + A_NOISED_START;
        trace.matrix.values[target] = <Val as QuotientMap<u64>>::from_int(0xDEAD_BEEF);

        trace.populate_lookup_freq();
        let res = run_batch(&cfg, &trace.matrix);
        assert!(
            res.is_err(),
            "tampered A_NOISED must reject via NOISED_PACKED bus; got {:?}",
            res
        );
    }

    #[test]
    fn positioned_lookup_accepts_exact_chunk_id_and_value() {
        use crate::composite_layout::TILE_D;

        let cfg = build_stark_config(&test_zk_params(), &CircuitConfig::TEST_PEARL);
        let mut trace = CompositeTrace::baseline_min();
        let real_id = crate::composite_trace::NOISED_CHUNK_ID_BASE;
        let committed = [3i8, 4, 5, 6, 7, 8, 9, 10];
        trace.place_noised_store_row(32, &committed, real_id as u32);

        let mut a = [[0i8; TILE_D]; crate::composite_layout::TILE_H];
        a[0][..8].copy_from_slice(&committed);
        let b = [[0i8; TILE_D]; crate::composite_layout::TILE_H];
        let mut a_ids = [0u64; A_ID_LEN];
        a_ids[0] = real_id;
        let b_ids = [0u64; B_ID_LEN];
        let zero = [0i32; crate::chips::matmul::compute::CUMSUM_LEN];
        let _ = trace.place_matmul_step_with_ids(8, &a, &b, &a_ids, &b_ids, true, false, &zero);
        trace.fill_cumsum_passthrough(9, &zero);

        trace.populate_lookup_freq();
        run_batch(&cfg, &trace.matrix)
            .expect("exact chunk ID + value must satisfy positioned noised_packed lookup");
    }

    #[test]
    fn positioned_lookup_rejects_zero_substitution_against_padding() {
        use crate::composite_layout::TILE_D;

        let cfg = build_stark_config(&test_zk_params(), &CircuitConfig::TEST_PEARL);
        let mut trace = CompositeTrace::baseline_min();
        let real_id = crate::composite_trace::NOISED_CHUNK_ID_BASE;
        let committed = [3i8, 4, 5, 6, 7, 8, 9, 10];

        trace.place_noised_store_row(32, &committed, real_id as u32);

        // Malicious sweep: query the exact committed position ID but
        // use a zero chunk. The low padding keys are all-zero, but
        // they are IDs 0..7; they must not satisfy `real_id`.
        let a = [[0i8; TILE_D]; crate::composite_layout::TILE_H];
        let b = [[0i8; TILE_D]; crate::composite_layout::TILE_H];
        let mut a_ids = [0u64; A_ID_LEN];
        a_ids[0] = real_id;
        let b_ids = [0u64; B_ID_LEN];
        let zero = [0i32; crate::chips::matmul::compute::CUMSUM_LEN];
        let _ = trace.place_matmul_step_with_ids(8, &a, &b, &a_ids, &b_ids, true, false, &zero);
        trace.fill_cumsum_passthrough(9, &zero);

        trace.populate_lookup_freq();
        let res = run_batch(&cfg, &trace.matrix);
        assert!(
            res.is_err(),
            "zero substitution at a real chunk ID must reject; got {:?}",
            res
        );
    }

    #[test]
    fn positioned_lookup_rejects_value_from_wrong_position() {
        use crate::composite_layout::TILE_D;

        let cfg = build_stark_config(&test_zk_params(), &CircuitConfig::TEST_PEARL);
        let mut trace = CompositeTrace::baseline_min();
        let id_a = crate::composite_trace::NOISED_CHUNK_ID_BASE;
        let id_b = id_a + 1;
        let chunk_a = [1i8, 2, 3, 4, 5, 6, 7, 8];
        let chunk_b = [11i8, 12, 13, 14, 15, 16, 17, 18];
        trace.place_noised_store_row(32, &chunk_a, id_a as u32);
        trace.place_noised_store_row(33, &chunk_b, id_b as u32);

        // Malicious sweep: consume `chunk_b` while claiming the
        // position ID for `chunk_a`. A value-only lookup accepts this
        // because `chunk_b` exists somewhere; the positioned key must
        // reject it.
        let mut a = [[0i8; TILE_D]; crate::composite_layout::TILE_H];
        a[0][..8].copy_from_slice(&chunk_b);
        let b = [[0i8; TILE_D]; crate::composite_layout::TILE_H];
        let mut a_ids = [0u64; A_ID_LEN];
        a_ids[0] = id_a;
        let b_ids = [0u64; B_ID_LEN];
        let zero = [0i32; crate::chips::matmul::compute::CUMSUM_LEN];
        let _ = trace.place_matmul_step_with_ids(8, &a, &b, &a_ids, &b_ids, true, false, &zero);
        trace.fill_cumsum_passthrough(9, &zero);

        trace.populate_lookup_freq();
        let res = run_batch(&cfg, &trace.matrix);
        assert!(
            res.is_err(),
            "value from the wrong chunk ID must reject; got {:?}",
            res
        );
    }

    /// Tamper MAT_FREQ to over-claim a table entry was consumed
    /// when it wasn't. The table side over-provides → unbalanced.
    #[test]
    fn tampered_mat_freq_rejected_by_logup() {
        let cfg = build_stark_config(&test_zk_params(), &CircuitConfig::TEST_PEARL);
        let mut trace = CompositeTrace::baseline_min();
        trace.populate_lookup_freq();
        // Inflate MAT_FREQ on row 0.
        let target = 0 * TOTAL_TRACE_WIDTH + MAT_FREQ;
        use p3_field::PrimeField64;
        let prev = trace.matrix.values[target].as_canonical_u64();
        trace.matrix.values[target] = <Val as QuotientMap<u64>>::from_int(prev + 1);
        let res = run_batch(&cfg, &trace.matrix);
        assert!(
            res.is_err(),
            "over-claimed MAT_FREQ must reject; got {:?}",
            res
        );
    }

    /// Tamper a NOISED_PACKED cell on row 0 to a non-zero value
    /// while no matmul row reads it. The table provides the
    /// modified entry but no query consumes it → MAT_FREQ would
    /// need to be 0 anyway (populate already sets it). But the
    /// table key changed, so any subsequent matmul query
    /// expecting the all-zero entry would no longer match.
    ///
    /// This test runs only the baseline (no matmul), so changing
    /// NOISED_PACKED doesn't break the bus IF MAT_FREQ stays 0 on
    /// that row. populate_lookup_freq should re-route any baseline
    /// matmul queries to a different table row. Since baseline
    /// has no matmul queries, this test verifies: with NOISED_PACKED
    /// non-zero and MAT_FREQ = 0, the bus still balances.
    #[test]
    fn isolated_noised_packed_change_with_no_queries_verifies() {
        let cfg = build_stark_config(&test_zk_params(), &CircuitConfig::TEST_PEARL);
        let mut trace = CompositeTrace::baseline_min();
        // Tamper NOISED_PACKED[0] on row 0 to 7. Also adjust
        // input chip's constraint: NOISED_PACKED[0] = polyval(MAT,
        // 256) + polyval(NOISE, 256). With MAT_UNPACK[0..4] = 0,
        // polyval = 0; with NOISE_UNPACK[0..4] = 0, polyval = 0.
        // So NOISED_PACKED[0] = 0 is forced. Setting it to 7
        // breaks the input chip constraint independently of the
        // LogUp.
        let target = 0 * TOTAL_TRACE_WIDTH + NOISED_PACKED_START;
        trace.matrix.values[target] = <Val as QuotientMap<u64>>::from_int(7);
        trace.populate_lookup_freq();
        let res = run_batch(&cfg, &trace.matrix);
        assert!(
            res.is_err(),
            "NOISED_PACKED inconsistent with MAT_UNPACK rejected (input chip + LogUp); got {:?}",
            res
        );
    }

    // =================================================================
    //  CV_ROUTING bus (BLAKE3 chaining-value routing)
    // =================================================================

    /// Baseline trace already balances CV_ROUTING: every row
    /// publishes (STARK_ROW_IDX, CV_OUT) with CV_OUT_FREQ = 0, and
    /// no row sets IS_CV_IN. populate_lookup_freq leaves
    /// CV_OUT_FREQ at zero. Verifies.
    #[test]
    fn baseline_cv_routing_balances() {
        let cfg = build_stark_config(&test_zk_params(), &CircuitConfig::TEST_PEARL);
        let mut trace = CompositeTrace::baseline_min();
        trace.populate_lookup_freq();
        run_batch(&cfg, &trace.matrix).expect("baseline cv_routing must verify");
    }

    /// Place a CV_IN reference: row 100 reads the CV_OUT
    /// "published" at row 50. With CV_OUT all zero on row 50
    /// (baseline), CV_IN must also be all zero on row 100;
    /// CV_OR_TWEAK_PREP holds the referenced row index = 50.
    /// populate_lookup_freq increments CV_OUT_FREQ[50].
    #[test]
    fn cv_routing_zero_cv_reference_balances() {
        let cfg = build_stark_config(&test_zk_params(), &CircuitConfig::TEST_PEARL);
        let mut trace = CompositeTrace::baseline_min();

        // Row 100: IS_CV_IN = 1, CV_OR_TWEAK_PREP = 50,
        // CV_IN[0..8] = 0 (matches row 50's all-zero CV_OUT).
        let base = 100 * TOTAL_TRACE_WIDTH;
        let row = &mut trace.matrix.values[base..base + TOTAL_TRACE_WIDTH];
        let mut selectors = [false; 21];
        selectors[7] = true; // IS_CV_IN
        ControlChip.fill_row(&selectors, 0, row);
        row[CV_OR_TWEAK_PREP] = <Val as QuotientMap<u64>>::from_int(50);
        // CV_IN[0..8] left at 0.

        trace.populate_lookup_freq();
        run_batch(&cfg, &trace.matrix).expect("cv_routing with zero CV at row 50 must verify");
    }

    /// Place a CV_IN reference to a NON-EXISTENT row+CV pair:
    /// (CV_OR_TWEAK_PREP=99999, CV_IN=0). No table row publishes
    /// this key → LogUp rejects.
    #[test]
    fn cv_routing_dangling_reference_rejected() {
        let cfg = build_stark_config(&test_zk_params(), &CircuitConfig::TEST_PEARL);
        let mut trace = CompositeTrace::baseline_min();

        let base = 100 * TOTAL_TRACE_WIDTH;
        let row = &mut trace.matrix.values[base..base + TOTAL_TRACE_WIDTH];
        let mut selectors = [false; 21];
        selectors[7] = true; // IS_CV_IN
        ControlChip.fill_row(&selectors, 0, row);
        // Reference row 99999 (out of trace) — STARK_ROW_IDX only
        // goes up to height-1 = 8191.
        row[CV_OR_TWEAK_PREP] = <Val as QuotientMap<u64>>::from_int(99999);

        trace.populate_lookup_freq();
        let res = run_batch(&cfg, &trace.matrix);
        assert!(
            res.is_err(),
            "dangling CV reference must reject; got {:?}",
            res
        );
    }

    /// Place a CV_IN reference to row 50 with CV_IN claiming a
    /// non-zero CV that doesn't match row 50's actual CV_OUT (= 0).
    /// LogUp rejects (the (50, non-zero) key isn't published).
    #[test]
    fn cv_routing_wrong_cv_value_rejected() {
        let cfg = build_stark_config(&test_zk_params(), &CircuitConfig::TEST_PEARL);
        let mut trace = CompositeTrace::baseline_min();

        let base = 100 * TOTAL_TRACE_WIDTH;
        let row = &mut trace.matrix.values[base..base + TOTAL_TRACE_WIDTH];
        let mut selectors = [false; 21];
        selectors[7] = true;
        ControlChip.fill_row(&selectors, 0, row);
        row[CV_OR_TWEAK_PREP] = <Val as QuotientMap<u64>>::from_int(50);
        // Claim CV_IN[0] = 12345 (no row published this).
        row[CV_IN_START] = <Val as QuotientMap<u64>>::from_int(12345);

        trace.populate_lookup_freq();
        let res = run_batch(&cfg, &trace.matrix);
        assert!(res.is_err(), "wrong CV value must reject; got {:?}", res);
    }

    // =================================================================
    //  End-to-end integration: real chip activity through ALL buses
    // =================================================================

    /// Build a composite trace with:
    ///   - A real BLAKE3 hash compression at rows 0..7,
    ///   - A 2-step matmul chain at rows 8..9,
    ///   - A jackpot rotate-XOR-13 step at row 10.
    /// All 7 LogUp buses fire across the relevant rows; the
    /// trace generator must produce a trace that balances every
    /// bus AND satisfies every chip's constraints. This is the
    /// regression anchor confirming the whole prover stack works.
    #[test]
    fn three_chip_activity_with_all_lookups_verifies() {
        use crate::chips::blake3::compress::{Blake3Tweak, BLAKE3_IV};
        use crate::composite_layout::TILE_D;

        let cfg = build_stark_config(&test_zk_params(), &CircuitConfig::TEST_PEARL);
        let mut trace = CompositeTrace::baseline_min();

        // (a) BLAKE3 hash at rows 0..7.
        let cv: [u32; 8] = core::array::from_fn(|i| BLAKE3_IV[i]);
        let msg: [u32; 16] = core::array::from_fn(|i| (i as u32 + 1) * 0xABCDEF);
        let tweak = Blake3Tweak {
            counter_low: 42,
            counter_high: 0,
            block_len: 64,
            flags: 0x1B,
        };
        let _cv_out = trace.place_blake3_hash(0, &msg, &cv, &tweak);

        // (b) Matmul step chain at rows 8..9. A/B unpack cells
        // stay zero (so IRange8 queries balance via FREQ[128]).
        let a = [[0i8; TILE_D]; crate::composite_layout::TILE_H];
        let b = [[0i8; TILE_D]; crate::composite_layout::TILE_H];
        let zero_cumsum = [0i32; crate::chips::matmul::compute::CUMSUM_LEN];
        let after_reset = trace.place_matmul_step(8, &a, &b, true, false, &zero_cumsum);
        let after_update = trace.place_matmul_step(9, &a, &b, false, true, &after_reset);
        trace.fill_cumsum_passthrough(10, &after_update);

        // (c) Jackpot step at row 10. Initial state is all-zero
        // so cross-row passthrough on rows 0..9 holds trivially
        // (already at zero from baseline).
        let initial_jackpot = [0u32; crate::composite_layout::JACKPOT_SIZE];
        let _jackpot_after = trace.place_jackpot_step(10, &initial_jackpot, 0, 0xDEAD_BEEF, true);
        trace.fill_jackpot_passthrough(
            11,
            &crate::chips::jackpot::compute::apply_jackpot_step(
                &initial_jackpot, 0, 0xDEAD_BEEF, true,
            ),
        );

        // (d) Populate every *_FREQ column from the trace.
        trace.populate_lookup_freq();

        // (e) Prove + verify via p3-batch-stark.
        run_batch(&cfg, &trace.matrix)
            .expect("three-chip activity with all LogUp buses must verify");
    }

    /// **PROD bench with LogUp enabled.** Run the full
    /// CompositeFullAirWithLookups under PROD profile (log_blowup
    /// = 4, num_queries = 15, pow_bits = 0 → 60 operational FRI query bits).
    /// Ignored by default; run with --ignored.
    #[test]
    #[ignore = "PROD bench — expensive; run with --ignored"]
    fn composite_full_air_with_lookups_prod_bench() {
        let cfg = build_stark_config(&test_zk_params(), &CircuitConfig::PROD);
        let mut trace = CompositeTrace::baseline_min();
        trace.populate_lookup_freq();

        let air = CompositeFullAirWithLookups;
        let instances = vec![StarkInstance {
            air: &air,
            trace: &trace.matrix,
            public_values: vec![],
        }];

        let t0 = std::time::Instant::now();
        let prover_data = ProverData::from_instances(&cfg, &instances);
        let proof = prove_batch(&cfg, &instances, &prover_data);
        let prove_ms = t0.elapsed().as_millis();

        let t1 = std::time::Instant::now();
        verify_batch(&cfg, &[air], &proof, &[vec![]], &prover_data.common)
            .expect("PROD verify with LogUp");
        let verify_ms = t1.elapsed().as_millis();

        println!(
            "ai-pow-zk PROD bench WITH LogUp (baseline @ MIN_STARK_LEN = {} rows × {} cols, 7 buses):",
            crate::composite_layout::MIN_STARK_LEN,
            TOTAL_TRACE_WIDTH
        );
        println!("  prove    : {prove_ms} ms");
        println!("  verify   : {verify_ms} ms");
    }

    /// Tamper CV_OUT_FREQ to over-claim consumption → reject.
    #[test]
    fn tampered_cv_out_freq_rejected_by_logup() {
        let cfg = build_stark_config(&test_zk_params(), &CircuitConfig::TEST_PEARL);
        let mut trace = CompositeTrace::baseline_min();
        trace.populate_lookup_freq();
        let target = 50 * TOTAL_TRACE_WIDTH + CV_OUT_FREQ;
        trace.matrix.values[target] = <Val as QuotientMap<u64>>::from_int(1);
        let res = run_batch(&cfg, &trace.matrix);
        assert!(
            res.is_err(),
            "over-claimed CV_OUT_FREQ must reject; got {:?}",
            res
        );
    }

    /// Tamper URANGE13_FREQ AFTER populate (over-claim) → reject.
    #[test]
    fn tampered_urange13_freq_rejected_by_logup() {
        let cfg = build_stark_config(&test_zk_params(), &CircuitConfig::TEST_PEARL);
        let mut trace = CompositeTrace::baseline_min();
        trace.populate_lookup_freq();
        // populate_lookup_freq set URANGE13_FREQ[0] = (2 + 4) ×
        // 8192 = 49152 (all baseline limbs are 0). Bump it.
        let target = 0 * TOTAL_TRACE_WIDTH + crate::composite_layout::URANGE13_FREQ;
        use p3_field::PrimeField64;
        let prev = trace.matrix.values[target].as_canonical_u64();
        trace.matrix.values[target] = <Val as QuotientMap<u64>>::from_int(prev + 1);
        let res = run_batch(&cfg, &trace.matrix);
        assert!(
            res.is_err(),
            "over-claimed URANGE13_FREQ[0] must reject; got {:?}",
            res
        );
    }

    // =================================================================
    //  Property tests — AIR ↔ trace-generator lookup agreement
    // =================================================================
    //
    // Catch the silent-drift class: any random in-range tamper should
    // verify via populate_lookup_freq, any random out-of-range tamper
    // should reject. We pick query cells that are NOT otherwise
    // constrained by chip-level equations on the targeted row, so the
    // rejection (or acceptance) is isolated to the LogUp argument.
    //
    // Case counts are kept small (proptest default is 256) because
    // each case runs a full prove_batch + verify_batch (~25 s at
    // TEST_PEARL). Sweep is bounded by the ProptestConfig::cases.
    use proptest::prelude::*;

    proptest::proptest! {
        #![proptest_config(ProptestConfig {
            cases: 4,
            .. ProptestConfig::default()
        })]

        /// IRange8 bus: random in-range i8 placed in A_NOISED_UNPACK[0]
        /// on a passthrough row (all selectors = 0, so no other chip's
        /// constraint reads it). populate_lookup_freq updates
        /// IRANGE8_FREQ correctly → balances.
        #[test]
        fn prop_a_noised_unpack_inrange_verifies(v in -128i64..=127i64) {
            let cfg = build_stark_config(&test_zk_params(), &CircuitConfig::TEST_PEARL);
            let mut trace = CompositeTrace::baseline_min();
            let target = 0 * TOTAL_TRACE_WIDTH
                + crate::composite_layout::A_NOISED_UNPACK_START;
            trace.matrix.values[target] = <Val as QuotientMap<i64>>::from_int(v);
            trace.populate_lookup_freq();
            run_batch(&cfg, &trace.matrix)
                .expect("in-range i8 must verify");
        }

        /// IRange8 bus: random out-of-i8-range value placed in
        /// A_NOISED_UNPACK[0]. The query (value, +1) has no matching
        /// table entry → unbalanced → reject.
        #[test]
        fn prop_a_noised_unpack_outofrange_rejects(
            v in proptest::sample::select(vec![-1000i64, -200, 128, 200, 1000, 12345])
        ) {
            let cfg = build_stark_config(&test_zk_params(), &CircuitConfig::TEST_PEARL);
            let mut trace = CompositeTrace::baseline_min();
            let target = 0 * TOTAL_TRACE_WIDTH
                + crate::composite_layout::A_NOISED_UNPACK_START;
            trace.matrix.values[target] = <Val as QuotientMap<i64>>::from_int(v);
            trace.populate_lookup_freq();
            let res = run_batch(&cfg, &trace.matrix);
            prop_assert!(
                res.is_err(),
                "out-of-range i8 ({}) must reject; got {:?}",
                v, res
            );
        }

        /// URange8 bus (gated): place a valid u8 query at row 0 via
        /// place_urange8_query. `place_urange8_query` couples UINT8_DATA to
        /// MAT_UNPACK's i8 view through the i8u8 bus, and MAT_UNPACK is now
        /// int7-constrained to [-64,64] (audit N2, Pearl parity). So the reachable
        /// (and honest) UINT8_DATA values are exactly the u8 view of [-64,64] =
        /// {0..=64} ∪ {192..=255}; those must verify.
        #[test]
        fn prop_urange8_valid_query_verifies(v in prop_oneof![0u32..=64, 192u32..=255]) {
            let cfg = build_stark_config(&test_zk_params(), &CircuitConfig::TEST_PEARL);
            let mut trace = CompositeTrace::baseline_min();
            place_urange8_query(&mut trace, 0, v);
            trace.populate_lookup_freq();
            run_batch(&cfg, &trace.matrix)
                .expect("valid u8 query must verify");
        }

        /// **Audit N2 — Pearl parity (the fix's teeth).** A CONSISTENT staging row
        /// (MAT_UNPACK = i8(u8), matching NOISED_PACKED / UINT8_DATA) whose
        /// MAT_UNPACK i8 view is OUT of int7 `[-64,64]` — i.e. u8 ∈ [65,191] →
        /// i8 ∈ [65,127]∪[-128,-65] — is now REJECTED by the `irange7p1` range bus.
        /// Before the fix, MAT_UNPACK was range-checked to full i8 `[-128,127]`
        /// (`irange8`), so this exact row VERIFIED — an accept-set divergence from
        /// Pearl (`pearl_stark.rs`: "// Signal is in [-64, 64]"). This is the
        /// regression that pins the fix.
        #[test]
        fn prop_mat_unpack_out_of_int7_rejects(u in prop_oneof![65u32..=127, 128u32..=191]) {
            let cfg = build_stark_config(&test_zk_params(), &CircuitConfig::TEST_PEARL);
            let mut trace = CompositeTrace::baseline_min();
            place_urange8_query(&mut trace, 0, u);
            trace.populate_lookup_freq();
            let res = run_batch(&cfg, &trace.matrix);
            prop_assert!(
                res.is_err(),
                "MAT_UNPACK i8 view of u8={} is out of int7 [-64,64]; must reject; got {:?}",
                u, res
            );
        }

        /// I8U8 bus: inconsistent (i8, u8) pair where unsigned ≠
        /// signed.rem_euclid(256) → no matching table entry → reject.
        /// We force inconsistency by setting MAT_UNPACK[0] = 0 and
        /// UINT8_DATA[0] = nonzero.
        #[test]
        fn prop_inconsistent_i8u8_pair_rejects(u in 1u32..256) {
            let cfg = build_stark_config(&test_zk_params(), &CircuitConfig::TEST_PEARL);
            let mut trace = CompositeTrace::baseline_min();

            let base = 0 * TOTAL_TRACE_WIDTH;
            let row = &mut trace.matrix.values[base..base + TOTAL_TRACE_WIDTH];
            let mut selectors = [false; 21];
            selectors[10] = true; // IS_MSG_MAT
            ControlChip.fill_row(&selectors, 0, row);
            // MAT_UNPACK[0] = 0; UINT8_DATA[0] = u. For u != 0,
            // the pack value 0*256 + u = u corresponds to table
            // entry (signed=0, unsigned=0) only when u=0. Any u>0
            // is inconsistent.
            row[crate::composite_layout::MAT_UNPACK_START] =
                <Val as QuotientMap<i64>>::from_int(0);
            row[UINT8_DATA_START] = <Val as QuotientMap<u64>>::from_int(u as u64);

            trace.populate_lookup_freq();
            let res = run_batch(&cfg, &trace.matrix);
            prop_assert!(
                res.is_err(),
                "inconsistent (0, {}) pair must reject; got {:?}",
                u, res
            );
        }

        /// CV_ROUTING bus: query at random row references a random
        /// CV_OR_TWEAK_PREP value. If the referenced row's CV_OUT is
        /// all-zero (baseline) AND the CV_IN cells are all-zero, the
        /// query (ref_row, 0, 0, ..., 0) matches a table entry, so
        /// populate_lookup_freq balances it → verifies. If we set a
        /// nonzero CV_IN cell, the key changes and the table doesn't
        /// have it → rejects.
        #[test]
        fn prop_cv_routing_valid_reference_verifies(
            row_idx in 100u64..200,
            ref_row in 0u64..200
        ) {
            let cfg = build_stark_config(&test_zk_params(), &CircuitConfig::TEST_PEARL);
            let mut trace = CompositeTrace::baseline_min();
            let base = (row_idx as usize) * TOTAL_TRACE_WIDTH;
            let row = &mut trace.matrix.values[base..base + TOTAL_TRACE_WIDTH];
            let mut selectors = [false; 21];
            selectors[7] = true; // IS_CV_IN
            ControlChip.fill_row(&selectors, 0, row);
            row[CV_OR_TWEAK_PREP] = <Val as QuotientMap<u64>>::from_int(ref_row);
            // CV_IN stays at 0; the referenced row's CV_OUT is 0; the
            // query matches the table → balance.
            trace.populate_lookup_freq();
            run_batch(&cfg, &trace.matrix)
                .expect("valid CV reference must verify");
        }

        /// CV_ROUTING bus: query with NONZERO CV_IN[0] → key doesn't
        /// match any published CV_OUT (all zero in baseline) →
        /// rejects.
        #[test]
        fn prop_cv_routing_nonzero_cv_rejects(
            cv_value in 1u64..1_000_000
        ) {
            let cfg = build_stark_config(&test_zk_params(), &CircuitConfig::TEST_PEARL);
            let mut trace = CompositeTrace::baseline_min();
            let base = 100 * TOTAL_TRACE_WIDTH;
            let row = &mut trace.matrix.values[base..base + TOTAL_TRACE_WIDTH];
            let mut selectors = [false; 21];
            selectors[7] = true;
            ControlChip.fill_row(&selectors, 0, row);
            row[CV_OR_TWEAK_PREP] = <Val as QuotientMap<u64>>::from_int(50);
            row[CV_IN_START] = <Val as QuotientMap<u64>>::from_int(cv_value);
            trace.populate_lookup_freq();
            let res = run_batch(&cfg, &trace.matrix);
            prop_assert!(
                res.is_err(),
                "nonzero CV ({}) for ref_row=50 must reject; got {:?}",
                cv_value, res
            );
        }
    }

    // =================================================================
    //  Per-chip offset consistency (HL2)
    // =================================================================
    //
    // Confirm each chip's LOCAL_OFFSETS and COMPOSITE_OFFSETS are
    // internally consistent — column slots fit within their
    // respective layouts and don't overlap. Catches the silent-drift
    // class where someone adds a column block but forgets to update
    // the per-chip COMPOSITE_OFFSETS.

    #[test]
    fn matmul_composite_offsets_fit_within_total_trace_width() {
        use crate::chips::matmul::chip::MatmulCumsumChip;
        use crate::composite_layout::{TILE_D, TILE_H, TOTAL_TRACE_WIDTH};
        let off = MatmulCumsumChip::COMPOSITE_OFFSETS;
        // A_UNPACK + B_UNPACK each cover TILE_H × TILE_D cells.
        let a_end = off.a_unpack_start + TILE_H * TILE_D;
        let b_end = off.b_unpack_start + TILE_H * TILE_D;
        let cumsum_end = off.cumsum_start + TILE_H * TILE_H;
        assert!(a_end <= TOTAL_TRACE_WIDTH);
        assert!(b_end <= TOTAL_TRACE_WIDTH);
        assert!(cumsum_end <= TOTAL_TRACE_WIDTH);
        assert!(off.is_reset_col < TOTAL_TRACE_WIDTH);
        assert!(off.is_update_col < TOTAL_TRACE_WIDTH);
    }

    #[test]
    fn blake3_composite_offsets_fit_within_total_trace_width() {
        use crate::chips::blake3::chip::Blake3Chip;
        use crate::chips::blake3::layout::LIMBS_PER_STATE_SNAPSHOT;
        use crate::composite_layout::TOTAL_TRACE_WIDTH;
        let off = Blake3Chip::COMPOSITE_OFFSETS;
        // 4 state snapshots × 264 cells.
        let state_end = off.state_start + 4 * LIMBS_PER_STATE_SNAPSHOT;
        assert!(state_end <= TOTAL_TRACE_WIDTH);
        assert!(off.msg_start + 16 <= TOTAL_TRACE_WIDTH);
        assert!(off.cv_start + 8 <= TOTAL_TRACE_WIDTH);
        assert!(off.cv_out_start + 8 <= TOTAL_TRACE_WIDTH);
        assert!(off.tweak_col < TOTAL_TRACE_WIDTH);
        assert!(off.is_new_blake_col < TOTAL_TRACE_WIDTH);
        assert!(off.is_last_round_col < TOTAL_TRACE_WIDTH);
    }

    #[test]
    fn jackpot_composite_offsets_fit_within_total_trace_width() {
        use crate::chips::jackpot::chip::JackpotChip;
        use crate::composite_layout::{JACKPOT_SIZE, TOTAL_TRACE_WIDTH};
        let off = JackpotChip::COMPOSITE_OFFSETS;
        assert!(off.jackpot_msg_start + JACKPOT_SIZE <= TOTAL_TRACE_WIDTH);
        assert!(off.v_bits_start + 32 <= TOTAL_TRACE_WIDTH);
        assert!(off.x_bits_start + 32 <= TOTAL_TRACE_WIDTH);
        assert!(off.slot_sel_start + JACKPOT_SIZE <= TOTAL_TRACE_WIDTH);
        assert!(off.is_active_col < TOTAL_TRACE_WIDTH);
    }

    /// Composite-offsets blocks for the three lookup-aware chips
    /// must not overlap each other on shared cells (with the
    /// expected exception of the BIT_REG-as-V_BITS overlap for
    /// jackpot — that's by design).
    #[test]
    fn chip_composite_blocks_dont_unexpectedly_overlap() {
        use crate::chips::blake3::chip::Blake3Chip;
        use crate::chips::blake3::layout::LIMBS_PER_STATE_SNAPSHOT;
        use crate::chips::jackpot::chip::JackpotChip;
        use crate::chips::matmul::chip::MatmulCumsumChip;
        use crate::composite_layout::{JACKPOT_SIZE, TILE_D, TILE_H};

        let mat = MatmulCumsumChip::COMPOSITE_OFFSETS;
        let bl = Blake3Chip::COMPOSITE_OFFSETS;
        let jp = JackpotChip::COMPOSITE_OFFSETS;

        // Matmul's A_UNPACK [start, start + TILE_H * TILE_D).
        let mat_a = (mat.a_unpack_start, mat.a_unpack_start + TILE_H * TILE_D);
        let mat_b = (mat.b_unpack_start, mat.b_unpack_start + TILE_H * TILE_D);
        let mat_cumsum = (mat.cumsum_start, mat.cumsum_start + TILE_H * TILE_H);

        // BLAKE3's STATE block [start, start + 4 * 264).
        let bl_state = (
            bl.state_start,
            bl.state_start + 4 * LIMBS_PER_STATE_SNAPSHOT,
        );
        let bl_msg = (bl.msg_start, bl.msg_start + 16);
        let bl_cv = (bl.cv_start, bl.cv_start + 8);
        let bl_cv_out = (bl.cv_out_start, bl.cv_out_start + 8);

        // Jackpot's blocks.
        let jp_msg = (jp.jackpot_msg_start, jp.jackpot_msg_start + JACKPOT_SIZE);
        let jp_x = (jp.x_bits_start, jp.x_bits_start + 32);
        let jp_sel = (jp.slot_sel_start, jp.slot_sel_start + JACKPOT_SIZE);

        let disjoint = |a: (usize, usize), b: (usize, usize)| a.1 <= b.0 || b.1 <= a.0;

        // Matmul vs. BLAKE3 vs. Jackpot — pairwise disjoint.
        for (an, a) in [("mat_a", mat_a), ("mat_b", mat_b), ("mat_cumsum", mat_cumsum)] {
            for (bn, b) in [
                ("bl_state", bl_state),
                ("bl_msg", bl_msg),
                ("bl_cv", bl_cv),
                ("bl_cv_out", bl_cv_out),
            ] {
                assert!(
                    disjoint(a, b),
                    "matmul block {an}={:?} overlaps blake3 block {bn}={:?}",
                    a,
                    b
                );
            }
            for (bn, b) in [("jp_msg", jp_msg), ("jp_x", jp_x), ("jp_sel", jp_sel)] {
                assert!(
                    disjoint(a, b),
                    "matmul block {an}={:?} overlaps jackpot block {bn}={:?}",
                    a,
                    b
                );
            }
        }
        for (an, a) in [
            ("bl_state", bl_state),
            ("bl_msg", bl_msg),
            ("bl_cv", bl_cv),
            ("bl_cv_out", bl_cv_out),
        ] {
            for (bn, b) in [("jp_msg", jp_msg), ("jp_x", jp_x), ("jp_sel", jp_sel)] {
                assert!(
                    disjoint(a, b),
                    "blake3 block {an}={:?} overlaps jackpot block {bn}={:?}",
                    a,
                    b
                );
            }
        }
    }
}

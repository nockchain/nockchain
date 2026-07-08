//! Jackpot AIR — 16-slot rotate-XOR-13 chip.
//!
//! **Pearl ISC.** This file is derived from Pearl source code
//! (Copyright (c) 2025-2026 Pearl Research Labs; 2015-2016 The Decred
//! developers); see `crates/ai-pow-zk/LICENSE-PEARL` for the full
//! permission notice.
//!
//! Port of `Pearl zk-pow chip/jackpot/jackpot_air.rs`.
//! Each row updates exactly one slot of a 16-slot state register
//! via the rotate-XOR-13 primitive.
//!
//! ## Chip-local layout
//!
//! Width = 16 + 32 + 32 + 16 + 1 = 97 cols. Independent of the
//! composite layout; Phase 12 will plug into the global trace.
//!
//! ```text
//!   col 0..16    : JACKPOT_MSG[i]  i ∈ {0..16}  (16-slot state)
//!   col 16..48   : V_BITS[k]      k ∈ {0..32}   (bit-decomp of selected slot's value)
//!   col 48..80   : X_BITS[k]      k ∈ {0..32}   (bit-decomp of XOR-fold value)
//!   col 80..96   : SLOT_SEL[i]    i ∈ {0..16}   (one-hot)
//!   col 96       : IS_ACTIVE                    (boolean — active/passthrough)
//! ```
//!
//! ## Per-row constraints
//!
//! 1. **Booleans on SLOT_SEL, V_BITS, X_BITS, IS_ACTIVE.** Each
//!    cell satisfies `b · (1 − b) = 0`.
//! 2. **Sum of SLOT_SEL = IS_ACTIVE.** When active, exactly one
//!    slot is selected; when inactive (passthrough row), all
//!    selectors are 0.
//! 3. **V_BITS = bit_decompose(JACKPOT_MSG[selected])**. Encoded
//!    as `Σ_i SLOT_SEL[i] · JACKPOT_MSG[i] = polyval(V_BITS, 2)`.
//!    Degree 2.
//! 4. **Cross-row rotate-XOR-13 update.** For each slot i:
//!    ```text
//!      JACKPOT_MSG_NEW[i]
//!         = SLOT_SEL[i] · (polyval(rot13(V_BITS) XOR X_BITS, 2))
//!         + (1 − SLOT_SEL[i]) · JACKPOT_MSG[i]
//!    ```
//!    Degree 3.

use p3_air::{Air, AirBuilder, BaseAir, WindowAccess};
use p3_field::PrimeCharacteristicRing;
use p3_matrix::dense::RowMajorMatrix;

use super::compute::{apply_jackpot_step, bit_decompose_u32, one_hot_select};
use crate::composite_layout::{JACKPOT_SIZE, LROT_PER_TILE};
use crate::Val;

/// Chip-local column offsets.
pub mod cols {
    use crate::composite_layout::JACKPOT_SIZE;

    pub const JACKPOT_MSG: usize = 0;
    pub const JACKPOT_MSG_LEN: usize = JACKPOT_SIZE; // 16

    pub const V_BITS: usize = JACKPOT_MSG + JACKPOT_MSG_LEN;
    pub const V_BITS_LEN: usize = 32;

    pub const X_BITS: usize = V_BITS + V_BITS_LEN;
    pub const X_BITS_LEN: usize = 32;

    pub const SLOT_SEL: usize = X_BITS + X_BITS_LEN;
    pub const SLOT_SEL_LEN: usize = JACKPOT_SIZE; // 16

    pub const IS_ACTIVE: usize = SLOT_SEL + SLOT_SEL_LEN;

    pub const ROW_W: usize = IS_ACTIVE + 1; // 97
}

/// Jackpot 16-slot rotate-XOR-13 chip.
#[derive(Copy, Clone, Debug, Default)]
pub struct JackpotChip;

impl<F> BaseAir<F> for JackpotChip {
    fn width(&self) -> usize {
        cols::ROW_W
    }
}

/// Column-offset bundle for [`JackpotChip::eval_at`].
#[derive(Copy, Clone, Debug)]
pub struct JackpotOffsets {
    pub jackpot_msg_start: usize,
    pub v_bits_start: usize,
    pub x_bits_start: usize,
    pub slot_sel_start: usize,
    pub is_active_col: usize,
}

impl JackpotChip {
    /// Chip-local offsets.
    pub const LOCAL_OFFSETS: JackpotOffsets = JackpotOffsets {
        jackpot_msg_start: cols::JACKPOT_MSG,
        v_bits_start: cols::V_BITS,
        x_bits_start: cols::X_BITS,
        slot_sel_start: cols::SLOT_SEL,
        is_active_col: cols::IS_ACTIVE,
    };

    /// Composite-layout offsets. Phase 12d wiring:
    ///   * `jackpot_msg_start` → `JACKPOT_MSG_START`
    ///   * `v_bits_start` → `BIT_REG_START` (the bit-decomp slot
    ///     Pearl uses)
    ///   * `x_bits_start` → `JACKPOT_X_BITS_START` (extension
    ///     added in Phase 12d)
    ///   * `slot_sel_start` → `JACKPOT_SLOT_SEL_START` (extension)
    ///   * `is_active_col` → `IS_HASH_JACKPOT` (CONTROL_PREP
    ///     selector bit)
    pub const COMPOSITE_OFFSETS: JackpotOffsets = JackpotOffsets {
        jackpot_msg_start: crate::composite_layout::JACKPOT_MSG_START,
        v_bits_start: crate::composite_layout::BIT_REG_START,
        x_bits_start: crate::composite_layout::JACKPOT_X_BITS_START,
        slot_sel_start: crate::composite_layout::JACKPOT_SLOT_SEL_START,
        is_active_col: crate::composite_layout::IS_HASH_JACKPOT,
    };

    /// Emit the jackpot chip's constraints at the given column
    /// offsets.
    pub fn eval_at<AB: AirBuilder>(builder: &mut AB, off: &JackpotOffsets) {
        let main = builder.main();
        let cur = main.current_slice();
        let nxt = main.next_slice();

        // ---- 1. Booleanity ----
        let is_active_var = cur[off.is_active_col];
        builder.assert_bool(is_active_var);
        let is_active: AB::Expr = is_active_var.into();

        for k in 0..16 {
            builder.assert_bool(cur[off.slot_sel_start + k]);
        }
        for k in 0..32 {
            builder.assert_bool(cur[off.v_bits_start + k]);
        }
        for k in 0..32 {
            builder.assert_bool(cur[off.x_bits_start + k]);
        }

        // ---- 2. Sum of SLOT_SEL == IS_ACTIVE ----
        let mut sel_sum: AB::Expr = <AB::Expr as PrimeCharacteristicRing>::ZERO;
        for k in 0..16 {
            sel_sum = sel_sum + cur[off.slot_sel_start + k];
        }
        builder.assert_eq(sel_sum, is_active.clone());

        // ---- 3. V_BITS == bit_decompose(JACKPOT_MSG[selected]) ----
        let mut selected_msg: AB::Expr = <AB::Expr as PrimeCharacteristicRing>::ZERO;
        for i in 0..JACKPOT_SIZE {
            let sel: AB::Expr = cur[off.slot_sel_start + i].into();
            let msg: AB::Expr = cur[off.jackpot_msg_start + i].into();
            selected_msg = selected_msg + sel * msg;
        }
        let v_packed = polyval_bits::<AB>(&cur[off.v_bits_start..off.v_bits_start + 32]);
        builder.assert_zero(is_active.clone() * (selected_msg - v_packed.clone()));

        // ---- 4. Cross-row rotate-XOR-13 update ----
        let mut rotated_xor_packed: AB::Expr = <AB::Expr as PrimeCharacteristicRing>::ZERO;
        let two = <AB::F as PrimeCharacteristicRing>::TWO;
        let mut pow: AB::F = <AB::F as PrimeCharacteristicRing>::ONE;
        for i in 0..32 {
            let src_bit_idx = (i + 32 - (LROT_PER_TILE as usize)) % 32;
            let v_bit: AB::Expr = cur[off.v_bits_start + src_bit_idx].into();
            let x_bit: AB::Expr = cur[off.x_bits_start + i].into();
            let xor_bit: AB::Expr = v_bit.clone() + x_bit.clone() - x_bit * v_bit * two.clone();
            rotated_xor_packed = rotated_xor_packed + xor_bit * pow.clone();
            pow = pow * two.clone();
        }

        {
            // The JACKPOT_MSG RAM recurrence
            //   nxt[i] = SLOT_SEL[i]·rotl13_xor + (1−SLOT_SEL[i])·cur[i]
            // is only meaningful while the jackpot is *active*
            // (`is_active` = IS_HASH_JACKPOT). It MUST be gated by
            // `is_active`: an inactive row has SLOT_SEL ≡ 0 (by
            // the `Σ SLOT_SEL == is_active` constraint above), so
            // ungated it would force `nxt.JACKPOT_MSG ==
            // cur.JACKPOT_MSG` on every inactive→* transition —
            // i.e. JACKPOT_MSG pinned constant across all
            // inactive rows. That makes the inactive→active
            // boundary forbid the active row from carrying a
            // freshly-placed JACKPOT_MSG: with `cur` inactive
            // (JACKPOT_MSG = 0) it requires `nxt.JACKPOT_MSG = 0`.
            // Latent for years because every jackpot placement
            // hashed an all-zero JACKPOT_MSG (0 == 0); surfaced
            // by HIGH-2.2 §4.A, the first non-zero JACKPOT_MSG
            // (the real folded `M`). Gating by `is_active`
            // matches Pearl (whose RAM persistence is store/
            // active-gated) and leaves real multi-row jackpot
            // sequences — where consecutive rows are active —
            // fully constrained.
            let mut tb = builder.when_transition();
            for i in 0..JACKPOT_SIZE {
                let sel_i: AB::Expr = cur[off.slot_sel_start + i].into();
                let cur_msg: AB::Expr = cur[off.jackpot_msg_start + i].into();
                let nxt_msg: AB::Expr = nxt[off.jackpot_msg_start + i].into();

                let one_minus_sel: AB::Expr =
                    <AB::Expr as PrimeCharacteristicRing>::ONE - sel_i.clone();
                let rhs = sel_i * rotated_xor_packed.clone() + one_minus_sel * cur_msg;

                tb.assert_zero(is_active.clone() * (nxt_msg - rhs));
            }
        }
    }

    /// Composite-layout entry point. Called from
    /// [`crate::composite_full_air::CompositeFullAir::eval`].
    pub fn eval_composite<AB: AirBuilder>(builder: &mut AB) {
        Self::eval_at(builder, &Self::COMPOSITE_OFFSETS);
    }
}

impl<AB: AirBuilder> Air<AB> for JackpotChip {
    fn eval(&self, builder: &mut AB) {
        JackpotChip::eval_at(builder, &JackpotChip::LOCAL_OFFSETS);
    }
}

/// `polyval(bits, 2)` over a 32-bit cell slice.
fn polyval_bits<AB: AirBuilder>(bits: &[AB::Var]) -> AB::Expr {
    assert_eq!(bits.len(), 32);
    let two = <AB::F as PrimeCharacteristicRing>::TWO;
    let mut acc: AB::Expr = <AB::Expr as PrimeCharacteristicRing>::ZERO;
    let mut pow: AB::F = <AB::F as PrimeCharacteristicRing>::ONE;
    for &b in bits {
        acc = acc + b * pow.clone();
        pow = pow * two.clone();
    }
    acc
}

/// Write one jackpot row from a (state, selected_slot, x,
/// is_active) tuple. `state` is the value seen on this row (the
/// "old" value before this step's update).
pub fn fill_row(
    dest: &mut [Val],
    state: &[u32; JACKPOT_SIZE],
    selected_slot: usize,
    x: u32,
    is_active: bool,
) {
    use p3_field::integers::QuotientMap;

    assert_eq!(dest.len(), cols::ROW_W);

    // JACKPOT_MSG.
    for i in 0..JACKPOT_SIZE {
        dest[cols::JACKPOT_MSG + i] = <Val as QuotientMap<u64>>::from_int(state[i] as u64);
    }
    // V_BITS: bit-decomp of state[selected_slot] when active; else zeros.
    if is_active {
        let bits = bit_decompose_u32(state[selected_slot]);
        for k in 0..32 {
            dest[cols::V_BITS + k] = <Val as QuotientMap<u64>>::from_int(bits[k] as u64);
        }
    } else {
        for k in 0..32 {
            dest[cols::V_BITS + k] = Val::default();
        }
    }
    // X_BITS.
    let x_bits = if is_active {
        bit_decompose_u32(x)
    } else {
        [0u32; 32]
    };
    for k in 0..32 {
        dest[cols::X_BITS + k] = <Val as QuotientMap<u64>>::from_int(x_bits[k] as u64);
    }
    // SLOT_SEL one-hot.
    let oh = if is_active {
        one_hot_select(selected_slot)
    } else {
        [0u32; JACKPOT_SIZE]
    };
    for i in 0..JACKPOT_SIZE {
        dest[cols::SLOT_SEL + i] = <Val as QuotientMap<u64>>::from_int(oh[i] as u64);
    }
    // IS_ACTIVE.
    dest[cols::IS_ACTIVE] = <Val as QuotientMap<u64>>::from_int(if is_active { 1 } else { 0 });
}

/// One step in the jackpot chain.
#[derive(Clone, Copy, Debug)]
pub struct Step {
    pub selected_slot: usize,
    pub x: u32,
    pub is_active: bool,
}

/// Build a full jackpot trace from a list of steps + an initial
/// state. Computes the consistent state chain and pads to the
/// next power of 2.
pub fn build_trace(initial: &[u32; JACKPOT_SIZE], steps: &[Step]) -> RowMajorMatrix<Val> {
    let n = steps.len().next_power_of_two().max(4);
    let mut flat = vec![Val::default(); n * cols::ROW_W];

    let mut state = *initial;

    for (i, step) in steps.iter().enumerate() {
        let row_start = i * cols::ROW_W;
        let row = &mut flat[row_start..row_start + cols::ROW_W];

        fill_row(row, &state, step.selected_slot, step.x, step.is_active);

        // Advance state for next row.
        state = apply_jackpot_step(&state, step.selected_slot, step.x, step.is_active);
    }
    // After the last step, `state` holds the final value; pad rows
    // hold zeros (constraints satisfied trivially since sel_sum =
    // 0, is_active = 0, V_BITS = X_BITS = 0).
    let _ = state;

    RowMajorMatrix::new(flat, cols::ROW_W)
}

#[cfg(test)]
mod tests {
    //! End-to-end jackpot chip tests.

    use p3_field::integers::QuotientMap;
    use p3_uni_stark::{prove, verify};

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

    fn initial_state() -> [u32; JACKPOT_SIZE] {
        core::array::from_fn(|i| (i as u32 + 1) * 0x01010101)
    }

    fn build_4step_trace() -> RowMajorMatrix<Val> {
        let initial = initial_state();
        let steps = vec![
            Step {
                selected_slot: 0,
                x: 0xCAFE,
                is_active: true,
            },
            Step {
                selected_slot: 3,
                x: 0xBABE,
                is_active: true,
            },
            Step {
                selected_slot: 15,
                x: 0xDEADBEEF,
                is_active: true,
            },
            Step {
                selected_slot: 7,
                x: 0xF00D,
                is_active: true,
            },
        ];
        build_trace(&initial, &steps)
    }

    #[test]
    fn prove_and_verify_4_step_chain() {
        let cfg = build_stark_config(&test_zk_params(), &CircuitConfig::TEST_PEARL);
        let trace = build_4step_trace();
        let proof = prove::<AiPowStarkConfig, _>(&cfg, &JackpotChip, trace, &[]);
        verify::<AiPowStarkConfig, _>(&cfg, &JackpotChip, &proof, &[])
            .expect("4-step jackpot chain must verify");
    }

    /// Pass-through row: is_active = 0, SLOT_SEL all zero,
    /// JACKPOT_MSG carries forward unchanged.
    #[test]
    fn prove_and_verify_passthrough_row() {
        let cfg = build_stark_config(&test_zk_params(), &CircuitConfig::TEST_PEARL);
        let initial = initial_state();
        let steps = vec![
            Step {
                selected_slot: 0,
                x: 0xCAFE,
                is_active: true,
            },
            Step {
                selected_slot: 0,
                x: 0,
                is_active: false,
            },
            Step {
                selected_slot: 5,
                x: 0xBABE,
                is_active: true,
            },
            Step {
                selected_slot: 0,
                x: 0,
                is_active: false,
            },
        ];
        let trace = build_trace(&initial, &steps);
        let proof = prove::<AiPowStarkConfig, _>(&cfg, &JackpotChip, trace, &[]);
        verify::<AiPowStarkConfig, _>(&cfg, &JackpotChip, &proof, &[])
            .expect("jackpot passthrough trace must verify");
    }

    #[test]
    fn verify_rejects_tampered_jackpot_msg() {
        let cfg = build_stark_config(&test_zk_params(), &CircuitConfig::TEST_PEARL);
        let mut trace = build_4step_trace();
        // Change JACKPOT_MSG[0] on row 1 — the cross-row constraint
        // ties it to row 0's update which targets slot 0.
        let target = 1 * cols::ROW_W + cols::JACKPOT_MSG;
        trace.values[target] = <Val as QuotientMap<u64>>::from_int(0xDEADBEEFu64);
        let proof = prove::<AiPowStarkConfig, _>(&cfg, &JackpotChip, trace, &[]);
        assert!(
            verify::<AiPowStarkConfig, _>(&cfg, &JackpotChip, &proof, &[]).is_err(),
            "tampered JACKPOT_MSG[0] must reject"
        );
    }

    #[test]
    fn verify_rejects_wrong_v_bits() {
        let cfg = build_stark_config(&test_zk_params(), &CircuitConfig::TEST_PEARL);
        let mut trace = build_4step_trace();
        // Flip a V_BITS bit on row 0 — V_BITS must match the
        // selected slot's bit decomposition.
        let target = 0 * cols::ROW_W + cols::V_BITS + 5;
        let val = trace.values[target];
        use p3_field::PrimeField64;
        let bit = val.as_canonical_u64();
        let flipped = <Val as QuotientMap<u64>>::from_int(1 - bit);
        trace.values[target] = flipped;
        let proof = prove::<AiPowStarkConfig, _>(&cfg, &JackpotChip, trace, &[]);
        assert!(
            verify::<AiPowStarkConfig, _>(&cfg, &JackpotChip, &proof, &[]).is_err(),
            "wrong V_BITS must reject"
        );
    }

    #[test]
    fn verify_rejects_non_boolean_slot_sel() {
        let cfg = build_stark_config(&test_zk_params(), &CircuitConfig::TEST_PEARL);
        let mut trace = build_4step_trace();
        let target = 0 * cols::ROW_W + cols::SLOT_SEL;
        trace.values[target] = <Val as QuotientMap<u64>>::from_int(2);
        let proof = prove::<AiPowStarkConfig, _>(&cfg, &JackpotChip, trace, &[]);
        assert!(
            verify::<AiPowStarkConfig, _>(&cfg, &JackpotChip, &proof, &[]).is_err(),
            "non-boolean SLOT_SEL must reject"
        );
    }

    #[test]
    fn verify_rejects_multiple_slots_selected() {
        let cfg = build_stark_config(&test_zk_params(), &CircuitConfig::TEST_PEARL);
        let mut trace = build_4step_trace();
        // Set SLOT_SEL[5] = 1 on row 0 (in addition to SLOT_SEL[0]
        // which is already 1). Sum becomes 2; constraint sel_sum =
        // is_active = 1 must reject.
        let target = 0 * cols::ROW_W + cols::SLOT_SEL + 5;
        trace.values[target] = <Val as QuotientMap<u64>>::from_int(1);
        let proof = prove::<AiPowStarkConfig, _>(&cfg, &JackpotChip, trace, &[]);
        assert!(
            verify::<AiPowStarkConfig, _>(&cfg, &JackpotChip, &proof, &[]).is_err(),
            "two simultaneously-selected slots must reject"
        );
    }

    #[test]
    fn verify_rejects_active_without_selection() {
        let cfg = build_stark_config(&test_zk_params(), &CircuitConfig::TEST_PEARL);
        let mut trace = build_4step_trace();
        // Set SLOT_SEL[0] = 0 on row 0 while IS_ACTIVE stays 1 —
        // sel_sum = 0 ≠ 1 = IS_ACTIVE. Reject.
        let target = 0 * cols::ROW_W + cols::SLOT_SEL;
        trace.values[target] = Val::default();
        let proof = prove::<AiPowStarkConfig, _>(&cfg, &JackpotChip, trace, &[]);
        assert!(
            verify::<AiPowStarkConfig, _>(&cfg, &JackpotChip, &proof, &[]).is_err(),
            "active row without slot selection must reject"
        );
    }

    #[test]
    fn verify_rejects_tampered_x_bits() {
        let cfg = build_stark_config(&test_zk_params(), &CircuitConfig::TEST_PEARL);
        let mut trace = build_4step_trace();
        let target = 0 * cols::ROW_W + cols::X_BITS;
        use p3_field::PrimeField64;
        let val = trace.values[target];
        let flipped = <Val as QuotientMap<u64>>::from_int(1 - val.as_canonical_u64());
        trace.values[target] = flipped;
        let proof = prove::<AiPowStarkConfig, _>(&cfg, &JackpotChip, trace, &[]);
        assert!(
            verify::<AiPowStarkConfig, _>(&cfg, &JackpotChip, &proof, &[]).is_err(),
            "tampered X_BITS must reject"
        );
    }

    #[test]
    fn verify_rejects_unrotated_value() {
        let cfg = build_stark_config(&test_zk_params(), &CircuitConfig::TEST_PEARL);
        let initial = initial_state();
        let steps = vec![
            Step {
                selected_slot: 0,
                x: 0xCAFE,
                is_active: true,
            },
            Step {
                selected_slot: 0,
                x: 0,
                is_active: false,
            },
            Step {
                selected_slot: 0,
                x: 0,
                is_active: false,
            },
            Step {
                selected_slot: 0,
                x: 0,
                is_active: false,
            },
        ];
        let mut trace = build_trace(&initial, &steps);
        // Force row 1's JACKPOT_MSG[0] to NOT be the rotated value
        // — set it to initial[0] (unrotated).
        let target = 1 * cols::ROW_W + cols::JACKPOT_MSG;
        trace.values[target] = <Val as QuotientMap<u64>>::from_int(initial[0] as u64);
        let proof = prove::<AiPowStarkConfig, _>(&cfg, &JackpotChip, trace, &[]);
        assert!(
            verify::<AiPowStarkConfig, _>(&cfg, &JackpotChip, &proof, &[]).is_err(),
            "missing rotation must reject"
        );
    }

    #[test]
    fn chip_width_pinned() {
        assert_eq!(cols::ROW_W, 97);
        assert_eq!(cols::JACKPOT_MSG_LEN, 16);
        assert_eq!(cols::V_BITS_LEN, 32);
        assert_eq!(cols::X_BITS_LEN, 32);
        assert_eq!(cols::SLOT_SEL_LEN, 16);
    }

    /// Production-shape stress test: 16 active updates (one per
    /// slot) + 16 passthrough rows = 32 rows = a full "rotate
    /// every slot once" pass.
    #[test]
    fn prove_and_verify_full_slot_pass() {
        let cfg = build_stark_config(&test_zk_params(), &CircuitConfig::TEST_PEARL);
        let initial = initial_state();
        let mut steps = Vec::with_capacity(32);
        for i in 0..JACKPOT_SIZE {
            steps.push(Step {
                selected_slot: i,
                x: (i as u32 + 1) * 0x12345,
                is_active: true,
            });
            steps.push(Step {
                selected_slot: 0,
                x: 0,
                is_active: false,
            });
        }
        let trace = build_trace(&initial, &steps);
        let proof = prove::<AiPowStarkConfig, _>(&cfg, &JackpotChip, trace, &[]);
        verify::<AiPowStarkConfig, _>(&cfg, &JackpotChip, &proof, &[])
            .expect("full-slot pass must verify");
    }
}

//! Composite trace generator for the M10.1c AIR.
//!
//! **Pearl ISC.** This file is derived from Pearl source code
//! (Copyright (c) 2025-2026 Pearl Research Labs; 2015-2016 The Decred
//! developers); see `crates/ai-pow-zk/LICENSE-PEARL` for the full
//! permission notice.
//!
//! Port of `Pearl zk-pow pearl_trace.rs` — produces a
//! `TOTAL_TRACE_WIDTH × N` trace matrix from a high-level
//! "instruction list" (the sequence of hashes, matmul tile
//! updates, jackpot rotations that the proof represents).
//!
//! ## Phase 13 scope
//!
//! This phase ships the **baseline trace builder** that fills the
//! constraint-bearing structural columns:
//!   * STARK_ROW_IDX = 0, 1, ..., N-1.
//!   * 4 range tables enumerate [MIN..=MAX] then replay MAX.
//!   * I8U8 table enumerates all 256 (i8, u8) pairs.
//!   * All remaining columns = 0 (no chip activity).
//!
//! Such a "passthrough" trace verifies under
//! [`crate::composite_full_air::CompositeFullAir`] but represents
//! no actual matmul / BLAKE3 / jackpot work. It's the foundation
//! every higher-level builder extends.
//!
//! ## Instruction-list shape (forward-looking)
//!
//! A full Pearl-style trace generator takes a list of high-level
//! instructions:
//!
//! ```text
//!   pub enum Instr {
//!       MatmulStep { a_id, b_id, is_reset, is_update },
//!       Blake3Hash { msg, cv_in, tweak },
//!       JackpotStep { slot, x, is_active },
//!       Padding,
//!   }
//! ```
//!
//! and compiles each into a contiguous block of rows in the
//! composite trace, threading state across blocks (matmul cumsum
//! chain, BLAKE3 CV routing, jackpot state evolution). The
//! instruction compilation also fills CONTROL_PREP and the
//! preprocessed columns ([`crate::composite_preprocess`]) so the
//! control chip's unpacking constraint is satisfied.
//!
//! Phase 13's minimal deliverable establishes the type surface and
//! the baseline; the multi-instruction generator is left as
//! follow-on work tied to Phase 14's lookup wiring (since
//! instruction blocks determine the lookup multiplicities).

use p3_matrix::dense::RowMajorMatrix;

use crate::chips::blake3::chip::pack_tweak;
use crate::chips::blake3::compress::{
    blake3_permute_msg, compress_full_state, round_with_snapshots, Blake3Tweak, BLAKE3_IV,
};
use crate::chips::blake3::layout::LIMBS_PER_STATE_SNAPSHOT;
use crate::chips::control::ControlChip;
use crate::chips::i8u8::I8U8Chip;
use crate::chips::jackpot::compute::{apply_jackpot_step, bit_decompose_u32, one_hot_select};
use crate::chips::matmul::compute::{compute_row, CUMSUM_LEN};
use crate::chips::range_table::{IRange7P1Chip, IRange8Chip, URange13Chip, URange8Chip};
use crate::composite_layout::{
    AB_ID_LIMBS_LEN, AB_ID_LIMBS_START, A_ID, A_ID_LEN, A_NOISED_LEN, A_NOISED_START,
    A_NOISED_UNPACK_LEN, A_NOISED_UNPACK_START, BIT_REG_START, BLAKE3_CV_START, BLAKE3_MSG_START,
    BLAKE3_ROUND_START, B_ID, B_ID_LEN, B_NOISED_LEN, B_NOISED_START, B_NOISED_UNPACK_LEN,
    B_NOISED_UNPACK_START, CONTROL_PREP, CUMSUM_TILE_START, CV_IN_LEN, CV_IN_START,
    CV_OR_TWEAK_PREP, CV_OUT_FREQ, CV_OUT_LEN, CV_OUT_START, FOLD_IS_FOLD, FOLD_MCUR_BITS_START,
    FOLD_SLOT_SEL_START, FOLD_STATE_START, FOLD_STRIPE_SEL_START, FOLD_XOR_OUT, FOLD_XSTEP,
    FOLD_XSTEP_BITS_START, I8U8_FREQ, IRANGE7P1_FREQ, IRANGE8_FREQ, IS_CV_IN, IS_MSG_MAT,
    IS_RESET_CUMSUM, IS_UPDATE_CUMSUM, JACKPOT_MSG_START, JACKPOT_SIZE, JACKPOT_SLOT_SEL_START,
    JACKPOT_X_BITS_START, MAT_FREQ, MAT_ID, MAT_ID_LIMBS_LEN, MAT_ID_LIMBS_START, MAT_UNPACK_START,
    MAT_UNPACK_WIN, NOISED_PACKED_START, NOISE_UNPACK_START, NOISE_UNPACK_WIN, STARK_ROW_IDX,
    STRIPE_MAX, SX_IN_BITS_START, SX_IN_START, SX_IS_ACTIVE, SX_LANE_SEL_START, SX_NEW_SEL,
    SX_NEW_SEL_BITS_START, SX_Q_START, SX_XR_SEL_BITS_START, SX_XR_START, TILE_D, TILE_H,
    TOTAL_TRACE_WIDTH, UINT8_DATA_START, UINT8_DATA_WIN, URANGE13_FREQ, URANGE8_FREQ,
};
use crate::Val;

/// Interpret a Goldilocks field element as a signed integer
/// (used when scanning trace cells that hold signed values like
/// i7+1 noise or i8 matrix bytes). Goldilocks' modulus is
/// `p = 2^64 − 2^32 + 1`. The canonical representation of `-k`
/// (for small `k`) is `p − k`, so any element above `p / 2`
/// represents a negative number.
fn goldilocks_to_signed(raw: u64) -> i64 {
    // Goldilocks modulus = 18446744069414584321 (2^64 − 2^32 + 1).
    const GOLDILOCKS_P: u64 = 0xFFFF_FFFF_0000_0001;
    if raw > GOLDILOCKS_P / 2 {
        (raw as i128 - GOLDILOCKS_P as i128) as i64
    } else {
        raw as i64
    }
}

/// §4.C.2 / A3.1 — a distinct `noised_packed` store chunk plus
/// the deterministic tile-strip source of each of its 8 bytes
/// (the verifier-recomputable map from a store row to its
/// `(plain, noise)` decomposition). `side_a`: this chunk is an
/// A-strip (`true`) or B-strip (`false`) micro-tile window;
/// `src[m] = Some((lane, l))` (lane = row-in-tile for A /
/// col-in-tile for B; `l` = `k`-column) or `None` (zero-pad).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NoisedChunkSrc {
    pub bytes: [i8; 8],
    pub side_a: bool,
    pub src: [Option<(u32, u32)>; 8],
}

/// Reserve low chunk IDs for all-zero padding rows. Real matrix
/// producer IDs start here so `(0..8, 0, 0)` padding table entries
/// cannot satisfy a malicious zero-substitution query.
pub const NOISED_CHUNK_ID_BASE: u64 = 8;

pub fn noised_chunk_id(id_base: u64, k: usize, src: &[Option<(u32, u32)>; 8]) -> u64 {
    if let Some((lane, l)) = src.iter().flatten().next().copied() {
        id_base + (((lane as usize) * k + l as usize) / 8) as u64
    } else {
        0
    }
}

/// A composite trace ready for proving by
/// [`crate::composite_full_air::CompositeFullAir`].
#[derive(Clone, Debug)]
pub struct CompositeTrace {
    /// The TOTAL_TRACE_WIDTH × N matrix; `N` is a power of 2 and
    /// `>= composite_layout::MIN_STARK_LEN = 8192`.
    pub matrix: RowMajorMatrix<Val>,
}

impl CompositeTrace {
    fn pack_ab_id(a_id: u64, b_id: u64) -> u64 {
        crate::composite_preprocess::pack_ab_id(a_id, b_id)
    }

    fn fill_ab_ids(row: &mut [Val], a_ids: &[u64; A_ID_LEN], b_ids: &[u64; B_ID_LEN]) {
        use p3_field::integers::QuotientMap;

        debug_assert_eq!(A_ID_LEN, 4);
        debug_assert_eq!(B_ID_LEN, 4);
        let ab = Self::pack_ab_id(a_ids[0], b_ids[0]);
        row[crate::composite_layout::AB_ID_PREP] = <Val as QuotientMap<u64>>::from_int(ab);
        let mask = (1u64 << crate::composite_layout::BITS_PER_LIMB) - 1;
        for i in 0..AB_ID_LIMBS_LEN {
            row[AB_ID_LIMBS_START + i] = <Val as QuotientMap<u64>>::from_int(
                (ab >> (i * crate::composite_layout::BITS_PER_LIMB)) & mask,
            );
        }
        for i in 0..A_ID_LEN {
            row[A_ID + i] = <Val as QuotientMap<u64>>::from_int(a_ids[i]);
        }
        for i in 0..B_ID_LEN {
            row[B_ID + i] = <Val as QuotientMap<u64>>::from_int(b_ids[i]);
        }
    }

    /// Number of rows.
    pub fn height(&self) -> usize {
        self.matrix.values.len() / TOTAL_TRACE_WIDTH
    }

    /// Number of columns. Always [`TOTAL_TRACE_WIDTH`].
    pub fn width(&self) -> usize {
        TOTAL_TRACE_WIDTH
    }

    /// Build a baseline-zero trace of `n` rows.
    ///
    /// `n` must be a power of 2 and at least
    /// `composite_layout::MIN_STARK_LEN = 8192`.
    ///
    /// The resulting trace satisfies every constraint wired into
    /// [`crate::composite_full_air::CompositeFullAir`] but
    /// represents no chip-level activity.
    pub fn baseline(n: usize) -> Self {
        use p3_field::integers::QuotientMap;

        assert!(n.is_power_of_two(), "trace length must be a power of 2");
        assert!(
            n >= crate::composite_layout::MIN_STARK_LEN,
            "trace length {n} below MIN_STARK_LEN = {}",
            crate::composite_layout::MIN_STARK_LEN
        );

        let mut flat = vec![Val::default(); n * TOTAL_TRACE_WIDTH];

        for r in 0..n {
            let row_start = r * TOTAL_TRACE_WIDTH;
            let row = &mut flat[row_start..row_start + TOTAL_TRACE_WIDTH];

            // STARK_ROW_IDX = r.
            row[STARK_ROW_IDX] = <Val as QuotientMap<u64>>::from_int(r as u64);

            // Range table cells: 4 range tables, plus I8U8.
            URange8Chip::default().fill_row(r, row);
            URange13Chip::default().fill_row(r, row);
            IRange7P1Chip::default().fill_row(r, row);
            IRange8Chip::default().fill_row(r, row);
            I8U8Chip.fill_row(r, row);

            // Everything else stays zero; chip-level constraints
            // (control, input, matmul, blake3) all degenerate to
            // satisfied for all-zero rows.
        }

        Self {
            matrix: RowMajorMatrix::new(flat, TOTAL_TRACE_WIDTH),
        }
    }

    /// Build a baseline trace at exactly `MIN_STARK_LEN`. The
    /// smallest verifiable composite proof shape.
    pub fn baseline_min() -> Self {
        Self::baseline(crate::composite_layout::MIN_STARK_LEN)
    }

    /// Place a single matmul step at row `row_idx`. The caller is
    /// responsible for supplying `cumsum_old`, the CUMSUM_TILE
    /// value entering this step (must equal the previous matmul
    /// step's `cumsum_new` for the chain to verify).
    ///
    /// Returns the resulting `cumsum_new` so the caller can thread
    /// it into the next step.
    ///
    /// This is the *single-row* primitive Phase 13b uses; the
    /// caller does the threading across rows. A higher-level
    /// `with_matmul_instrs` builder will land alongside the
    /// instruction-list compiler.
    pub fn place_matmul_step(
        &mut self,
        row_idx: usize,
        a: &[[i8; TILE_D]; TILE_H],
        b: &[[i8; TILE_D]; TILE_H],
        is_reset: bool,
        is_update: bool,
        cumsum_old: &[i32; CUMSUM_LEN],
    ) -> [i32; CUMSUM_LEN] {
        self.place_matmul_step_with_ids(
            row_idx, a, b, &[0; A_ID_LEN], &[0; B_ID_LEN], is_reset, is_update, cumsum_old,
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub fn place_matmul_step_with_ids(
        &mut self,
        row_idx: usize,
        a: &[[i8; TILE_D]; TILE_H],
        b: &[[i8; TILE_D]; TILE_H],
        a_ids: &[u64; A_ID_LEN],
        b_ids: &[u64; B_ID_LEN],
        is_reset: bool,
        is_update: bool,
        cumsum_old: &[i32; CUMSUM_LEN],
    ) -> [i32; CUMSUM_LEN] {
        use p3_field::integers::QuotientMap;

        assert!(row_idx < self.height(), "row {row_idx} out of bounds");

        // Selector + CONTROL_PREP via control chip's fill_row.
        // Build the 21-bit selector array, with IS_RESET_CUMSUM
        // and IS_UPDATE_CUMSUM at their composite-layout positions.
        let mut selectors = [false; 21];
        // Index of IS_RESET_CUMSUM = 0 (it's the first selector bit
        // packed into CONTROL_PREP); index of IS_UPDATE_CUMSUM = 1.
        // These match composite_layout::SELECTOR_COLS ordering.
        selectors[0] = is_reset;
        selectors[1] = is_update;

        let row_start = row_idx * TOTAL_TRACE_WIDTH;
        let row = &mut self.matrix.values[row_start..row_start + TOTAL_TRACE_WIDTH];

        // Write control + selector + MAT_ID columns.
        // MAT_ID = 0 (we're not using NOISED_PACKED RAM-lookup yet
        // — that's Phase 14b's LogUp wiring).
        ControlChip.fill_row(&selectors, 0, row);
        Self::fill_ab_ids(row, a_ids, b_ids);

        // Write A / B unpack cells.
        for i in 0..TILE_H {
            for d in 0..TILE_D {
                row[A_NOISED_UNPACK_START + i * TILE_D + d] =
                    <Val as QuotientMap<i64>>::from_int(a[i][d] as i64);
                row[B_NOISED_UNPACK_START + i * TILE_D + d] =
                    <Val as QuotientMap<i64>>::from_int(b[i][d] as i64);
            }
        }

        // M-S1 — the pack-link trace side: write packed A_NOISED /
        // B_NOISED = base-256 polyval of the 4 i8 unpack lanes each
        // covers (the encoding `InputChip`/`BUS_MATMUL_INPUT` use).
        // Flat lane f = blk[f/TILE_D][f%TILE_D]; cell c packs
        // [4c, 4c+4). Required so the CompositeFullAir pack-link
        // holds on matmul rows (else §6(b) sweep rows violate it).
        let pack4 = |blk: &[[i8; TILE_D]; TILE_H], c: usize| -> i64 {
            let mut acc: i64 = 0;
            let mut pow: i64 = 1;
            for j in 0..4 {
                let f = c * 4 + j;
                acc += (blk[f / TILE_D][f % TILE_D] as i64) * pow;
                pow *= 256;
            }
            acc
        };
        for c in 0..A_NOISED_LEN {
            row[A_NOISED_START + c] = <Val as QuotientMap<i64>>::from_int(pack4(a, c));
        }
        for c in 0..B_NOISED_LEN {
            row[B_NOISED_START + c] = <Val as QuotientMap<i64>>::from_int(pack4(b, c));
        }

        // Write CUMSUM = cumsum_old (the "entering" cumsum).
        for k in 0..CUMSUM_LEN {
            row[CUMSUM_TILE_START + k] = <Val as QuotientMap<i64>>::from_int(cumsum_old[k] as i64);
        }

        // Compute and return the post-step cumsum.
        compute_row(a, b, cumsum_old, is_reset, is_update)
    }

    /// Patch the CUMSUM_TILE cells at `row_idx`. Used to thread
    /// the "exit" cumsum value into the row following the last
    /// matmul step (so the AIR's cross-row equation
    /// `nxt.CUMSUM = cur.CUMSUM` is satisfied when the next row is
    /// not itself an active matmul step).
    pub fn set_cumsum_row(&mut self, row_idx: usize, cumsum: &[i32; CUMSUM_LEN]) {
        use p3_field::integers::QuotientMap;
        assert!(row_idx < self.height());
        let base = row_idx * TOTAL_TRACE_WIDTH;
        for k in 0..CUMSUM_LEN {
            self.matrix.values[base + CUMSUM_TILE_START + k] =
                <Val as QuotientMap<i64>>::from_int(cumsum[k] as i64);
        }
    }

    /// Place an 8-row BLAKE3 hash compression block starting at
    /// `row_start`. Writes BLAKE3_ROUND (4 state snapshots),
    /// BLAKE3_MSG, BLAKE3_CV, CV_OR_TWEAK_PREP, CV_OUT, and the
    /// IS_LAST_ROUND selector on the finalize row.
    ///
    /// `row_start + 8` must be ≤ `height()`. Returns the BLAKE3
    /// output CV (8 packed u32s) so the caller can chain it into
    /// downstream usage (e.g. the next hash's CV_IN).
    pub fn place_blake3_hash(
        &mut self,
        row_start: usize,
        message: &[u32; 16],
        cv_in: &[u32; 8],
        tweak: &Blake3Tweak,
    ) -> [u32; 8] {
        self.place_blake3_hash_with_selectors(row_start, message, cv_in, tweak, &[])
    }

    /// Variant of [`place_blake3_hash`] that ORs additional
    /// selector indices into the finalize-row CONTROL_PREP. Used
    /// by `place_matrix_hash_a` / `place_matrix_hash_b` to set
    /// `IS_HASH_A` / `IS_HASH_B` on the chunk-Merkle root row.
    /// `extra_selectors_on_finalize` indexes into `SELECTOR_COLS`
    /// (so `IS_HASH_A` = 4, `IS_HASH_B` = 5).
    pub fn place_blake3_hash_with_selectors(
        &mut self,
        row_start: usize,
        message: &[u32; 16],
        cv_in: &[u32; 8],
        tweak: &Blake3Tweak,
        extra_selectors_on_finalize: &[usize],
    ) -> [u32; 8] {
        use p3_field::integers::QuotientMap;

        assert!(
            row_start + 8 <= self.height(),
            "BLAKE3 block needs 8 rows; row_start={row_start}, height={}",
            self.height()
        );

        let tweak_packed = pack_tweak(tweak);

        // Run BLAKE3 once to get the final CV_OUT for the finalize
        // row.
        let full_state = compress_full_state(
            cv_in, message, tweak.counter_low, tweak.counter_high as u32, tweak.block_len,
            tweak.flags,
        );
        let cv_out: [u32; 8] = core::array::from_fn(|i| full_state[i]);

        // Per-round permuted messages.
        let mut round_msgs: Vec<[u32; 16]> = Vec::with_capacity(7);
        let mut cur_msg = *message;
        round_msgs.push(cur_msg);
        for _ in 1..7 {
            blake3_permute_msg(&mut cur_msg);
            round_msgs.push(cur_msg);
        }

        // Initial state at the start of hash: cv ++ IV[0..4] ++ tweak words.
        let mut state = [0u32; 16];
        for i in 0..8 {
            state[i] = cv_in[i];
        }
        for i in 0..4 {
            state[8 + i] = BLAKE3_IV[i];
        }
        state[12] = tweak.counter_low;
        state[13] = tweak.counter_high as u32;
        state[14] = tweak.block_len;
        state[15] = tweak.flags;

        // Compute snapshots for the 7 mixing rounds + the
        // finalize row.
        let mut current_input_state = state;

        // Helper that writes a 16-word state into the row's 264
        // BLAKE3_ROUND cells for a specific snapshot slot.
        fn write_state(row: &mut [Val], snapshot_offset: usize, state: &[u32; 16]) {
            let dest = &mut row[snapshot_offset..snapshot_offset + LIMBS_PER_STATE_SNAPSHOT];
            let mut off = 0;
            for i in 0..4 {
                dest[off] = <Val as QuotientMap<u64>>::from_int(state[i] as u64);
                off += 1;
            }
            for i in 4..8 {
                for bit in 0..32 {
                    dest[off] = <Val as QuotientMap<u64>>::from_int(((state[i] >> bit) & 1) as u64);
                    off += 1;
                }
            }
            for i in 8..12 {
                dest[off] = <Val as QuotientMap<u64>>::from_int(state[i] as u64);
                off += 1;
            }
            for i in 12..16 {
                for bit in 0..32 {
                    dest[off] = <Val as QuotientMap<u64>>::from_int(((state[i] >> bit) & 1) as u64);
                    off += 1;
                }
            }
            debug_assert_eq!(off, LIMBS_PER_STATE_SNAPSHOT);
        }

        // For each of the 7 mixing-round rows, run round_with_snapshots
        // and place the 4 snapshots at BLAKE3_ROUND_START + i * STATE_W.
        for r in 0..7 {
            let row_idx = row_start + r;
            let base = row_idx * TOTAL_TRACE_WIDTH;
            let row = &mut self.matrix.values[base..base + TOTAL_TRACE_WIDTH];

            let mut s = current_input_state;
            let snaps = round_with_snapshots(&mut s, &round_msgs[r]);

            write_state(row, BLAKE3_ROUND_START, &current_input_state);
            write_state(
                row,
                BLAKE3_ROUND_START + LIMBS_PER_STATE_SNAPSHOT,
                &snaps[0],
            );
            write_state(
                row,
                BLAKE3_ROUND_START + 2 * LIMBS_PER_STATE_SNAPSHOT,
                &snaps[1],
            );
            write_state(
                row,
                BLAKE3_ROUND_START + 3 * LIMBS_PER_STATE_SNAPSHOT,
                &snaps[2],
            );

            // BLAKE3_MSG (this row's permuted message).
            for i in 0..16 {
                row[BLAKE3_MSG_START + i] =
                    <Val as QuotientMap<u64>>::from_int(round_msgs[r][i] as u64);
            }
            // BLAKE3_CV (replicated across all 8 rows).
            for i in 0..8 {
                row[BLAKE3_CV_START + i] = <Val as QuotientMap<u64>>::from_int(cv_in[i] as u64);
            }
            // CV_OR_TWEAK_PREP.
            row[CV_OR_TWEAK_PREP] = <Val as QuotientMap<u64>>::from_int(tweak_packed);

            // Selectors: IS_NEW_BLAKE on row 0.
            let mut selectors = [false; 21];
            if r == 0 {
                selectors[8] = true; // IS_NEW_BLAKE index in SELECTOR_COLS
            }
            ControlChip.fill_row(&selectors, 0, row);

            current_input_state = snaps[3];
        }

        // Finalize row (row_start + 7). STATE0 = final round output,
        // STATE1 encoded so finalize_blake's "abuse" packing works.
        let final_input = current_input_state;
        let mut state1_for_finalize = [0u32; 16];
        // row1 cells (state[0..4]) — free, set to final_input for cleanness.
        state1_for_finalize[0] = final_input[0];
        state1_for_finalize[1] = final_input[1];
        state1_for_finalize[2] = final_input[2];
        state1_for_finalize[3] = final_input[3];
        // row2 bit-decomp slots (state[4..8]) — bits of final_input[0..4].
        state1_for_finalize[4] = final_input[0];
        state1_for_finalize[5] = final_input[1];
        state1_for_finalize[6] = final_input[2];
        state1_for_finalize[7] = final_input[3];
        // row3 cells (state[8..12]) — free.
        state1_for_finalize[8] = final_input[8];
        state1_for_finalize[9] = final_input[9];
        state1_for_finalize[10] = final_input[10];
        state1_for_finalize[11] = final_input[11];
        // row4 bit-decomp slots (state[12..16]) — bits of final_input[8..12].
        state1_for_finalize[12] = final_input[8];
        state1_for_finalize[13] = final_input[9];
        state1_for_finalize[14] = final_input[10];
        state1_for_finalize[15] = final_input[11];

        let row_idx = row_start + 7;
        let base = row_idx * TOTAL_TRACE_WIDTH;
        let row = &mut self.matrix.values[base..base + TOTAL_TRACE_WIDTH];

        write_state(row, BLAKE3_ROUND_START, &final_input);
        write_state(
            row,
            BLAKE3_ROUND_START + LIMBS_PER_STATE_SNAPSHOT,
            &state1_for_finalize,
        );
        // STATE2 and STATE3 stay zero (the chip's eval doesn't
        // constrain them on the finalize row).

        // Last-permuted message + CV + tweak.
        let last_msg = round_msgs[6];
        for i in 0..16 {
            row[BLAKE3_MSG_START + i] = <Val as QuotientMap<u64>>::from_int(last_msg[i] as u64);
        }
        for i in 0..8 {
            row[BLAKE3_CV_START + i] = <Val as QuotientMap<u64>>::from_int(cv_in[i] as u64);
        }
        row[CV_OR_TWEAK_PREP] = <Val as QuotientMap<u64>>::from_int(tweak_packed);

        // CV_OUT cells (only meaningful on the finalize row).
        for i in 0..8 {
            row[CV_OUT_START + i] = <Val as QuotientMap<u64>>::from_int(cv_out[i] as u64);
        }

        // Selectors: IS_LAST_ROUND on row 7, plus any extras the
        // caller requested (e.g. IS_HASH_A on the matrix-hash root).
        let mut selectors = [false; 21];
        selectors[9] = true; // IS_LAST_ROUND index in SELECTOR_COLS
        for &idx in extra_selectors_on_finalize {
            selectors[idx] = true;
        }
        ControlChip.fill_row(&selectors, 0, row);

        cv_out
    }

    /// Place a BLAKE3 keyed chunk-Merkle commitment over
    /// `matrix_bytes` into the trace, starting at `row_start`.
    /// Mirrors `crates/ai-pow/src/commit.rs::matrix_commitment`
    /// byte-for-byte:
    ///   `H_A = BLAKE3(pad_to_chunk_boundary(A_bytes), key=κ)`.
    ///
    /// `selector_idx` is the position in `SELECTOR_COLS` to set on
    /// the chunk-Merkle root row (use `4` for `IS_HASH_A`, `5` for
    /// `IS_HASH_B`). The convenience wrappers
    /// [`place_matrix_hash_a`](Self::place_matrix_hash_a) and
    /// [`place_matrix_hash_b`](Self::place_matrix_hash_b) hide
    /// this index.
    ///
    /// Returns `(next_row, root_cv)`: the row immediately after
    /// the last placed BLAKE3 block, and the 8-u32 commitment that
    /// matches `matrix_commitment(matrix_bytes, key)`. The caller
    /// must ensure `self.height() >= next_row`.
    pub fn place_matrix_hash(
        &mut self,
        row_start: usize,
        matrix_bytes: &[u8],
        key: &[u8; 32],
        selector_idx: usize,
    ) -> (usize, [u32; 8]) {
        // BLAKE3 standard flag bits.
        const F_CHUNK_START: u32 = 1 << 0;
        const F_CHUNK_END: u32 = 1 << 1;
        const F_PARENT: u32 = 1 << 2;
        const F_ROOT: u32 = 1 << 3;
        const F_KEYED_HASH: u32 = 1 << 4;
        const BLAKE3_CHUNK_LEN: usize = 1024;
        const BLAKE3_BLOCK_LEN: usize = 64;

        // Pad input to a multiple of CHUNK_LEN (matches
        // ai-pow/src/commit.rs::pad_to_chunk_boundary).
        let mut padded = matrix_bytes.to_vec();
        let pad_to = padded.len().div_ceil(BLAKE3_CHUNK_LEN) * BLAKE3_CHUNK_LEN;
        padded.resize(pad_to.max(BLAKE3_CHUNK_LEN), 0);
        let num_chunks = padded.len() / BLAKE3_CHUNK_LEN;

        // Key as 8 LE u32 words.
        let key_words: [u32; 8] = core::array::from_fn(|i| {
            u32::from_le_bytes([key[i * 4], key[i * 4 + 1], key[i * 4 + 2], key[i * 4 + 3]])
        });

        let mut row = row_start;
        let mut chunk_cvs: Vec<[u32; 8]> = Vec::with_capacity(num_chunks);

        // CHUNK LAYER — for each chunk, 16 keyed BLAKE3 compressions.
        for c in 0..num_chunks {
            let mut chunk_cv = key_words;
            for b in 0..16 {
                let block_off = c * BLAKE3_CHUNK_LEN + b * BLAKE3_BLOCK_LEN;
                let block_bytes = &padded[block_off..block_off + BLAKE3_BLOCK_LEN];
                let message: [u32; 16] = core::array::from_fn(|i| {
                    u32::from_le_bytes([
                        block_bytes[i * 4],
                        block_bytes[i * 4 + 1],
                        block_bytes[i * 4 + 2],
                        block_bytes[i * 4 + 3],
                    ])
                });

                let mut flags = F_KEYED_HASH;
                if b == 0 {
                    flags |= F_CHUNK_START;
                }
                if b == 15 {
                    flags |= F_CHUNK_END;
                }
                let is_single_chunk_root = num_chunks == 1 && b == 15;
                if is_single_chunk_root {
                    flags |= F_ROOT;
                }

                let tweak = Blake3Tweak {
                    counter_low: c as u32,
                    counter_high: (c >> 32) as u16,
                    block_len: BLAKE3_BLOCK_LEN as u32,
                    flags,
                };

                let extras: &[usize] = if is_single_chunk_root {
                    core::slice::from_ref(&selector_idx)
                } else {
                    &[]
                };
                chunk_cv =
                    self.place_blake3_hash_with_selectors(row, &message, &chunk_cv, &tweak, extras);
                row += 8;
            }
            chunk_cvs.push(chunk_cv);
        }

        // PARENT LAYER — binary-tree reduce. Promote unpaired CVs
        // (BLAKE3 spec for non-power-of-2 chunk counts).
        while chunk_cvs.len() > 1 {
            let is_top_layer = chunk_cvs.len() == 2;
            let mut next: Vec<[u32; 8]> = Vec::with_capacity((chunk_cvs.len() + 1) / 2);
            let mut i = 0;
            while i + 1 < chunk_cvs.len() {
                let left = chunk_cvs[i];
                let right = chunk_cvs[i + 1];
                let mut message = [0u32; 16];
                for j in 0..8 {
                    message[j] = left[j];
                    message[8 + j] = right[j];
                }

                let is_root_parent = is_top_layer && i + 2 == chunk_cvs.len();
                let mut flags = F_KEYED_HASH | F_PARENT;
                if is_root_parent {
                    flags |= F_ROOT;
                }
                let tweak = Blake3Tweak {
                    counter_low: 0,
                    counter_high: 0,
                    block_len: BLAKE3_BLOCK_LEN as u32,
                    flags,
                };

                let extras: &[usize] = if is_root_parent {
                    core::slice::from_ref(&selector_idx)
                } else {
                    &[]
                };
                let parent_cv = self
                    .place_blake3_hash_with_selectors(row, &message, &key_words, &tweak, extras);
                next.push(parent_cv);
                row += 8;
                i += 2;
            }
            if i < chunk_cvs.len() {
                next.push(chunk_cvs[i]); // promote unpaired CV
            }
            chunk_cvs = next;
        }

        let root_cv = chunk_cvs[0];
        (row, root_cv)
    }

    /// Convenience: keyed chunk-Merkle for matrix A. Sets
    /// `IS_HASH_A = 1` on the root row, binding the computed
    /// digest to public input `PI_HASH_A` (see
    /// [`composite_public`]).
    pub fn place_matrix_hash_a(
        &mut self,
        row_start: usize,
        matrix_bytes: &[u8],
        key: &[u8; 32],
    ) -> (usize, [u32; 8]) {
        // 4 = IS_HASH_A position in SELECTOR_COLS (see chips::control).
        self.place_matrix_hash(row_start, matrix_bytes, key, 4)
    }

    /// Convenience: keyed chunk-Merkle for matrix B. Sets
    /// `IS_HASH_B = 1` on the root row.
    pub fn place_matrix_hash_b(
        &mut self,
        row_start: usize,
        matrix_bytes: &[u8],
        key: &[u8; 32],
    ) -> (usize, [u32; 8]) {
        // 5 = IS_HASH_B position in SELECTOR_COLS.
        self.place_matrix_hash(row_start, matrix_bytes, key, 5)
    }

    /// P-B.2.2 — Pearl §4.6 **strip opening**. Recompute the
    /// committed BLAKE3 chunk-Merkle root from ONLY the
    /// contiguous chunk range `[c0, c1)` of the (padded) matrix
    /// plus the off-range authentication siblings — instead of
    /// re-hashing the whole matrix ([`place_matrix_hash`]). The
    /// recomputed root is bound to `PI_HASH_A`/`PI_HASH_B` by the
    /// **unchanged** C3 constraint
    /// (`IS_HASH_A/B · (CV_OUT − PI) = 0`): same soundness model,
    /// `O((c1-c0)·1024)` rows instead of `O(|matrix|)`.
    ///
    /// * `strip_bytes` — witness bytes of chunks `[c0, c1)`
    ///   (contiguous, length `(c1-c0)·1024`); the in-circuit
    ///   leaf layer hashes these, binding the revealed strip.
    /// * `auth_siblings` — off-range subtree-root CVs from
    ///   [`crate::blake3_tree::open_strip`], in the post-order
    ///   the true BLAKE3 tree consumes them (a wrong sibling ⇒
    ///   wrong root ⇒ C3 reject — BLAKE3 collision resistance).
    /// * `num_chunks` — total chunks of the *full padded* matrix
    ///   (the tree shape; verifier-fixed from params — P-B.2.3).
    /// * `selector_idx` — `4` = `IS_HASH_A`, `5` = `IS_HASH_B`
    ///   (set on the recomputed-root compression's finalize row).
    ///
    /// Every placed BLAKE3 leaf/parent compression is
    /// byte-identical to the one [`place_matrix_hash`] would
    /// place for that node (P-B.2.0: pairwise ≡ true tree), so
    /// the recomputed root equals `commit::matrix_commitment` of
    /// the full matrix for honest inputs.
    pub fn place_matrix_strip_opening(
        &mut self,
        row_start: usize,
        strip_bytes: &[u8],
        c0: usize,
        c1: usize,
        num_chunks: usize,
        auth_siblings: &[crate::blake3_tree::AuthSibling],
        kappa: &[u8; 32],
        selector_idx: usize,
        // §4.C.2 c-exact cx.2 g=1: `Some` ⇒ the Pearl `noise_ref`
        // byte parallel to `strip_bytes` (same length); each leaf
        // round-0 row becomes the M-S1 `noised_packed` producer
        // for its block (SEC_4C2 §8.9). `None` ⇒ pre-cx.2
        // hash-only behaviour (g=0, zero-blast).
        noise_strip: Option<&[i8]>,
        mat_id_base: Option<u64>,
    ) -> (usize, [u32; 8]) {
        assert!(
            c0 < c1 && c1 <= num_chunks,
            "range [{c0},{c1}) out of 0..{num_chunks}"
        );
        assert_eq!(
            strip_bytes.len(),
            (c1 - c0) * 1024,
            "strip_bytes must be exactly (c1-c0)*1024"
        );
        if let Some(n) = noise_strip {
            assert_eq!(
                n.len(),
                strip_bytes.len(),
                "noise_strip must be parallel to strip_bytes"
            );
        }
        let key_words: [u32; 8] = core::array::from_fn(|i| {
            u32::from_le_bytes([kappa[i * 4], kappa[i * 4 + 1], kappa[i * 4 + 2], kappa[i * 4 + 3]])
        });
        let mut row = row_start;
        if num_chunks == 1 {
            assert!(
                c0 == 0 && c1 == 1 && auth_siblings.is_empty(),
                "lone chunk: must open the single chunk, no siblings"
            );
            let cv = self.place_leaf_chunk(
                &mut row,
                &strip_bytes[0..1024],
                0,
                &key_words,
                true,
                selector_idx,
                noise_strip.map(|n| &n[0..1024]),
                mat_id_base,
                c0,
            );
            return (row, cv);
        }
        let mut si = 0usize;
        let root = self.fold_strip(
            &mut row, 0, num_chunks, c0, c1, strip_bytes, auth_siblings, &mut si, &key_words, true,
            selector_idx, noise_strip, mat_id_base, c0,
        );
        assert_eq!(
            si,
            auth_siblings.len(),
            "unconsumed authentication siblings"
        );
        (row, root)
    }

    /// Place the 16-compression keyed BLAKE3 chunk-hash of one
    /// 1024-byte chunk (global `chunk_index` ⇒ counter tweak),
    /// byte-identical to [`place_matrix_hash`]'s chunk layer.
    #[allow(clippy::too_many_arguments)]
    fn place_leaf_chunk(
        &mut self,
        row: &mut usize,
        chunk_bytes: &[u8],
        chunk_index: u64,
        key_words: &[u32; 8],
        single_chunk_root: bool,
        selector_idx: usize,
        // §4.C.2 c-exact cx.2 g=1 co-location: when `Some`, the
        // Pearl `noise_ref` byte for each position of this chunk,
        // parallel to `chunk_bytes` (0 on chunk-padding positions;
        // precomputed by the bridge per the cx.2-coloc.0-validated
        // (chunk,block,sub-slice)→matrix-position→noise_ref map).
        // Each leaf round-0 row then becomes the M-S1
        // `noised_packed` producer for its 64-byte block's 8
        // sub-slices (the X1 design, SEC_4C2 §8.6/§8.9).
        noise_chunk: Option<&[i8]>,
        mat_id_base: Option<u64>,
        strip_c0: usize,
    ) -> [u32; 8] {
        use p3_field::integers::QuotientMap;

        use crate::composite_layout::{
            MAT_UNPACK_START, MSG_PAIR_SEL_START, NOISED_PACKED_START, NOISE_PACKED_PREP,
            NOISE_UNPACK_START, UINT8_DATA_START,
        };
        const F_CHUNK_START: u32 = 1 << 0;
        const F_CHUNK_END: u32 = 1 << 1;
        const F_ROOT: u32 = 1 << 3;
        const F_KEYED_HASH: u32 = 1 << 4;
        let mut cv = *key_words;
        for b in 0..16 {
            let blk = &chunk_bytes[b * 64..b * 64 + 64];
            let message: [u32; 16] = core::array::from_fn(|i| {
                u32::from_le_bytes([blk[i * 4], blk[i * 4 + 1], blk[i * 4 + 2], blk[i * 4 + 3]])
            });
            let mut flags = F_KEYED_HASH;
            if b == 0 {
                flags |= F_CHUNK_START;
            }
            if b == 15 {
                flags |= F_CHUNK_END;
            }
            let is_root = single_chunk_root && b == 15;
            if is_root {
                flags |= F_ROOT;
            }
            let tweak = Blake3Tweak {
                counter_low: chunk_index as u32,
                counter_high: (chunk_index >> 32) as u16,
                block_len: 64,
                flags,
            };
            let extras: &[usize] = if is_root {
                core::slice::from_ref(&selector_idx)
            } else {
                &[]
            };
            let cr = *row; // the round-0 (IS_NEW_BLAKE) row of this block
            cv = self.place_blake3_hash_with_selectors(cr, &message, &cv, &tweak, extras);
            *row += 8;

            // §4.C.2 c-exact cx.2 g=1 co-location. On the round-0
            // row: the unpermuted BLAKE3_MSG holds this block's 64
            // committed plain bytes (∈ HASH_A via cx.2-c3
            // whole-block C3). Make the row the M-S1 producer for
            // its 8 sub-slices: write MAT_UNPACK/UINT8_DATA =
            // committed plain, NOISE_UNPACK = noise_ref,
            // NOISED_PACKED = a′ = plain+noise (InputChip-8),
            // NOISE_PACKED_PREP[s] = polyval(noise_s,129) (the
            // CRIT-1 pin), IS_MSG_MAT=1 (⇒ g=1), MSG_PAIR_SEL[0]=1
            // (cx.1b Σ==g; vestigial under X1's whole-block C3),
            // CONTROL_PREP msg_pair=0 (cx.1c pin). The blake3
            // columns (disjoint) stay intact.
            if let Some(nz) = noise_chunk {
                let nblk = &nz[b * 64..b * 64 + 64];
                let base = cr * TOTAL_TRACE_WIDTH;
                for s in 0..8 {
                    // NOISE_PACKED_PREP[s] = Σ_{m<8} noise·129^m.
                    let mut npp: i64 = 0;
                    let mut pw: i64 = 1;
                    for m in 0..8 {
                        let pl = blk[s * 8 + m] as i8;
                        let no = nblk[s * 8 + m];
                        let ap = pl.wrapping_add(no);
                        let g = s * 8 + m;
                        self.matrix.values[base + MAT_UNPACK_START + g] =
                            <Val as QuotientMap<i64>>::from_int(pl as i64);
                        self.matrix.values[base + UINT8_DATA_START + g] =
                            <Val as QuotientMap<u64>>::from_int((pl as u8) as u64);
                        self.matrix.values[base + NOISE_UNPACK_START + g] =
                            <Val as QuotientMap<i64>>::from_int(no as i64);
                        npp += (no as i64) * pw;
                        pw *= crate::chips::input::NOISE_PACKING_BASE as i64;
                        let _ = ap;
                    }
                    self.matrix.values[base + NOISE_PACKED_PREP + s] =
                        <Val as QuotientMap<i64>>::from_int(npp);
                    // NOISED_PACKED[2s+c] = Σ_{j<4}(plain+noise)·256^j.
                    for c in 0..2 {
                        let mut acc: i64 = 0;
                        let mut mult: i64 = 1;
                        for j in 0..4 {
                            let pl = blk[s * 8 + c * 4 + j] as i8 as i64;
                            let no = nblk[s * 8 + c * 4 + j] as i64;
                            acc += (pl + no) * mult;
                            mult *= 256;
                        }
                        self.matrix.values[base + NOISED_PACKED_START + 2 * s + c] =
                            <Val as QuotientMap<i64>>::from_int(acc);
                    }
                }
                // IS_MSG_MAT (idx 10) + IS_NEW_BLAKE (idx 8) +
                // CONTROL_PREP(msg_pair=0, MAT_ID = first 8-byte
                // sub-slice ID for this 64-byte block). Re-fill the
                // control cells (disjoint from the blake3 block).
                let mut sel = [false; 21];
                sel[8] = true; // IS_NEW_BLAKE (preserve the blake3 selector)
                sel[10] = true; // IS_MSG_MAT ⇒ g = 1
                ControlChip.fill_row(
                    &sel,
                    mat_id_base
                        .map(|base_id| {
                            base_id + ((chunk_index as usize - strip_c0) * 128 + b * 8) as u64
                        })
                        .unwrap_or(0)
                        .try_into()
                        .expect("co-located MAT_ID must fit 26 bits"),
                    &mut self.matrix.values[base..base + TOTAL_TRACE_WIDTH],
                );
                self.matrix.values[base + MSG_PAIR_SEL_START] =
                    <Val as QuotientMap<u64>>::from_int(1);
            }
        }
        cv
    }

    /// Place one keyed BLAKE3 PARENT compression of `left‖right`
    /// (byte-identical to [`place_matrix_hash`]'s parent layer).
    fn place_parent(
        &mut self,
        row: &mut usize,
        left: &[u32; 8],
        right: &[u32; 8],
        key_words: &[u32; 8],
        is_root: bool,
        selector_idx: usize,
    ) -> [u32; 8] {
        const F_PARENT: u32 = 1 << 2;
        const F_ROOT: u32 = 1 << 3;
        const F_KEYED_HASH: u32 = 1 << 4;
        let mut message = [0u32; 16];
        for j in 0..8 {
            message[j] = left[j];
            message[8 + j] = right[j];
        }
        let mut flags = F_KEYED_HASH | F_PARENT;
        if is_root {
            flags |= F_ROOT;
        }
        let tweak = Blake3Tweak {
            counter_low: 0,
            counter_high: 0,
            block_len: 64,
            flags,
        };
        let extras: &[usize] = if is_root {
            core::slice::from_ref(&selector_idx)
        } else {
            &[]
        };
        let cv = self.place_blake3_hash_with_selectors(*row, &message, key_words, &tweak, extras);
        *row += 8;
        cv
    }

    /// True-BLAKE3-tree fold mirroring
    /// [`crate::blake3_tree`]'s `fold_opening`: subtree fully
    /// outside `[c0,c1)` ⇒ consume one witness sibling (no rows);
    /// fully inside ⇒ recompute from leaf bytes; straddling ⇒
    /// split at `left_len` and recurse. Sibling order is the
    /// post-order `crate::blake3_tree::open_strip` produces.
    #[allow(clippy::too_many_arguments)]
    fn fold_strip(
        &mut self,
        row: &mut usize,
        lo: usize,
        hi: usize,
        c0: usize,
        c1: usize,
        strip_bytes: &[u8],
        sibs: &[crate::blake3_tree::AuthSibling],
        si: &mut usize,
        key_words: &[u32; 8],
        is_root: bool,
        selector_idx: usize,
        noise_strip: Option<&[i8]>,
        mat_id_base: Option<u64>,
        strip_c0: usize,
    ) -> [u32; 8] {
        if hi <= c0 || lo >= c1 {
            let s = &sibs[*si];
            *si += 1;
            assert!(
                s.lo == lo && s.hi == hi,
                "auth sibling range ({},{}) != node ({lo},{hi})",
                s.lo,
                s.hi
            );
            return core::array::from_fn(|i| {
                u32::from_le_bytes([s.cv[i * 4], s.cv[i * 4 + 1], s.cv[i * 4 + 2], s.cv[i * 4 + 3]])
            });
        }
        if c0 <= lo && hi <= c1 {
            return self.subtree_inside(
                row, lo, hi, c0, strip_bytes, key_words, is_root, selector_idx, noise_strip,
                mat_id_base, strip_c0,
            );
        }
        let mid = lo + crate::blake3_tree::left_len((hi - lo) as u64) as usize;
        let l = self.fold_strip(
            row, lo, mid, c0, c1, strip_bytes, sibs, si, key_words, false, selector_idx,
            noise_strip, mat_id_base, strip_c0,
        );
        let r = self.fold_strip(
            row, mid, hi, c0, c1, strip_bytes, sibs, si, key_words, false, selector_idx,
            noise_strip, mat_id_base, strip_c0,
        );
        self.place_parent(row, &l, &r, key_words, is_root, selector_idx)
    }

    /// Recompute a fully-opened subtree's true-tree root from the
    /// witness strip bytes (leaf chunk hashes + parents).
    #[allow(clippy::too_many_arguments)]
    fn subtree_inside(
        &mut self,
        row: &mut usize,
        lo: usize,
        hi: usize,
        c0: usize,
        strip_bytes: &[u8],
        key_words: &[u32; 8],
        is_root: bool,
        selector_idx: usize,
        noise_strip: Option<&[i8]>,
        mat_id_base: Option<u64>,
        strip_c0: usize,
    ) -> [u32; 8] {
        if hi - lo == 1 {
            let off = (lo - c0) * 1024;
            // num_chunks > 1 in this path ⇒ a leaf is never the
            // root (the lone-chunk case is handled before fold).
            return self.place_leaf_chunk(
                row,
                &strip_bytes[off..off + 1024],
                lo as u64,
                key_words,
                false,
                selector_idx,
                noise_strip.map(|n| &n[off..off + 1024]),
                mat_id_base,
                strip_c0,
            );
        }
        let mid = lo + crate::blake3_tree::left_len((hi - lo) as u64) as usize;
        let l = self.subtree_inside(
            row, lo, mid, c0, strip_bytes, key_words, false, selector_idx, noise_strip,
            mat_id_base, strip_c0,
        );
        let r = self.subtree_inside(
            row, mid, hi, c0, strip_bytes, key_words, false, selector_idx, noise_strip,
            mat_id_base, strip_c0,
        );
        self.place_parent(row, &l, &r, key_words, is_root, selector_idx)
    }

    /// Selective (disjoint-chunk) counterpart of [`place_matrix_strip_opening`]:
    /// place the trace rows for a BLAKE3 **multi-proof** over the sorted, distinct
    /// chunk set `sel`. `strip_bytes` is the selected chunks' bytes concatenated in
    /// `sel` order (`sel.len()*1024`); `noise_strip` (if any) is parallel. The
    /// producer keying is `strip_c0 = sel[0]` (⇒ key `chunk_index - strip_c0`),
    /// identical to the range opening, so the §6(b) sweep's `noised_packed`
    /// consumers match unchanged — only the SET of emitted leaf rows differs, so a
    /// scattered opening costs O(|sel|·log n) rows instead of the O(max−min)
    /// covering range (the production trace-bound fix for MoE).
    #[allow(clippy::too_many_arguments)]
    pub fn place_matrix_strip_opening_set(
        &mut self,
        row_start: usize,
        strip_bytes: &[u8],
        sel: &[usize],
        num_chunks: usize,
        auth_siblings: &[crate::blake3_tree::AuthSibling],
        kappa: &[u8; 32],
        selector_idx: usize,
        noise_strip: Option<&[i8]>,
        mat_id_base: Option<u64>,
    ) -> (usize, [u32; 8]) {
        assert!(!sel.is_empty(), "selective opening requires >= 1 chunk");
        assert!(
            sel.windows(2).all(|w| w[0] < w[1]),
            "sel must be sorted + distinct"
        );
        assert!(*sel.last().unwrap() < num_chunks, "sel chunk out of range");
        assert_eq!(strip_bytes.len(), sel.len() * 1024, "strip_bytes must be |sel|*1024");
        if let Some(n) = noise_strip {
            assert_eq!(n.len(), strip_bytes.len(), "noise_strip must be parallel to strip_bytes");
        }
        let key_words: [u32; 8] = core::array::from_fn(|i| {
            u32::from_le_bytes([kappa[i * 4], kappa[i * 4 + 1], kappa[i * 4 + 2], kappa[i * 4 + 3]])
        });
        let strip_c0 = sel[0];
        let mut row = row_start;
        if num_chunks == 1 {
            assert!(sel == [0] && auth_siblings.is_empty(), "lone chunk: sel=[0], no siblings");
            let cv = self.place_leaf_chunk(
                &mut row,
                &strip_bytes[0..1024],
                0,
                &key_words,
                true,
                selector_idx,
                noise_strip.map(|n| &n[0..1024]),
                mat_id_base,
                strip_c0,
            );
            return (row, cv);
        }
        let mut si = 0usize;
        let root = self.fold_strip_set(
            &mut row, 0, num_chunks, sel, strip_bytes, auth_siblings, &mut si, &key_words, true,
            selector_idx, noise_strip, mat_id_base, strip_c0,
        );
        assert_eq!(si, auth_siblings.len(), "unconsumed authentication siblings");
        (row, root)
    }

    #[allow(clippy::too_many_arguments)]
    fn fold_strip_set(
        &mut self,
        row: &mut usize,
        lo: usize,
        hi: usize,
        sel: &[usize],
        strip_bytes: &[u8],
        sibs: &[crate::blake3_tree::AuthSibling],
        si: &mut usize,
        key_words: &[u32; 8],
        is_root: bool,
        selector_idx: usize,
        noise_strip: Option<&[i8]>,
        mat_id_base: Option<u64>,
        strip_c0: usize,
    ) -> [u32; 8] {
        let cnt = sel.partition_point(|&c| c < hi) - sel.partition_point(|&c| c < lo);
        if cnt == 0 {
            let s = &sibs[*si];
            *si += 1;
            assert!(
                s.lo == lo && s.hi == hi,
                "auth sibling range ({},{}) != node ({lo},{hi})",
                s.lo,
                s.hi
            );
            return core::array::from_fn(|i| {
                u32::from_le_bytes([s.cv[i * 4], s.cv[i * 4 + 1], s.cv[i * 4 + 2], s.cv[i * 4 + 3]])
            });
        }
        if cnt == hi - lo {
            return self.subtree_inside_set(
                row, lo, hi, sel, strip_bytes, key_words, is_root, selector_idx, noise_strip,
                mat_id_base, strip_c0,
            );
        }
        let mid = lo + crate::blake3_tree::left_len((hi - lo) as u64) as usize;
        let l = self.fold_strip_set(
            row, lo, mid, sel, strip_bytes, sibs, si, key_words, false, selector_idx, noise_strip,
            mat_id_base, strip_c0,
        );
        let r = self.fold_strip_set(
            row, mid, hi, sel, strip_bytes, sibs, si, key_words, false, selector_idx, noise_strip,
            mat_id_base, strip_c0,
        );
        self.place_parent(row, &l, &r, key_words, is_root, selector_idx)
    }

    #[allow(clippy::too_many_arguments)]
    fn subtree_inside_set(
        &mut self,
        row: &mut usize,
        lo: usize,
        hi: usize,
        sel: &[usize],
        strip_bytes: &[u8],
        key_words: &[u32; 8],
        is_root: bool,
        selector_idx: usize,
        noise_strip: Option<&[i8]>,
        mat_id_base: Option<u64>,
        strip_c0: usize,
    ) -> [u32; 8] {
        if hi - lo == 1 {
            // Fully selected ⇒ lo ∈ sel; its bytes sit at rank(lo) in the strip.
            let off = sel.partition_point(|&c| c < lo) * 1024;
            return self.place_leaf_chunk(
                row,
                &strip_bytes[off..off + 1024],
                lo as u64,
                key_words,
                false,
                selector_idx,
                noise_strip.map(|n| &n[off..off + 1024]),
                mat_id_base,
                strip_c0,
            );
        }
        let mid = lo + crate::blake3_tree::left_len((hi - lo) as u64) as usize;
        let l = self.subtree_inside_set(
            row, lo, mid, sel, strip_bytes, key_words, false, selector_idx, noise_strip,
            mat_id_base, strip_c0,
        );
        let r = self.subtree_inside_set(
            row, mid, hi, sel, strip_bytes, key_words, false, selector_idx, noise_strip,
            mat_id_base, strip_c0,
        );
        self.place_parent(row, &l, &r, key_words, is_root, selector_idx)
    }

    /// F1 (C1) — place a "key-pin" row binding the chain-pinned
    /// BLAKE3 key into `CV_IN`.
    ///
    /// `kind = false` → `IS_USE_JOB_KEY` (binds `PI_JOB_KEY` = κ).
    /// `kind = true`  → `IS_USE_COMMITMENT_HASH` (binds
    /// `PI_COMMITMENT_HASH` = the caller-supplied jackpot key).
    ///
    /// The row carries no blake3 / jackpot / matmul activity, so
    /// only the C1 selector-gated constraint
    /// `IS_USE_* · (CV_IN[i] − PI_*[i]) = 0` is live on it (and
    /// the control chip's CONTROL_PREP packing). This is what
    /// anchors the SNARK to a specific block — without it the
    /// proof is unbounded. Encapsulates the CV_IN / ControlChip
    /// internals so `ai-pow`'s F1 bridge stays on the public API.
    pub fn place_key_pin_row(&mut self, row_idx: usize, commitment: bool, cv_in: &[u32; 8]) {
        use p3_field::integers::QuotientMap;

        use crate::composite_layout::CV_IN_START;

        assert!(
            row_idx < self.height(),
            "key-pin row_idx {row_idx} out of bounds (height {})",
            self.height()
        );
        let base = row_idx * TOTAL_TRACE_WIDTH;
        let row = &mut self.matrix.values[base..base + TOTAL_TRACE_WIDTH];
        for i in 0..8 {
            row[CV_IN_START + i] = <Val as QuotientMap<u64>>::from_int(cv_in[i] as u64);
        }
        let mut sel = [false; 21];
        // SELECTOR_COLS: idx 2 = IS_USE_JOB_KEY, idx 3 = IS_USE_COMMITMENT_HASH.
        sel[if commitment { 3 } else { 2 }] = true;
        ControlChip.fill_row(&sel, 0, row);
    }

    /// M52 step 4.2 — write the "matrix staging" cells on `row_idx`.
    ///
    /// Pearl's BLAKE3 chip loads matrix bytes via the staging buffer
    /// across multiple rows; each load-row sets `IS_MSG_MAT = 1` and
    /// publishes 8 i8 matrix bytes into `MAT_UNPACK` plus the u8
    /// view into `UINT8_DATA`. With `NOISE_UNPACK = 0`, the input
    /// chip's `NOISED_PACKED = polyval(MAT_UNPACK) + polyval(NOISE_UNPACK)`
    /// constraint collapses to plain-byte polyval. The row's
    /// `(MAT_ID, NOISED_PACKED)` becomes the canonical "plain" entry
    /// the noised_packed LogUp bus's BLAKE3-side query (step 4.1)
    /// self-references.
    ///
    /// Returns the 2 polyval-packed Goldilocks values written into
    /// `NOISED_PACKED[0..2]` — useful for cross-row consistency
    /// checks in tests.
    ///
    /// Constraints satisfied by this write (all are existing
    /// per-row chip constraints, no new AIR work in this helper):
    /// - anchored URange8 on UINT8_DATA[0] when IS_MSG_MAT=1
    /// - IRange8 on MAT_UNPACK[0..64] (signed bytes ∈ [-128, 127])
    /// - i8u8 bus: MAT_UNPACK[i] (i8) ↔ UINT8_DATA[i] (u8) when IS_MSG_MAT=1
    /// - Input chip: NOISED_PACKED[i] = polyval(MAT_UNPACK[..]) + polyval(NOISE_UNPACK[..])
    /// - noised_packed bus self-query (step 4.1): MAT_FREQ on this row balances the query
    ///
    /// On co-located compression rows, C3 binds every
    /// `BLAKE3_MSG[0..16]` word to `UINT8_DATA[0..64]` under
    /// `IS_MSG_MAT * IS_NEW_BLAKE`. Combined with the full-width
    /// i8/u8 bus, the bytes hashed by BLAKE3 and the bytes feeding
    /// the matrix store cannot split views across sub-slices.
    pub fn place_matrix_staging_row(
        &mut self,
        row_idx: usize,
        bytes: &[i8; 8],
        mat_id: u32,
    ) -> [u64; 2] {
        self.write_noised_row(row_idx, bytes, mat_id, true)
    }

    /// M-S1 (§4.C.11) — place ONE pure `noised_packed` **producer**
    /// (table) row: identical column writes to
    /// [`place_matrix_staging_row`] (MAT_UNPACK / UINT8_DATA /
    /// NOISE_UNPACK = 0 / NOISED_PACKED = polyval, satisfying the
    /// input chip) **but with `IS_MSG_MAT = 0`** — so it issues NO
    /// self-query and triggers NO BLAKE3-side / i8u8 / urange8
    /// interaction. It only *supplies* the multiset entry
    /// `(MAT_ID, NOISED_PACKED[0..2])` (table side is ungated:
    /// every row publishes ×`-MAT_FREQ`). `populate_lookup_freq`
    /// sets `MAT_FREQ` to the count of matmul A/B chunk queries
    /// that hit this key. This is the canonical store the §6(b)
    /// sweep's whole-micro-tile A/B input is bound to. (Crucially
    /// NOT `IS_MSG_MAT = 1`: that would assert the blake3-chip
    /// `BLAKE3_MSG == base256(UINT8_DATA)` relation on a
    /// non-compression row — see the C3 note in
    /// `place_matrix_staging_row`.)
    pub fn place_noised_store_row(
        &mut self,
        row_idx: usize,
        bytes: &[i8; 8],
        mat_id: u32,
    ) -> [u64; 2] {
        self.write_noised_row(row_idx, bytes, mat_id, false)
    }

    /// §4.C.2 / A3.2b — place a pure `noised_packed` producer row
    /// with an explicit `(plain, noise)` decomposition (W2): the
    /// store row carries `MAT_UNPACK = plain` (the committed-matrix
    /// bytes — B1/C3 ties these to `HASH_A`), `NOISE_UNPACK =
    /// noise` (the Pearl `noise_ref` bytes), and
    /// `NOISE_PACKED_PREP = polyval(noise, 129)` (the CRIT-1-pinned
    /// preprocessed noise; InputChip forces `NOISE_PACKED_PREP ==
    /// polyval(NOISE_UNPACK,129)` so the prover cannot deviate).
    /// `NOISED_PACKED = polyval(plain) + polyval(noise) = a′`
    /// (since `a′ = plain + noise` fits i8 with **no wrap**,
    /// `|A+E| ≤ 127`). Together with the caller-supplied `mat_id`,
    /// this publishes the position-keyed M-S1 store entry consumed by
    /// matmul reads. Returns the 2 packed `NOISED_PACKED` cells.
    pub fn place_noised_store_row_split(
        &mut self,
        row_idx: usize,
        plain: &[i8; 8],
        noise: &[i8; 8],
        mat_id: u32,
    ) -> [u64; 2] {
        self.write_noised_row_split(row_idx, plain, noise, mat_id, false)
    }

    /// Shared column-writer for the two `noised_packed` table-row
    /// helpers. `is_msg_mat` selects staging (BLAKE3 absorb /
    /// self-query, `true`) vs. pure producer (`false`).
    fn write_noised_row(
        &mut self,
        row_idx: usize,
        bytes: &[i8; 8],
        mat_id: u32,
        is_msg_mat: bool,
    ) -> [u64; 2] {
        // Single-byte (no-noise) path: NOISE_UNPACK=0 ⇒
        // NOISE_PACKED_PREP=polyval([0;8],129)=0 (== the baseline
        // default), NOISED_PACKED=polyval(bytes)+0 — bit-identical
        // to the pre-A3.2b behaviour.
        self.write_noised_row_split(row_idx, bytes, &[0i8; 8], mat_id, is_msg_mat)
    }

    /// §4.C.2 / A3.2b — the `(plain, noise)`-split column writer
    /// (W1/W2). `MAT_UNPACK = plain`, `UINT8_DATA = u8(plain)`,
    /// `NOISE_UNPACK = noise`, `NOISED_PACKED[cell] =
    /// polyval(plain[4c..],256) + polyval(noise[4c..],256)` (= a′,
    /// since `a′ = plain+noise` fits i8 with no wrap), and
    /// `NOISE_PACKED_PREP = polyval(noise[0..8],129)` so the
    /// InputChip's `NOISE_PACKED_PREP == polyval(NOISE_UNPACK,129)`
    /// holds and the CRIT-1 pin fixes the noise. `write_noised_row`
    /// is `noise = 0` (M-S1's prior behaviour, unchanged).
    fn write_noised_row_split(
        &mut self,
        row_idx: usize,
        plain: &[i8; 8],
        noise: &[i8; 8],
        mat_id: u32,
        is_msg_mat: bool,
    ) -> [u64; 2] {
        use p3_field::integers::QuotientMap;

        use crate::composite_layout::{
            MAT_UNPACK_START, NOISED_PACKED_START, NOISE_PACKED_PREP, NOISE_UNPACK_START,
            UINT8_DATA_START,
        };

        assert!(
            row_idx < self.height(),
            "row_idx {row_idx} out of bounds (height = {})",
            self.height()
        );
        let base = row_idx * TOTAL_TRACE_WIDTH;
        let row = &mut self.matrix.values[base..base + TOTAL_TRACE_WIDTH];

        // MAT_UNPACK: the committed-plain i8 bytes (B1/C3 ties
        // these to HASH_A); UINT8_DATA: their u8 view.
        for i in 0..plain.len() {
            row[MAT_UNPACK_START + i] = <Val as QuotientMap<i64>>::from_int(plain[i] as i64);
            row[UINT8_DATA_START + i] =
                <Val as QuotientMap<u64>>::from_int((plain[i] as u8) as u64);
        }
        // NOISE_UNPACK: the Pearl `noise_ref` bytes.
        for i in 0..noise.len() {
            row[NOISE_UNPACK_START + i] = <Val as QuotientMap<i64>>::from_int(noise[i] as i64);
        }
        // NOISE_PACKED_PREP = polyval(NOISE_UNPACK, base=129) — the
        // CRIT-1-pinned preprocessed noise (InputChip eqn 1).
        // |noise|≤63, 63·129^7 ≈ 2.9e16 ≪ i64::MAX & ≪ Goldilocks p.
        let mut npp: i64 = 0;
        let mut pw: i64 = 1;
        for &nb in noise.iter() {
            npp += (nb as i64) * pw;
            pw *= crate::chips::input::NOISE_PACKING_BASE as i64;
        }
        row[NOISE_PACKED_PREP] = <Val as QuotientMap<i64>>::from_int(npp);
        // NOISED_PACKED[cell] = polyval(plain[4c..],256)
        //                     + polyval(noise[4c..],256)  (= a′).
        let mut packs = [0i64; 2];
        for cell in 0..packs.len() {
            let mut acc: i64 = 0;
            let mut mult: i64 = 1;
            for j in 0..4 {
                acc += (plain[cell * 4 + j] as i64 + noise[cell * 4 + j] as i64) * mult;
                mult *= 256;
            }
            packs[cell] = acc;
            row[NOISED_PACKED_START + cell] = <Val as QuotientMap<i64>>::from_int(acc);
        }

        // Selectors + CONTROL_PREP + MAT_ID (single source of
        // truth; do NOT mix with place_blake3_hash_* on the same
        // row). IS_MSG_MAT at SELECTOR_COLS idx 10.
        let mut selectors = [false; 21];
        selectors[10] = is_msg_mat;
        ControlChip.fill_row(&selectors, mat_id, row);

        [packs[0] as u64, packs[1] as u64]
    }

    /// Place one jackpot step at `row_idx`. Updates slot
    /// `selected_slot` of the 16-slot state via rotate-XOR-13 with
    /// `x`. `state` is the 16-slot value visible on this row (the
    /// "old" value entering the step). Returns the post-step
    /// state.
    ///
    /// The caller is responsible for threading the resulting state
    /// into the next row's JACKPOT_MSG (see
    /// [`fill_jackpot_passthrough`](Self::fill_jackpot_passthrough)
    /// for the bulk-fill helper).
    pub fn place_jackpot_step(
        &mut self,
        row_idx: usize,
        state: &[u32; JACKPOT_SIZE],
        selected_slot: usize,
        x: u32,
        is_active: bool,
    ) -> [u32; JACKPOT_SIZE] {
        use p3_field::integers::QuotientMap;

        assert!(row_idx < self.height());
        assert!(selected_slot < JACKPOT_SIZE);

        let row_start = row_idx * TOTAL_TRACE_WIDTH;
        let row = &mut self.matrix.values[row_start..row_start + TOTAL_TRACE_WIDTH];

        // Selector + CONTROL_PREP. IS_HASH_JACKPOT is at selector
        // index 6 in SELECTOR_COLS.
        let mut selectors = [false; 21];
        selectors[6] = is_active;
        ControlChip.fill_row(&selectors, 0, row);

        // JACKPOT_MSG (16 slots).
        for i in 0..JACKPOT_SIZE {
            row[JACKPOT_MSG_START + i] = <Val as QuotientMap<u64>>::from_int(state[i] as u64);
        }

        // V_BITS == bit-decomp of state[selected_slot] when active;
        // else zeros.
        let v_bits = if is_active {
            bit_decompose_u32(state[selected_slot])
        } else {
            [0u32; 32]
        };
        for k in 0..32 {
            row[BIT_REG_START + k] = <Val as QuotientMap<u64>>::from_int(v_bits[k] as u64);
        }

        // X_BITS.
        let x_bits = if is_active {
            bit_decompose_u32(x)
        } else {
            [0u32; 32]
        };
        for k in 0..32 {
            row[JACKPOT_X_BITS_START + k] = <Val as QuotientMap<u64>>::from_int(x_bits[k] as u64);
        }

        // SLOT_SEL one-hot.
        let oh = if is_active {
            one_hot_select(selected_slot)
        } else {
            [0u32; JACKPOT_SIZE]
        };
        for i in 0..JACKPOT_SIZE {
            row[JACKPOT_SLOT_SEL_START + i] = <Val as QuotientMap<u64>>::from_int(oh[i] as u64);
        }

        // Compute and return the post-step state for the caller
        // to thread into the next row.
        apply_jackpot_step(state, selected_slot, x, is_active)
    }

    /// Bulk-fill JACKPOT_MSG on rows `[from_row, self.height())`
    /// with `state`. Required because the jackpot chip's
    /// cross-row constraint collapses to `nxt.JACKPOT_MSG[i] =
    /// cur.JACKPOT_MSG[i]` for every slot when both selectors are
    /// 0 (passthrough rows). Every passthrough row must thus hold
    /// the value the chain ended at.
    pub fn fill_jackpot_passthrough(&mut self, from_row: usize, state: &[u32; JACKPOT_SIZE]) {
        use p3_field::integers::QuotientMap;
        for r in from_row..self.height() {
            let base = r * TOTAL_TRACE_WIDTH;
            for i in 0..JACKPOT_SIZE {
                self.matrix.values[base + JACKPOT_MSG_START + i] =
                    <Val as QuotientMap<u64>>::from_int(state[i] as u64);
            }
        }
    }

    /// F1 (C4) — final jackpot-hash block:
    /// `HASH_JACKPOT = BLAKE3(JACKPOT_MSG, key = COMMITMENT_HASH)`.
    ///
    /// Mirrors Pearl's `structure_jackpot_blake`
    /// (`pearl_program.rs:195`): an 8-row keyed BLAKE3 compression
    /// (flags `KEYED_HASH|CHUNK_START|CHUNK_END|ROOT` = 0x1B),
    /// CV = `commitment_words` (= the caller-supplied jackpot key), message = the jackpot
    /// state.
    ///
    /// **Must be the trace's final 8 rows** (`row_start =
    /// height − 8`). Row 7 is then the last trace row, so the
    /// jackpot chip's cross-row `when_transition` is vacuous there
    /// — the only way an `IS_HASH_JACKPOT=1` row (forced to be a
    /// jackpot step by `Σ slot_sel == is_active`) can also be a
    /// clean BLAKE3 finalize emitting `HASH_JACKPOT` in `CV_OUT`.
    /// The row additionally carries a degenerate-but-valid jackpot
    /// step (slot 0, `V_BITS = bitdecomp(JACKPOT_MSG[0])`,
    /// `X_BITS = 0`).
    ///
    /// Relies on the `verify_round` leading-boundary gate fix
    /// (`2026-05-15_BLAKE3_CHIP_ROUND_GATE_BUG.md`) — before it, no
    /// non-row-0 blake block verified.
    ///
    /// `jackpot_state` must be all-zero in the current bridge: the
    /// preceding rows carry no jackpot activity, so the passthrough
    /// transition forces the state constant up to the last row.
    /// Threading the real tile-state fold is the remaining
    /// matmul→jackpot interleave; the C4 *binding* is fully
    /// exercised either way (`BLAKE3(zeros, key=jackpot_key)` is a genuine
    /// non-vacuous keyed digest). Returns the 8-word digest.
    pub fn place_jackpot_hash_block(
        &mut self,
        row_start: usize,
        jackpot_state: &[u32; JACKPOT_SIZE],
        commitment_words: &[u32; 8],
    ) -> [u32; 8] {
        use p3_field::integers::QuotientMap;

        assert_eq!(
            row_start + 8,
            self.height(),
            "jackpot-hash block must be the final 8 rows (row_start = height-8)"
        );

        let tweak = Blake3Tweak {
            counter_low: 0,
            counter_high: 0,
            block_len: 64,
            flags: 0x1B, // KEYED_HASH|CHUNK_START|CHUNK_END|ROOT
        };
        let msg: [u32; 16] = *jackpot_state;

        // 8-row keyed BLAKE3 block; row 7 (= last trace row) gets
        // IS_LAST_ROUND + IS_HASH_JACKPOT (selector idx 6).
        let digest =
            self.place_blake3_hash_with_selectors(row_start, &msg, commitment_words, &tweak, &[6]);

        // Co-write the degenerate jackpot step on row 7 (disjoint
        // columns from the blake3 chip; the unified selector set
        // {IS_LAST_ROUND, IS_HASH_JACKPOT} was written above).
        let r7 = row_start + 7;
        let base = r7 * TOTAL_TRACE_WIDTH;
        let row = &mut self.matrix.values[base..base + TOTAL_TRACE_WIDTH];
        for i in 0..JACKPOT_SIZE {
            row[JACKPOT_MSG_START + i] =
                <Val as QuotientMap<u64>>::from_int(jackpot_state[i] as u64);
        }
        let v_bits = bit_decompose_u32(jackpot_state[0]);
        for k in 0..32 {
            row[BIT_REG_START + k] = <Val as QuotientMap<u64>>::from_int(v_bits[k] as u64);
        }
        for k in 0..32 {
            row[JACKPOT_X_BITS_START + k] = Val::default();
        }
        let oh = one_hot_select(0);
        for i in 0..JACKPOT_SIZE {
            row[JACKPOT_SLOT_SEL_START + i] = <Val as QuotientMap<u64>>::from_int(oh[i] as u64);
        }

        digest
    }

    /// HIGH-2.2 §4.A: place the Pearl §4.5 rotl13-XOR fold chain
    /// for a real solved tile's per-stripe `x_steps` (from
    /// `ai-pow::matmul::compute_tile_trace`) into the FOLD_*
    /// composite block, starting at `row_start`. Row `row_start+t`
    /// folds `x_steps[t]` into slot `t % 16`; `FOLD_STATE` then
    /// propagates unchanged (FoldChip passthrough) so **every row
    /// from the chain's end through the last trace row carries the
    /// final `TileState M`** — which the §4.D keystone binds to
    /// the last-row `JACKPOT_MSG`. Returns the final `M` (16 u32)
    /// so the caller feeds it to `place_jackpot_hash_block` as the
    /// hashed message (⇒ `HASH_JACKPOT = BLAKE3(M, key=jackpot_key)`, the
    /// real PoW digest). Mirrors `chips::fold::build_trace` at
    /// composite offsets.
    pub fn place_fold_chain(&mut self, row_start: usize, x_steps: &[i32]) -> [u32; 16] {
        use p3_field::integers::QuotientMap;
        let len = x_steps.len();
        let h = self.height();
        assert!(
            !x_steps.is_empty() && row_start + len < h,
            "fold chain ({len}) must fit before the last row: row_start={row_start}, height={h}"
        );
        assert!(
            len <= STRIPE_MAX,
            "fold chain length {len} exceeds STRIPE_MAX={STRIPE_MAX} \
             (§6(b)-G2 per-stripe lanes); needs segmentation (G3)"
        );

        let set_u32 = |row: &mut [Val], at: usize, v: u32| {
            row[at] = <Val as QuotientMap<u64>>::from_int(v as u64);
        };
        let set_bits = |row: &mut [Val], at: usize, v: u32| {
            for i in 0..32 {
                row[at + i] = <Val as QuotientMap<u64>>::from_int(((v >> i) & 1) as u64);
            }
        };

        let mut m = [0u32; 16];
        for t in 0..len {
            let slot = t % 16;
            let x = x_steps[t] as u32;
            let base = (row_start + t) * TOTAL_TRACE_WIDTH;
            let row = &mut self.matrix.values[base..base + TOTAL_TRACE_WIDTH];
            // HIGH-2.2 §6 — pin this row's fold schedule into the
            // CRIT-1 CONTROL_PREP. Reset the control cells (selectors
            // = 0, MAT_ID = 0 — fold rows carry no other control
            // activity) then write the fold-extended pack so
            // `ControlChip`'s `CONTROL_PREP == polyval(.., is_fold,
            // slot)` holds and `extract_program` lifts the schedule
            // into the verifier-fixed canonical program.
            ControlChip.fill_row(&[false; crate::chips::control::NUM_SELECTORS], 0, row);
            row[CONTROL_PREP] =
                <Val as QuotientMap<u64>>::from_int(ControlChip::pack_control_prep_full(
                    &[false; crate::chips::control::NUM_SELECTORS],
                    0,
                    true,
                    slot as u8,
                    t as u8, // §6(b)-G2: the stripe index (= fold-row t)
                    0,       // §4.C.2 c-exact: fold rows are not C3-leaf rows
                ));
            row[FOLD_IS_FOLD] = <Val as QuotientMap<u64>>::from_int(1);
            row[FOLD_SLOT_SEL_START + slot] = <Val as QuotientMap<u64>>::from_int(1);
            // §6(b)-G2: per-fold-row stripe one-hot (the keystone's
            // SX_XR lane selector; its 6-bit index pinned above).
            row[FOLD_STRIPE_SEL_START + t] = <Val as QuotientMap<u64>>::from_int(1);
            set_u32(row, FOLD_XSTEP, x);
            set_bits(row, FOLD_XSTEP_BITS_START, x);
            for s in 0..16 {
                set_u32(row, FOLD_STATE_START + s, m[s]);
            }
            set_bits(row, FOLD_MCUR_BITS_START, m[slot]);
            let folded = m[slot].rotate_left(13) ^ x;
            set_u32(row, FOLD_XOR_OUT, folded);
            m[slot] = folded;
        }
        // Propagate the final state through every remaining row
        // (incl. the jackpot-hash block) so the last row — where
        // the §4.D keystone reads it — carries `M`.
        for r in (row_start + len)..h {
            let base = r * TOTAL_TRACE_WIDTH;
            let row = &mut self.matrix.values[base..base + TOTAL_TRACE_WIDTH];
            for s in 0..16 {
                set_u32(row, FOLD_STATE_START + s, m[s]);
            }
        }
        m
    }

    /// HIGH-2.2 §6(b) — place the co-located sub-block-major matmul
    /// sweep **and** the StripeXor reduction for one tile, from its
    /// extracted `a_prime` row-strips / `b_prime` col-strips (`t·k`
    /// i8 each, the `ai-pow::matmul::compute_tile_from_slices`
    /// layout). For each of the `(t/TILE_H)²` 2×2 sub-blocks it
    /// places `num_stripes` `place_matmul_step` rows as ONE threaded
    /// cumsum chain with `is_reset` only on each run's first row
    /// (the §6(b) spike GATE-2 row-ordering — chip-valid because the
    /// run-boundary carry is discarded by the matmul `(1−is_reset)`
    /// term), and co-locates a StripeXor active row that folds that
    /// step's accumulator-after-step (`SX_IN`, bound by
    /// `eval_composite` to `nxt.CUMSUM_TILE`) into lane = stripe
    /// index. `CUMSUM` and the StripeXor register are threaded to
    /// the trace end so the §6(b) keystone (`FOLD_XSTEP ==
    /// SX_XR[stripe]`) reads the final register on the fold rows.
    ///
    /// Returns `(rows_used, x_steps)` where `x_steps[step] =
    /// SX_XR[step] = ⊕` of the whole `t·t` accumulator after stripe
    /// `step`. Feed `&x_steps[..num_stripes]` to
    /// [`Self::place_fold_chain`] at a `row_start ≥ this
    /// row_start + rows_used`.
    #[allow(clippy::too_many_arguments)]
    pub fn place_useful_work_chain(
        &mut self,
        row_start: usize,
        a_prime_rows: &[i8],
        b_prime_cols: &[i8],
        t: usize,
        r: usize,
        num_stripes: usize,
    ) -> (usize, [u32; STRIPE_MAX]) {
        self.place_useful_work_chain_hw(row_start, a_prime_rows, b_prime_cols, t, t, r, num_stripes)
    }

    #[allow(clippy::too_many_arguments)]
    pub fn place_useful_work_chain_hw(
        &mut self,
        row_start: usize,
        a_prime_rows: &[i8],
        b_prime_cols: &[i8],
        h_tile: usize,
        w_tile: usize,
        r: usize,
        num_stripes: usize,
    ) -> (usize, [u32; STRIPE_MAX]) {
        // Default (contiguous tile): the opened row/col lane == tile-local index.
        let a_lanes: Vec<usize> = (0..h_tile).collect();
        let b_lanes: Vec<usize> = (0..w_tile).collect();
        self.place_useful_work_chain_hw_indexed(
            row_start,
            a_prime_rows,
            b_prime_cols,
            h_tile,
            w_tile,
            r,
            num_stripes,
            &a_lanes,
            &b_lanes,
        )
    }

    /// B5b: like [`place_useful_work_chain_hw`] but with explicit per-row/col
    /// **lanes** — the covering-range position (`opened_index − chunk_base`) of
    /// each opened row/column. For a contiguous tile `a_lanes[i] == i` so this is
    /// byte-identical; for a non-contiguous opened pattern the lanes carry the
    /// actual opened positions so the sweep's `noised_packed` keys match the
    /// strip-opening producer (which is keyed by the real matrix position).
    #[allow(clippy::too_many_arguments)]
    pub fn place_useful_work_chain_hw_indexed(
        &mut self,
        row_start: usize,
        a_prime_rows: &[i8],
        b_prime_cols: &[i8],
        h_tile: usize,
        w_tile: usize,
        r: usize,
        num_stripes: usize,
        a_lanes: &[usize],
        b_lanes: &[usize],
    ) -> (usize, [u32; STRIPE_MAX]) {
        use p3_field::integers::QuotientMap;
        assert_eq!(a_lanes.len(), h_tile, "a_lanes must have h_tile entries");
        assert_eq!(b_lanes.len(), w_tile, "b_lanes must have w_tile entries");
        assert!(
            h_tile % TILE_H == 0,
            "tile height must split into TILE_H sub-blocks"
        );
        assert!(
            w_tile % TILE_H == 0,
            "tile width must split into TILE_H sub-blocks"
        );
        assert!(
            num_stripes <= STRIPE_MAX,
            "num_stripes {num_stripes} > STRIPE_MAX={STRIPE_MAX} (§6(b)-G2); \
             larger needs segmentation (G3)"
        );
        // §6(b)-G1: an `r`-wide stripe dot is covered by
        // `C = ⌈r/TILE_D⌉` accumulating TILE_D-wide micro-steps.
        let chunks = r.div_ceil(TILE_D).max(1);
        let k = if h_tile == 0 {
            0
        } else {
            a_prime_rows.len() / h_tile
        };
        assert_eq!(a_prime_rows.len(), h_tile * k, "a_prime_rows must be h*k");
        assert_eq!(b_prime_cols.len(), w_tile * k, "b_prime_cols must be w*k");
        let n_sbi = h_tile / TILE_H;
        let n_sbj = w_tile / TILE_H;
        let a_id_base = NOISED_CHUNK_ID_BASE;
        let b_id_base = a_id_base + ((h_tile * k).div_ceil(8)) as u64;
        let trace_h = self.height();
        assert!(
            row_start + n_sbi * n_sbj * num_stripes * chunks < trace_h,
            "useful-work sweep must fit before the last row"
        );

        let set_bits = |row: &mut [Val], at: usize, v: u32| {
            for i in 0..32 {
                row[at + i] = <Val as QuotientMap<u64>>::from_int(((v >> i) & 1) as u64);
            }
        };

        let mut xr = [0u32; STRIPE_MAX];
        let mut carry = [0i32; CUMSUM_LEN];
        let mut row = row_start;
        for sbi in 0..n_sbi {
            for sbj in 0..n_sbj {
                for step in 0..num_stripes {
                    let lo = step * r;
                    for chunk in 0..chunks {
                        // §6(b)-G1: this chunk covers stripe lanes
                        // [c·TILE_D, min((c+1)·TILE_D, r)); the rest
                        // of the TILE_D-wide micro-dot is zero-pad.
                        let c0 = chunk * TILE_D;
                        let w = (r - c0).min(TILE_D);
                        let mut a_blk = [[0i8; TILE_D]; TILE_H];
                        let mut b_blk = [[0i8; TILE_D]; TILE_H];
                        for di in 0..TILE_H {
                            let rr = (sbi * TILE_H + di) * k + lo + c0;
                            a_blk[di][..w].copy_from_slice(&a_prime_rows[rr..rr + w]);
                        }
                        for dj in 0..TILE_H {
                            let cc = (sbj * TILE_H + dj) * k + lo + c0;
                            b_blk[dj][..w].copy_from_slice(&b_prime_cols[cc..cc + w]);
                        }
                        let ids_for = |side_a: bool, sb_base: usize| -> [u64; A_ID_LEN] {
                            let (lanes, id_base) = if side_a {
                                (a_lanes, a_id_base)
                            } else {
                                (b_lanes, b_id_base)
                            };
                            core::array::from_fn(|jc| {
                                let mut src = [None; 8];
                                for m in 0..8 {
                                    let f = jc * 8 + m;
                                    let (di, col) = (f / TILE_D, f % TILE_D);
                                    if col < w {
                                        // B5b: opened row/col lane (covering-range position).
                                        src[m] = Some((
                                            lanes[sb_base + di] as u32,
                                            (lo + c0 + col) as u32,
                                        ));
                                    }
                                }
                                noised_chunk_id(id_base, k, &src)
                            })
                        };
                        let a_ids = ids_for(true, sbi * TILE_H);
                        let b_ids = ids_for(false, sbj * TILE_H);
                        // is_reset only on the sub-block's very first
                        // micro-step; all chunks (within a stripe and
                        // across stripes) accumulate into the same
                        // c_blk cell via is_update (matches
                        // `compute_tile_trace`'s `c_blk[idx] += Σδ`).
                        let is_reset = step == 0 && chunk == 0;
                        let is_update = !is_reset;
                        let cumsum_new = self.place_matmul_step_with_ids(
                            row, &a_blk, &b_blk, &a_ids, &b_ids, is_reset, is_update, &carry,
                        );

                        let base = row * TOTAL_TRACE_WIDTH;
                        let rs = &mut self.matrix.values[base..base + TOTAL_TRACE_WIDTH];
                        // SX_XR register entering this row (every
                        // sweep row carries it for the StripeXor
                        // passthrough; only the stripe's *last* chunk
                        // is an active fold row).
                        for s in 0..STRIPE_MAX {
                            rs[SX_XR_START + s] = <Val as QuotientMap<u64>>::from_int(xr[s] as u64);
                        }
                        if chunk == chunks - 1 {
                            // Stripe complete ⇒ `cumsum_new` is the
                            // accumulator-after-stripe-`step`. Fold it
                            // into lane = `step`. `SX_IN` = cumsum_new
                            // (= nxt.CUMSUM_TILE — the matmul-chip-
                            // forced value the keystone binds).
                            rs[SX_IS_ACTIVE] = <Val as QuotientMap<u64>>::from_int(1);
                            rs[SX_LANE_SEL_START + step] = <Val as QuotientMap<u64>>::from_int(1);
                            let mut xin = 0u32;
                            for c in 0..CUMSUM_LEN {
                                let u = cumsum_new[c] as u32;
                                rs[SX_IN_START + c] =
                                    <Val as QuotientMap<i64>>::from_int(cumsum_new[c] as i64);
                                set_bits(rs, SX_IN_BITS_START + c * 32, u);
                                xin ^= u;
                            }
                            let sel_val = xr[step];
                            set_bits(rs, SX_XR_SEL_BITS_START, sel_val);
                            let new_sel = sel_val ^ xin;
                            rs[SX_NEW_SEL] = <Val as QuotientMap<u64>>::from_int(new_sel as u64);
                            set_bits(rs, SX_NEW_SEL_BITS_START, new_sel);
                            for i in 0..32 {
                                let mut col_sum: u32 = (sel_val >> i) & 1;
                                for c in 0..CUMSUM_LEN {
                                    col_sum += (cumsum_new[c] as u32 >> i) & 1;
                                }
                                let q = (col_sum - ((new_sel >> i) & 1)) / 2;
                                // 2026-05-21 width reduction: Q[i] ∈ {0,1,2}
                                // stored as one value column (was 2 bits).
                                rs[SX_Q_START + i] = <Val as QuotientMap<u64>>::from_int(q as u64);
                            }
                            xr[step] = new_sel;
                        }
                        carry = cumsum_new;
                        row += 1;
                    }
                }
            }
        }
        let rows_used = row - row_start;
        // CUMSUM passthrough (matmul recurrence collapses to
        // nxt==cur once both selectors are 0).
        self.fill_cumsum_passthrough(row, &carry);
        // StripeXor register passthrough: SX_IS_ACTIVE = 0 on every
        // post-sweep row ⇒ all STRIPE_MAX lanes pass through, so the
        // final register reaches the fold rows + the last row where
        // the §6(b) keystone reads it.
        for rr in row..trace_h {
            let base = rr * TOTAL_TRACE_WIDTH;
            let rs = &mut self.matrix.values[base..base + TOTAL_TRACE_WIDTH];
            for s in 0..STRIPE_MAX {
                rs[SX_XR_START + s] = <Val as QuotientMap<u64>>::from_int(xr[s] as u64);
            }
        }
        (rows_used, xr)
    }

    /// M-S1 (§4.C.11) — enumerate the **distinct** 8-i8 micro-tile
    /// chunks the [`place_useful_work_chain`] sweep consumes, in
    /// first-seen (deterministic) order.
    ///
    /// Mirrors the sweep's `a_blk`/`b_blk` construction
    /// **byte-for-byte** (same `(sbi,sbj,step,chunk)` nest, same
    /// `rr/cc/c0/w` index math, same zero-pad). Each
    /// `place_matmul_step` row writes `A_NOISED[c] = pack4(a_blk,c)`
    /// where flat lane `f = 4c+j` maps to `a_blk[f/TILE_D][f%TILE_D]`;
    /// the `noised_packed` bus query splits the `A_NOISED_LEN`
    /// packed cells into `A_NOISED_LEN/2` 2-cell chunks, i.e. the
    /// 8-i8 sub-vectors `a_blk_flat[8j .. 8j+8]`. The producer store
    /// must publish exactly these so every swept chunk is a
    /// multiset member (else LogUp rejects). De-duplicated to the
    /// working-set size (≪ `sweep_rows`); zero-pad chunks collapse
    /// to the single all-zero key.
    ///
    /// `(a, b)` chunks are interleaved per matmul row in sweep
    /// order so the store layout is itself deterministic and
    /// bit-identical across serial/parallel builds.
    pub fn enumerate_noised_chunks(
        a_prime_rows: &[i8],
        b_prime_cols: &[i8],
        t: usize,
        r: usize,
        num_stripes: usize,
    ) -> Vec<[i8; 8]> {
        Self::enumerate_noised_chunks_hw(a_prime_rows, b_prime_cols, t, t, r, num_stripes)
    }

    pub fn enumerate_noised_chunks_hw(
        a_prime_rows: &[i8],
        b_prime_cols: &[i8],
        h_tile: usize,
        w_tile: usize,
        r: usize,
        num_stripes: usize,
    ) -> Vec<[i8; 8]> {
        let chunks = r.div_ceil(TILE_D).max(1);
        let k = if h_tile == 0 {
            0
        } else {
            a_prime_rows.len() / h_tile
        };
        assert_eq!(a_prime_rows.len(), h_tile * k, "a_prime_rows must be h*k");
        assert_eq!(b_prime_cols.len(), w_tile * k, "b_prime_cols must be w*k");
        let n_sbi = h_tile / TILE_H;
        let n_sbj = w_tile / TILE_H;
        let n_chunk = A_NOISED_LEN / 2; // 2 packed cells = 8 i8 each

        let mut seen: hashbrown::HashSet<[i8; 8]> = hashbrown::HashSet::new();
        let mut out: Vec<[i8; 8]> = Vec::new();
        let push = |blk: &[[i8; TILE_D]; TILE_H],
                    seen: &mut hashbrown::HashSet<[i8; 8]>,
                    out: &mut Vec<[i8; 8]>| {
            for jc in 0..n_chunk {
                let mut bytes = [0i8; 8];
                for (m, slot) in bytes.iter_mut().enumerate() {
                    let f = jc * 8 + m;
                    *slot = blk[f / TILE_D][f % TILE_D];
                }
                if seen.insert(bytes) {
                    out.push(bytes);
                }
            }
        };

        for sbi in 0..n_sbi {
            for sbj in 0..n_sbj {
                for step in 0..num_stripes {
                    let lo = step * r;
                    for chunk in 0..chunks {
                        let c0 = chunk * TILE_D;
                        let w = (r - c0).min(TILE_D);
                        let mut a_blk = [[0i8; TILE_D]; TILE_H];
                        let mut b_blk = [[0i8; TILE_D]; TILE_H];
                        for di in 0..TILE_H {
                            let rr = (sbi * TILE_H + di) * k + lo + c0;
                            a_blk[di][..w].copy_from_slice(&a_prime_rows[rr..rr + w]);
                        }
                        for dj in 0..TILE_H {
                            let cc = (sbj * TILE_H + dj) * k + lo + c0;
                            b_blk[dj][..w].copy_from_slice(&b_prime_cols[cc..cc + w]);
                        }
                        push(&a_blk, &mut seen, &mut out);
                        push(&b_blk, &mut seen, &mut out);
                    }
                }
            }
        }
        out
    }

    /// §4.C.2 / A3.1 — like [`enumerate_noised_chunks`] but also
    /// returns, per distinct chunk, **which tile-strip position
    /// each byte came from** (`side` = A/B; `src[m] =
    /// Some((lane, l))` where `lane` = row-in-tile for A /
    /// col-in-tile for B and `l` = the `k`-column, or `None` for
    /// a zero-pad byte). This is the *deterministic
    /// verifier-recomputable* map a store row needs to know its
    /// `(plain, noise)` decomposition: `plain = committed[lane@tile,l]`,
    /// `noise = noise_ref::{e,f}_value(s, ·)`, and
    /// `chunk[m] == plain + noise` (Pearl `a′ = A + E`). De-dup is
    /// by chunk *value*, recording the **first** source — sound,
    /// because identical `a′` bytes contribute identically to the
    /// dot regardless of origin (§4.C.2 binds the multiset of
    /// swept values ⊆ `noise(committed)`).
    pub fn enumerate_noised_chunks_with_src(
        a_prime_rows: &[i8],
        b_prime_cols: &[i8],
        t: usize,
        r: usize,
        num_stripes: usize,
    ) -> Vec<NoisedChunkSrc> {
        Self::enumerate_noised_chunks_with_src_hw(a_prime_rows, b_prime_cols, t, t, r, num_stripes)
    }

    pub fn enumerate_noised_chunks_with_src_hw(
        a_prime_rows: &[i8],
        b_prime_cols: &[i8],
        h_tile: usize,
        w_tile: usize,
        r: usize,
        num_stripes: usize,
    ) -> Vec<NoisedChunkSrc> {
        let chunks = r.div_ceil(TILE_D).max(1);
        let k = if h_tile == 0 {
            0
        } else {
            a_prime_rows.len() / h_tile
        };
        assert_eq!(a_prime_rows.len(), h_tile * k, "a_prime_rows must be h*k");
        assert_eq!(b_prime_cols.len(), w_tile * k, "b_prime_cols must be w*k");
        let n_sbi = h_tile / TILE_H;
        let n_sbj = w_tile / TILE_H;
        let n_chunk = A_NOISED_LEN / 2;

        let mut seen: hashbrown::HashSet<[i8; 8]> = hashbrown::HashSet::new();
        let mut out: Vec<NoisedChunkSrc> = Vec::new();
        // `lane_base` = sub-block-major row/col-in-tile of blk[*][0];
        // `col0 = lo + c0`; width `w`. side: true=A,false=B.
        let push = |blk: &[[i8; TILE_D]; TILE_H],
                    side_a: bool,
                    lane_base: usize,
                    col0: usize,
                    w: usize,
                    seen: &mut hashbrown::HashSet<[i8; 8]>,
                    out: &mut Vec<NoisedChunkSrc>| {
            for jc in 0..n_chunk {
                let mut bytes = [0i8; 8];
                let mut src = [None; 8];
                for m in 0..8 {
                    let f = jc * 8 + m;
                    let di = f / TILE_D;
                    let col = f % TILE_D;
                    bytes[m] = blk[di][col];
                    if col < w {
                        src[m] = Some(((lane_base + di) as u32, (col0 + col) as u32));
                    }
                }
                if seen.insert(bytes) {
                    out.push(NoisedChunkSrc { bytes, side_a, src });
                }
            }
        };

        for sbi in 0..n_sbi {
            for sbj in 0..n_sbj {
                for step in 0..num_stripes {
                    let lo = step * r;
                    for chunk in 0..chunks {
                        let c0 = chunk * TILE_D;
                        let w = (r - c0).min(TILE_D);
                        let mut a_blk = [[0i8; TILE_D]; TILE_H];
                        let mut b_blk = [[0i8; TILE_D]; TILE_H];
                        for di in 0..TILE_H {
                            let rr = (sbi * TILE_H + di) * k + lo + c0;
                            a_blk[di][..w].copy_from_slice(&a_prime_rows[rr..rr + w]);
                        }
                        for dj in 0..TILE_H {
                            let cc = (sbj * TILE_H + dj) * k + lo + c0;
                            b_blk[dj][..w].copy_from_slice(&b_prime_cols[cc..cc + w]);
                        }
                        push(&a_blk, true, sbi * TILE_H, lo + c0, w, &mut seen, &mut out);
                        push(&b_blk, false, sbj * TILE_H, lo + c0, w, &mut seen, &mut out);
                    }
                }
            }
        }
        out
    }

    /// §4.C.2 / A3.2a — the **position-addressed, params-
    /// deterministic** store layout: one [`NoisedChunkSrc`] per
    /// sweep-read position, in sub-block-major schedule order,
    /// **with no value de-duplication**. Unlike
    /// [`enumerate_noised_chunks_with_src`] (value-deduped, so
    /// the row↔`(i,l)` map needs the witness), the `(side_a, src)`
    /// layout here is a **pure function of `(t, r, num_stripes,
    /// k)`** — the byte *values* depend on `a′/b′` but the
    /// *positions* do not — so the CRIT-1 program rebuild can
    /// reconstruct each store row's `(i,l)` (hence its
    /// `noise_ref` noise / `NOISE_PACKED_PREP`) **witness-free**.
    /// This is what unblocks W1 (verifier-pinned noise) for
    /// §4.C.2. M-S1's LogUp still balances with the resulting
    /// duplicate producer rows (`populate_lookup_freq` routes a
    /// key's freq to its first row; dupes carry `MAT_FREQ=0`) —
    /// the dedup was only a row-count optimization.
    pub fn enumerate_noised_chunks_positioned(
        a_prime_rows: &[i8],
        b_prime_cols: &[i8],
        t: usize,
        r: usize,
        num_stripes: usize,
    ) -> Vec<NoisedChunkSrc> {
        Self::enumerate_noised_chunks_positioned_hw(
            a_prime_rows, b_prime_cols, t, t, r, num_stripes,
        )
    }

    pub fn enumerate_noised_chunks_positioned_hw(
        a_prime_rows: &[i8],
        b_prime_cols: &[i8],
        h_tile: usize,
        w_tile: usize,
        r: usize,
        num_stripes: usize,
    ) -> Vec<NoisedChunkSrc> {
        let chunks = r.div_ceil(TILE_D).max(1);
        let k = if h_tile == 0 {
            0
        } else {
            a_prime_rows.len() / h_tile
        };
        assert_eq!(a_prime_rows.len(), h_tile * k, "a_prime_rows must be h*k");
        assert_eq!(b_prime_cols.len(), w_tile * k, "b_prime_cols must be w*k");
        let n_sbi = h_tile / TILE_H;
        let n_sbj = w_tile / TILE_H;
        let n_chunk = A_NOISED_LEN / 2;
        let mut out: Vec<NoisedChunkSrc> = Vec::new();
        let push = |blk: &[[i8; TILE_D]; TILE_H],
                    side_a: bool,
                    lane_base: usize,
                    col0: usize,
                    w: usize,
                    out: &mut Vec<NoisedChunkSrc>| {
            for jc in 0..n_chunk {
                let mut bytes = [0i8; 8];
                let mut src = [None; 8];
                for m in 0..8 {
                    let f = jc * 8 + m;
                    let (di, col) = (f / TILE_D, f % TILE_D);
                    bytes[m] = blk[di][col];
                    if col < w {
                        src[m] = Some(((lane_base + di) as u32, (col0 + col) as u32));
                    }
                }
                out.push(NoisedChunkSrc { bytes, side_a, src });
            }
        };
        for sbi in 0..n_sbi {
            for sbj in 0..n_sbj {
                for step in 0..num_stripes {
                    let lo = step * r;
                    for chunk in 0..chunks {
                        let c0 = chunk * TILE_D;
                        let w = (r - c0).min(TILE_D);
                        let mut a_blk = [[0i8; TILE_D]; TILE_H];
                        let mut b_blk = [[0i8; TILE_D]; TILE_H];
                        for di in 0..TILE_H {
                            let rr = (sbi * TILE_H + di) * k + lo + c0;
                            a_blk[di][..w].copy_from_slice(&a_prime_rows[rr..rr + w]);
                        }
                        for dj in 0..TILE_H {
                            let cc = (sbj * TILE_H + dj) * k + lo + c0;
                            b_blk[dj][..w].copy_from_slice(&b_prime_cols[cc..cc + w]);
                        }
                        push(&a_blk, true, sbi * TILE_H, lo + c0, w, &mut out);
                        push(&b_blk, false, sbj * TILE_H, lo + c0, w, &mut out);
                    }
                }
            }
        }
        out
    }

    /// §4.C.2 / A3.2a — the verifier-recomputable store **layout
    /// skeleton**: the `(side_a, src)` of every positioned store
    /// row, as a **pure function of `(t, r, num_stripes, k)`**
    /// (no `a′/b′` values). The CRIT-1 program rebuild calls this
    /// (with `k` from public params) to reconstruct each store
    /// row's `(i,l)` — hence its pinned `NOISE_PACKED_PREP` via
    /// `noise_ref` — **witness-free**. Identical ordering to
    /// [`enumerate_noised_chunks_positioned`]; only `bytes` are
    /// omitted (they are the witness).
    pub fn noised_store_layout(
        t: usize,
        r: usize,
        num_stripes: usize,
        k: usize,
    ) -> Vec<(bool, [Option<(u32, u32)>; 8])> {
        Self::noised_store_layout_hw(t, t, r, num_stripes, k)
    }

    pub fn noised_store_layout_hw(
        h_tile: usize,
        w_tile: usize,
        r: usize,
        num_stripes: usize,
        k: usize,
    ) -> Vec<(bool, [Option<(u32, u32)>; 8])> {
        let a_zeros = vec![0i8; h_tile * k];
        let b_zeros = vec![0i8; w_tile * k];
        Self::enumerate_noised_chunks_positioned_hw(
            &a_zeros, &b_zeros, h_tile, w_tile, r, num_stripes,
        )
        .into_iter()
        .map(|c| (c.side_a, c.src))
        .collect()
    }

    /// M-S1 (§4.C.11) — place the `noised_packed` producer store:
    /// one pure table row ([`place_noised_store_row`], `MAT_ID =
    /// mat_id`, no `IS_MSG_MAT`) per caller-supplied chunk. Modern
    /// production traces use exact per-sub-slice IDs; this helper is
    /// retained for focused fixtures where every supplied chunk shares
    /// the same caller-assigned ID. Call AFTER `place_useful_work_chain`
    /// (so the SX/CUMSUM passthrough on `[row_start, h)` is already
    /// written — this only adds the disjoint MAT_UNPACK /
    /// NOISED_PACKED / CONTROL columns on top) and BEFORE the fold
    /// chain (store rows must not overlap it). Returns the number of
    /// store rows placed (= distinct chunk count). `MAT_FREQ` is set
    /// later by [`Self::populate_lookup_freq`].
    pub fn place_noised_store(
        &mut self,
        row_start: usize,
        chunks: &[[i8; 8]],
        mat_id: u32,
    ) -> usize {
        assert!(
            row_start + chunks.len() < self.height(),
            "noised store [{row_start}, {}+{}) must fit before the last row \
             (height {})",
            row_start,
            chunks.len(),
            self.height()
        );
        for (i, c) in chunks.iter().enumerate() {
            self.place_noised_store_row(row_start + i, c, mat_id);
        }
        chunks.len()
    }

    /// Populate every `*_FREQ` column in the trace so the LogUp
    /// argument balances when proven via
    /// [`crate::composite_full_air_with_lookups::CompositeFullAirWithLookups`].
    ///
    /// Each lookup bus has a fixed set of "query cells" — trace
    /// cells whose value is checked against a range/conversion
    /// table. This routine scans every row, counts how many times
    /// each table value is queried, and writes the count into the
    /// corresponding `*_FREQ` column at the row where the table
    /// holds that value.
    ///
    /// The current implementation handles four range buses:
    ///   * `urange8` — anchored UINT8_DATA[0] query; remaining
    ///     UINT8_DATA validity is implied by `irange8(MAT_UNPACK)` +
    ///     `i8u8`.
    ///   * `urange13` — MAT_ID_LIMBS[0..2] + AB_ID_LIMBS[0..4]
    ///     unconditionally.
    ///   * `irange7p1` — NOISE_UNPACK[0..64] unconditionally.
    ///   * `irange8` — A_NOISED_UNPACK[0..32] +
    ///     B_NOISED_UNPACK[0..32] + MAT_UNPACK[0..64]
    ///     unconditionally.
    ///
    /// Call this after constructing a trace (baseline + any
    /// instruction placements) and BEFORE proving via
    /// `prove_batch`. The LogUp constraints will reject any trace
    /// where a query cell holds an out-of-range value, regardless
    /// of how `*_FREQ` is set.
    pub fn populate_lookup_freq(&mut self) {
        use p3_field::integers::QuotientMap;
        use p3_field::PrimeField64;

        let n = self.height();

        // ---- URange8 (u8 ∈ [0, 256)) ----
        // One anchored query on UINT8_DATA[0] when IS_MSG_MAT=1. The
        // other 63 bytes are checked by IRANGE8(MAT_UNPACK) + I8U8,
        // so repeating u8 range queries for them only widens the
        // permutation trace.
        let mut u8_count = [0u64; 256];
        for r in 0..n {
            let base = r * TOTAL_TRACE_WIDTH;
            let is_msg_mat = self.matrix.values[base + IS_MSG_MAT].as_canonical_u64();
            if is_msg_mat == 1 {
                let v = self.matrix.values[base + UINT8_DATA_START].as_canonical_u64();
                if (v as usize) < 256 {
                    u8_count[v as usize] += 1;
                }
            }
        }
        for v in 0..256.min(n) {
            self.matrix.values[v * TOTAL_TRACE_WIDTH + URANGE8_FREQ] =
                <Val as QuotientMap<u64>>::from_int(u8_count[v]);
        }
        for v in 256.min(n)..n {
            self.matrix.values[v * TOTAL_TRACE_WIDTH + URANGE8_FREQ] = Val::default();
        }

        // ---- URange13 (u13 ∈ [0, 8192)) ----
        // Queries: MAT_ID_LIMBS[0..2] + AB_ID_LIMBS[0..4] every row.
        let mut u13_count = vec![0u64; 8192];
        for r in 0..n {
            let base = r * TOTAL_TRACE_WIDTH;
            for i in 0..MAT_ID_LIMBS_LEN {
                let v = self.matrix.values[base + MAT_ID_LIMBS_START + i].as_canonical_u64();
                if (v as usize) < 8192 {
                    u13_count[v as usize] += 1;
                }
            }
            for i in 0..AB_ID_LIMBS_LEN {
                let v = self.matrix.values[base + AB_ID_LIMBS_START + i].as_canonical_u64();
                if (v as usize) < 8192 {
                    u13_count[v as usize] += 1;
                }
            }
        }
        for v in 0..8192.min(n) {
            self.matrix.values[v * TOTAL_TRACE_WIDTH + URANGE13_FREQ] =
                <Val as QuotientMap<u64>>::from_int(u13_count[v]);
        }
        for v in 8192.min(n)..n {
            self.matrix.values[v * TOTAL_TRACE_WIDTH + URANGE13_FREQ] = Val::default();
        }

        // ---- IRange7P1 (i7+1 ∈ [-64, 64], 129 values) ----
        // Queries: NOISE_UNPACK[0..64] every row.
        // Map signed value v → table-row index (v + 64).
        let mut i7p1_count = [0u64; 129];
        for r in 0..n {
            let base = r * TOTAL_TRACE_WIDTH;
            for i in 0..NOISE_UNPACK_WIN {
                let raw = self.matrix.values[base + NOISE_UNPACK_START + i].as_canonical_u64();
                let signed = goldilocks_to_signed(raw);
                if (-64..=64).contains(&signed) {
                    i7p1_count[(signed + 64) as usize] += 1;
                }
            }
        }
        for v in 0..129.min(n) {
            self.matrix.values[v * TOTAL_TRACE_WIDTH + IRANGE7P1_FREQ] =
                <Val as QuotientMap<u64>>::from_int(i7p1_count[v]);
        }
        for v in 129.min(n)..n {
            self.matrix.values[v * TOTAL_TRACE_WIDTH + IRANGE7P1_FREQ] = Val::default();
        }

        // ---- IRange8 (i8 ∈ [-128, 127], 256 values) ----
        // Queries: A_NOISED_UNPACK[0..32] + B_NOISED_UNPACK[0..32]
        // + MAT_UNPACK[0..64] every row.
        let mut i8_count = [0u64; 256];
        let scan_i8_cells = |start: usize, len: usize, dst: &mut [u64; 256], values: &[Val]| {
            for r in 0..n {
                let base = r * TOTAL_TRACE_WIDTH;
                for i in 0..len {
                    let raw = values[base + start + i].as_canonical_u64();
                    let signed = goldilocks_to_signed(raw);
                    if (-128..=127).contains(&signed) {
                        dst[(signed + 128) as usize] += 1;
                    }
                }
            }
        };
        scan_i8_cells(
            A_NOISED_UNPACK_START, A_NOISED_UNPACK_LEN, &mut i8_count, &self.matrix.values,
        );
        scan_i8_cells(
            B_NOISED_UNPACK_START, B_NOISED_UNPACK_LEN, &mut i8_count, &self.matrix.values,
        );
        scan_i8_cells(
            MAT_UNPACK_START, MAT_UNPACK_WIN, &mut i8_count, &self.matrix.values,
        );
        for v in 0..256.min(n) {
            self.matrix.values[v * TOTAL_TRACE_WIDTH + IRANGE8_FREQ] =
                <Val as QuotientMap<u64>>::from_int(i8_count[v]);
        }
        for v in 256.min(n)..n {
            self.matrix.values[v * TOTAL_TRACE_WIDTH + IRANGE8_FREQ] = Val::default();
        }

        // ---- I8U8 paired i8↔u8 bus ----
        // Queries (gated by IS_MSG_MAT): for i in 0..MIN(MAT_UNPACK, UINT8_DATA),
        // pack = signed*256 + unsigned. Table entries enumerate
        // (signed, signed.rem_euclid(256)) for signed ∈ [-128, 127].
        // The table row for a valid pair is at row (signed + 128).
        let mut i8u8_count = vec![0u64; 256];
        let pair_len = MAT_UNPACK_WIN.min(UINT8_DATA_WIN);
        for r in 0..n {
            let base = r * TOTAL_TRACE_WIDTH;
            let is_msg_mat = self.matrix.values[base + IS_MSG_MAT].as_canonical_u64();
            if is_msg_mat != 1 {
                continue;
            }
            for i in 0..pair_len {
                let signed_raw = self.matrix.values[base + MAT_UNPACK_START + i].as_canonical_u64();
                let signed = goldilocks_to_signed(signed_raw);
                let unsigned = self.matrix.values[base + UINT8_DATA_START + i].as_canonical_u64();
                // The valid pair condition: unsigned == signed.rem_euclid(256).
                let expected_unsigned = (signed.rem_euclid(256)) as u64;
                if (-128..=127).contains(&signed) && unsigned == expected_unsigned {
                    let row_idx = (signed + 128) as usize;
                    i8u8_count[row_idx] += 1;
                }
                // Inconsistent (i8, u8) pairs (e.g. signed=42,
                // unsigned=43) have a pack value that's not in
                // the table at all. LogUp catches them at proof
                // time.
            }
        }
        for v in 0..256.min(n) {
            self.matrix.values[v * TOTAL_TRACE_WIDTH + I8U8_FREQ] =
                <Val as QuotientMap<u64>>::from_int(i8u8_count[v]);
        }
        for v in 256.min(n)..n {
            self.matrix.values[v * TOTAL_TRACE_WIDTH + I8U8_FREQ] = Val::default();
        }

        // ---- NOISED_PACKED RAM-lookup bus ----
        //
        // Table side: row r emits key (MAT_ID[r], NOISED_PACKED[r..r+2]).
        // Query side: matmul-active row emits keys
        //   (A_ID[r], A_NOISED[r..r+2])
        //   (B_ID[r], B_NOISED[r..r+2])
        // We walk all matmul-active rows, find matching table
        // rows by key, and increment MAT_FREQ.
        //
        // Strategy: build a hashmap (Vec<Val>, row_idx) → first
        // table row index. For each query, look up the key; if
        // present, increment MAT_FREQ at that row. Multiple table
        // rows may share the same key — we route the multiplicity
        // to the FIRST such row (any choice would work for LogUp
        // balance as long as the sum across all matching rows
        // equals the query count, but a single-row assignment
        // simplifies the trace generator).
        // §4.C.2 c-exact (cx.2-bus/X1): per-(row, sub-slice)
        // accounting — the producer publishes 8 sub-slice keys
        // per row (key s = (MAT_ID, NOISED_PACKED[2s..2s+2]) ×
        // −MAT_FREQ[s]); a query's freq routes to the FIRST
        // (row, sub-slice) carrying that key value. ZERO-BLAST:
        // on current traces sub-slice 0 = the M-S1 store value
        // (unchanged routing) and sub-slices 1..7 = (MAT_ID,0,0)
        // ⇒ MAT_FREQ[1..8]=0 / ×0 no-ops; the all-zero key may
        // route to an earlier (row, s) but LogUp balance is
        // routing-invariant (Σ producer −freq + Σ query +1 = 0
        // per key for any single-(row,s) assignment).
        let fl = crate::composite_layout::MAT_FREQ_LEN; // 8
        let mut mat_freq = vec![0u64; n * fl];
        let mut key_to_first_row: hashbrown::HashMap<(u64, u64, u64), (usize, usize)> =
            hashbrown::HashMap::new();
        for r in 0..n {
            let base = r * TOTAL_TRACE_WIDTH;
            let mat_id = self.matrix.values[base + MAT_ID].as_canonical_u64();
            let is_msg_mat = self.matrix.values[base + IS_MSG_MAT].as_canonical_u64();
            for s in 0..fl {
                if s > 0 && is_msg_mat != 1 {
                    continue;
                }
                let key = (
                    mat_id + s as u64,
                    self.matrix.values[base + NOISED_PACKED_START + 2 * s].as_canonical_u64(),
                    self.matrix.values[base + NOISED_PACKED_START + 2 * s + 1].as_canonical_u64(),
                );
                key_to_first_row.entry(key).or_insert((r, s));
            }
        }
        for r in 0..n {
            let base = r * TOTAL_TRACE_WIDTH;
            let is_reset = self.matrix.values[base + IS_RESET_CUMSUM].as_canonical_u64();
            let is_update = self.matrix.values[base + IS_UPDATE_CUMSUM].as_canonical_u64();
            let active = is_reset + is_update;
            if active > 0 {
                // M-S1 (§4.C.11) — one query per 2-cell chunk so
                // the WHOLE micro-tile A/B input is bound (mirrors
                // the `bus_emit::noised_packed` chunked emission).
                for j in 0..A_ID_LEN {
                    let a_id = self.matrix.values[base + A_ID + j].as_canonical_u64();
                    let a_key = (
                        a_id,
                        self.matrix.values[base + A_NOISED_START + 2 * j].as_canonical_u64(),
                        self.matrix.values[base + A_NOISED_START + 2 * j + 1].as_canonical_u64(),
                    );
                    if let Some(&(tr, ts)) = key_to_first_row.get(&a_key) {
                        mat_freq[tr * fl + ts] += active;
                    }
                }
                for j in 0..B_ID_LEN {
                    let b_id = self.matrix.values[base + B_ID + j].as_canonical_u64();
                    let b_key = (
                        b_id,
                        self.matrix.values[base + B_NOISED_START + 2 * j].as_canonical_u64(),
                        self.matrix.values[base + B_NOISED_START + 2 * j + 1].as_canonical_u64(),
                    );
                    if let Some(&(tr, ts)) = key_to_first_row.get(&b_key) {
                        mat_freq[tr * fl + ts] += active;
                    }
                }
                // Queries with no matching table key contribute
                // nothing to MAT_FREQ → bus is unbalanced → LogUp
                // rejects at proof time.
            }

            // M52 step 4-B / cx.2: BLAKE3-side self-queries
            // (gated by IS_MSG_MAT). The row's own eight
            // sub-slice keys are self-referential.
            let is_msg_mat = self.matrix.values[base + IS_MSG_MAT].as_canonical_u64();
            if is_msg_mat == 1 {
                for s in 0..fl {
                    let key = (
                        self.matrix.values[base + MAT_ID].as_canonical_u64() + s as u64,
                        self.matrix.values[base + NOISED_PACKED_START + 2 * s].as_canonical_u64(),
                        self.matrix.values[base + NOISED_PACKED_START + 2 * s + 1]
                            .as_canonical_u64(),
                    );
                    if let Some(&(tr, ts)) = key_to_first_row.get(&key) {
                        mat_freq[tr * fl + ts] += 1;
                    }
                }
            }
        }
        for r in 0..n {
            for s in 0..fl {
                self.matrix.values[r * TOTAL_TRACE_WIDTH + MAT_FREQ + s] =
                    <Val as QuotientMap<u64>>::from_int(mat_freq[r * fl + s]);
            }
        }

        // ---- CV_ROUTING bus ----
        //
        // Table key: (STARK_ROW_IDX[r], CV_OUT[r][0..8]).
        // Each row publishes one entry. Queries from rows with
        // IS_CV_IN=1 emit (CV_OR_TWEAK_PREP[r], CV_IN[r][0..8]).
        let mut cv_freq = vec![0u64; n];
        let mut cv_key_to_first_row: hashbrown::HashMap<Vec<u64>, usize> =
            hashbrown::HashMap::new();
        for r in 0..n {
            let base = r * TOTAL_TRACE_WIDTH;
            let mut key = Vec::with_capacity(1 + CV_OUT_LEN);
            key.push(self.matrix.values[base + STARK_ROW_IDX].as_canonical_u64());
            for i in 0..CV_OUT_LEN {
                key.push(self.matrix.values[base + CV_OUT_START + i].as_canonical_u64());
            }
            cv_key_to_first_row.entry(key).or_insert(r);
        }
        for r in 0..n {
            let base = r * TOTAL_TRACE_WIDTH;
            let is_cv_in = self.matrix.values[base + IS_CV_IN].as_canonical_u64();
            if is_cv_in == 0 {
                continue;
            }
            let mut query = Vec::with_capacity(1 + CV_IN_LEN);
            query.push(self.matrix.values[base + CV_OR_TWEAK_PREP].as_canonical_u64());
            for i in 0..CV_IN_LEN {
                query.push(self.matrix.values[base + CV_IN_START + i].as_canonical_u64());
            }
            if let Some(&tr) = cv_key_to_first_row.get(&query) {
                cv_freq[tr] += is_cv_in;
            }
            // No-match queries → unbalanced bus → LogUp rejects.
        }
        for r in 0..n {
            self.matrix.values[r * TOTAL_TRACE_WIDTH + CV_OUT_FREQ] =
                <Val as QuotientMap<u64>>::from_int(cv_freq[r]);
        }
    }

    /// Bulk-fill CUMSUM_TILE on rows `[from_row, self.height())`
    /// with `cumsum`. After a matmul-step chain ends at some
    /// intermediate row, the remaining rows are passthrough
    /// (selectors all 0) and the AIR's cross-row equation collapses
    /// to `nxt.CUMSUM = cur.CUMSUM`. So every subsequent row must
    /// hold the same cumsum value.
    ///
    /// `when_transition()` silences the wraparound constraint at
    /// the very last row, so the trace doesn't need to "close the
    /// loop" — the last row's cumsum doesn't have to equal row 0's.
    pub fn fill_cumsum_passthrough(&mut self, from_row: usize, cumsum: &[i32; CUMSUM_LEN]) {
        for r in from_row..self.height() {
            self.set_cumsum_row(r, cumsum);
        }
    }
}

#[cfg(test)]
mod tests {
    use p3_uni_stark::{prove, verify};

    use super::*;
    use crate::circuit::{build_stark_config, AiPowStarkConfig, CircuitConfig};
    use crate::composite_full_air::CompositeFullAir;
    use crate::composite_layout::MIN_STARK_LEN;
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

    fn expected_pattern_x_steps(
        a_prime_rows: &[i8],
        b_prime_cols: &[i8],
        h: usize,
        w: usize,
        k: usize,
        r: usize,
        num_stripes: usize,
    ) -> Vec<u32> {
        let mut accum = vec![0i32; h * w];
        let mut out = Vec::with_capacity(num_stripes);
        for step in 0..num_stripes {
            let lo = step * r;
            for u in 0..h {
                let a_row = &a_prime_rows[u * k + lo..u * k + lo + r];
                for v in 0..w {
                    let b_col = &b_prime_cols[v * k + lo..v * k + lo + r];
                    let mut delta = 0i32;
                    for l in 0..r {
                        delta = delta.wrapping_add((a_row[l] as i32) * (b_col[l] as i32));
                    }
                    let idx = u * w + v;
                    accum[idx] = accum[idx].wrapping_add(delta);
                }
            }
            out.push(accum.iter().fold(0i32, |acc, &value| acc ^ value) as u32);
        }
        out
    }

    #[test]
    fn baseline_trace_has_correct_shape() {
        let trace = CompositeTrace::baseline(MIN_STARK_LEN);
        assert_eq!(trace.height(), MIN_STARK_LEN);
        assert_eq!(trace.width(), TOTAL_TRACE_WIDTH);
        assert_eq!(trace.matrix.values.len(), MIN_STARK_LEN * TOTAL_TRACE_WIDTH);
    }

    #[test]
    fn baseline_min_matches_min_stark_len() {
        let trace = CompositeTrace::baseline_min();
        assert_eq!(trace.height(), MIN_STARK_LEN);
    }

    #[test]
    fn baseline_trace_verifies_through_composite_full_air() {
        let cfg = build_stark_config(&test_zk_params(), &CircuitConfig::TEST_PEARL);
        let trace = CompositeTrace::baseline_min();
        let pis = crate::composite_public::CompositePublicInputs::derive_from_matrix(&trace.matrix)
            .to_vec();
        let proof = prove::<AiPowStarkConfig, _>(&cfg, &CompositeFullAir, trace.matrix, &pis);
        verify::<AiPowStarkConfig, _>(&cfg, &CompositeFullAir, &proof, &pis)
            .expect("baseline trace must verify");
    }

    #[test]
    #[should_panic(expected = "below MIN_STARK_LEN")]
    fn baseline_panics_below_min_stark_len() {
        let _ = CompositeTrace::baseline(1024);
    }

    #[test]
    #[should_panic(expected = "power of 2")]
    fn baseline_panics_for_non_power_of_two() {
        // 16384 is a power of 2 (above MIN), but 17000 is not.
        let _ = CompositeTrace::baseline(17000);
    }

    #[test]
    fn baseline_larger_than_min_also_verifies() {
        let cfg = build_stark_config(&test_zk_params(), &CircuitConfig::TEST_PEARL);
        let trace = CompositeTrace::baseline(MIN_STARK_LEN * 2);
        assert_eq!(trace.height(), MIN_STARK_LEN * 2);
        let pis = crate::composite_public::CompositePublicInputs::derive_from_matrix(&trace.matrix)
            .to_vec();
        let proof = prove::<AiPowStarkConfig, _>(&cfg, &CompositeFullAir, trace.matrix, &pis);
        verify::<AiPowStarkConfig, _>(&cfg, &CompositeFullAir, &proof, &pis)
            .expect("2× baseline must verify");
    }

    #[test]
    fn useful_work_chain_hw_matches_rectangular_pattern_x_steps() {
        let h = 4usize;
        let w = 8usize;
        let k = 64usize;
        let r = 16usize;
        let num_stripes = k / r;
        let mut a_rows = vec![0i8; h * k];
        let mut b_cols = vec![0i8; w * k];
        for (idx, cell) in a_rows.iter_mut().enumerate() {
            *cell = ((idx * 13 + 5) % 89) as i8 - 44;
        }
        for (idx, cell) in b_cols.iter_mut().enumerate() {
            *cell = ((idx * 19 + 7) % 83) as i8 - 41;
        }

        let mut trace = CompositeTrace::baseline_min();
        let (rows_used, x_steps) =
            trace.place_useful_work_chain_hw(8, &a_rows, &b_cols, h, w, r, num_stripes);
        assert_eq!(
            rows_used,
            (h / TILE_H) * (w / TILE_H) * num_stripes * r.div_ceil(TILE_D)
        );
        assert_eq!(
            &x_steps[..num_stripes],
            expected_pattern_x_steps(&a_rows, &b_cols, h, w, k, r, num_stripes).as_slice()
        );
    }

    #[test]
    fn useful_work_chain_square_wrapper_matches_hw_entrypoint() {
        let t = 8usize;
        let k = 64usize;
        let r = 16usize;
        let num_stripes = k / r;
        let mut a_rows = vec![0i8; t * k];
        let mut b_cols = vec![0i8; t * k];
        for (idx, cell) in a_rows.iter_mut().enumerate() {
            *cell = ((idx * 11 + 3) % 79) as i8 - 39;
        }
        for (idx, cell) in b_cols.iter_mut().enumerate() {
            *cell = ((idx * 23 + 17) % 73) as i8 - 36;
        }

        let mut square = CompositeTrace::baseline_min();
        let (square_rows, square_x) =
            square.place_useful_work_chain(8, &a_rows, &b_cols, t, r, num_stripes);
        let mut hw = CompositeTrace::baseline_min();
        let (hw_rows, hw_x) =
            hw.place_useful_work_chain_hw(8, &a_rows, &b_cols, t, t, r, num_stripes);
        assert_eq!(square_rows, hw_rows);
        assert_eq!(&square_x[..num_stripes], &hw_x[..num_stripes]);
    }

    #[test]
    fn noised_chunk_hw_square_wrappers_match_hw_variants() {
        let t = 8usize;
        let k = 64usize;
        let r = 16usize;
        let num_stripes = k / r;
        let mut a_rows = vec![0i8; t * k];
        let mut b_cols = vec![0i8; t * k];
        for (idx, cell) in a_rows.iter_mut().enumerate() {
            *cell = ((idx * 7 + 1) % 67) as i8 - 33;
        }
        for (idx, cell) in b_cols.iter_mut().enumerate() {
            *cell = ((idx * 31 + 9) % 71) as i8 - 35;
        }

        assert_eq!(
            CompositeTrace::enumerate_noised_chunks(&a_rows, &b_cols, t, r, num_stripes),
            CompositeTrace::enumerate_noised_chunks_hw(&a_rows, &b_cols, t, t, r, num_stripes)
        );
        assert_eq!(
            CompositeTrace::enumerate_noised_chunks_with_src(&a_rows, &b_cols, t, r, num_stripes),
            CompositeTrace::enumerate_noised_chunks_with_src_hw(
                &a_rows, &b_cols, t, t, r, num_stripes
            )
        );
        assert_eq!(
            CompositeTrace::enumerate_noised_chunks_positioned(&a_rows, &b_cols, t, r, num_stripes),
            CompositeTrace::enumerate_noised_chunks_positioned_hw(
                &a_rows, &b_cols, t, t, r, num_stripes
            )
        );
        assert_eq!(
            CompositeTrace::noised_store_layout(t, r, num_stripes, k),
            CompositeTrace::noised_store_layout_hw(t, t, r, num_stripes, k)
        );
    }

    #[test]
    fn noised_chunk_hw_rectangular_layout_is_params_pure_and_bounded() {
        let h = 4usize;
        let w = 8usize;
        let k = 64usize;
        let r = 16usize;
        let num_stripes = k / r;
        let chunks = r.div_ceil(TILE_D).max(1);
        let n_chunk = A_NOISED_LEN / 2;
        let mut a_rows = vec![0i8; h * k];
        let mut b_cols = vec![0i8; w * k];
        for (idx, cell) in a_rows.iter_mut().enumerate() {
            *cell = ((idx * 5 + 11) % 61) as i8 - 30;
        }
        for (idx, cell) in b_cols.iter_mut().enumerate() {
            *cell = ((idx * 37 + 13) % 59) as i8 - 29;
        }

        let positioned = CompositeTrace::enumerate_noised_chunks_positioned_hw(
            &a_rows, &b_cols, h, w, r, num_stripes,
        );
        assert_eq!(
            positioned.len(),
            (h / TILE_H) * (w / TILE_H) * num_stripes * chunks * 2 * n_chunk
        );

        let layout = CompositeTrace::noised_store_layout_hw(h, w, r, num_stripes, k);
        assert_eq!(
            layout,
            positioned
                .iter()
                .map(|chunk| (chunk.side_a, chunk.src))
                .collect::<Vec<_>>()
        );

        for chunk in positioned {
            for source in chunk.src.into_iter().flatten() {
                let (lane, l) = source;
                assert!((l as usize) < k, "source k-index must be in bounds");
                if chunk.side_a {
                    assert!((lane as usize) < h, "A source lane must be in h");
                } else {
                    assert!((lane as usize) < w, "B source lane must be in w");
                }
            }
        }

        let deduped =
            CompositeTrace::enumerate_noised_chunks_hw(&a_rows, &b_cols, h, w, r, num_stripes);
        let deduped_with_src = CompositeTrace::enumerate_noised_chunks_with_src_hw(
            &a_rows, &b_cols, h, w, r, num_stripes,
        );
        assert_eq!(deduped.len(), deduped_with_src.len());
        assert!(deduped.len() <= layout.len());
    }

    #[test]
    fn baseline_stark_row_idx_is_monotonic() {
        use p3_field::PrimeField64;
        let trace = CompositeTrace::baseline_min();
        for r in 0..trace.height() {
            let val = trace.matrix.values[r * TOTAL_TRACE_WIDTH + STARK_ROW_IDX];
            assert_eq!(val.as_canonical_u64(), r as u64);
        }
    }

    /// Place 3 matmul instructions starting at row 0, then thread
    /// the final cumsum into row 3 so the cross-row passthrough
    /// constraint (`cur.CUMSUM = nxt.CUMSUM` when both selectors
    /// are 0) holds on the boundary.
    #[test]
    fn matmul_step_chain_verifies_through_composite_full_air() {
        use crate::composite_layout::TILE_D;

        let cfg = build_stark_config(&test_zk_params(), &CircuitConfig::TEST_PEARL);
        let mut trace = CompositeTrace::baseline_min();

        let mut a = [[0i8; TILE_D]; crate::composite_layout::TILE_H];
        let mut b = [[0i8; TILE_D]; crate::composite_layout::TILE_H];
        for d in 0..TILE_D {
            a[0][d] = (d as i8 + 1) % 5;
            a[1][d] = ((d as i8) * 3) % 7 - 3;
            b[0][d] = ((d as i8 + 2) % 6) - 3;
            b[1][d] = ((d as i8 + 3) % 11) - 5;
        }

        // Step 0: reset.
        let zero: [i32; CUMSUM_LEN] = [0; CUMSUM_LEN];
        let after_reset =
            trace.place_matmul_step(0, &a, &b, /*reset*/ true, /*update*/ false, &zero);
        // Step 1: update.
        let after_u1 = trace.place_matmul_step(1, &a, &b, false, true, &after_reset);
        // Step 2: update.
        let after_u2 = trace.place_matmul_step(2, &a, &b, false, true, &after_u1);
        // Thread the final cumsum across all subsequent passthrough
        // rows. The matmul cross-row constraint silences only at
        // the trace's very last row (via when_transition), so every
        // intermediate row must hold the value the chain ended at.
        trace.fill_cumsum_passthrough(3, &after_u2);

        let pis = crate::composite_public::CompositePublicInputs::derive_from_matrix(&trace.matrix)
            .to_vec();
        let proof = prove::<AiPowStarkConfig, _>(&cfg, &CompositeFullAir, trace.matrix, &pis);
        verify::<AiPowStarkConfig, _>(&cfg, &CompositeFullAir, &proof, &pis)
            .expect("matmul chain must verify through composite_full_air");
    }

    /// Place one BLAKE3 hash at row 0 of the baseline trace; the
    /// composite AIR should still verify (the BLAKE3 chip's
    /// round / init / finalize constraints all fire correctly on
    /// the 8-row block, while the remaining 8184 baseline rows
    /// have all-zero BLAKE3 columns).
    #[test]
    fn blake3_hash_block_at_row_0_verifies() {
        let cfg = build_stark_config(&test_zk_params(), &CircuitConfig::TEST_PEARL);
        let mut trace = CompositeTrace::baseline_min();

        let cv: [u32; 8] = core::array::from_fn(|i| BLAKE3_IV[i]);
        let msg: [u32; 16] = core::array::from_fn(|i| (i as u32 + 1) * 0x01020304);
        let tweak = Blake3Tweak {
            counter_low: 0,
            counter_high: 0,
            block_len: 64,
            flags: 0x1B,
        };

        let cv_out = trace.place_blake3_hash(0, &msg, &cv, &tweak);
        // Sanity: the returned cv_out matches a fresh BLAKE3 run.
        let full = compress_full_state(
            &cv, &msg, tweak.counter_low, tweak.counter_high as u32, tweak.block_len, tweak.flags,
        );
        for i in 0..8 {
            assert_eq!(cv_out[i], full[i]);
        }

        let pis = crate::composite_public::CompositePublicInputs::derive_from_matrix(&trace.matrix)
            .to_vec();
        let proof = prove::<AiPowStarkConfig, _>(&cfg, &CompositeFullAir, trace.matrix, &pis);
        verify::<AiPowStarkConfig, _>(&cfg, &CompositeFullAir, &proof, &pis)
            .expect("BLAKE3 hash block must verify through composite_full_air");
    }

    /// Tamper the BLAKE3 hash block's CV_OUT — the finalize
    /// constraint rejects.
    #[test]
    fn blake3_hash_block_rejects_tampered_cv_out() {
        use p3_field::integers::QuotientMap;

        use crate::composite_layout::CV_OUT_START;

        let cfg = build_stark_config(&test_zk_params(), &CircuitConfig::TEST_PEARL);
        let mut trace = CompositeTrace::baseline_min();

        let cv: [u32; 8] = core::array::from_fn(|i| BLAKE3_IV[i]);
        let msg: [u32; 16] = core::array::from_fn(|i| (i as u32 + 1) * 0x01020304);
        let tweak = Blake3Tweak {
            counter_low: 0,
            counter_high: 0,
            block_len: 64,
            flags: 0x1B,
        };

        let _ = trace.place_blake3_hash(0, &msg, &cv, &tweak);
        // Tamper row 7's CV_OUT[0].
        let target = 7 * TOTAL_TRACE_WIDTH + CV_OUT_START;
        trace.matrix.values[target] = <Val as QuotientMap<u64>>::from_int(0xDEAD_BEEFu64);

        let pis = crate::composite_public::CompositePublicInputs::derive_from_matrix(&trace.matrix)
            .to_vec();
        let proof = prove::<AiPowStarkConfig, _>(&cfg, &CompositeFullAir, trace.matrix, &pis);
        assert!(
            verify::<AiPowStarkConfig, _>(&cfg, &CompositeFullAir, &proof, &pis).is_err(),
            "tampered CV_OUT must reject"
        );
    }

    /// §M-S5b Path A column-overlay — adversarial.
    ///
    /// The StripeXor bit runs (`SX_IN_BITS`, `SX_XR_SEL_BITS`,
    /// `SX_NEW_SEL_BITS`) physically alias `blake3_round[0..192]`.
    /// This confirms the overlay did NOT weaken the BLAKE3 round
    /// AIR: on a genuine BLAKE3 round row those same columns hold
    /// BLAKE3 state and `verify_round` still constrains them, so
    /// corrupting one rejects. (`verify_round` is gated off only on
    /// *matmul* rows — `round_gate_excl` — never on BLAKE3 rows.)
    #[test]
    fn path_a_overlay_aliased_blake_columns_still_constrained() {
        use p3_field::integers::QuotientMap;

        use crate::composite_layout::SX_IN_BITS_START;

        let cfg = build_stark_config(&test_zk_params(), &CircuitConfig::TEST_PEARL);
        let mut trace = CompositeTrace::baseline_min();

        let cv: [u32; 8] = core::array::from_fn(|i| BLAKE3_IV[i]);
        let msg: [u32; 16] = core::array::from_fn(|i| (i as u32 + 1) * 0x01020304);
        let tweak = Blake3Tweak {
            counter_low: 0,
            counter_high: 0,
            block_len: 64,
            flags: 0x1B,
        };

        let _ = trace.place_blake3_hash(0, &msg, &cv, &tweak);
        // Tamper a column inside the SX-overlay sub-window
        // (`blake3_round[0..192]`, i.e. `SX_IN_BITS_START + k`) on a
        // genuine BLAKE3 round row (row 2). On a BLAKE3 row that
        // column holds BLAKE3 state, so `verify_round` must reject.
        let target = 2 * TOTAL_TRACE_WIDTH + SX_IN_BITS_START + 8;
        trace.matrix.values[target] = <Val as QuotientMap<u64>>::from_int(0xDEAD_BEEFu64);

        let pis = crate::composite_public::CompositePublicInputs::derive_from_matrix(&trace.matrix)
            .to_vec();
        let proof = prove::<AiPowStarkConfig, _>(&cfg, &CompositeFullAir, trace.matrix, &pis);
        assert!(
            verify::<AiPowStarkConfig, _>(&cfg, &CompositeFullAir, &proof, &pis).is_err(),
            "tampered blake3_round column in the SX-overlay window must reject"
        );
    }

    /// Place a 3-step jackpot chain at rows 0..2 of the baseline
    /// trace and thread the final state through the rest. The
    /// composite AIR enforces the rotate-XOR-13 chain end-to-end.
    #[test]
    fn jackpot_step_chain_verifies_through_composite_full_air() {
        let cfg = build_stark_config(&test_zk_params(), &CircuitConfig::TEST_PEARL);
        let mut trace = CompositeTrace::baseline_min();

        let initial: [u32; JACKPOT_SIZE] = core::array::from_fn(|i| (i as u32 + 1) * 0xCAFE_BABE);

        let s1 = trace.place_jackpot_step(0, &initial, 0, 0xDEAD_BEEF, true);
        let s2 = trace.place_jackpot_step(1, &s1, 3, 0xF00D_F00D, true);
        let s3 = trace.place_jackpot_step(2, &s2, 15, 0x12345_678, true);

        trace.fill_jackpot_passthrough(3, &s3);

        let pis = crate::composite_public::CompositePublicInputs::derive_from_matrix(&trace.matrix)
            .to_vec();
        let proof = prove::<AiPowStarkConfig, _>(&cfg, &CompositeFullAir, trace.matrix, &pis);
        verify::<AiPowStarkConfig, _>(&cfg, &CompositeFullAir, &proof, &pis)
            .expect("jackpot chain must verify through composite_full_air");
    }

    /// Tamper a JACKPOT_MSG slot mid-chain — the cross-row
    /// rotate-XOR-13 constraint rejects.
    #[test]
    fn jackpot_step_chain_rejects_tampered_msg() {
        use p3_field::integers::QuotientMap;
        let cfg = build_stark_config(&test_zk_params(), &CircuitConfig::TEST_PEARL);
        let mut trace = CompositeTrace::baseline_min();

        let initial: [u32; JACKPOT_SIZE] = [0; JACKPOT_SIZE];
        let s1 = trace.place_jackpot_step(0, &initial, 0, 0xCAFE, true);
        trace.fill_jackpot_passthrough(1, &s1);

        // Tamper row 1's JACKPOT_MSG[0] — should equal
        // rotate_xor_13(0, 0xCAFE) = 0xCAFE, change it to 0xBEEF.
        let target = 1 * TOTAL_TRACE_WIDTH + JACKPOT_MSG_START;
        trace.matrix.values[target] = <Val as QuotientMap<u64>>::from_int(0xBEEFu64);

        let pis = crate::composite_public::CompositePublicInputs::derive_from_matrix(&trace.matrix)
            .to_vec();
        let proof = prove::<AiPowStarkConfig, _>(&cfg, &CompositeFullAir, trace.matrix, &pis);
        assert!(
            verify::<AiPowStarkConfig, _>(&cfg, &CompositeFullAir, &proof, &pis).is_err(),
            "tampered JACKPOT_MSG must reject"
        );
    }

    /// Three-chip integration: a BLAKE3 hash (rows 0..7), a 2-step
    /// matmul chain (rows 8..9), and a 2-step jackpot chain (rows
    /// 10..11). Each chip family is active on disjoint row
    /// ranges; the composite AIR enforces all three sets of
    /// constraints simultaneously. End-to-end verification proves
    /// the wiring is sound across chip families.
    #[test]
    fn three_chip_integration_verifies() {
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
        let _ = trace.place_blake3_hash(0, &msg, &cv, &tweak);

        // (b) Matmul step chain at rows 8..9.
        let mut a = [[0i8; TILE_D]; crate::composite_layout::TILE_H];
        let mut b = [[0i8; TILE_D]; crate::composite_layout::TILE_H];
        for d in 0..TILE_D {
            a[0][d] = (d as i8) - 8;
            a[1][d] = ((d as i8) * 7) % 11 - 5;
            b[0][d] = ((d as i8) * 3) % 5 - 2;
            b[1][d] = ((d as i8) + 5) % 9 - 4;
        }
        let zero_cumsum: [i32; CUMSUM_LEN] = [0; CUMSUM_LEN];
        let cumsum_after_reset = trace.place_matmul_step(8, &a, &b, true, false, &zero_cumsum);
        let cumsum_final = trace.place_matmul_step(9, &a, &b, false, true, &cumsum_after_reset);

        // (c) Jackpot step chain at rows 10..11. The initial
        //     jackpot state must be present on every row before
        //     the first active step (rows 0..9 here) so the
        //     cross-row passthrough constraint `nxt = cur` holds
        //     across the row-9 → row-10 boundary.
        let initial_jackpot: [u32; JACKPOT_SIZE] = core::array::from_fn(|i| 0xDEAD_0000 + i as u32);
        trace.fill_jackpot_passthrough(0, &initial_jackpot);

        let jackpot_after_step1 =
            trace.place_jackpot_step(10, &initial_jackpot, 5, 0xCAFE_BABE, true);
        let jackpot_final =
            trace.place_jackpot_step(11, &jackpot_after_step1, 12, 0xF00D_F00D, true);

        // (d) Thread both accumulators through the rest of the trace.
        // For matmul: rows 10..N hold the final cumsum value.
        trace.fill_cumsum_passthrough(10, &cumsum_final);
        // For jackpot: rows 12..N hold the post-step-2 state.
        trace.fill_jackpot_passthrough(12, &jackpot_final);

        let pis = crate::composite_public::CompositePublicInputs::derive_from_matrix(&trace.matrix)
            .to_vec();
        let proof = prove::<AiPowStarkConfig, _>(&cfg, &CompositeFullAir, trace.matrix, &pis);
        verify::<AiPowStarkConfig, _>(&cfg, &CompositeFullAir, &proof, &pis)
            .expect("three-chip composite trace must verify");
    }

    /// Combined trace: a BLAKE3 hash at rows 0..7, then a 2-step
    /// matmul chain at rows 8..10, then passthrough. Tests that
    /// the composite AIR enforces *both* chip families' constraints
    /// simultaneously without cross-talk.
    #[test]
    fn blake3_then_matmul_combined_verifies() {
        use crate::composite_layout::TILE_D;

        let cfg = build_stark_config(&test_zk_params(), &CircuitConfig::TEST_PEARL);
        let mut trace = CompositeTrace::baseline_min();

        // BLAKE3 hash at rows 0..7.
        let cv: [u32; 8] = core::array::from_fn(|i| BLAKE3_IV[i]);
        let msg: [u32; 16] = core::array::from_fn(|i| (i as u32 + 1) * 0xCAFE);
        let tweak = Blake3Tweak {
            counter_low: 7,
            counter_high: 0,
            block_len: 64,
            flags: 0x1B,
        };
        let _ = trace.place_blake3_hash(0, &msg, &cv, &tweak);

        // Matmul at rows 8..9.
        let mut a = [[0i8; TILE_D]; crate::composite_layout::TILE_H];
        let mut b = [[0i8; TILE_D]; crate::composite_layout::TILE_H];
        for d in 0..TILE_D {
            a[0][d] = (d as i8 + 1) % 7;
            a[1][d] = ((d as i8) * 2) % 5 - 2;
            b[0][d] = ((d as i8 + 1) % 3) - 1;
            b[1][d] = ((d as i8 + 2) % 4) - 1;
        }
        let zero: [i32; CUMSUM_LEN] = [0; CUMSUM_LEN];
        let after_reset = trace.place_matmul_step(8, &a, &b, true, false, &zero);
        let after_update = trace.place_matmul_step(9, &a, &b, false, true, &after_reset);

        // Thread the final cumsum through all subsequent rows.
        trace.fill_cumsum_passthrough(10, &after_update);

        let pis = crate::composite_public::CompositePublicInputs::derive_from_matrix(&trace.matrix)
            .to_vec();
        let proof = prove::<AiPowStarkConfig, _>(&cfg, &CompositeFullAir, trace.matrix, &pis);
        verify::<AiPowStarkConfig, _>(&cfg, &CompositeFullAir, &proof, &pis)
            .expect("combined BLAKE3 + matmul trace must verify");
    }

    /// Tamper a matmul step's input — the chain breaks because
    /// the cross-row cumsum constraint depends on the dot product.
    #[test]
    fn matmul_step_chain_rejects_tampered_input() {
        use crate::composite_layout::TILE_D;
        let cfg = build_stark_config(&test_zk_params(), &CircuitConfig::TEST_PEARL);
        let mut trace = CompositeTrace::baseline_min();
        let a = [[1i8; TILE_D]; crate::composite_layout::TILE_H];
        let b = [[1i8; TILE_D]; crate::composite_layout::TILE_H];

        let zero: [i32; CUMSUM_LEN] = [0; CUMSUM_LEN];
        let after_step = trace.place_matmul_step(0, &a, &b, true, false, &zero);
        trace.fill_cumsum_passthrough(1, &after_step);

        // Tamper row 0's A_NOISED_UNPACK[0]: change from 1 to 2.
        // The dot product changes, so the constraint
        // `nxt.CUMSUM = (1+0) * dot + (0) * cur.CUMSUM` rejects.
        use p3_field::integers::QuotientMap;

        use crate::composite_layout::A_NOISED_UNPACK_START;
        let target = 0 * TOTAL_TRACE_WIDTH + A_NOISED_UNPACK_START;
        trace.matrix.values[target] = <Val as QuotientMap<i64>>::from_int(2);

        let pis = crate::composite_public::CompositePublicInputs::derive_from_matrix(&trace.matrix)
            .to_vec();
        let proof = prove::<AiPowStarkConfig, _>(&cfg, &CompositeFullAir, trace.matrix, &pis);
        assert!(
            verify::<AiPowStarkConfig, _>(&cfg, &CompositeFullAir, &proof, &pis).is_err(),
            "tampered matmul input must reject"
        );
    }

    /// M-S1 (§4.C.11) pack-link acceptance: a matmul row whose
    /// packed `A_NOISED` cell ≠ the base-256 polyval of its
    /// `A_NOISED_UNPACK` lanes must reject. Tampering only the
    /// *packed* cell (dot / unpack / cumsum intact) isolates the
    /// new pack-link — in the unit `CompositeFullAir` it is the
    /// sole reader of `A_NOISED_START`, so this proves it ties the
    /// store-bound packed value to the dot inputs (without it the
    /// §4.C committed-store binding would not constrain the matmul).
    #[test]
    fn matmul_pack_link_rejects_inconsistent_a_noised() {
        use p3_field::integers::QuotientMap;

        use crate::composite_layout::{A_NOISED_START, TILE_D};
        let cfg = build_stark_config(&test_zk_params(), &CircuitConfig::TEST_PEARL);
        let mut trace = CompositeTrace::baseline_min();
        let a = [[3i8; TILE_D]; crate::composite_layout::TILE_H];
        let b = [[5i8; TILE_D]; crate::composite_layout::TILE_H];
        let zero: [i32; CUMSUM_LEN] = [0; CUMSUM_LEN];
        let after = trace.place_matmul_step(0, &a, &b, true, false, &zero);
        trace.fill_cumsum_passthrough(1, &after);
        // Corrupt ONLY packed A_NOISED[0] ⇒ ≠ polyval(unpack[0..4]).
        let target = 0 * TOTAL_TRACE_WIDTH + A_NOISED_START;
        trace.matrix.values[target] = <Val as QuotientMap<i64>>::from_int(0xDEAD);
        let pis = crate::composite_public::CompositePublicInputs::derive_from_matrix(&trace.matrix)
            .to_vec();
        let proof = prove::<AiPowStarkConfig, _>(&cfg, &CompositeFullAir, trace.matrix, &pis);
        assert!(
            verify::<AiPowStarkConfig, _>(&cfg, &CompositeFullAir, &proof, &pis).is_err(),
            "A_NOISED ≠ pack(A_NOISED_UNPACK) on a matmul row must reject"
        );
    }

    /// `place_matrix_hash` produces the same 32-byte digest as
    /// `blake3::Hasher::new_keyed(&kappa).update(&padded).finalize()`
    /// — byte-equivalent to `ai-pow::commit::matrix_commitment`.
    /// Tested at TEST_SMALL shape (4 KiB matrix = 4 chunks +
    /// 3 parents = 67 BLAKE3 instructions = 536 trace rows).
    #[test]
    fn place_matrix_hash_byte_equivalent_to_blake3_keyed() {
        const MATRIX_BYTES: usize = 4096; // 4 KiB → 4 chunks → multi-chunk path.
        let mut matrix = vec![0u8; MATRIX_BYTES];
        for (i, b) in matrix.iter_mut().enumerate() {
            *b = ((i * 31 + 7) & 0xFF) as u8; // deterministic but non-trivial
        }
        let key = [0xA5u8; 32];

        // Reference: standard BLAKE3 keyed-hash on the (already
        // chunk-aligned) byte stream.
        let mut hasher = blake3::Hasher::new_keyed(&key);
        hasher.update(&matrix);
        let expected = *hasher.finalize().as_bytes();
        let expected_words: [u32; 8] = core::array::from_fn(|i| {
            u32::from_le_bytes([
                expected[i * 4],
                expected[i * 4 + 1],
                expected[i * 4 + 2],
                expected[i * 4 + 3],
            ])
        });

        // Place into a CompositeTrace and compare.
        let mut trace = CompositeTrace::baseline_min();
        let (next_row, root_cv) = trace.place_matrix_hash_a(0, &matrix, &key);

        assert_eq!(
            root_cv, expected_words,
            "place_matrix_hash_a must match blake3::Hasher::new_keyed(...).finalize()"
        );
        // 4 chunks × 16 blocks + 3 parents = 67 instructions × 8 rows = 536.
        assert_eq!(next_row, 4 * 16 * 8 + 3 * 8);
    }

    #[test]
    fn place_matrix_hash_matches_blake3_for_non_power_of_two_chunk_counts() {
        let key = [0xC3u8; 32];
        for &chunks in &[3usize, 5, 9, 17, 31, 33] {
            let mut matrix = vec![0u8; chunks * 1024 - 13];
            for (i, b) in matrix.iter_mut().enumerate() {
                *b = ((i * 17 + chunks * 29) & 0xff) as u8;
            }
            let mut padded = matrix.clone();
            padded.resize(chunks * 1024, 0);
            let expected = *blake3::Hasher::new_keyed(&key)
                .update(&padded)
                .finalize()
                .as_bytes();
            let expected_words: [u32; 8] = core::array::from_fn(|i| {
                u32::from_le_bytes([
                    expected[i * 4],
                    expected[i * 4 + 1],
                    expected[i * 4 + 2],
                    expected[i * 4 + 3],
                ])
            });

            let mut trace = CompositeTrace::baseline_min();
            let (_, root_cv) = trace.place_matrix_hash_a(0, &matrix, &key);
            assert_eq!(
                root_cv, expected_words,
                "place_matrix_hash_a must match canonical BLAKE3 at {chunks} chunks"
            );
        }
    }

    /// Single-chunk path: 1 KiB input → 1 chunk → 16 blocks, no
    /// parents. The chunk's last block carries the ROOT flag.
    #[test]
    fn place_matrix_hash_single_chunk() {
        let matrix = vec![0x55u8; 1024];
        let key = [0x33u8; 32];

        let expected = *blake3::Hasher::new_keyed(&key)
            .update(&matrix)
            .finalize()
            .as_bytes();
        let expected_words: [u32; 8] = core::array::from_fn(|i| {
            u32::from_le_bytes([
                expected[i * 4],
                expected[i * 4 + 1],
                expected[i * 4 + 2],
                expected[i * 4 + 3],
            ])
        });

        let mut trace = CompositeTrace::baseline_min();
        let (next_row, root_cv) = trace.place_matrix_hash_b(0, &matrix, &key);
        assert_eq!(root_cv, expected_words);
        assert_eq!(next_row, 16 * 8); // 16 blocks × 8 rows, no parents
    }

    /// `place_matrix_hash` must pad sub-chunk inputs out to the
    /// next 1024-byte boundary (matches `pad_to_chunk_boundary`).
    #[test]
    fn place_matrix_hash_pads_to_chunk_boundary() {
        let matrix = vec![0xCCu8; 500]; // not a multiple of 1024
        let key = [0x77u8; 32];

        // Reference: pad matrix to 1024 then hash.
        let mut padded = matrix.clone();
        padded.resize(1024, 0);
        let expected = *blake3::Hasher::new_keyed(&key)
            .update(&padded)
            .finalize()
            .as_bytes();
        let expected_words: [u32; 8] = core::array::from_fn(|i| {
            u32::from_le_bytes([
                expected[i * 4],
                expected[i * 4 + 1],
                expected[i * 4 + 2],
                expected[i * 4 + 3],
            ])
        });

        let mut trace = CompositeTrace::baseline_min();
        let (_, root_cv) = trace.place_matrix_hash_a(0, &matrix, &key);
        assert_eq!(root_cv, expected_words);
    }

    /// End-to-end: place a small matrix hash via
    /// `place_matrix_hash_a`, derive PIs (including `HASH_A`),
    /// prove + verify. Validates that the BLAKE3 chip's per-row
    /// constraints accept the chunk-Merkle instruction sequence
    /// AND that the new selector-gated PI binding fires
    /// correctly.
    #[test]
    fn place_matrix_hash_full_air_prove_and_verify() {
        let matrix = vec![0x11u8; 1024]; // single-chunk: 128 trace rows
        let key = [0x99u8; 32];

        let mut trace = CompositeTrace::baseline_min();
        trace.place_matrix_hash_a(0, &matrix, &key);

        let cfg = build_stark_config(&test_zk_params(), &CircuitConfig::TEST_PEARL);
        let pis = crate::composite_public::CompositePublicInputs::derive_from_matrix(&trace.matrix)
            .to_vec();
        let proof = prove::<AiPowStarkConfig, _>(&cfg, &CompositeFullAir, trace.matrix, &pis);
        verify::<AiPowStarkConfig, _>(&cfg, &CompositeFullAir, &proof, &pis)
            .expect("composite proof with matrix hash must verify");
    }

    /// M52 step 4.2 (superseded by C3): `place_matrix_staging_row`
    /// writes coherent MAT_UNPACK / UINT8_DATA / NOISE_UNPACK=0 /
    /// NOISED_PACKED / MAT_ID / IS_MSG_MAT / CONTROL_PREP. C3
    /// revealed the "separate staging row" model is not provable
    /// (BLAKE3_MSG is blake3-chip-owned, so IS_MSG_MAT must live
    /// on real compression rows — the F1 integration path), so
    /// this is now a column-write derivation check rather than a
    /// full prove+verify.
    #[test]
    fn place_matrix_staging_row_writes_expected_columns() {
        use p3_field::integers::QuotientMap;
        use p3_field::PrimeField64;

        use crate::composite_layout::{
            IS_MSG_MAT, MAT_ID, NOISED_PACKED_START, NOISE_UNPACK_START, UINT8_DATA_START,
        };

        let mut trace = CompositeTrace::baseline_min();
        let bytes: [i8; 8] = [1, -2, 3, -4, 5, -6, 7, -8];
        let packs = trace.place_matrix_staging_row(5, &bytes, 42);

        let base = 5 * TOTAL_TRACE_WIDTH;
        let v = &trace.matrix.values;
        assert_eq!(v[base + IS_MSG_MAT].as_canonical_u64(), 1);
        assert_eq!(v[base + MAT_ID].as_canonical_u64(), 42);
        for i in 0..8 {
            assert_eq!(
                v[base + UINT8_DATA_START + i].as_canonical_u64(),
                bytes[i] as u8 as u64
            );
            assert_eq!(v[base + NOISE_UNPACK_START + i].as_canonical_u64(), 0);
        }
        // NOISED_PACKED = polyval(MAT_UNPACK, 256) with NOISE=0.
        let expect_np0 = <Val as QuotientMap<i64>>::from_int(packs[0] as i64);
        assert_eq!(
            v[base + NOISED_PACKED_START].as_canonical_u64(),
            expect_np0.as_canonical_u64()
        );
    }

    /// C3: the IS_MSG_MAT-gated cross-column constraint
    /// `IS_MSG_MAT · (BLAKE3_MSG[j] − base256(UINT8_DATA[4j..4j+4])) = 0`
    /// rejects a trace where a row claims IS_MSG_MAT but its
    /// hashed message word does not equal the matrix-byte view
    /// the i8u8 / noised_packed buses bind. This is the residual
    /// soundness gap (M52 step 4.3+): without it an adversary
    /// hashes matrix Y while the buses bind matrix X.
    ///
    /// Negative test: a hand-crafted row with IS_MSG_MAT=1,
    /// UINT8_DATA != 0, BLAKE3_MSG = 0 violates C3 ⇒ verify must
    /// reject. (The consistent+globally-valid positive case needs
    /// IS_MSG_MAT on a real blake3 compression row carrying
    /// matrix bytes — the F1 integration path; C3's constraint is
    /// what makes that path sound.)
    #[test]
    fn c3_rejects_is_msg_mat_row_with_mismatched_blake_msg() {
        use p3_field::integers::QuotientMap;

        use crate::composite_layout::{IS_MSG_MAT, IS_NEW_BLAKE, UINT8_DATA_START};

        let cfg = build_stark_config(&test_zk_params(), &CircuitConfig::TEST_PEARL);

        let mut trace = CompositeTrace::baseline_min();
        let r = 5usize;
        let base = r * TOTAL_TRACE_WIDTH;
        // C3 gate is IS_MSG_MAT · IS_NEW_BLAKE. Set both, BLAKE3_MSG
        // stays 0 while UINT8_DATA[0] = 7 ⇒ base256(UINT8_DATA[0..4])
        // = 7 ≠ 0 ⇒ C3 fails. (Bare IS_MSG_MAT without IS_NEW_BLAKE
        // is the i8u8/range bus data-validation case — C3 vacuous
        // there, which is what restores the 6 LogUp tests.)
        trace.matrix.values[base + IS_MSG_MAT] = <Val as QuotientMap<u64>>::from_int(1);
        trace.matrix.values[base + IS_NEW_BLAKE] = <Val as QuotientMap<u64>>::from_int(1);
        trace.matrix.values[base + UINT8_DATA_START] = <Val as QuotientMap<u64>>::from_int(7);

        let pis = crate::composite_public::CompositePublicInputs::derive_from_matrix(&trace.matrix)
            .to_vec();
        let proof = prove::<AiPowStarkConfig, _>(&cfg, &CompositeFullAir, trace.matrix, &pis);
        assert!(
            verify::<AiPowStarkConfig, _>(&cfg, &CompositeFullAir, &proof, &pis).is_err(),
            "C3 must reject an IS_MSG_MAT row whose BLAKE3_MSG != base256(UINT8_DATA)"
        );
    }

    /// Tampering with `PI_HASH_A` must make verify reject. This
    /// exercises the selector-gated binding constraint from step 2.
    #[test]
    fn full_air_rejects_tampered_hash_a_pi() {
        let matrix = vec![0xABu8; 1024];
        let key = [0xCDu8; 32];

        let mut trace = CompositeTrace::baseline_min();
        trace.place_matrix_hash_a(0, &matrix, &key);

        let cfg = build_stark_config(&test_zk_params(), &CircuitConfig::TEST_PEARL);
        let mut pis =
            crate::composite_public::CompositePublicInputs::derive_from_matrix(&trace.matrix);
        // Flip a bit in HASH_A — should make the PI binding fail.
        pis.hash_a[0] ^= 1;
        let pis_vec = pis.to_vec();

        let proof = prove::<AiPowStarkConfig, _>(&cfg, &CompositeFullAir, trace.matrix, &pis_vec);
        assert!(
            verify::<AiPowStarkConfig, _>(&cfg, &CompositeFullAir, &proof, &pis_vec).is_err(),
            "tampered HASH_A PI must be rejected"
        );
    }

    /// C4: `derive_from_matrix` reads `CV_OUT` from the
    /// `IS_HASH_JACKPOT` row into `pi.hash_jackpot`. The
    /// selector-gated AIR constraint
    /// `IS_HASH_JACKPOT · (CV_OUT[i] − PI_HASH_JACKPOT[i]) = 0`
    /// is structurally identical to the HASH_A binding that is
    /// proven end-to-end by
    /// `full_air_rejects_tampered_hash_a_pi`; the difference is
    /// only the selector column (6 = IS_HASH_JACKPOT) and the
    /// PI offset. Full prove+verify of a HASH_JACKPOT trace
    /// additionally needs a valid jackpot→blake3 chain (the
    /// `IS_HASH_JACKPOT` column is multiplexed as the jackpot
    /// chip's `is_active`), which is the F1 integration work.
    #[test]
    fn c4_hash_jackpot_derives_from_selector_row() {
        use p3_field::integers::QuotientMap;

        use crate::composite_layout::{CV_OUT_START, IS_HASH_JACKPOT};

        let mut trace = CompositeTrace::baseline_min();
        let n = trace.matrix.values.len() / TOTAL_TRACE_WIDTH;
        let r = 9usize.min(n - 1);
        let base = r * TOTAL_TRACE_WIDTH;
        trace.matrix.values[base + IS_HASH_JACKPOT] = <Val as QuotientMap<u64>>::from_int(1);
        let expected: [u32; 8] = core::array::from_fn(|i| 0xABCD_0000u32 + i as u32 * 0x1111);
        for i in 0..8 {
            trace.matrix.values[base + CV_OUT_START + i] =
                <Val as QuotientMap<u64>>::from_int(expected[i] as u64);
        }
        let pis = crate::composite_public::CompositePublicInputs::derive_from_matrix(&trace.matrix);
        assert_eq!(pis.hash_jackpot, expected);
    }

    /// C1: `derive_from_matrix` reads `CV_IN` from the
    /// `IS_USE_JOB_KEY` / `IS_USE_COMMITMENT_HASH` rows into
    /// `pi.job_key` / `pi.commitment_hash`. Same selector-gated
    /// binding form as HASH_A (proven end-to-end elsewhere);
    /// here we validate the derivation reads the correct cells
    /// for the chain-pinning PIs.
    #[test]
    fn c1_job_key_and_commitment_hash_derive_from_cv_in() {
        use p3_field::integers::QuotientMap;

        use crate::composite_layout::{CV_IN_START, IS_USE_COMMITMENT_HASH, IS_USE_JOB_KEY};

        let mut trace = CompositeTrace::baseline_min();
        let n = trace.matrix.values.len() / TOTAL_TRACE_WIDTH;

        let jk_row = 4usize;
        let jk_base = jk_row * TOTAL_TRACE_WIDTH;
        trace.matrix.values[jk_base + IS_USE_JOB_KEY] = <Val as QuotientMap<u64>>::from_int(1);
        let job_key: [u32; 8] = core::array::from_fn(|i| 0xCAFE_0000 + i as u32);
        for i in 0..8 {
            trace.matrix.values[jk_base + CV_IN_START + i] =
                <Val as QuotientMap<u64>>::from_int(job_key[i] as u64);
        }

        let ch_row = 11usize.min(n - 1);
        let ch_base = ch_row * TOTAL_TRACE_WIDTH;
        trace.matrix.values[ch_base + IS_USE_COMMITMENT_HASH] =
            <Val as QuotientMap<u64>>::from_int(1);
        let commit: [u32; 8] = core::array::from_fn(|i| 0xBEEF_0000 + i as u32);
        for i in 0..8 {
            trace.matrix.values[ch_base + CV_IN_START + i] =
                <Val as QuotientMap<u64>>::from_int(commit[i] as u64);
        }

        let pis = crate::composite_public::CompositePublicInputs::derive_from_matrix(&trace.matrix);
        assert_eq!(pis.job_key, job_key, "JOB_KEY from IS_USE_JOB_KEY row");
        assert_eq!(
            pis.commitment_hash, commit,
            "COMMITMENT_HASH from IS_USE_COMMITMENT_HASH row"
        );
    }

    /// F1-deep make-or-break: a "key-pin" row (IS_USE_JOB_KEY = 1,
    /// CV_IN_START = κ, no blake/jackpot activity) must prove +
    /// verify, with the C1 binding firing non-vacuously. If this
    /// holds, JOB_KEY / COMMITMENT_HASH can be made real PIs
    /// without the full Pearl per-row interleave.
    #[test]
    fn c1_key_pin_row_proves_and_verifies() {
        use p3_field::integers::QuotientMap;

        use crate::chips::control::ControlChip;
        use crate::composite_layout::CV_IN_START;

        let mut trace = CompositeTrace::baseline_min();

        // Row 5: IS_USE_JOB_KEY = 1 (SELECTOR_COLS idx 2), CV_IN = κ.
        let jk: [u32; 8] = core::array::from_fn(|i| 0xC0FE_0000 + i as u32);
        let r1 = 5usize;
        let b1 = r1 * TOTAL_TRACE_WIDTH;
        {
            let row = &mut trace.matrix.values[b1..b1 + TOTAL_TRACE_WIDTH];
            for i in 0..8 {
                row[CV_IN_START + i] = <Val as QuotientMap<u64>>::from_int(jk[i] as u64);
            }
            let mut sel = [false; 21];
            sel[2] = true; // IS_USE_JOB_KEY
            ControlChip.fill_row(&sel, 0, row);
        }

        // Row 9: IS_USE_COMMITMENT_HASH = 1 (idx 3), CV_IN = jackpot key.
        let ch: [u32; 8] = core::array::from_fn(|i| 0x5EED_0000 + i as u32);
        let r2 = 9usize;
        let b2 = r2 * TOTAL_TRACE_WIDTH;
        {
            let row = &mut trace.matrix.values[b2..b2 + TOTAL_TRACE_WIDTH];
            for i in 0..8 {
                row[CV_IN_START + i] = <Val as QuotientMap<u64>>::from_int(ch[i] as u64);
            }
            let mut sel = [false; 21];
            sel[3] = true; // IS_USE_COMMITMENT_HASH
            ControlChip.fill_row(&sel, 0, row);
        }

        let cfg = build_stark_config(&test_zk_params(), &CircuitConfig::TEST_PEARL);
        let pis = crate::composite_public::CompositePublicInputs::derive_from_matrix(&trace.matrix);
        assert_eq!(pis.job_key, jk);
        assert_eq!(pis.commitment_hash, ch);
        let pv = pis.to_vec();
        let proof = prove::<AiPowStarkConfig, _>(&cfg, &CompositeFullAir, trace.matrix, &pv);
        verify::<AiPowStarkConfig, _>(&cfg, &CompositeFullAir, &proof, &pv)
            .expect("key-pin row must prove+verify (C1 non-vacuous, tractable)");
    }

    /// Regression for the `verify_round` leading-boundary gate fix
    /// (`2026-05-15_BLAKE3_CHIP_ROUND_GATE_BUG.md`): a bare blake3 block
    /// (no jackpot / no extra selectors) must now prove+verify
    /// at a mid-trace offset AND trace-terminal — not just
    /// contiguous from row 0.
    #[test]
    fn blake_block_verifies_off_row_zero_after_gate_fix() {
        let tweak = crate::chips::blake3::compress::Blake3Tweak {
            counter_low: 0,
            counter_high: 0,
            block_len: 64,
            flags: 0x1B,
        };
        let msg = [0u32; 16];
        let cv: [u32; 8] = core::array::from_fn(|i| 0x11 * (i as u32 + 1));
        let cfg = build_stark_config(&test_zk_params(), &CircuitConfig::TEST_PEARL);

        for &row_start in &[100usize, /* trace-terminal */ usize::MAX] {
            let mut trace = CompositeTrace::baseline_min();
            let rs = if row_start == usize::MAX {
                trace.height() - 8
            } else {
                row_start
            };
            trace.place_blake3_hash_with_selectors(rs, &msg, &cv, &tweak, &[]);
            let pis =
                crate::composite_public::CompositePublicInputs::derive_from_matrix(&trace.matrix)
                    .to_vec();
            let proof = prove::<AiPowStarkConfig, _>(&cfg, &CompositeFullAir, trace.matrix, &pis);
            verify::<AiPowStarkConfig, _>(&cfg, &CompositeFullAir, &proof, &pis)
                .unwrap_or_else(|e| panic!("blake block at row {rs} must verify post-fix: {e:?}"));
        }
    }

    /// F1 (C4) — the final jackpot-hash block makes HASH_JACKPOT a
    /// non-vacuous bound PI: `BLAKE3(JACKPOT_MSG, key=COMMITMENT)`.
    /// Row 7 (= last trace row) co-carries the blake3 finalize AND
    /// a valid degenerate jackpot step. Depends on the
    /// `verify_round` gate fix.
    #[test]
    fn c4_jackpot_hash_block_binds_hash_jackpot() {
        let mut trace = CompositeTrace::baseline_min();
        let h = trace.height();
        let jackpot_state = [0u32; JACKPOT_SIZE];
        let commitment: [u32; 8] = core::array::from_fn(|i| 0x5EED_0000 + i as u32);

        let digest = trace.place_jackpot_hash_block(h - 8, &jackpot_state, &commitment);
        assert_ne!(digest, [0u32; 8], "BLAKE3(·, key) is non-zero");

        let pis = crate::composite_public::CompositePublicInputs::derive_from_matrix(&trace.matrix);
        assert_eq!(
            pis.hash_jackpot, digest,
            "C4: HASH_JACKPOT PI must equal the keyed-hash digest"
        );

        let cfg = build_stark_config(&test_zk_params(), &CircuitConfig::TEST_PEARL);
        let pv = pis.to_vec();
        let proof = prove::<AiPowStarkConfig, _>(&cfg, &CompositeFullAir, trace.matrix, &pv);
        verify::<AiPowStarkConfig, _>(&cfg, &CompositeFullAir, &proof, &pv).expect(
            "jackpot-hash block must prove+verify (C4 non-vacuous: blake finalize + \
             degenerate jackpot step co-located on the last row)",
        );
    }

    /// After `place_matrix_hash_a`, the root row has
    /// `IS_HASH_A = 1` and `CV_OUT` matches the digest. The
    /// centralized `CompositePublicInputs::derive_from_matrix`
    /// surfaces it.
    #[test]
    fn place_matrix_hash_sets_is_hash_a_selector() {
        use p3_field::PrimeField64;

        use crate::composite_layout::{IS_HASH_A, IS_HASH_B};

        let matrix = vec![0x42u8; 1024];
        let key = [0x88u8; 32];

        let mut trace = CompositeTrace::baseline_min();
        let (_, root_cv) = trace.place_matrix_hash_a(0, &matrix, &key);

        // Scan for the IS_HASH_A=1 row.
        let mut found_a = None;
        let mut count_a = 0;
        let mut count_b = 0;
        let height = trace.height();
        for r in 0..height {
            let base = r * TOTAL_TRACE_WIDTH;
            if trace.matrix.values[base + IS_HASH_A].as_canonical_u64() == 1 {
                found_a = Some(r);
                count_a += 1;
            }
            if trace.matrix.values[base + IS_HASH_B].as_canonical_u64() == 1 {
                count_b += 1;
            }
        }
        assert_eq!(count_a, 1, "exactly one IS_HASH_A row");
        assert_eq!(count_b, 0, "no IS_HASH_B set");

        let pis = crate::composite_public::CompositePublicInputs::derive_from_matrix(&trace.matrix);
        assert_eq!(pis.hash_a, root_cv);
        assert_eq!(pis.hash_b, [0u32; 8]);

        // The IS_HASH_A=1 row should be the last block of the
        // single chunk: row 15 of the 16-block chunk → 15 × 8 + 7 = 127.
        assert_eq!(found_a, Some(127));
    }

    // ─────────────── P-B.2.2: strip opening ───────────────

    fn b32_to_words(b: &[u8; 32]) -> [u32; 8] {
        core::array::from_fn(|i| {
            u32::from_le_bytes([b[i * 4], b[i * 4 + 1], b[i * 4 + 2], b[i * 4 + 3]])
        })
    }

    /// **Core honest-equivalence (in-circuit).** For many
    /// `(num_chunks, c0, c1)` — incl. non-power-of-two trees,
    /// the full range, boundary-straddling ranges, and the
    /// lone-chunk case — `place_matrix_strip_opening` recomputes
    /// **exactly** the root `place_matrix_hash` produces for the
    /// whole matrix, which is real `blake3::Hasher::new_keyed`
    /// (= `commit::matrix_commitment`). Pure (no prove) ⇒ fast.
    #[test]
    fn strip_opening_root_equals_full_matrix_hash() {
        let key: [u8; 32] = core::array::from_fn(|i| (i as u8) ^ 0x5A);
        for &nc in &[1usize, 2, 3, 5, 8, 13] {
            let raw: Vec<u8> = (0..nc * 1024)
                .map(|i| ((i.wrapping_mul(2654435761)) ^ (i >> 4)) as u8)
                .collect();
            // Reference roots: full in-circuit hash + blake3.
            let full_root = {
                let mut t = CompositeTrace::baseline_min();
                t.place_matrix_hash_a(0, &raw, &key).1
            };
            let blake = b32_to_words(
                blake3::Hasher::new_keyed(&key)
                    .update(&raw)
                    .finalize()
                    .as_bytes(),
            );
            assert_eq!(full_root, blake, "sanity: full hash == blake3 @ {nc}");

            for c0 in 0..nc {
                for c1 in (c0 + 1)..=nc {
                    let (_opened, sibs) = crate::blake3_tree::open_strip(&raw, &key, c0, c1);
                    let strip_bytes = &raw[c0 * 1024..c1 * 1024];
                    let mut t = CompositeTrace::baseline_min();
                    let (n, strip_root) = t.place_matrix_strip_opening(
                        0, strip_bytes, c0, c1, nc, &sibs, &key, 4,    // IS_HASH_A
                        None, // hash-only (pre-cx.2; g=0)
                        None,
                    );
                    assert_eq!(
                        strip_root, full_root,
                        "strip [{c0},{c1}) of {nc} chunks != committed root"
                    );
                    // Phase A-CR (CR.0): the params-pure row count
                    // matches the actual placement, for every
                    // (nc, c0, c1) — `row_schedule`/`canonical_
                    // program` rely on this for the strip-opening
                    // A/B regions.
                    assert_eq!(
                        n,
                        crate::blake3_tree::strip_opening_rows(c0, c1, nc),
                        "strip_opening_rows({c0},{c1},{nc}) != placed rows"
                    );
                }
            }
        }
    }

    /// Trace-side selective (disjoint-chunk) multi-proof recomputes exactly the
    /// committed full-matrix root, for scattered/every-other/endpoint/full sets;
    /// and is byte-identical to the range opening (same root + same placed rows)
    /// for a contiguous set — the trace counterpart of the off-circuit
    /// `open_strip_set` KATs. Pure (no prove) ⇒ fast.
    #[test]
    fn selective_strip_opening_root_equals_full_matrix_hash() {
        let key: [u8; 32] = core::array::from_fn(|i| (i as u8) ^ 0x5A);
        for &nc in &[2usize, 3, 5, 8, 13, 31] {
            let raw: Vec<u8> = (0..nc * 1024)
                .map(|i| ((i.wrapping_mul(2654435761)) ^ (i >> 4)) as u8)
                .collect();
            let full_root = {
                let mut t = CompositeTrace::baseline_min();
                t.place_matrix_hash_a(0, &raw, &key).1
            };
            let mut sets: Vec<Vec<usize>> = vec![
                vec![0],
                vec![nc - 1],
                vec![0, nc - 1],
                (0..nc).step_by(2).collect(),
                (0..nc).collect(),
            ];
            if nc >= 4 {
                sets.push(vec![0, 1, nc - 2, nc - 1]);
                sets.push(vec![1, nc / 2, nc - 1]);
            }
            for sel in &sets {
                let (_opened, sibs) = crate::blake3_tree::open_strip_set(&raw, &key, sel);
                let strip_bytes: Vec<u8> = sel
                    .iter()
                    .flat_map(|&c| raw[c * 1024..(c + 1) * 1024].iter().copied())
                    .collect();
                let mut t = CompositeTrace::baseline_min();
                let (_n, root) = t.place_matrix_strip_opening_set(
                    0, &strip_bytes, sel, nc, &sibs, &key, 4, None, None,
                );
                assert_eq!(root, full_root, "selective open {sel:?} of {nc} != committed root");
            }
            // Contiguous set ⇒ identical to the range opening (root + placed rows).
            for c0 in 0..nc {
                for c1 in (c0 + 1)..=nc {
                    let sel: Vec<usize> = (c0..c1).collect();
                    let (_o, sibs) = crate::blake3_tree::open_strip_set(&raw, &key, &sel);
                    let strip_bytes = &raw[c0 * 1024..c1 * 1024];
                    let (n_set, r_set) = {
                        let mut t = CompositeTrace::baseline_min();
                        t.place_matrix_strip_opening_set(0, strip_bytes, &sel, nc, &sibs, &key, 4, None, None)
                    };
                    let (n_rng, r_rng) = {
                        let (_o, sibs_r) = crate::blake3_tree::open_strip(&raw, &key, c0, c1);
                        let mut t = CompositeTrace::baseline_min();
                        t.place_matrix_strip_opening(0, strip_bytes, c0, c1, nc, &sibs_r, &key, 4, None, None)
                    };
                    assert_eq!((n_set, r_set), (n_rng, r_rng), "selective != range for [{c0},{c1}) of {nc}");
                }
            }
        }
    }

    /// **Positive AIR.** A strip opening verifies through the
    /// composite AIR, with the recomputed root bound to
    /// `PI_HASH_A` by the unchanged C3 constraint — and that PI
    /// equals the real full-matrix commitment.
    #[test]
    fn strip_opening_full_air_prove_and_verify() {
        let key = [0x99u8; 32];
        let nc = 4usize;
        let raw: Vec<u8> = (0..nc * 1024).map(|i| (i * 7 + 1) as u8).collect();
        let (c0, c1) = (1usize, 3usize);
        let (_o, sibs) = crate::blake3_tree::open_strip(&raw, &key, c0, c1);

        let mut trace = CompositeTrace::baseline_min();
        let (_n, root) = trace.place_matrix_strip_opening(
            0,
            &raw[c0 * 1024..c1 * 1024],
            c0,
            c1,
            nc,
            &sibs,
            &key,
            4,
            None, // hash-only (pre-cx.2; g=0)
            None,
        );
        let committed = b32_to_words(
            blake3::Hasher::new_keyed(&key)
                .update(&raw)
                .finalize()
                .as_bytes(),
        );
        assert_eq!(root, committed, "opened root must be the real commitment");

        let cfg = build_stark_config(&test_zk_params(), &CircuitConfig::TEST_PEARL);
        let pis = crate::composite_public::CompositePublicInputs::derive_from_matrix(&trace.matrix);
        assert_eq!(pis.hash_a, committed, "C3 PI bound to the commitment");
        let pis_vec = pis.to_vec();
        let proof = prove::<AiPowStarkConfig, _>(&cfg, &CompositeFullAir, trace.matrix, &pis_vec);
        verify::<AiPowStarkConfig, _>(&cfg, &CompositeFullAir, &proof, &pis_vec)
            .expect("strip opening must verify through the composite AIR");
    }

    /// **Adversarial — the §4.6 soundness statement.** Opening a
    /// *tampered* strip while the verifier holds the genuine
    /// committed `PI_HASH_A` ⇒ recomputed root ≠ commitment ⇒ C3
    /// rejects (BLAKE3 collision resistance). Same for a forged
    /// authentication sibling.
    #[test]
    fn strip_opening_rejects_tampered_strip_or_sibling() {
        let key = [0x2Au8; 32];
        let nc = 8usize; // non-trivial tree ⇒ ≥1 auth sibling
        let raw: Vec<u8> = (0..nc * 1024).map(|i| (i ^ 0xA5) as u8).collect();
        let (c0, c1) = (3usize, 5usize);
        let committed = b32_to_words(
            blake3::Hasher::new_keyed(&key)
                .update(&raw)
                .finalize()
                .as_bytes(),
        );
        let cfg = build_stark_config(&test_zk_params(), &CircuitConfig::TEST_PEARL);

        // (a) tampered strip byte.
        {
            let (_o, sibs) = crate::blake3_tree::open_strip(&raw, &key, c0, c1);
            let mut strip = raw[c0 * 1024..c1 * 1024].to_vec();
            strip[10] ^= 1;
            let mut trace = CompositeTrace::baseline_min();
            trace.place_matrix_strip_opening(0, &strip, c0, c1, nc, &sibs, &key, 4, None, None);
            let mut pis =
                crate::composite_public::CompositePublicInputs::derive_from_matrix(&trace.matrix);
            pis.hash_a = committed; // verifier holds the true commitment
            let pis_vec = pis.to_vec();
            let proof =
                prove::<AiPowStarkConfig, _>(&cfg, &CompositeFullAir, trace.matrix, &pis_vec);
            assert!(
                verify::<AiPowStarkConfig, _>(&cfg, &CompositeFullAir, &proof, &pis_vec).is_err(),
                "tampered strip must fail the C3 commitment binding"
            );
        }
        // (b) forged authentication sibling.
        {
            let (_o, mut sibs) = crate::blake3_tree::open_strip(&raw, &key, c0, c1);
            assert!(!sibs.is_empty(), "expected auth siblings for this range");
            sibs[0].cv[0] ^= 0x80;
            let mut trace = CompositeTrace::baseline_min();
            trace.place_matrix_strip_opening(
                0,
                &raw[c0 * 1024..c1 * 1024],
                c0,
                c1,
                nc,
                &sibs,
                &key,
                4,
                None,
                None,
            );
            let mut pis =
                crate::composite_public::CompositePublicInputs::derive_from_matrix(&trace.matrix);
            pis.hash_a = committed;
            let pis_vec = pis.to_vec();
            let proof =
                prove::<AiPowStarkConfig, _>(&cfg, &CompositeFullAir, trace.matrix, &pis_vec);
            assert!(
                verify::<AiPowStarkConfig, _>(&cfg, &CompositeFullAir, &proof, &pis_vec).is_err(),
                "forged auth sibling must fail the C3 commitment binding"
            );
        }
    }
}

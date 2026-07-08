//! Column layout for the M10.1c Pearl-style composite AIR.
//!
//! Direct port of `Pearl zk-pow pearl_layout.rs`. The full
//! column set is mirrored, including Pearl's `NOISED_PACKED` /
//! `A_NOISED` / `B_NOISED` / `MAT_ID` machinery for the matrix-tile
//! RAM lookups. The lookups are essential for production-scale
//! matmul (see `2026-05-14_M10_1C_DESIGN.md` decision 2 — without them, large
//! matrices would force per-row inline duplication of matrix bytes
//! that scales with the number of output tile cells rather than the
//! matrix size).
//!
//! Each row of the composite AIR's trace carries columns from every
//! chip side-by-side. Preprocessed control columns determine which
//! chip's constraints fire on which row. This file pins the column
//! offsets and lengths so the constraint code, trace generator, and
//! lookup configurations all agree.
//!
//! Phases that consume the layout:
//!
//! | Phase | Module |
//! |---|---|
//! | 3 | `stark_row_chip` (Pearl's `monotonic_increment.rs`) |
//! | 4 | `chips::input`, `chips::i8u8`, `chips::range_table` |
//! | 5 | `control_chip` (Pearl's `control_and_matid_packed.rs`) |
//! | 6 | preprocessed-trace generation |
//! | 7-8 | `blake3` chip (one round per row) |
//! | 9 | `matmul` chip |
//! | 10 | `jackpot` chip |
//! | 11 | `composite_lookups` (logUp configuration) |
//! | 12 | `composite_full_air::eval` (top-level) |
//! | 13 | `composite_trace` (top-level) |
//! | 14 | `lib::{prove, verify}` plumbing |
//!
//! ## Per-row schedule
//!
//! The trace is the union of two instruction streams:
//!
//!   * **BLAKE3 stream** — one ROUND per row. Pearl's
//!     `ROUNDS_PER_BLAKE_INSTRUCTION = 8`; an entire BLAKE3 hash
//!     occupies 8 consecutive rows.
//!   * **Matmul + Jackpot stream** — one stripe step or one
//!     XOR-fold instruction per row. Pearl interleaves these with
//!     BLAKE3 rounds; rows are "shared" via preprocessed selectors.
//!
//! Rows where no chip is active are padding (all selectors zero).
//!
//! ## Pearl bytes ↔ Plonky3 mapping
//!
//! Pearl packs `4 × i8` per Goldilocks element via the
//! `BYTES_PER_GOLDILOCKS = 4` constant. We keep that for the
//! `NOISED_PACKED`-equivalent inline a/b storage so byte-equivalence
//! with Pearl's matrix encoding is preserved (important for the
//! cross-AIR LogUp lookup against `blake3::Hasher::new_keyed`).

#![allow(clippy::module_inception)]

/// Packing factor: 4 i8 elements per Goldilocks element.
///
/// Matches Pearl's `pearl_columns::BYTES_PER_GOLDILOCKS` so the
/// underlying matrix-byte representation is byte-equivalent to ai-pow
/// / Pearl. Required for the merge-mining compat guarantee — the
/// in-circuit hash sees the same bytes a Pearl miner hashes.
pub const BYTES_PER_GOLDILOCKS: usize = 4;

/// Bits per range-table limb. Matches Pearl's
/// `pearl_columns::BITS_PER_LIMB = 13` — the u13 range table is
/// what decomposes `MAT_ID` (RAM-lookup index into `NOISED_PACKED`)
/// into limbs for range-checking. Production matrices need MAT_ID
/// values up to `(m + n) × k / BYTES_PER_GOLDILOCKS`, comfortably
/// within `2^26 = 2 × 13` bits.
pub const BITS_PER_LIMB: usize = 13;

/// Block-commitment fixed size in bytes. Pinned by M10.1c design (see
/// `2026-05-14_M10_1C_DESIGN.md` decision 4): 32 bytes = 8 × `u32` LE, matching
/// the Tip5 digest size we use elsewhere for Merkle commitments. The
/// in-circuit κ derivation hashes `block_commitment ‖ params_tag` (32
/// + 32 = 64 bytes = one BLAKE3 block, single compression call).
pub const BLOCK_COMMITMENT_BYTES: usize = 32;

/// Same in Goldilocks elements (4 bytes per u32, 8 u32s).
pub const BLOCK_COMMITMENT_WORDS: usize = BLOCK_COMMITMENT_BYTES / BYTES_PER_GOLDILOCKS;

/// Pearl-style tile dimensions (matches `Pearl zk-pow
/// pearl_program.rs:23-25`).
pub const TILE_D: usize = 16;
pub const TILE_H: usize = 2;
pub const JACKPOT_SIZE: usize = 16;

/// Left-rotation amount for Pearl's tile-state update (`pearl_program.rs:27`).
pub const LROT_PER_TILE: u32 = 13;

/// Per-instruction round count (matches Pearl's
/// `ROUNDS_PER_BLAKE_INSTRUCTION = 8`). BLAKE3 has 7 mixing rounds; the
/// 8th row in Pearl's layout is the output-XOR finalisation step.
pub const ROUNDS_PER_BLAKE_INSTRUCTION: usize = 8;

/// Minimum STARK trace length (matches Pearl's `MIN_STARK_LEN = 1 <<
/// 13 = 8192`). Below this, FRI doesn't have enough rows for our
/// `log_blowup + log_final_poly_len` setup; padding kicks in.
pub const MIN_STARK_LEN: usize = 1 << 13;

// =====================================================================
//  Column groups
//
//  Mirrors `Pearl zk-pow pearl_layout.rs:7-81`'s
//  `pearl_columns` block. We use a manual const-offset scheme rather
//  than a macro to keep dependencies tight.
//
//  Layout order:
//    range tables           (11 cols)
//    control flags          (22 cols incl. CONTROL_PREP)
//    input chip unpacking   (25 cols: MAT_UNPACK, UINT8_DATA, NOISE_PACKED_PREP, NOISE_UNPACK)
//    NOISED_PACKED + indexing (18 cols: NOISED_PACKED, MAT_FREQ, MAT_ID, MAT_ID_LIMBS, AB_ID_PREP, AB_ID_LIMBS, A_ID[4], B_ID[4])
//    STARK_ROW_IDX          (1 col)
//    matmul tile data       (80 cols: A_NOISED, A_NOISED_UNPACK, B_NOISED, B_NOISED_UNPACK)
//    matmul accumulator     (8 cols: CUMSUM_TILE, CUMSUM_BUFFER)
//    jackpot state          (56 cols: JACKPOT_MSG, BIT_REG, JACKPOT_IDX)
//    blake3 buffers         (49 cols: BLAKE3_MSG_BUFFER, CV_OR_TWEAK_PREP, CV_IN, BLAKE3_MSG, BLAKE3_CV)
//    blake3 round AIR       (1056 cols: BLAKE3_ROUND)
//    blake3 output          (9 cols: CV_OUT, CV_OUT_FREQ)
//    ───────────────────────────────────
//    TOTAL_TRACE_WIDTH      ≈ 1328 cols
//
//  All "PREP" columns are PREPROCESSED — committed at setup. Other
//  columns are part of the main trace, generated per-proof.
// =====================================================================

/// Range and conversion table columns. Each `*_TABLE` column lists
/// every value in the range exactly once; `*_FREQ` carries the
/// multiplicities used in the logUp argument. Pearl uses these to
/// verify all input values stay in their declared ranges.
pub const URANGE8_TABLE: usize = 0; // 0..=255
pub const URANGE8_FREQ: usize = 1;
pub const URANGE13_TABLE: usize = 2; // 0..=8191 (BITS_PER_LIMB = 13)
pub const URANGE13_FREQ: usize = 3;
pub const IRANGE7P1_TABLE: usize = 4; // -64..=64 (Pearl's signed-noise range)
pub const IRANGE7P1_FREQ: usize = 5;
pub const IRANGE8_TABLE: usize = 6; // -128..=127
pub const IRANGE8_FREQ: usize = 7;
pub const I8U8_TABLE: usize = 8; // i8 -> u8 conversion (size = 1 << 8)
pub const I8U8_AUX: usize = 9;
pub const I8U8_FREQ: usize = 10;

/// Number of range / conversion table columns.
pub const NUM_RANGE_COLS: usize = 11;

/// Preprocessed control word. Packs the selector bits *and* MAT_ID
/// (the index used for NOISED_PACKED RAM lookups). The composite
/// trace generator emits a little-endian unpacking constraint over
/// this column.
pub const CONTROL_PREP: usize = NUM_RANGE_COLS;

/// Per-chip selector bits (unpacked from `CONTROL_PREP`).
/// Pearl-mirror; matches `pearl_layout.rs:22-42` verbatim.
pub const IS_RESET_CUMSUM: usize = CONTROL_PREP + 1;
pub const IS_UPDATE_CUMSUM: usize = IS_RESET_CUMSUM + 1;
pub const IS_USE_JOB_KEY: usize = IS_UPDATE_CUMSUM + 1;
pub const IS_USE_COMMITMENT_HASH: usize = IS_USE_JOB_KEY + 1;
pub const IS_HASH_A: usize = IS_USE_COMMITMENT_HASH + 1;
pub const IS_HASH_B: usize = IS_HASH_A + 1;
pub const IS_HASH_JACKPOT: usize = IS_HASH_B + 1;
pub const IS_CV_IN: usize = IS_HASH_JACKPOT + 1;
pub const IS_NEW_BLAKE: usize = IS_CV_IN + 1;
pub const IS_LAST_ROUND: usize = IS_NEW_BLAKE + 1;
pub const IS_MSG_MAT: usize = IS_LAST_ROUND + 1;
pub const IS_MSG_JACKPOT: usize = IS_MSG_MAT + 1;
pub const IS_MSG_AUX_DATA: usize = IS_MSG_JACKPOT + 1;
pub const IS_MSG_CV: usize = IS_MSG_AUX_DATA + 1;
pub const IS_LOAD: usize = IS_MSG_CV + 1;
pub const IS_XOR: usize = IS_LOAD + 1;
pub const IS_SHIFT3: usize = IS_XOR + 1;
pub const IS_STORE0: usize = IS_SHIFT3 + 1;
pub const IS_STORE1: usize = IS_STORE0 + 1;
pub const IS_STORE2: usize = IS_STORE1 + 1;
pub const IS_DUMP_CUMSUM_BUFFER: usize = IS_STORE2 + 1;

/// Number of control / selector columns (CONTROL_PREP + 21 bits).
pub const NUM_CONTROL_COLS: usize = 22;

// =====================================================================
//  RAM-lookup machinery + input chip columns
//
//  Direct port of `pearl_layout.rs:43-80` covering the input-chip
//  unpacking columns (`MAT_UNPACK`, `UINT8_DATA`, `NOISE_PACKED_PREP`,
//  `NOISE_UNPACK`), the canonical noised-matrix store
//  (`NOISED_PACKED`, `MAT_FREQ`), the matmul-side per-row reads
//  (`A_NOISED`, `A_NOISED_UNPACK`, `B_NOISED`, `B_NOISED_UNPACK`),
//  the MAT_ID / AB_ID indexing infrastructure, and the BLAKE3 +
//  Jackpot column blocks.
//
//  Widths match Pearl exactly so byte-equivalence of the in-circuit
//  hash output to `blake3::Hasher::new_keyed` is preserved at the
//  trace level.
// =====================================================================

// ---- Matrix-byte unpacking ----

/// MAT_UNPACK. **§4.C.2 c-exact (cx.2/X1):** widened 8→64 — the
/// i8 committed-plain view of a co-located leaf round-0 row's
/// whole 64-byte block (mirrors `UINT8_DATA`'s u8 view), so
/// `InputChip`'s `NOISED_PACKED = polyval(MAT_UNPACK) +
/// polyval(NOISE_UNPACK)` extends to all 8 sub-slices reusing
/// M-S1's *exact* packing.
pub const MAT_UNPACK_LEN: usize = 64;
/// Active `MAT_UNPACK` lookup window. cx.2 makes the full
/// 64-byte co-located block live so hash and matmul cannot split
/// views across sub-slices.
pub const MAT_UNPACK_WIN: usize = MAT_UNPACK_LEN;
pub const MAT_UNPACK_START: usize = CONTROL_PREP + NUM_CONTROL_COLS;

/// UINT8_DATA. **§4.C.2 c-exact (cx.2/X1):** widened 8→64 so a
/// strip-opening leaf round-0 row carries its whole 64-byte
/// committed block (per-word C3 binds all 16 `BLAKE3_MSG` words
/// to it ⇒ `UINT8_DATA[0..64]` ≡ the committed block ∈ `HASH_A`;
/// each swept store window is the 8-byte sub-slice
/// `UINT8_DATA[8p..8p+8]`).
pub const UINT8_DATA_LEN: usize = 64;
/// Active `UINT8_DATA` lookup window. cx.2 makes all 64 bytes of
/// a co-located block live under the u8 and i8-u8 buses.
pub const UINT8_DATA_WIN: usize = UINT8_DATA_LEN;
pub const UINT8_DATA_START: usize = MAT_UNPACK_START + MAT_UNPACK_LEN;

/// NOISE_PACKED_PREP: **§4.C.2 c-exact (cx.2-pcols/X1):** widened
/// 1→8 preprocessed cols (one `polyval(noise_subslice,129)` per
/// 8-byte sub-slice). On a co-located strip-opening leaf round-0
/// row the verifier pins all 8 sub-slices' noise (the A3.2b
/// discipline, per-block); `NOISE_PACKED_PREP` (= the START
/// index) keeps the cell-0 semantics every current consumer uses
/// (InputChip eqn1, the A3.2b store row), so the 7 added cols are
/// zero-default while `g = IS_MSG_MAT·IS_NEW_BLAKE = 0`
/// everywhere (no co-location yet) ⇒ byte-identical (zero-blast).
/// Mirrors Pearl `pearl_layout.rs:51`.
pub const NOISE_PACKED_PREP: usize = UINT8_DATA_START + UINT8_DATA_LEN;
/// Number of `NOISE_PACKED_PREP` preprocessed cols (cx.2/X1: 8
/// sub-slice pins per co-located leaf block; was 1).
pub const NOISE_PACKED_PREP_LEN: usize = 8;

/// NOISE_UNPACK. **§4.C.2 c-exact (cx.2/X1):** widened 8→64 so a
/// co-located leaf round-0 row carries its block's per-position
/// `noise_ref` for all 8 sub-slices.
pub const NOISE_UNPACK_LEN: usize = 64;
/// Active `NOISE_UNPACK` lookup window for the full co-located block.
pub const NOISE_UNPACK_WIN: usize = NOISE_UNPACK_LEN;
pub const NOISE_UNPACK_START: usize = NOISE_PACKED_PREP + NOISE_PACKED_PREP_LEN;

// ---- NOISED_PACKED: canonical noised-matrix data table ----

/// NOISED_PACKED: 2 cols, holding `MAT + NOISE` packed as 4 i8 per
/// Goldilocks. Read by both the matmul chip (via `A_NOISED` /
/// `B_NOISED` LogUp on `MAT_ID`) and the BLAKE3 leaf rows (via
/// `IS_MSG_MAT` → `UINT8_DATA` after i8↔u8 conversion). This is
/// the cryptographic glue forcing matmul and BLAKE3 to see the
/// same bytes.
/// **§4.C.2 c-exact (cx.2/X1):** widened 2→16 (2 packed cells ×
/// 8 sub-slices) so the co-located leaf round-0 row is the M-S1
/// `noised_packed` producer for every swept 8-byte sub-slice of
/// its block, reusing M-S1's *exact* 2-cell packing per
/// sub-slice (avoids any i8/u8 bus-key reconciliation).
/// Consumers use the 2-cell window [`NOISED_PACKED_WIN`] until
/// cx.2 activation (zero-blast: 14 added cols zero-default).
pub const NOISED_PACKED_LEN: usize = 16;
/// The active 2-cell `NOISED_PACKED` window until cx.2/X1 activation.
pub const NOISED_PACKED_WIN: usize = 2;
pub const NOISED_PACKED_START: usize = NOISE_UNPACK_START + NOISE_UNPACK_LEN;

/// MAT_FREQ. **§4.C.2 c-exact (cx.2/X1):** widened 1→8 — one
/// `noised_packed` table-side multiplicity per 8-byte sub-slice
/// of a co-located leaf round-0 row (the row is the M-S1
/// producer for all 8 of its block's sub-slices, so it publishes
/// 8 keys, each with its own `MAT_FREQ`). Consumers use
/// `MAT_FREQ` (= cell 0) until the cx.2 g=1 activation ⇒ the 7
/// added cols are zero-default/unused, byte-identical
/// (zero-blast — the cx.1b-layout discipline).
pub const MAT_FREQ: usize = NOISED_PACKED_START + NOISED_PACKED_LEN;
/// Number of `MAT_FREQ` cols (cx.2/X1: 8 sub-slice freqs/row;
/// was 1).
pub const MAT_FREQ_LEN: usize = 8;

// ---- MAT_ID and AB_ID indexing ----

/// MAT_ID: 1 col. Compact identifier of this row's matrix position
/// in NOISED_PACKED. Derived from `CONTROL_PREP`.
pub const MAT_ID: usize = MAT_FREQ + MAT_FREQ_LEN;

/// MAT_ID_LIMBS: 2 cols. Range check for MAT_ID via two 13-bit
/// limbs (since `BITS_PER_LIMB = 13`, 2 limbs cover values up to
/// `2^26` which accommodates all production matrix sizes).
pub const MAT_ID_LIMBS_LEN: usize = 2;
pub const MAT_ID_LIMBS_START: usize = MAT_ID + 1;

/// AB_ID_PREP: 1 preprocessed col. Packs the first `A_ID || B_ID`
/// pair for the matmul tile load. The full per-sub-slice IDs below
/// are pinned directly as program columns; this legacy pack keeps
/// the original limb decomposition useful for the first pair.
pub const AB_ID_PREP: usize = MAT_ID_LIMBS_START + MAT_ID_LIMBS_LEN;

/// AB_ID_LIMBS: 4 cols. Range check for AB_ID_PREP.
pub const AB_ID_LIMBS_LEN: usize = 4;
pub const AB_ID_LIMBS_START: usize = AB_ID_PREP + 1;

/// A_ID/B_ID: one position ID per consumed 8-byte sub-slice. These
/// index into NOISED_PACKED via LogUp on the matmul chip.
pub const A_ID_LEN: usize = A_NOISED_LEN / 2;
pub const B_ID_LEN: usize = B_NOISED_LEN / 2;
pub const A_ID: usize = AB_ID_LIMBS_START + AB_ID_LIMBS_LEN;
pub const B_ID: usize = A_ID + A_ID_LEN;

// ---- STARK_ROW_IDX ----

/// STARK_ROW_IDX: 1 col, monotonically increasing from 0. Used by
/// the CV-routing lookup to address arbitrary previous rows.
pub const STARK_ROW_IDX: usize = B_ID + B_ID_LEN;

// ---- Per-row matmul tile data ----

/// A_NOISED: 8 packed Goldilocks cols (TILE_H × TILE_D / 4).
/// Per-matmul-row read of A's tile from NOISED_PACKED[A_ID].
pub const A_NOISED_LEN: usize = TILE_H * TILE_D / BYTES_PER_GOLDILOCKS;
pub const A_NOISED_START: usize = STARK_ROW_IDX + 1;

/// A_NOISED_UNPACK: 32 i8 cols (TILE_H × TILE_D). Unpacked from
/// A_NOISED; range-checked via IRANGE8.
pub const A_NOISED_UNPACK_LEN: usize = TILE_H * TILE_D;
pub const A_NOISED_UNPACK_START: usize = A_NOISED_START + A_NOISED_LEN;

/// B_NOISED: 8 packed Goldilocks cols (TILE_W × TILE_D / 4 = TILE_H
/// × TILE_D / 4 since Pearl uses TILE_W = TILE_H). Per-matmul-row
/// read of B's tile from NOISED_PACKED[B_ID].
pub const B_NOISED_LEN: usize = TILE_H * TILE_D / BYTES_PER_GOLDILOCKS;
pub const B_NOISED_START: usize = A_NOISED_UNPACK_START + A_NOISED_UNPACK_LEN;

/// B_NOISED_UNPACK: 32 i8 cols.
pub const B_NOISED_UNPACK_LEN: usize = TILE_H * TILE_D;
pub const B_NOISED_UNPACK_START: usize = B_NOISED_START + B_NOISED_LEN;

// ---- Cumulative-sum (matmul accumulator) + jackpot ----

/// CUMSUM_TILE: 4 i32 cols (TILE_H × TILE_W = 2 × 2 = 4).
pub const CUMSUM_TILE_LEN: usize = TILE_H * TILE_H;
pub const CUMSUM_TILE_START: usize = B_NOISED_UNPACK_START + B_NOISED_UNPACK_LEN;

/// CUMSUM_BUFFER: 4 i32 cols. Used for buffering CUMSUM_TILE across
/// the jackpot XOR-fold steps.
pub const CUMSUM_BUFFER_LEN: usize = TILE_H * TILE_H;
pub const CUMSUM_BUFFER_START: usize = CUMSUM_TILE_START + CUMSUM_TILE_LEN;

/// JACKPOT_MSG: 16 u32 cols. The 16-slot tile-state value M that
/// the jackpot chip evolves via rotate-XOR-13.
pub const JACKPOT_MSG_LEN: usize = JACKPOT_SIZE;
pub const JACKPOT_MSG_START: usize = CUMSUM_BUFFER_START + CUMSUM_BUFFER_LEN;

/// BIT_REG: 32 boolean cols. Bitwise representation of one u32
/// from JACKPOT_MSG, used for the XOR + rotate operations.
pub const BIT_REG_LEN: usize = 32;
pub const BIT_REG_START: usize = JACKPOT_MSG_START + JACKPOT_MSG_LEN;

/// JACKPOT_IDX: 8 cols. Indicators `is_store[i]` / `is_load[i]`
/// for i in 0..16 — one-hot per row.
pub const JACKPOT_IDX_LEN: usize = 8;
pub const JACKPOT_IDX_START: usize = BIT_REG_START + BIT_REG_LEN;

// ---- BLAKE3 buffers, message, CVs ----

/// BLAKE3_MSG_BUFFER: 16 u32 cols. Multi-round staging buffer; in
/// round 8 holds the message that entered round 1.
pub const BLAKE3_MSG_BUFFER_LEN: usize = 16;
pub const BLAKE3_MSG_BUFFER_START: usize = JACKPOT_IDX_START + JACKPOT_IDX_LEN;

/// CV_OR_TWEAK_PREP: 1 preprocessed col. Either a row index (CV
/// lookup) or BLAKE3 compression tweak flags (counter / block_len
/// / flags).
pub const CV_OR_TWEAK_PREP: usize = BLAKE3_MSG_BUFFER_START + BLAKE3_MSG_BUFFER_LEN;

/// CV_IN: 8 u32 cols. Read from `CV_OUT_PACKED` at the row indexed
/// by `CV_OR_TWEAK_PREP` via LogUp.
pub const CV_IN_LEN: usize = 8;
pub const CV_IN_START: usize = CV_OR_TWEAK_PREP + 1;

/// BLAKE3_MSG: 16 u32 cols. Message entering BLAKE3, packed LE 4
/// bytes per Goldilocks (u8 view).
pub const BLAKE3_MSG_LEN: usize = 16;
pub const BLAKE3_MSG_START: usize = CV_IN_START + CV_IN_LEN;

/// BLAKE3_CV: 8 u32 cols. CVs ready for BLAKE3.
pub const BLAKE3_CV_LEN: usize = 8;
pub const BLAKE3_CV_START: usize = BLAKE3_MSG_START + BLAKE3_MSG_LEN;

/// BLAKE3_ROUND: 1056 cols. The per-round AIR ensuring this row's
/// BLAKE3 round was executed correctly. Pearl pins this at exactly
/// 1056; CV_OUT of the last round contains the BLAKE3 output.
pub const BLAKE3_ROUND_LEN: usize = 1056;
pub const BLAKE3_ROUND_START: usize = BLAKE3_CV_START + BLAKE3_CV_LEN;

/// CV_OUT: 8 u32 cols. Output CV of BLAKE3 for this row.
pub const CV_OUT_LEN: usize = 8;
pub const CV_OUT_START: usize = BLAKE3_ROUND_START + BLAKE3_ROUND_LEN;

// ---- Jackpot chip extensions (Phase 12d) ----
//
// The jackpot chip uses three column blocks not present in
// Pearl's original layout: 32 boolean cells for the rotate-XOR
// operand's bit decomposition, and 16 boolean cells encoding the
// active slot as a one-hot indicator. Appended after CV_OUT to
// preserve all earlier offsets.

/// JACKPOT_X_BITS: 32 boolean cells. Bit-decomposition of the
/// XOR-fold value `x` consumed by the jackpot chip's rotate-XOR-13
/// update. Sourced upstream (Phase 14b will tie these via LogUp to
/// CUMSUM_BUFFER's bit decomposition).
pub const JACKPOT_X_BITS_START: usize = CV_OUT_START + CV_OUT_LEN;
pub const JACKPOT_X_BITS_LEN: usize = 32;

/// JACKPOT_SLOT_SEL: 16 boolean cells, one-hot indicator selecting
/// which slot the current row updates. Sum equals
/// `IS_HASH_JACKPOT` (1 on an active jackpot row, 0 otherwise).
pub const JACKPOT_SLOT_SEL_START: usize = JACKPOT_X_BITS_START + JACKPOT_X_BITS_LEN;
pub const JACKPOT_SLOT_SEL_LEN: usize = 16;

/// CV_OUT_FREQ: 1 col. LogUp multiplicity for the CV-routing
/// lookup (how many later rows read this row's CV_OUT).
pub const CV_OUT_FREQ: usize = JACKPOT_SLOT_SEL_START + JACKPOT_SLOT_SEL_LEN;

// =====================================================================
//  HIGH-2.2 §4.A/§4.D — FoldChip composite block (Option B2)
//
//  Appended *after* all Pearl-mirrored columns (the standing
//  invariant is that our SNARK is deliberately NOT trace-byte-
//  equivalent to Pearl — only the mineable unit of work is;
//  2026-05-15_HIGH2_2_DESIGN.md §9.5). Appending here shifts no existing
//  offset (every column above is fixed); only TOTAL_TRACE_WIDTH
//  grows. The FoldChip is a pure function of the per-stripe
//  X_STEP sequence (§4.0), so this block carries exactly its
//  standalone columns at composite offsets.
// =====================================================================

/// 1 = active fold row, 0 = passthrough/padding.
pub const FOLD_IS_FOLD: usize = CV_OUT_FREQ + 1;
/// One-hot slot selector (Σ == FOLD_IS_FOLD); slot = stripe % 16.
pub const FOLD_SLOT_SEL_START: usize = FOLD_IS_FOLD + 1;
pub const FOLD_SLOT_SEL_LEN: usize = JACKPOT_SIZE; // 16
/// Per-stripe X_STEP value (u32 reinterpretation of the i32 fold
/// input — Pearl §4.5 `x_ℓ`).
pub const FOLD_XSTEP: usize = FOLD_SLOT_SEL_START + FOLD_SLOT_SEL_LEN;
/// 32 LE bits of FOLD_XSTEP.
pub const FOLD_XSTEP_BITS_START: usize = FOLD_XSTEP + 1;
pub const FOLD_XSTEP_BITS_LEN: usize = 32;
/// 16-word fold state entering this row (`M_cur`), u32 each.
pub const FOLD_STATE_START: usize = FOLD_XSTEP_BITS_START + FOLD_XSTEP_BITS_LEN;
pub const FOLD_STATE_LEN: usize = JACKPOT_SIZE; // 16
/// 32 LE bits of the currently addressed slot (`M_cur[slot]`).
pub const FOLD_MCUR_BITS_START: usize = FOLD_STATE_START + FOLD_STATE_LEN;
pub const FOLD_MCUR_BITS_LEN: usize = 32;
/// Materialised XOR result `X_STEP ⊕ rotl13(M_cur[slot])` (u32).
/// Splitting the fold's `is_fold·(Σ sel·nxt − acc)` (degree 3)
/// into `FOLD_XOR_OUT == acc` (deg 2) + `Σ sel·nxt ==
/// is_fold·FOLD_XOR_OUT` (deg 2) keeps every FoldChip constraint
/// ≤ degree 2 — required for the batch-stark quotient (§4.A
/// root cause; the deg-3 form failed only there, with non-zero
/// FOLD values).
pub const FOLD_XOR_OUT: usize = FOLD_MCUR_BITS_START + FOLD_MCUR_BITS_LEN;

// =====================================================================
//  HIGH-2.2 §6(b) — StripeXorChip composite block
//
//  Cross-row transport that reduces the sub-block-major matmul
//  sweep's per-row accumulator-after-step to the per-stripe
//  `x_steps` scalars (a `STATE_LEN`-lane carried XOR register),
//  binding `FOLD_XSTEP` to the committed-matrix accumulator. Mirror
//  of `chips::stripe_xor::cols` at composite offsets (appended after
//  the FoldChip block; shifts no existing offset). This is the
//  "dominant new width" the design anticipated (~294 cols) — main
//  trace only, NOT preprocessed (the §4.C.8 ~10x trap is about
//  preprocessed width; the schedule is pinned via the §6(a)
//  CONTROL_PREP pattern, not a wide preprocessed block).
// =====================================================================

/// HIGH-2.2 §6(b)-G2 — per-stripe lane count (`STRIPE_MAX`),
/// decoupled from the FoldChip's Pearl-fixed 16 M-slots
/// (`JACKPOT_SIZE`). Covers every single-Layer-0 params set
/// (rectangular `llm_shape` `k/r = 20`; PROD-per-segment
/// `k/r = 64`). MUST equal `chips::stripe_xor::STATE_LEN`.
pub const STRIPE_MAX: usize = 64;

// =====================================================================
//  HIGH-2.2 §6(b) StripeXor block — split into the block-own
//  contiguous chain and the §M-S5b Path A column-overlay region.
//
//  Block-own (this chain): the columns that are NOT overlay-eligible
//  — `SX_IS_ACTIVE`, `SX_LANE_SEL`, `SX_XR` (the stateful cross-row
//  register), `SX_NEW_SEL` (read cross-row, ungated, by the register
//  passthrough), `SX_Q`, and `SX_IN`. `SX_IN` is **not** overlay-
//  eligible despite being O0-Stage-1-gated: it holds signed-i32
//  accumulator values, and the BLAKE3 round AIR boolean-checks its
//  XOR-input bit columns *unconditionally* (`round_ops.rs`
//  `xor_32_shift_if` — an ungated `assert_bool`), so only genuine
//  bit-valued columns may alias the BLAKE3-round region.
//
//  Overlaid (aliased into `blake3_round`, defined after `SX_END`):
//  the genuine bit columns `SX_IN_BITS`, `SX_XR_SEL_BITS`,
//  `SX_NEW_SEL_BITS` — all 0/1, so they satisfy the BLAKE3 ungated
//  booleanity wherever they land.
// =====================================================================

/// 1 = active stripe-XOR fold row, 0 = padding/passthrough.
pub const SX_IS_ACTIVE: usize = FOLD_XOR_OUT + 1;
/// One-hot stripe-lane selector (`Σ == SX_IS_ACTIVE`).
pub const SX_LANE_SEL_START: usize = SX_IS_ACTIVE + 1;
pub const SX_LANE_SEL_LEN: usize = STRIPE_MAX;
/// The 4 accumulator cells folded this row (= the matmul
/// accumulator-after-step; bound cross-row to `nxt.CUMSUM_TILE`).
/// Block-own: signed-i32 values, not bit columns ⇒ not overlay-
/// eligible (see the block header).
pub const SX_IN_START: usize = SX_LANE_SEL_START + SX_LANE_SEL_LEN;
pub const SX_IN_LEN: usize = 4;
/// `STRIPE_MAX`-lane register entering this row (the stateful
/// cross-row register — not overlay-eligible).
pub const SX_XR_START: usize = SX_IN_START + SX_IN_LEN;
pub const SX_XR_LEN: usize = STRIPE_MAX;
/// New selected-lane value (`= XR[lane] ⊕ ⊕SX_IN`). Read cross-row
/// (ungated) by the register passthrough ⇒ block-own.
pub const SX_NEW_SEL: usize = SX_XR_START + SX_XR_LEN;
/// Parity quotient — one value column per output-bit position,
/// `Q[i] ∈ {0,1,2}`, range-bounded by the cubic `Q(Q−1)(Q−2)=0`.
/// **2026-05-21 width reduction:** was `32 × 2` boolean columns
/// (`SX_Q_BITS`); reclaimed 32 columns by storing the value
/// directly. Block-own — the cubic is ungated (degree 3 = the
/// budget), so `SX_Q` stays dedicated + zero on non-SX rows.
pub const SX_Q_START: usize = SX_NEW_SEL + 1;
pub const SX_Q_LEN: usize = 32;
/// End-of-StripeXor **block-own** cursor.
pub const SX_END: usize = SX_Q_START + SX_Q_LEN;

// =====================================================================
//  §M-S5b Path A column-overlay — StripeXor bit columns aliased into
//  the `blake3_round` region.
//
//  StripeXor activity is co-located on the matmul sweep
//  (`composite_trace.rs`), which is mutually exclusive with BLAKE3
//  rounds. Soundness of the alias: each overlaid run's StripeXor
//  constraints are `SX_IS_ACTIVE`-gated (Path A O0-Stage-1,
//  `chips/stripe_xor.rs`) so they are vacuous on BLAKE3 rows, and
//  `verify_round` is gated off on matmul rows (`chips/blake3/chip.rs`
//  `round_gate_excl`, via the pinned matmul selectors) so it is
//  vacuous where the columns hold StripeXor data. The runs are all
//  genuine 0/1 bit columns, so they also satisfy the BLAKE3 round
//  AIR's *ungated* bit booleanity (`round_ops.rs xor_32_shift_if`).
//  They occupy a contiguous sub-window at the head of `blake3_round`.
// =====================================================================

/// 32 LE bits per `SX_IN` cell.
pub const SX_IN_BITS_LEN: usize = SX_IN_LEN * 32; // 128
/// 32 LE bits of the selected lane value.
pub const SX_XR_SEL_BITS_LEN: usize = 32;
/// 32 LE bits of `SX_NEW_SEL`.
pub const SX_NEW_SEL_BITS_LEN: usize = 32;

/// `SX_IN_BITS` aliases `blake3_round[0 .. 128]`.
pub const SX_IN_BITS_START: usize = BLAKE3_ROUND_START;
/// `SX_XR_SEL_BITS` aliases `blake3_round[128 .. 160]`.
pub const SX_XR_SEL_BITS_START: usize = SX_IN_BITS_START + SX_IN_BITS_LEN;
/// `SX_NEW_SEL_BITS` aliases `blake3_round[160 .. 192]`.
pub const SX_NEW_SEL_BITS_START: usize = SX_XR_SEL_BITS_START + SX_XR_SEL_BITS_LEN;
/// Total `blake3_round` sub-window consumed by the SX overlay.
pub const SX_OVERLAY_LEN: usize = SX_IN_BITS_LEN + SX_XR_SEL_BITS_LEN + SX_NEW_SEL_BITS_LEN;

// =====================================================================
//  HIGH-2.2 §6(b)-G2 — per-fold-row stripe selector
//
//  The §6(b) keystone binds `FOLD_XSTEP == SX_XR[stripe]`. The
//  FoldChip's `FOLD_SLOT_SEL` addresses M-slot `stripe % 16` (16
//  wide) — only equal to the stripe for `num_stripes ≤ 16`. This
//  one-hot (set by `place_fold_chain`, summing to `FOLD_IS_FOLD`,
//  with its 6-bit index pinned into `CONTROL_PREP` via the §6(a)
//  pattern) is the verifier-fixed `stripe` selector the keystone
//  uses. Appended after the SX block (shifts no existing offset).
// =====================================================================

/// Per-fold-row one-hot stripe selector (`Σ == FOLD_IS_FOLD`);
/// its 6-bit index is CRIT-1-pinned in `CONTROL_PREP` (§6(a)).
pub const FOLD_STRIPE_SEL_START: usize = SX_END;
pub const FOLD_STRIPE_SEL_LEN: usize = STRIPE_MAX;
/// End-of-FOLD_STRIPE_SEL cursor.
pub const FOLD_STRIPE_SEL_END: usize = FOLD_STRIPE_SEL_START + FOLD_STRIPE_SEL_LEN;

// =====================================================================
//  §4.C.2 c-exact (cx.1) — per-row BLAKE3 message word-pair selector
//
//  c-exact co-locates the M-S1 store rows onto the strip-opening
//  leaf-chunk round-0 rows so the *proven* C3 binds `MAT_UNPACK`
//  to the EXACT committed bytes ∈ `HASH_A`. cx.0 (KAT, `2bbf4cd`)
//  proved each store window lives at leaf message word-pair
//  `(2p, 2p+1)`, `p = word_off/2 ∈ 0..8`, at a witness-free
//  address. This one-hot selects that pair so the generalized C3
//  binds `BLAKE3_MSG[2p+j]` (not the fixed words {0,1}); its
//  3-bit index `p` is CRIT-1-pinned in `CONTROL_PREP` (the proven
//  §6(b)/G2 `FOLD_STRIPE_SEL` pattern). Appended after
//  `FOLD_STRIPE_SEL` (shifts no existing offset); main-trace only
//  (NOT preprocessed — the §4.C.8 trap is preprocessed width).
//  Zero-default ⇒ `Σ MSG_PAIR_SEL = 0` and `pair_idx = 0` on
//  every existing trace ⇒ generalized C3 vacuous + `CONTROL_PREP`
//  byte-identical (the §6(a) zero-blast property; cx.1b-layout
//  here adds only the zero columns — no constraint yet).
// =====================================================================

/// Per-(matrix-leaf round-0) one-hot BLAKE3 message word-pair
/// selector for the generalized C3 (`Σ == IS_MSG_MAT·IS_NEW_BLAKE`,
/// enforced by `CompositeFullAir` at cx.1b-constraints); its 3-bit
/// index is CRIT-1-pinned in `CONTROL_PREP` at cx.1c.
pub const MSG_PAIR_SEL_START: usize = FOLD_STRIPE_SEL_END;
pub const MSG_PAIR_SEL_LEN: usize = 8;
/// End-of-MSG_PAIR_SEL cursor.
pub const MSG_PAIR_SEL_END: usize = MSG_PAIR_SEL_START + MSG_PAIR_SEL_LEN;

// =====================================================================
//  Total trace width
// =====================================================================

/// Total trace width: pinned end-of-layout cursor. Phases 3+ extend
/// chip-internal sub-columns but must not exceed this without bumping
/// the constant.
pub const TOTAL_TRACE_WIDTH: usize = MSG_PAIR_SEL_END;

#[cfg(test)]
mod tests {
    use super::*;

    /// **Inner composite-AIR column inventory (2026-05-21).**
    ///
    /// Exhaustive per-chip-group accounting of all `TOTAL_TRACE_WIDTH`
    /// columns — the measurement underpinning the inner-AIR
    /// width-reduction analysis (`docs/2026-05-21_INNER_AIR_WIDTH_REDUCTION.md`).
    /// The 15 groups partition the trace; the test asserts (a) each
    /// group's column count from the layout constants, (b) the groups
    /// sum to `TOTAL_TRACE_WIDTH`, and (c) the dominant groups. Any
    /// future layout change that shifts the per-group split trips this
    /// test, keeping the width-reduction analysis honest.
    ///
    /// Run with `--nocapture` to print the breakdown table.
    #[test]
    fn inner_air_column_inventory() {
        // Per-group column counts, derived from the layout constants.
        let range_tables = NUM_RANGE_COLS; // 11
        let control = NUM_CONTROL_COLS; // 22
        let input_unpacking =
            MAT_UNPACK_LEN + UINT8_DATA_LEN + NOISE_PACKED_PREP_LEN + NOISE_UNPACK_LEN; // 200
        let noised_packed_indexing = NOISED_PACKED_LEN
            + MAT_FREQ_LEN
            + 1 // MAT_ID
            + MAT_ID_LIMBS_LEN
            + 1 // AB_ID_PREP
            + AB_ID_LIMBS_LEN
            + A_ID_LEN
            + B_ID_LEN
            + 1; // STARK_ROW_IDX
        let matmul_tile = A_NOISED_LEN + A_NOISED_UNPACK_LEN + B_NOISED_LEN + B_NOISED_UNPACK_LEN; // 80
        let matmul_accum = CUMSUM_TILE_LEN + CUMSUM_BUFFER_LEN; // 8
        let jackpot_state = JACKPOT_MSG_LEN + BIT_REG_LEN + JACKPOT_IDX_LEN; // 56
        let blake3_buffers = BLAKE3_MSG_BUFFER_LEN + 1 /* CV_OR_TWEAK_PREP */ + CV_IN_LEN + BLAKE3_MSG_LEN
                + BLAKE3_CV_LEN; // 49
        let blake3_round = BLAKE3_ROUND_LEN; // 1056
        let blake3_output = CV_OUT_LEN; // 8
        let jackpot_xbits = JACKPOT_X_BITS_LEN + JACKPOT_SLOT_SEL_LEN + 1 /* CV_OUT_FREQ */; // 49
        let fold = 1 /* FOLD_IS_FOLD */
            + FOLD_SLOT_SEL_LEN
            + 1 /* FOLD_XSTEP */
            + FOLD_XSTEP_BITS_LEN
            + FOLD_STATE_LEN
            + FOLD_MCUR_BITS_LEN
            + 1; // FOLD_XOR_OUT
                 // §M-S5b Path A column-overlay: the SX bit runs (SX_IN_BITS,
                 // SX_XR_SEL_BITS, SX_NEW_SEL_BITS) are aliased into the
                 // BLAKE3-round region (counted under `blake3_round`); the
                 // StripeXor block-own columns — incl. SX_IN (signed-i32, not
                 // overlay-eligible) — remain here.
        let sx = 1 /* SX_IS_ACTIVE */
            + SX_LANE_SEL_LEN
            + SX_IN_LEN
            + SX_XR_LEN
            + 1 /* SX_NEW_SEL */
            + SX_Q_LEN;
        let fold_stripe_sel = FOLD_STRIPE_SEL_LEN; // 64
        let msg_pair_sel = MSG_PAIR_SEL_LEN; // 8

        let groups: [(&str, usize); 15] = [
            ("range_tables", range_tables),
            ("control", control),
            ("input_unpacking", input_unpacking),
            ("noised_packed_indexing", noised_packed_indexing),
            ("matmul_tile", matmul_tile),
            ("matmul_accum", matmul_accum),
            ("jackpot_state", jackpot_state),
            ("blake3_buffers", blake3_buffers),
            ("blake3_round", blake3_round),
            ("blake3_output", blake3_output),
            ("jackpot_xbits", jackpot_xbits),
            ("fold", fold),
            ("sx_stripe", sx),
            ("fold_stripe_sel", fold_stripe_sel),
            ("msg_pair_sel", msg_pair_sel),
        ];

        let sum: usize = groups.iter().map(|(_, n)| *n).sum();

        std::eprintln!("\n=== INNER COMPOSITE-AIR COLUMN INVENTORY ===");
        std::eprintln!("  {:<26} {:>6} {:>8}", "group", "cols", "% width");
        std::eprintln!("  {}", "-".repeat(44));
        let mut sorted = groups;
        sorted.sort_by(|a, b| b.1.cmp(&a.1));
        for (name, n) in sorted {
            std::eprintln!(
                "  {:<26} {:>6} {:>7.1}%",
                name,
                n,
                100.0 * n as f64 / TOTAL_TRACE_WIDTH as f64
            );
        }
        std::eprintln!("  {}", "-".repeat(44));
        std::eprintln!("  {:<26} {:>6}", "TOTAL", sum);
        std::eprintln!();

        // (a) per-group pinned counts.
        assert_eq!(range_tables, 11);
        assert_eq!(control, 22);
        assert_eq!(input_unpacking, 200);
        assert_eq!(noised_packed_indexing, 41);
        assert_eq!(matmul_tile, 80);
        assert_eq!(matmul_accum, 8);
        assert_eq!(jackpot_state, 56);
        assert_eq!(blake3_buffers, 49);
        assert_eq!(blake3_round, 1056);
        assert_eq!(blake3_output, 8);
        assert_eq!(jackpot_xbits, 49);
        assert_eq!(fold, 99);
        // sx_stripe: 166 — the StripeXor block-own columns
        // (SX_IS_ACTIVE 1 + SX_LANE_SEL 64 + SX_IN 4 + SX_XR 64 +
        // SX_NEW_SEL 1 + SX_Q 32). The 192 bit columns (SX_IN_BITS,
        // SX_XR_SEL_BITS, SX_NEW_SEL_BITS) are aliased into the
        // BLAKE3-round region (§M-S5b Path A column-overlay).
        assert_eq!(sx, 166);
        assert_eq!(fold_stripe_sel, 64);
        assert_eq!(msg_pair_sel, 8);

        // (b) the 15 groups exactly partition the trace.
        assert_eq!(
            sum, TOTAL_TRACE_WIDTH,
            "inventory groups ({sum}) must sum to TOTAL_TRACE_WIDTH ({TOTAL_TRACE_WIDTH})"
        );

        // (c) the dominant group is the BLAKE3 round AIR — ~half the
        // trace (49.5%). This is the primary inner-AIR width-reduction
        // target. Of its 1056 columns, 1024 (4 snapshots × 2×128
        // bit-decomposition rows) are full 32-bit bit-decompositions
        // of the XOR-side state cells — the single largest reducible
        // structure in the inner AIR.
        assert!(
            blake3_round * 100 >= TOTAL_TRACE_WIDTH * 49,
            "blake3_round ({blake3_round}) should be ≥49% of the {TOTAL_TRACE_WIDTH}-col trace"
        );
        // §M-S5b Path A column-overlay folded the SX bit/data runs
        // into blake3_round, shrinking the trace; the block-own
        // `sx_stripe` group is now small and the two-group share is
        // ≈64% of the smaller trace (was ≈68% pre-overlay).
        assert!(
            (blake3_round + sx) * 100 >= TOTAL_TRACE_WIDTH * 60,
            "blake3_round + sx_stripe should be ≥60% of the trace"
        );
    }

    /// Phase-2 / Phase-2.5 const-pinning anchor: any reshuffling
    /// of the layout triggers this test, forcing the constraint
    /// code / trace generator / lookups to update in lockstep.
    #[test]
    fn layout_constants_pin() {
        assert_eq!(BYTES_PER_GOLDILOCKS, 4);
        assert_eq!(BITS_PER_LIMB, 13);
        assert_eq!(BLOCK_COMMITMENT_BYTES, 32);
        assert_eq!(BLOCK_COMMITMENT_WORDS, 8);
        assert_eq!(TILE_D, 16);
        assert_eq!(TILE_H, 2);
        assert_eq!(JACKPOT_SIZE, 16);
        assert_eq!(LROT_PER_TILE, 13);
        assert_eq!(ROUNDS_PER_BLAKE_INSTRUCTION, 8);
        assert_eq!(MIN_STARK_LEN, 8192);

        assert_eq!(NUM_RANGE_COLS, 11);
        assert_eq!(CONTROL_PREP, 11);
        assert_eq!(NUM_CONTROL_COLS, 22);
    }

    /// Pearl byte-equivalence anchor: every column width matches
    /// Pearl's `pearl_layout.rs` definitions verbatim. If Pearl
    /// changes these (extremely unlikely; their column shape is
    /// pinned by the BLAKE3 round-AIR), the lookup configurations
    /// downstream would silently diverge.
    #[test]
    fn ram_lookup_column_widths_match_pearl() {
        assert_eq!(JACKPOT_X_BITS_LEN, 32);
        assert_eq!(JACKPOT_SLOT_SEL_LEN, 16);
        assert_eq!(MAT_UNPACK_LEN, 64); // cx.2/X1 full-block activation
        assert_eq!(MAT_UNPACK_WIN, 64);
        assert_eq!(UINT8_DATA_LEN, 64); // cx.2/X1 full-block activation
        assert_eq!(UINT8_DATA_WIN, 64);
        assert_eq!(NOISE_UNPACK_LEN, 64); // cx.2/X1 (was 8)
        assert_eq!(NOISE_UNPACK_WIN, 64);
        assert_eq!(NOISED_PACKED_LEN, 16); // cx.2/X1 (was 2)
        assert_eq!(NOISED_PACKED_WIN, 2);
        assert_eq!(MAT_ID_LIMBS_LEN, 2);
        assert_eq!(AB_ID_LIMBS_LEN, 4);
        assert_eq!(A_NOISED_LEN, 8); // TILE_H × TILE_D / 4
        assert_eq!(A_NOISED_UNPACK_LEN, 32); // TILE_H × TILE_D
        assert_eq!(B_NOISED_LEN, 8);
        assert_eq!(B_NOISED_UNPACK_LEN, 32);
        assert_eq!(CUMSUM_TILE_LEN, 4);
        assert_eq!(CUMSUM_BUFFER_LEN, 4);
        assert_eq!(JACKPOT_MSG_LEN, 16);
        assert_eq!(BIT_REG_LEN, 32);
        assert_eq!(JACKPOT_IDX_LEN, 8);
        assert_eq!(BLAKE3_MSG_BUFFER_LEN, 16);
        assert_eq!(CV_IN_LEN, 8);
        assert_eq!(BLAKE3_MSG_LEN, 16);
        assert_eq!(BLAKE3_CV_LEN, 8);
        assert_eq!(BLAKE3_ROUND_LEN, 1056);
        assert_eq!(CV_OUT_LEN, 8);
    }

    /// All column offsets must be strictly increasing and contiguous.
    #[test]
    fn layout_offsets_are_contiguous() {
        let checkpoints: &[(usize, usize, &str)] = &[
            (NUM_RANGE_COLS, CONTROL_PREP, "range → CONTROL_PREP"),
            (
                CONTROL_PREP + NUM_CONTROL_COLS,
                MAT_UNPACK_START,
                "control → MAT_UNPACK",
            ),
            (
                MAT_UNPACK_START + MAT_UNPACK_LEN,
                UINT8_DATA_START,
                "MAT_UNPACK → UINT8_DATA",
            ),
            (
                UINT8_DATA_START + UINT8_DATA_LEN,
                NOISE_PACKED_PREP,
                "UINT8_DATA → NOISE_PACKED_PREP",
            ),
            (
                NOISE_PACKED_PREP + NOISE_PACKED_PREP_LEN,
                NOISE_UNPACK_START,
                "NOISE_PACKED_PREP → NOISE_UNPACK",
            ),
            (
                NOISE_UNPACK_START + NOISE_UNPACK_LEN,
                NOISED_PACKED_START,
                "NOISE_UNPACK → NOISED_PACKED",
            ),
            (
                NOISED_PACKED_START + NOISED_PACKED_LEN,
                MAT_FREQ,
                "NOISED_PACKED → MAT_FREQ",
            ),
            (MAT_FREQ + MAT_FREQ_LEN, MAT_ID, "MAT_FREQ → MAT_ID"),
            (MAT_ID + 1, MAT_ID_LIMBS_START, "MAT_ID → MAT_ID_LIMBS"),
            (
                MAT_ID_LIMBS_START + MAT_ID_LIMBS_LEN,
                AB_ID_PREP,
                "MAT_ID_LIMBS → AB_ID_PREP",
            ),
            (
                AB_ID_PREP + 1,
                AB_ID_LIMBS_START,
                "AB_ID_PREP → AB_ID_LIMBS",
            ),
            (
                AB_ID_LIMBS_START + AB_ID_LIMBS_LEN,
                A_ID,
                "AB_ID_LIMBS → A_ID",
            ),
            (A_ID + A_ID_LEN, B_ID, "A_ID → B_ID"),
            (B_ID + B_ID_LEN, STARK_ROW_IDX, "B_ID → STARK_ROW_IDX"),
            (
                STARK_ROW_IDX + 1,
                A_NOISED_START,
                "STARK_ROW_IDX → A_NOISED",
            ),
            (
                A_NOISED_START + A_NOISED_LEN,
                A_NOISED_UNPACK_START,
                "A_NOISED → A_NOISED_UNPACK",
            ),
            (
                A_NOISED_UNPACK_START + A_NOISED_UNPACK_LEN,
                B_NOISED_START,
                "A_NOISED_UNPACK → B_NOISED",
            ),
            (
                B_NOISED_START + B_NOISED_LEN,
                B_NOISED_UNPACK_START,
                "B_NOISED → B_NOISED_UNPACK",
            ),
            (
                B_NOISED_UNPACK_START + B_NOISED_UNPACK_LEN,
                CUMSUM_TILE_START,
                "B_NOISED_UNPACK → CUMSUM_TILE",
            ),
            (
                CUMSUM_TILE_START + CUMSUM_TILE_LEN,
                CUMSUM_BUFFER_START,
                "CUMSUM_TILE → CUMSUM_BUFFER",
            ),
            (
                CUMSUM_BUFFER_START + CUMSUM_BUFFER_LEN,
                JACKPOT_MSG_START,
                "CUMSUM_BUFFER → JACKPOT_MSG",
            ),
            (
                JACKPOT_MSG_START + JACKPOT_MSG_LEN,
                BIT_REG_START,
                "JACKPOT_MSG → BIT_REG",
            ),
            (
                BIT_REG_START + BIT_REG_LEN,
                JACKPOT_IDX_START,
                "BIT_REG → JACKPOT_IDX",
            ),
            (
                JACKPOT_IDX_START + JACKPOT_IDX_LEN,
                BLAKE3_MSG_BUFFER_START,
                "JACKPOT_IDX → BLAKE3_MSG_BUFFER",
            ),
            (
                BLAKE3_MSG_BUFFER_START + BLAKE3_MSG_BUFFER_LEN,
                CV_OR_TWEAK_PREP,
                "BLAKE3_MSG_BUFFER → CV_OR_TWEAK_PREP",
            ),
            (
                CV_OR_TWEAK_PREP + 1,
                CV_IN_START,
                "CV_OR_TWEAK_PREP → CV_IN",
            ),
            (
                CV_IN_START + CV_IN_LEN,
                BLAKE3_MSG_START,
                "CV_IN → BLAKE3_MSG",
            ),
            (
                BLAKE3_MSG_START + BLAKE3_MSG_LEN,
                BLAKE3_CV_START,
                "BLAKE3_MSG → BLAKE3_CV",
            ),
            (
                BLAKE3_CV_START + BLAKE3_CV_LEN,
                BLAKE3_ROUND_START,
                "BLAKE3_CV → BLAKE3_ROUND",
            ),
            (
                BLAKE3_ROUND_START + BLAKE3_ROUND_LEN,
                CV_OUT_START,
                "BLAKE3_ROUND → CV_OUT",
            ),
            (
                CV_OUT_START + CV_OUT_LEN,
                JACKPOT_X_BITS_START,
                "CV_OUT → JACKPOT_X_BITS",
            ),
            (
                JACKPOT_X_BITS_START + JACKPOT_X_BITS_LEN,
                JACKPOT_SLOT_SEL_START,
                "JACKPOT_X_BITS → JACKPOT_SLOT_SEL",
            ),
            (
                JACKPOT_SLOT_SEL_START + JACKPOT_SLOT_SEL_LEN,
                CV_OUT_FREQ,
                "JACKPOT_SLOT_SEL → CV_OUT_FREQ",
            ),
            // HIGH-2.2 §4.B — the FoldChip block, appended after the
            // last Pearl-mirrored column (`CV_OUT_FREQ`). Must remain
            // contiguous through to `TOTAL_TRACE_WIDTH`.
            (CV_OUT_FREQ + 1, FOLD_IS_FOLD, "CV_OUT_FREQ → FOLD_IS_FOLD"),
            (
                FOLD_IS_FOLD + 1,
                FOLD_SLOT_SEL_START,
                "FOLD_IS_FOLD → FOLD_SLOT_SEL",
            ),
            (
                FOLD_SLOT_SEL_START + FOLD_SLOT_SEL_LEN,
                FOLD_XSTEP,
                "FOLD_SLOT_SEL → FOLD_XSTEP",
            ),
            (
                FOLD_XSTEP + 1,
                FOLD_XSTEP_BITS_START,
                "FOLD_XSTEP → FOLD_XSTEP_BITS",
            ),
            (
                FOLD_XSTEP_BITS_START + FOLD_XSTEP_BITS_LEN,
                FOLD_STATE_START,
                "FOLD_XSTEP_BITS → FOLD_STATE",
            ),
            (
                FOLD_STATE_START + FOLD_STATE_LEN,
                FOLD_MCUR_BITS_START,
                "FOLD_STATE → FOLD_MCUR_BITS",
            ),
            (
                FOLD_MCUR_BITS_START + FOLD_MCUR_BITS_LEN,
                FOLD_XOR_OUT,
                "FOLD_MCUR_BITS → FOLD_XOR_OUT",
            ),
            // HIGH-2.2 §6(b) — StripeXorChip block, appended after
            // the FoldChip block; contiguous to TOTAL_TRACE_WIDTH.
            (
                FOLD_XOR_OUT + 1,
                SX_IS_ACTIVE,
                "FOLD_XOR_OUT → SX_IS_ACTIVE",
            ),
            (
                SX_IS_ACTIVE + 1,
                SX_LANE_SEL_START,
                "SX_IS_ACTIVE → SX_LANE_SEL",
            ),
            // §M-S5b Path A column-overlay — the StripeXor block-own
            // chain is SX_IS_ACTIVE → SX_LANE_SEL → SX_IN → SX_XR →
            // SX_NEW_SEL → SX_Q. The bit runs (SX_IN_BITS,
            // SX_XR_SEL_BITS, SX_NEW_SEL_BITS) are aliased into the
            // BLAKE3-round region (checked separately below) and are
            // not in this chain.
            (
                SX_LANE_SEL_START + SX_LANE_SEL_LEN,
                SX_IN_START,
                "SX_LANE_SEL → SX_IN",
            ),
            (SX_IN_START + SX_IN_LEN, SX_XR_START, "SX_IN → SX_XR"),
            (SX_XR_START + SX_XR_LEN, SX_NEW_SEL, "SX_XR → SX_NEW_SEL"),
            (SX_NEW_SEL + 1, SX_Q_START, "SX_NEW_SEL → SX_Q"),
            (
                SX_Q_START + SX_Q_LEN,
                FOLD_STRIPE_SEL_START,
                "SX_Q → FOLD_STRIPE_SEL",
            ),
            (
                FOLD_STRIPE_SEL_START + FOLD_STRIPE_SEL_LEN,
                MSG_PAIR_SEL_START,
                "FOLD_STRIPE_SEL → MSG_PAIR_SEL",
            ),
            (
                MSG_PAIR_SEL_START + MSG_PAIR_SEL_LEN,
                TOTAL_TRACE_WIDTH,
                "MSG_PAIR_SEL → TOTAL_TRACE_WIDTH",
            ),
        ];
        for &(end, next, name) in checkpoints {
            assert_eq!(end, next, "layout discontinuity at {name}: {end} != {next}");
        }

        // §M-S5b Path A column-overlay — the four aliased StripeXor
        // runs form one contiguous sub-window at the head of the
        // BLAKE3-round region. Pin the layout so any reshuffle that
        // breaks the alias (or makes a run escape `blake3_round`)
        // trips here.
        let overlay_chain: [(usize, usize, &str); 3] = [
            (
                SX_IN_BITS_START, BLAKE3_ROUND_START, "SX_IN_BITS @ BLAKE3_ROUND_START",
            ),
            (
                SX_XR_SEL_BITS_START,
                SX_IN_BITS_START + SX_IN_BITS_LEN,
                "SX_XR_SEL_BITS after SX_IN_BITS",
            ),
            (
                SX_NEW_SEL_BITS_START,
                SX_XR_SEL_BITS_START + SX_XR_SEL_BITS_LEN,
                "SX_NEW_SEL_BITS after SX_XR_SEL_BITS",
            ),
        ];
        for &(got, want, name) in overlay_chain.iter() {
            assert_eq!(
                got, want,
                "SX overlay misaligned at {name}: {got} != {want}"
            );
        }
        assert_eq!(
            SX_OVERLAY_LEN,
            SX_IN_BITS_LEN + SX_XR_SEL_BITS_LEN + SX_NEW_SEL_BITS_LEN,
            "SX_OVERLAY_LEN must total the three aliased bit runs"
        );
        assert!(
            BLAKE3_ROUND_START + SX_OVERLAY_LEN <= BLAKE3_ROUND_START + BLAKE3_ROUND_LEN,
            "SX overlay window must fit within the BLAKE3-round region"
        );
    }

    /// TOTAL_TRACE_WIDTH ≈ Pearl's pinned width (~1300 cols) plus
    /// the HIGH-2.2 §4.B FoldChip block (+99 cols). The number is
    /// dominated by `BLAKE3_ROUND_LEN = 1056` — the per-round AIR —
    /// which is the bulk of any row regardless of which chip is
    /// active. HIGH-2.2 additions: the FoldChip block
    /// (FOLD_IS_FOLD..=FOLD_XOR_OUT, +99) and the §6(b)
    /// StripeXorChip block (SX_IS_ACTIVE..SX_END, +294 — the
    /// "dominant new width" the design anticipated). §6(a) pins the
    /// fold *schedule* into the existing CONTROL_PREP rather than
    /// widening the trace; the StripeXor block is main-trace only
    /// (NOT preprocessed — the §4.C.8 trap is preprocessed width).
    #[test]
    fn total_trace_width_in_pearl_ballpark() {
        // Sanity: ≥ 1200 cols (BLAKE3 round dominates) and ≤ 2200
        // (Pearl-mirrored ≈1378 + FoldChip 99 + StripeXor block
        // with STRIPE_MAX=64 lanes + FOLD_STRIPE_SEL 64 ≈ 1931;
        // bound has headroom but still catches an accidental
        // column explosion).
        assert!(
            TOTAL_TRACE_WIDTH > 1200,
            "TOTAL_TRACE_WIDTH suspiciously small: {TOTAL_TRACE_WIDTH}"
        );
        assert!(
            TOTAL_TRACE_WIDTH < 2200,
            "TOTAL_TRACE_WIDTH suspiciously large: {TOTAL_TRACE_WIDTH} — check for unintended column duplication"
        );
    }

    /// Pearl-byte-compat anchor: 8 × u32 little-endian = 32 bytes
    /// (Tip5 digest size). If anyone bumps `BLOCK_COMMITMENT_*` we
    /// catch the inconsistency here.
    #[test]
    fn block_commitment_layout_matches_8_u32_le() {
        // 8 LE u32s packed into 32 bytes — the in-circuit κ derivation
        // (Phase 7) consumes these as message[0..8] of a single
        // BLAKE3 block (with params_tag as message[8..16]).
        assert_eq!(
            BLOCK_COMMITMENT_WORDS * BYTES_PER_GOLDILOCKS,
            BLOCK_COMMITMENT_BYTES
        );
        assert_eq!(BLOCK_COMMITMENT_WORDS, 8);
    }
}

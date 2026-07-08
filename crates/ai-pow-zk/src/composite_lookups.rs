//! Cross-chip lookup design for the M10.1c composite AIR.
//!
//! This module is the **design pin** for the LogUp argument that
//! ties the per-chip AIRs together. The chips themselves (Phases
//! 3-10) are individually sound; this module documents how their
//! columns relate cryptographically across rows.
//!
//! ## Lookup buses
//!
//! Pearl uses ~10 lookup buses spread across the composite AIR.
//! Each bus has a TABLE side (a chip that provides entries with
//! their consumption count) and a QUERY side (chips that look up
//! entries with multiplicities). For LogUp to be sound, the sum
//! of `multiplicity` across all sides must be 0 per (bus, key).
//!
//! ### Bus inventory
//!
//! | Bus name | Table chip | Queriers | Purpose |
//! |---|---|---|---|
//! | `urange8` | `range_table` (URANGE8) | `i8u8`, any u8 consumer | u8 range check |
//! | `urange13` | `range_table` (URANGE13) | `control`, `input` (MAT_ID limbs) | u13 range check (BITS_PER_LIMB) |
//! | `irange7p1` | `range_table` (IRANGE7P1) | `input` (NOISE_UNPACK) | i7+1 ∈ [−64, 64] noise values |
//! | `irange8` | `range_table` (IRANGE8) | `matmul` (A_NOISED_UNPACK / B_NOISED_UNPACK) | i8 range check |
//! | `i8u8` | `i8u8` chip's 256-row table | (none directly — used cross-bus to bridge i8 ↔ u8) | sign-conversion table |
//! | `noised_packed` | `input` chip (NOISED_PACKED) | `matmul` (A_NOISED via A_ID; B_NOISED via B_ID), `blake3` (UINT8_DATA when IS_MSG_MAT) | per-row matrix bytes |
//! | `cv_routing` | `blake3` chip's CV_OUT cells | `blake3` chip's CV_IN cells (when IS_CV_IN) | inter-hash chaining value routing |
//! | `stark_row_idx` | `stark_row` chip | (any bus that indexes by row position) | monotonic row identifier |
//!
//! Pearl's `pearl_lookups.rs` configures the LogUp argument with
//! these buses. We mirror the structure here but defer the
//! proving-side wiring (LookupBus + InteractionBuilder hookup) to
//! Phase 14 — when we switch from plain `p3-uni-stark` to a
//! lookup-aware prover.
//!
//! ## Multiplicity calculus
//!
//! Each bus has TABLE-side `*_FREQ` columns recording how many
//! times each table row is queried. The trace generator (Phase 13)
//! computes these by scanning the query side and counting.
//!
//! For example, the NOISED_PACKED table at row `mat_id = M` is
//! queried:
//!   * Once per matmul row where `A_ID == M` (contributes to
//!     `MAT_FREQ[M]` by `n_a`).
//!   * Once per matmul row where `B_ID == M` (contributes by `n_b`).
//!   * Once per BLAKE3 row where `IS_MSG_MAT == 1` and the row's
//!     `UINT8_DATA` comes from NOISED_PACKED[M] (by `n_msg`).
//!
//! The sum `MAT_FREQ[M] = n_a + n_b + n_msg` is the table-side
//! multiplicity. Pearl uses LogUp's `multiplicity = total
//! consumption count` semantics.
//!
//! ## Cryptographic role of each bus
//!
//! **Range buses (`urange*`, `irange*`).** Prevent malicious
//! provers from putting out-of-range values into trace cells. With
//! TEST_PEARL's quotient budget, range columns are committed once
//! per circuit and shared via LogUp. The alternative — inlining
//! range bits per cell — would inflate the trace width
//! ~50× for the noise columns alone.
//!
//! **`noised_packed`.** The critical RAM lookup. Without it, the
//! matmul chip would have to re-derive matrix bytes from a
//! committed source every row (or duplicate them inline). The
//! lookup guarantees the matrix the matmul chip computes against
//! and the matrix the BLAKE3 chip hashes are the same bytes —
//! merge-mining compat anchor.
//!
//! **`cv_routing`.** Pearl threads BLAKE3 chaining values across
//! hash instructions; one hash's CV_OUT becomes the next hash's
//! CV_IN. The lookup binds these cells across non-adjacent rows.
//! Without it, an adversary could substitute arbitrary CVs.
//!
//! **`stark_row_idx`.** Pearl's `STARK_ROW_IDX` column is a
//! universal row identifier consumed by `cv_routing` (and any
//! other bus that needs to reference specific rows). The
//! monotonic-increment constraint on `stark_row` (Phase 3)
//! together with this lookup makes the row index untamperable.

use crate::composite_layout::JACKPOT_SIZE;

// =====================================================================
//  Bus name constants
// =====================================================================

/// `urange8` bus — u8 range check `[0, 256)`.
pub const BUS_URANGE8: &str = "urange8";

/// `urange13` bus — u13 range check `[0, 8192)`. Used by MAT_ID /
/// AB_ID limb decompositions.
pub const BUS_URANGE13: &str = "urange13";

/// `irange7p1` bus — i7+1 range check `[-64, 64]`. Used by NOISE
/// values (Pearl's signed-noise range).
pub const BUS_IRANGE7P1: &str = "irange7p1";

/// `irange8` bus — i8 range check `[-128, 127]`. Used by
/// A_NOISED_UNPACK and B_NOISED_UNPACK matmul cells.
pub const BUS_IRANGE8: &str = "irange8";

/// `i8u8` bus — sign-conversion table `(i8, u8)`. Used to bridge
/// signed/unsigned matrix-byte representations.
pub const BUS_I8U8: &str = "i8u8";

/// `noised_packed` bus — per-row matrix bytes. The cryptographic
/// glue between matmul and BLAKE3 sides.
pub const BUS_NOISED_PACKED: &str = "noised_packed";

/// `cv_routing` bus — BLAKE3 chaining value routing. Ties one
/// hash's CV_OUT to the consuming hash's CV_IN across non-adjacent
/// rows.
pub const BUS_CV_ROUTING: &str = "cv_routing";

/// `stark_row_idx` bus — universal row identifier. Used by buses
/// that need row-level addressing.
pub const BUS_STARK_ROW_IDX: &str = "stark_row_idx";

/// All bus names in the composite AIR.
pub const ALL_BUSES: &[&str] = &[
    BUS_URANGE8, BUS_URANGE13, BUS_IRANGE7P1, BUS_IRANGE8, BUS_I8U8, BUS_NOISED_PACKED,
    BUS_CV_ROUTING, BUS_STARK_ROW_IDX,
];

// =====================================================================
//  Multiplicity helpers (scalar — used by Phase 13's trace generator)
// =====================================================================

/// Compute the table-side multiplicity for one NOISED_PACKED row.
///
/// `n_matmul_reads` is the count of matmul rows that read this
/// row via `A_ID` or `B_ID` (combined). `n_msg_reads` is the count
/// of BLAKE3 rows where `IS_MSG_MAT == 1` and the row's
/// `UINT8_DATA` derives from this NOISED_PACKED entry.
#[inline]
pub fn noised_packed_freq(n_matmul_reads: u32, n_msg_reads: u32) -> u32 {
    n_matmul_reads + n_msg_reads
}

/// Compute the table-side multiplicity for one CV_OUT row in the
/// cv_routing bus. `n_consumers` is the number of BLAKE3 rows
/// downstream that consume this row's CV_OUT as their CV_IN.
#[inline]
pub fn cv_out_freq(n_consumers: u32) -> u32 {
    n_consumers
}

/// Compute the BLAKE3 chip's per-row query multiplicity for the
/// `cv_routing` bus. Each new-blake row queries CV_OUT of the
/// previous-hash row; an in-progress-hash row queries the
/// previous round's CV.
#[inline]
pub fn blake3_cv_query_count(is_new_blake: bool, is_cv_in: bool) -> u32 {
    if is_new_blake || is_cv_in {
        1
    } else {
        0
    }
}

/// Compute the matmul chip's per-row query count for the
/// `noised_packed` bus. The matmul chip always queries exactly 2
/// entries per active row: one for A_NOISED (key = A_ID), one for
/// B_NOISED (key = B_ID). On inactive rows (selectors all-zero),
/// it queries 0.
#[inline]
pub fn matmul_noised_packed_query_count(is_active: bool) -> u32 {
    if is_active {
        2
    } else {
        0
    }
}

/// Compute the BLAKE3 message-buffer chip's per-row query count
/// for the `noised_packed` bus. When `IS_MSG_MAT == 1`, the row
/// reads matrix bytes from NOISED_PACKED into its UINT8_DATA
/// columns; one query per row.
#[inline]
pub fn blake3_msg_mat_query_count(is_msg_mat: bool) -> u32 {
    if is_msg_mat {
        1
    } else {
        0
    }
}

// =====================================================================
//  Sanity checks
// =====================================================================

/// Number of slots in a jackpot state; pinned at
/// `JACKPOT_SIZE = 16`. Re-exported here so downstream lookup-bus
/// callers don't have to import composite_layout directly.
pub const JACKPOT_SLOTS: usize = JACKPOT_SIZE;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn all_buses_are_pairwise_unique() {
        let mut seen = std::collections::HashSet::new();
        for &bus in ALL_BUSES {
            assert!(seen.insert(bus), "duplicate bus name {bus:?}");
        }
        assert_eq!(seen.len(), 8);
    }

    #[test]
    fn noised_packed_freq_pure_function() {
        assert_eq!(noised_packed_freq(0, 0), 0);
        assert_eq!(noised_packed_freq(3, 5), 8);
        assert_eq!(noised_packed_freq(u32::MAX - 1, 1), u32::MAX);
    }

    #[test]
    fn cv_out_freq_pure_function() {
        assert_eq!(cv_out_freq(0), 0);
        assert_eq!(cv_out_freq(42), 42);
    }

    #[test]
    fn blake3_cv_query_count_returns_1_when_either_flag_set() {
        assert_eq!(blake3_cv_query_count(false, false), 0);
        assert_eq!(blake3_cv_query_count(true, false), 1);
        assert_eq!(blake3_cv_query_count(false, true), 1);
        // Both — still 1 (Pearl's contract treats these as mutually
        // exclusive selectors but the function is lenient).
        assert_eq!(blake3_cv_query_count(true, true), 1);
    }

    #[test]
    fn matmul_noised_packed_query_count_fn() {
        assert_eq!(matmul_noised_packed_query_count(false), 0);
        assert_eq!(matmul_noised_packed_query_count(true), 2);
    }

    #[test]
    fn blake3_msg_mat_query_count_fn() {
        assert_eq!(blake3_msg_mat_query_count(false), 0);
        assert_eq!(blake3_msg_mat_query_count(true), 1);
    }

    /// Validate that ALL_BUSES is in sync with the documented bus
    /// inventory in the module doc.
    #[test]
    fn all_buses_count_matches_design() {
        // 4 range-table buses + i8u8 + noised_packed + cv_routing
        // + stark_row_idx = 8.
        assert_eq!(ALL_BUSES.len(), 8);
    }

    /// Multi-step scenario: simulate one BLAKE3 hash instruction
    /// (8 rows) and check the table-side multiplicity computations.
    ///
    /// Hash A: rows 0..7. Row 0 has IS_NEW_BLAKE = 1 (queries the
    /// previous hash's CV_OUT, contributing 1 to cv_routing query).
    /// Rows 1..6 have IS_CV_IN = 1 maybe? Pearl's exact contract
    /// here is per-design; this test just exercises the helper.
    #[test]
    fn cv_routing_multi_hash_balance_simulation() {
        // 2 hashes: hash A produces a CV_OUT consumed by hash B's
        // first row. So table-side: cv_out_freq = 1; query-side:
        // hash B contributes 1 query.
        assert_eq!(cv_out_freq(1), 1);
        let q_b = blake3_cv_query_count(true, false);
        assert_eq!(q_b, 1);
        // For LogUp soundness, table multiplicity − query
        // multiplicity must sum to 0:
        let table = cv_out_freq(1) as i32;
        let queries = q_b as i32;
        assert_eq!(table - queries, 0);
    }

    /// Multi-querier scenario for `noised_packed`: a single
    /// NOISED_PACKED entry is read by 3 matmul rows (via A_ID = M
    /// in 2 rows + B_ID = M in 1 row) and 5 BLAKE3 message rows.
    #[test]
    fn noised_packed_multi_querier_balance() {
        let n_matmul = 3;
        let n_msg = 5;
        let table = noised_packed_freq(n_matmul, n_msg);
        // Matmul query side: 3 (counted from query-side
        // contributions). Plus 5 from BLAKE3. Total = 8.
        let queries = n_matmul + n_msg;
        assert_eq!(table, queries);
    }

    #[test]
    fn bus_name_strings_match_documentation() {
        // Pin the exact string values used as bus names.
        assert_eq!(BUS_URANGE8, "urange8");
        assert_eq!(BUS_URANGE13, "urange13");
        assert_eq!(BUS_IRANGE7P1, "irange7p1");
        assert_eq!(BUS_IRANGE8, "irange8");
        assert_eq!(BUS_I8U8, "i8u8");
        assert_eq!(BUS_NOISED_PACKED, "noised_packed");
        assert_eq!(BUS_CV_ROUTING, "cv_routing");
        assert_eq!(BUS_STARK_ROW_IDX, "stark_row_idx");
    }
}

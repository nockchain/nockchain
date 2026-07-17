//! Phase A-CR — params-pure `canonical_program` (CRIT-1
//! reconstruction hardening). Design + decisions D-CR1..4 +
//! staged plan CR.0..7: `2026-05-17_CANONICAL_PROGRAM_DESIGN.md`.
//!
//! **CR.0 (this module so far): the single params-pure row
//! schedule.** [`row_schedule`] assigns each trace row a
//! [`RowClass`] from `(ZkParams, tile_i, tile_j, trace_len)`
//! alone — *no witness* — reproducing the exact layout
//! `ai-pow::zk_bridge::prove_and_verify_tiled` builds on the
//! **production-faithful 16|r co-location path** (Pearl §4.8 is
//! always 16|r). It is *the* single source of truth for "which
//! row class sits where": CR.1..CR.5 build `canonical_program`'s
//! per-row `RowDescriptor` from this schedule + `block_public` +
//! `noise_ref`; CR.6 flips the verify path to commit to it.
//!
//! Validated by a cross-crate KAT (`ai-pow`,
//! `cr0_row_schedule_matches_real_bridge_trace`) that the
//! schedule's region boundaries match the real `P16`(16|r)
//! bridge trace's unambiguous selector anchors (KeyPin, the
//! Fold range, JackpotHash, the strip-opening / `HASH_A`/`HASH_B`
//! roots, the co-located `IS_MSG_MAT` leaf rows) — the
//! cx.0/cx.2-coloc.0 KAT-first discipline. **No verify-path
//! change in CR.0.**

use p3_matrix::dense::RowMajorMatrix;

#[cfg(test)]
use crate::blake3_tree::strip_opening_rows;
use crate::blake3_tree::{indexed_strips_chunk_range, left_len};
use crate::chips::blake3::chip::pack_tweak;
use crate::chips::blake3::compress::Blake3Tweak;
use crate::chips::control::NUM_SELECTORS;
use crate::chips::input::NOISE_PACKING_BASE;
use crate::composite_layout::{TILE_D, TILE_H};
use crate::composite_preprocess::{build_preprocessed_columns, RowDescriptor};
use crate::noise_ref::{e_value, f_value};
use crate::params::ZkParams;
use crate::Val;

/// Coarse per-row class — the CR.0 granularity (the bridge's
/// top-level row regions). CR.1..CR.5 refine the
/// PROGRAM_COL-bearing classes (Store sub-slices on the
/// co-located `StripOpen*` leaf round-0 rows; the §6(b) sweep
/// fold-schedule) into the per-cell `RowDescriptor`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RowClass {
    /// A-side strip-opening BLAKE3 compression rows (rows
    /// `[0, na)`). On the 16|r co-location path the leaf round-0
    /// rows here are also the M-S1 `noised_packed` producers.
    StripOpenA,
    /// B-side strip-opening compression rows (`[na, na+nb)`).
    StripOpenB,
    /// C1 key-pin rows (JOB_KEY = κ, then COMMITMENT_HASH slot = jackpot key).
    KeyPin,
    /// §6(b)-G1/G2 sub-block-major matmul sweep + StripeXor.
    Sweep,
    /// FoldChip rows (`num_stripes`).
    Fold,
    /// Final keyed-BLAKE3 jackpot-hash block (trace's last 8 rows).
    JackpotHash,
    /// Padding / inter-region gap (all selectors zero).
    Pad,
}

/// CR.0 — the params-pure row schedule for the **16|r
/// co-location production path**. Returns a `trace_len`-long
/// `Vec<RowClass>` reproducing `prove_and_verify_tiled`'s exact
/// row layout from public data only: `params` + the attested
/// `(tile_i, tile_j)` (MED-3-derived) + `trace_len`
/// (`Layer0RowBudget::required_trace_len`, itself params-pure,
/// P-B). Panics if `params.noise_rank % 16 != 0` (non-16|r is
/// the documented A3.2b *test* path whose separate-store row
/// count is value-deduped / data-dependent — out of the
/// params-pure / `canonical_program` scope; Pearl/production is
/// always 16|r).
pub fn row_schedule(
    params: &ZkParams,
    tile_i: u32,
    tile_j: u32,
    trace_len: usize,
) -> Vec<RowClass> {
    let l = schedule_layout(params, tile_i, tile_j, trace_len);
    (0..trace_len).map(|r| l.class_of(r)).collect()
}

/// CR.0 — the single params-pure source of truth for the bridge's
/// region boundaries (16|r co-location path). `row_schedule`
/// **and** `canonical_program`'s per-row `row_descriptor` both
/// derive from this *one* layout (the CR.0 invariant: there is
/// one schedule, not two constructions ⇒ no prover/verifier
/// divergence). All offsets are params-pure (CR.0a
/// `strip_opening_rows` + A1 `tile_chunk_range` + the §6(b) sweep
/// formula + the 16|r co-located store=0 + fold/jackpot offsets).
/// §6(b)-G3 — max segments = ⌈max_num_stripes / STRIPE_MAX⌉ =
/// ⌈512 / 64⌉ = 8 (Pearl's num_stripes ceiling ÷ the pinned lane count).
pub(crate) const MAX_SEGMENTS: usize = 8;

/// §6(b)-G3 — the `[sweep, +4 gap, fold]` region for ONE ≤`STRIPE_MAX`
/// stripe segment. `num_stripes > STRIPE_MAX` is folded as
/// `⌈num_stripes/STRIPE_MAX⌉` of these, INTERLEAVED (each segment's fold
/// immediately follows its sweep, before the next segment's SEG_RESET),
/// so the 64-lane StripeXor register is reused per segment.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) struct SegmentLayout {
    /// First §6(b) sweep row of this segment.
    pub sweep_start: usize,
    /// One past this segment's last sweep row.
    pub store_start: usize,
    /// First FoldChip row of this segment (`store_start + 4`).
    pub fold_start: usize,
    /// One past this segment's last fold row.
    pub fold_end: usize,
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct ScheduleLayout {
    /// A-side strip-opening row count (`[0, na)` = StripOpenA).
    pub na: usize,
    /// End of strip-opening (`[na, mh_end)` = StripOpenB; row
    /// `mh_end` is the Pad gap).
    pub mh_end: usize,
    /// First §6(b) sweep row (`mh_end + 3`) — `== segments[0].sweep_start`.
    pub sweep_start: usize,
    /// One past segment 0's last sweep row (`== segments[0].store_start`).
    #[allow(dead_code)] // read by layout tests; kept as the pre-G3 single-segment view
    pub store_start: usize,
    /// First FoldChip row of segment 0 (`== segments[0].fold_start`).
    pub fold_start: usize,
    /// One past segment 0's last fold row (`== segments[0].fold_end`).
    #[allow(dead_code)] // read by layout tests; kept as the pre-G3 single-segment view
    pub fold_end: usize,
    /// First jackpot-hash row (`trace_len - 8`).
    pub jpot_start: usize,
    /// §6(b)-G3 — number of active stripe segments (`⌈num_stripes/64⌉`; 1
    /// for the pre-G3 single-segment path).
    pub num_segments: usize,
    /// §6(b)-G3 — per-segment `[sweep, fold]` regions; only
    /// `[0, num_segments)` are valid. The single-field accessors above
    /// mirror `segments[0]` (unchanged for the single-segment path).
    pub segments: [SegmentLayout; MAX_SEGMENTS],
    /// R-b (stripe-major, `num_stripes > STRIPE_MAX`) — when `true`,
    /// the sweep region is a SINGLE contiguous stripe-major block with
    /// one FoldChip row interleaved after each stripe's sub-block sweep
    /// (mirrors `composite_trace::place_useful_work_chain_rb`). The
    /// segmented sub-block-major fields above are NOT used; `class_of`
    /// / `row_descriptor` dispatch to the R-b path. `false` on the
    /// legacy ≤`STRIPE_MAX` path (all fields below `Default`).
    pub rb: bool,
    /// R-b — rows per stripe = `n_sb · chunks + 1` (the `+1` is the
    /// interleaved fold row). The R-b region is
    /// `[sweep_start, sweep_start + rb_num_stripes · rb_per_stripe)`.
    pub rb_per_stripe: usize,
    /// R-b — `num_stripes` (`k / r`), the stripe count.
    pub rb_num_stripes: usize,
    /// R-b — sub-blocks per stripe = `(h/TILE_H)·(w/TILE_H)`.
    pub rb_n_sb: usize,
    /// R-b — `⌈r/TILE_D⌉` micro-steps per sub-block.
    pub rb_chunks: usize,
}

impl ScheduleLayout {
    /// §6(b)-G3 — if `r` is a Fold row, return `(segment, local_stripe)`
    /// where `local_stripe = r - segments[segment].fold_start ∈ 0..64`
    /// (the per-segment stripe index the CONTROL_PREP / FOLD_STRIPE_SEL
    /// pin uses). `None` on non-fold rows.
    pub fn fold_segment_and_local(&self, r: usize) -> Option<(usize, usize)> {
        for g in 0..self.num_segments {
            let seg = &self.segments[g];
            if (seg.fold_start..seg.fold_end).contains(&r) {
                return Some((g, r - seg.fold_start));
            }
        }
        None
    }

    /// §6(b)-G3 — if `r` is a Sweep row, return `(segment, sweep_offset)`
    /// where `sweep_offset = r - segments[segment].sweep_start`. `None`
    /// on non-sweep rows.
    pub fn sweep_segment_and_offset(&self, r: usize) -> Option<(usize, usize)> {
        for g in 0..self.num_segments {
            let seg = &self.segments[g];
            if (seg.sweep_start..seg.store_start).contains(&r) {
                return Some((g, r - seg.sweep_start));
            }
        }
        None
    }
}

impl ScheduleLayout {
    /// The [`RowClass`] of `row_idx` (the *one* classification). §6(b)-G3:
    /// Sweep/Fold are matched against ALL `num_segments` segments (for the
    /// single-segment path `segments[0]` == the single fields, so this is
    /// identical to the pre-G3 classification).
    pub fn class_of(&self, r: usize) -> RowClass {
        if self.rb {
            return self.class_of_rb(r);
        }
        if r < self.na {
            RowClass::StripOpenA
        } else if r < self.mh_end {
            RowClass::StripOpenB
        } else if r == self.mh_end + 1 || r == self.mh_end + 2 {
            RowClass::KeyPin
        } else if self.sweep_segment_and_offset(r).is_some() {
            RowClass::Sweep
        } else if self.fold_segment_and_local(r).is_some() {
            RowClass::Fold
        } else if r >= self.jpot_start {
            RowClass::JackpotHash
        } else {
            RowClass::Pad
        }
    }

    /// R-b classification: the sweep region is a single stripe-major
    /// block; within each stripe's `rb_per_stripe` rows the first
    /// `n_sb·chunks` are `Sweep`, the last is the interleaved `Fold`.
    fn class_of_rb(&self, r: usize) -> RowClass {
        let region_end = self.sweep_start + self.rb_num_stripes * self.rb_per_stripe;
        if r < self.na {
            RowClass::StripOpenA
        } else if r < self.mh_end {
            RowClass::StripOpenB
        } else if r == self.mh_end + 1 || r == self.mh_end + 2 {
            RowClass::KeyPin
        } else if (self.sweep_start..region_end).contains(&r) {
            let within = (r - self.sweep_start) % self.rb_per_stripe;
            if within < self.rb_n_sb * self.rb_chunks {
                RowClass::Sweep
            } else {
                RowClass::Fold
            }
        } else if r >= self.jpot_start {
            RowClass::JackpotHash
        } else {
            RowClass::Pad
        }
    }

    /// R-b — if `r` is a Sweep row, return `(stripe, sb, chunk)`
    /// (stripe-major nested loop, sub-block-major within the stripe).
    /// `None` on non-sweep / non-R-b rows.
    fn rb_sweep_coords(&self, r: usize) -> Option<(usize, usize, usize)> {
        if !self.rb {
            return None;
        }
        let region_end = self.sweep_start + self.rb_num_stripes * self.rb_per_stripe;
        if !(self.sweep_start..region_end).contains(&r) {
            return None;
        }
        let local = r - self.sweep_start;
        let stripe = local / self.rb_per_stripe;
        let within = local % self.rb_per_stripe;
        if within >= self.rb_n_sb * self.rb_chunks {
            return None; // fold row
        }
        let sb = within / self.rb_chunks;
        let chunk = within % self.rb_chunks;
        Some((stripe, sb, chunk))
    }

    /// R-b — if `r` is a Fold row, return its `stripe` index. `None`
    /// on non-fold / non-R-b rows.
    fn rb_fold_stripe(&self, r: usize) -> Option<usize> {
        if !self.rb {
            return None;
        }
        let region_end = self.sweep_start + self.rb_num_stripes * self.rb_per_stripe;
        if !(self.sweep_start..region_end).contains(&r) {
            return None;
        }
        let local = r - self.sweep_start;
        let within = local % self.rb_per_stripe;
        if within < self.rb_n_sb * self.rb_chunks {
            return None; // sweep row
        }
        Some(local / self.rb_per_stripe)
    }
}

/// Compute the [`ScheduleLayout`] from public data only. Panics
/// on non-16|r (the documented A3.2b test path — out of the
/// params-pure / `canonical_program` scope; Pearl §4.8 is always
/// 16|r).
pub(crate) fn schedule_layout(
    params: &ZkParams,
    tile_i: u32,
    tile_j: u32,
    trace_len: usize,
) -> ScheduleLayout {
    let strip_schedule = StripIndexSchedule::from_tile(params, tile_i, tile_j)
        .expect("schedule_layout requires a valid contiguous tile schedule");
    schedule_layout_for_strip_schedule(params, &strip_schedule, trace_len)
}

pub(crate) fn schedule_layout_for_strip_schedule(
    params: &ZkParams,
    strip_schedule: &StripIndexSchedule,
    trace_len: usize,
) -> ScheduleLayout {
    assert_eq!(
        params.noise_rank % 16,
        0,
        "schedule_layout is params-pure only on the 16|r \
         co-location path (Pearl §4.8 is always 16|r); non-16|r \
         is the documented A3.2b test path"
    );
    let h_tile = strip_schedule.a_indices.len();
    let w_tile = strip_schedule.b_indices.len();
    assert!(
        h_tile.is_multiple_of(TILE_H),
        "schedule_layout requires h_tile divisible by TILE_H"
    );
    assert!(
        w_tile.is_multiple_of(TILE_H),
        "schedule_layout requires w_tile divisible by TILE_H"
    );
    let k = params.k as usize;
    let r = params.noise_rank as usize;
    let num_stripes = k / r;

    // Strip-opening A then B (P-B.2.4 + public strip schedule).
    // B5b selective opening: the row count is over the SET of chunks the opened
    // rows/cols touch (== the range for a contiguous tile), matching StripPlan's
    // strip_blocks_set + place_matrix_strip_opening_set.
    let ((_ca0, _ca1, a_nc), (_cb0, _cb1, b_nc)) = strip_schedule
        .chunk_ranges(params)
        .expect("schedule_layout requires valid strip chunk ranges");
    let (a_chunks, _) =
        crate::blake3_tree::indexed_strips_chunk_set(&strip_schedule.a_indices, k, a_nc * 1024);
    let (b_chunks, _) =
        crate::blake3_tree::indexed_strips_chunk_set(&strip_schedule.b_indices, k, b_nc * 1024);
    let na = strip_opening_rows_set(&a_chunks, a_nc);
    let nb = strip_opening_rows_set(&b_chunks, b_nc);
    let mh_end = na + nb;

    // Key-pin: row mh_end is the gap; mh_end+1 = JOB_KEY,
    // mh_end+2 = COMMITMENT_HASH; sweep_start = mh_end+3.
    let sweep_start = mh_end + 3;
    // §6(b)-G1/G2 sweep-rows-PER-STRIPE = (h/TILE_H) · (w/TILE_H) ·
    // ⌈r/TILE_D⌉; a stripe segment of `stripes_g` stripes uses
    // `sweep_per_stripe · stripes_g` sweep rows (== place_useful_work_
    // chain_hw's rows_used for that segment).
    let sweep_per_stripe = (h_tile / TILE_H) * (w_tile / TILE_H) * r.div_ceil(TILE_D);

    // §6(b)-G3 — INTERLEAVED segments of ≤ STRIPE_MAX stripes:
    // `[seg sweep][+4 store gap][seg fold]` back to back, so each
    // segment's fold reads its own StripeXor register before the next
    // segment's SEG_RESET. Single segment (num_stripes ≤ STRIPE_MAX) ⇒
    // one region identical to the pre-G3 layout.
    let stripe_max = crate::composite_layout::STRIPE_MAX;
    let num_segments = num_stripes.div_ceil(stripe_max).max(1);
    assert!(
        num_segments <= MAX_SEGMENTS,
        "num_stripes={num_stripes} ⇒ {num_segments} segments exceeds MAX_SEGMENTS={MAX_SEGMENTS}"
    );
    let mut segments = [SegmentLayout::default(); MAX_SEGMENTS];
    let mut cursor = sweep_start;
    for (g, seg) in segments.iter_mut().enumerate().take(num_segments) {
        let stripes_g = (num_stripes - g * stripe_max).min(stripe_max);
        let seg_sweep_start = cursor;
        let seg_store_start = seg_sweep_start + sweep_per_stripe * stripes_g;
        // 16|r: producers co-located in StripOpen ⇒ 0 separate store
        // rows; the +4 is the store/transition gap (Pad).
        let seg_fold_start = seg_store_start + 4;
        let seg_fold_end = seg_fold_start + stripes_g;
        *seg = SegmentLayout {
            sweep_start: seg_sweep_start,
            store_start: seg_store_start,
            fold_start: seg_fold_start,
            fold_end: seg_fold_end,
        };
        cursor = seg_fold_end;
    }
    let last = segments[num_segments - 1];

    assert!(
        trace_len >= 8 && last.fold_end <= trace_len - 8,
        "schedule overflows trace_len={trace_len} (fold_end={})",
        last.fold_end
    );
    let jpot_start = trace_len - 8;

    ScheduleLayout {
        na,
        mh_end,
        // Single-field accessors mirror segment 0 (the pre-G3 view).
        sweep_start: segments[0].sweep_start,
        store_start: segments[0].store_start,
        fold_start: segments[0].fold_start,
        fold_end: segments[0].fold_end,
        jpot_start,
        num_segments,
        segments,
        rb: false,
        rb_per_stripe: 0,
        rb_num_stripes: 0,
        rb_n_sb: 0,
        rb_chunks: 0,
    }
}

/// R-b (stripe-major) params-pure schedule for `num_stripes >
/// STRIPE_MAX`. Mirrors `composite_trace::place_useful_work_chain_rb`:
/// a SINGLE contiguous region `[sweep_start, sweep_start +
/// num_stripes · (n_sb·chunks + 1))` where each stripe contributes
/// `n_sb·chunks` matmul-active sweep rows (sub-block-major within the
/// stripe) followed by ONE interleaved FoldChip row. The prefix
/// (strip-opening + key-pin) and suffix (jackpot-hash) are identical
/// to the legacy layout; only the sweep/fold region differs. 16|r
/// co-location ⇒ 0 separate store rows (producers co-located in
/// StripOpen). Panics on the same invariants as
/// `schedule_layout_for_strip_schedule` plus `num_stripes > 0`.
pub(crate) fn schedule_layout_rb(
    params: &ZkParams,
    strip_schedule: &StripIndexSchedule,
    trace_len: usize,
) -> ScheduleLayout {
    assert_eq!(
        params.noise_rank % 16,
        0,
        "schedule_layout_rb is params-pure only on the 16|r co-location path"
    );
    let h_tile = strip_schedule.a_indices.len();
    let w_tile = strip_schedule.b_indices.len();
    assert!(
        h_tile.is_multiple_of(TILE_H) && w_tile.is_multiple_of(TILE_H),
        "schedule_layout_rb requires h_tile,w_tile divisible by TILE_H"
    );
    let k = params.k as usize;
    let r = params.noise_rank as usize;
    let num_stripes = k / r;
    let stripe_max = crate::composite_layout::STRIPE_MAX;
    assert!(
        num_stripes > stripe_max,
        "schedule_layout_rb is the num_stripes>{stripe_max} path; got {num_stripes}"
    );

    // Prefix: identical strip-opening + key-pin accounting.
    let ((_ca0, _ca1, a_nc), (_cb0, _cb1, b_nc)) = strip_schedule
        .chunk_ranges(params)
        .expect("schedule_layout_rb requires valid strip chunk ranges");
    let (a_chunks, _) =
        crate::blake3_tree::indexed_strips_chunk_set(&strip_schedule.a_indices, k, a_nc * 1024);
    let (b_chunks, _) =
        crate::blake3_tree::indexed_strips_chunk_set(&strip_schedule.b_indices, k, b_nc * 1024);
    let na = strip_opening_rows_set(&a_chunks, a_nc);
    let nb = strip_opening_rows_set(&b_chunks, b_nc);
    let mh_end = na + nb;
    let sweep_start = mh_end + 3;

    let n_sbi = h_tile / TILE_H;
    let n_sbj = w_tile / TILE_H;
    let n_sb = n_sbi * n_sbj;
    let chunks = r.div_ceil(TILE_D).max(1);
    let rb_per_stripe = n_sb * chunks + 1;
    let region_end = sweep_start + num_stripes * rb_per_stripe;

    assert!(
        trace_len >= 8 && region_end <= trace_len - 8,
        "R-b schedule overflows trace_len={trace_len} (region_end={region_end})"
    );
    let jpot_start = trace_len - 8;

    ScheduleLayout {
        na,
        mh_end,
        sweep_start,
        // Region end doubles as store_start/fold_start/fold_end for the
        // bounds check; the R-b classifier uses the rb_* fields, not these.
        store_start: region_end,
        fold_start: region_end,
        fold_end: region_end,
        jpot_start,
        num_segments: 1,
        segments: [SegmentLayout::default(); MAX_SEGMENTS],
        rb: true,
        rb_per_stripe,
        rb_num_stripes: num_stripes,
        rb_n_sb: n_sb,
        rb_chunks: chunks,
    }
}

/// Verifier-known per-block public inputs that, with `params`,
/// fully determine the canonical program (no witness). The
/// MED-3-attested tile, the C1-pinned BLAKE3 key/seeds. `hash_a`
/// / `hash_b` (the strip-opening roots) are *PI-bound*, not
/// PROGRAM_COLS, so they are not needed to build `RowDescriptor`s
/// — included in the design's `BlockPublic` for completeness but
/// omitted here until a class needs them.
#[derive(Debug, Clone, Copy)]
pub struct BlockPublic {
    /// MED-3-attested A-side tile row index.
    pub tile_i: u32,
    /// MED-3-attested B-side tile col index.
    pub tile_j: u32,
    /// C1-pinned keyed-BLAKE3 key κ (JOB_KEY).
    pub kappa: [u8; 32],
    /// Verifier-provided A-side public seed s_a. It remains the
    /// `noise_ref` seed for the §4.C.2/b2 store-noise pin; Nockchain
    /// AI-PoW uses the C1 COMMITMENT_HASH PI slot for the nonce-derived
    /// jackpot key.
    pub s_a: [u8; 32],
    /// C1-pinned B-side public seed s_b.
    pub s_b: [u8; 32],
}

pub type StripChunkRange = (usize, usize, usize);

/// Verifier-known Pearl-style strip schedule.
///
/// Pearl tickets are identified by explicit shifted row/column sets derived
/// from `PeriodicPattern.indices_with_offset(t_rows/t_cols)`, not only by a
/// square-contiguous `(tile_i, tile_j)`. This public schedule is the verifier
/// boundary for that requirement set: callers provide the exact A rows and B
/// columns, and the schedule validates that they are nonempty, canonical
/// strictly-increasing sets inside the committed matrices.
///
/// The current recursive AIR still admits only the contiguous-square subset
/// through [`BlockPublic`]. This type exists so future recursive statements can
/// bind Pearl's full public ticket without silently rewriting it to one tile.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StripIndexSchedule {
    pub a_indices: Vec<u32>,
    pub b_indices: Vec<u32>,
}

impl StripIndexSchedule {
    pub fn from_indices(
        params: &ZkParams,
        a_indices: Vec<u32>,
        b_indices: Vec<u32>,
    ) -> Result<Self, String> {
        params.validate_base()?;
        validate_strip_indices("A", &a_indices, params.m)?;
        validate_strip_indices("B", &b_indices, params.n)?;
        Ok(Self {
            a_indices,
            b_indices,
        })
    }

    pub fn from_tile(params: &ZkParams, tile_i: u32, tile_j: u32) -> Result<Self, String> {
        params.validate()?;
        let row_tiles = params.m / params.tile;
        let col_tiles = params.n / params.tile;
        if tile_i >= row_tiles || tile_j >= col_tiles {
            return Err(format!(
                "strip schedule tile_i={tile_i} or tile_j={tile_j} out of grid \
                 ({row_tiles}x{col_tiles} tiles)"
            ));
        }
        let a0 = tile_i
            .checked_mul(params.tile)
            .ok_or_else(|| "strip schedule A tile offset overflow".to_string())?;
        let b0 = tile_j
            .checked_mul(params.tile)
            .ok_or_else(|| "strip schedule B tile offset overflow".to_string())?;
        let a_indices = (0..params.tile)
            .map(|di| {
                a0.checked_add(di)
                    .ok_or_else(|| "strip schedule A index overflow".to_string())
            })
            .collect::<Result<Vec<_>, _>>()?;
        let b_indices = (0..params.tile)
            .map(|dj| {
                b0.checked_add(dj)
                    .ok_or_else(|| "strip schedule B index overflow".to_string())
            })
            .collect::<Result<Vec<_>, _>>()?;
        Self::from_indices(params, a_indices, b_indices)
    }

    pub fn from_block_public(params: &ZkParams, bp: &BlockPublic) -> Result<Self, String> {
        Self::from_tile(params, bp.tile_i, bp.tile_j)
    }

    pub fn chunk_ranges(
        &self,
        params: &ZkParams,
    ) -> Result<(StripChunkRange, StripChunkRange), String> {
        params.validate_base()?;
        validate_strip_indices("A", &self.a_indices, params.m)?;
        validate_strip_indices("B", &self.b_indices, params.n)?;
        let k = params.k as usize;
        let a_total = (params.m as usize)
            .checked_mul(k)
            .ok_or_else(|| "A matrix byte length overflow".to_string())?;
        let b_total = (params.n as usize)
            .checked_mul(k)
            .ok_or_else(|| "B matrix byte length overflow".to_string())?;
        Ok((
            indexed_strips_chunk_range(&self.a_indices, k, a_total),
            indexed_strips_chunk_range(&self.b_indices, k, b_total),
        ))
    }
}

fn validate_strip_indices(name: &str, indices: &[u32], dimension: u32) -> Result<(), String> {
    if indices.is_empty() {
        return Err(format!("{name} strip schedule must be nonempty"));
    }
    if indices.windows(2).any(|w| w[0] >= w[1]) {
        return Err(format!(
            "{name} strip schedule must be strictly increasing with no duplicates"
        ));
    }
    let last = *indices
        .last()
        .expect("nonempty strip schedule has last index");
    if last >= dimension {
        return Err(format!(
            "{name} strip schedule index {last} out of committed dimension {dimension}"
        ));
    }
    Ok(())
}

/// Phase A-CR — which [`RowClass`]es `canonical_program` already
/// reconstructs **params-pure and `== extract_program`-validated**
/// (the §5 staged-migration gate set). CR.6 (verify-path flip) is
/// permitted only once this is *every* class. Staged per
/// `2026-05-17_CANONICAL_PROGRAM_DESIGN.md` §7 (R1 discipline).
///
/// - **CR.1 (landed): `Pad`** — witness-free, exactly
///   [`RowDescriptor::padding`] (all PROGRAM_COLS zero except
///   `STARK_ROW_IDX = row_idx`).
/// - **CR.2 (landed): `KeyPin`** — witness-free; the two
///   `place_key_pin_row` rows: `mh_end+1` → `IS_USE_JOB_KEY`
///   (SELECTOR_COLS idx 2), `mh_end+2` → `IS_USE_COMMITMENT_HASH`
///   (idx 3); `mat_id=0`, all other PROGRAM_COLS zero. (The
///   pinned PI κ/s_a lives in `CV_IN`, a chip column, *not* a
///   PROGRAM_COL ⇒ the descriptor is `bp`-independent.)
/// - **CR.3 (landed): `JackpotHash`** — witness-free; the final
///   8-row keyed-BLAKE3 block (`place_jackpot_hash_block` →
///   `place_blake3_hash_with_selectors`). Every row:
///   `CV_OR_TWEAK_PREP = pack_tweak(JACKPOT_TWEAK)` (the
///   *params-pure constant* `{counter=0, block_len=64,
///   flags=0x1B}` — the hashed M/key are CV columns, not
///   PROGRAM_COLS); mat_id=0; row 0 → IS_NEW_BLAKE (idx 8); row
///   7 → IS_LAST_ROUND (idx 9) + IS_HASH_JACKPOT (idx 6).
/// - **CR.4 (landed): `StripOpenA/B`** — params-pure
///   strip-opening: CR.4a the `strip_blocks` walker (mirrors
///   `fold_strip`/`subtree_inside`/`place_leaf_chunk`) +
///   per-block leaf/parent/root tweak + `IS_HASH_A/B` finalize
///   selector; CR.4b co-located leaf round-0 `IS_MSG_MAT` (idx
///   10); CR.4c the 8 `NOISE_PACKED_PREP[0..8]` pins =
///   `polyval(noise_ref(bp.s_a/s_b at p=chunk·1024+b·64+g), 129)`
///   (the §4.C.2/b2 core — verifier obtains canonical noise
///   params-pure, not extract-of-reference; **`bp.s_a/s_b`
///   dependent**).
/// - **CR.5 (landed): `Sweep`/`Fold`** — params-pure §6(b)
///   schedule. Sweep (`place_useful_work_chain`→
///   `place_matmul_step`): per the nested (sbi,sbj,step,chunk)
///   loop, IS_RESET_CUMSUM (idx 0) on each sub-block's first
///   micro-step else IS_UPDATE_CUMSUM (idx 1). Fold
///   (`place_fold_chain`): CONTROL_PREP packs is_fold=1,
///   fold_slot=offset%16, fold_stripe=offset.
///
/// **`is_class_canonical` = EVERY class ⇒ CR.6 may flip the
/// verify path (the R1 soundness linchpin: VK commits to
/// `canonical_program`; gate Route-A + crit1_* +
/// debug-assertions-ON + a new adversarial before the flip).**
pub fn is_class_canonical(_class: RowClass) -> bool {
    // CR.0–CR.5: every RowClass is reconstructed params-pure and
    // `== extract_program(real P16(16|r) trace)`-validated.
    true
}

/// `pack_tweak` of the final jackpot-hash block's tweak — the
/// params-pure constant `Blake3Tweak { counter_low: 0,
/// counter_high: 0, block_len: 64, flags: 0x1B }`
/// (KEYED_HASH|CHUNK_START|CHUNK_END|ROOT) that
/// `place_jackpot_hash_block` hard-codes. Witness-independent.
pub(crate) fn jackpot_tweak_packed() -> u64 {
    pack_tweak(&Blake3Tweak {
        counter_low: 0,
        counter_high: 0,
        block_len: 64,
        flags: 0x1B,
    })
}

/// Params-pure PROGRAM_COL descriptor for offset `j` (0..8)
/// within an 8-row BLAKE3 compression block — the *one* schedule
/// `place_blake3_hash_with_selectors` writes: every row carries
/// `CV_OR_TWEAK_PREP = tweak_packed` and `mat_id = 0`; row 0 sets
/// `IS_NEW_BLAKE` (SELECTOR_COLS idx 8); row 7 (finalize) sets
/// `IS_LAST_ROUND` (idx 9) plus `finalize_extra`. Shared by CR.3
/// `JackpotHash` and (CR.4) `StripOpen*` — they differ *only* in
/// the tweak and the finalize-extra selectors (+ CR.4's
/// co-located leaf noise sub-slice pins, layered on top).
fn blake3_block_descriptor(j: usize, tweak_packed: u64, finalize_extra: &[usize]) -> RowDescriptor {
    let mut selectors = [false; NUM_SELECTORS];
    if j == 0 {
        selectors[8] = true; // IS_NEW_BLAKE
    }
    if j == 7 {
        selectors[9] = true; // IS_LAST_ROUND
        for &idx in finalize_extra {
            selectors[idx] = true;
        }
    }
    RowDescriptor {
        selectors,
        cv_or_tweak: tweak_packed,
        ..RowDescriptor::padding()
    }
}

/// **Phase A-CR — the params-pure canonical program.** Builds the
/// `trace_len × PROGRAM_COLS.len()` preprocessed matrix the CRIT-1
/// pin commits to, from public data **only** (`params` + the
/// attested/pinned `BlockPublic` + the params-pure `trace_len`) —
/// *no witness*. Per row: `row_schedule` (CR.0) → [`RowClass`] →
/// a params-pure [`RowDescriptor`] → the existing
/// [`build_preprocessed_columns`] packing (the *one* shared
/// schedule + the *one* packing — no prover/verifier divergence).
///
/// **Staged (R1 / §7).** Classes in [`is_class_canonical`] are
/// reconstructed exactly; all others currently fall back to
/// [`RowDescriptor::padding`] — a deliberate, KAT-fenced
/// *placeholder*, NOT a soundness claim. The §5 KAT
/// (`canonical_program == extract_program(honest_trace)`) asserts
/// equality **only on `is_class_canonical` rows**, widening as
/// CR.2–CR.5 land. **The verify path is NOT flipped to this until
/// CR.6, gated on every class canonical + the full KAT/Route-A/
/// crit1_*/debug-assertions-ON suite** (the soundness linchpin —
/// R1). Until then this is dead w.r.t. prove/verify.
pub fn canonical_program(
    params: &ZkParams,
    bp: &BlockPublic,
    trace_len: usize,
) -> Result<RowMajorMatrix<Val>, String> {
    // M3 (DoS audit): defense-in-depth at the verify-side params-pure
    // entry. The deep helpers (`schedule_layout`, `tile_chunk_range`,
    // `strip_opening_rows`) `assert!` invariants that *hold under
    // validated `ZkParams` + in-range `tile_i/j` + a non-degenerate
    // `trace_len`*. The production verifier reaches here only with
    // M2-validated params (via `ai-pow::zk_bridge`), so the asserts
    // are unreachable on the chain path. This entry validation turns
    // an attacker-controlled-params bypass (broken CRIT-1 trust pin)
    // into a typed `Err` rather than a deep cryptic panic.
    params.validate()?;
    if !params.noise_rank.is_multiple_of(16) {
        return Err(format!(
            "canonical_program requires 16 | noise_rank (Pearl §4.8 \
             always-16|r co-location path); got noise_rank={}",
            params.noise_rank
        ));
    }
    let row_tiles = params.m / params.tile;
    let col_tiles = params.n / params.tile;
    if bp.tile_i >= row_tiles || bp.tile_j >= col_tiles {
        return Err(format!(
            "canonical_program: tile_i={} or tile_j={} out of grid \
             ({}×{} tiles)",
            bp.tile_i, bp.tile_j, row_tiles, col_tiles
        ));
    }
    // Lower bound (the schedule needs ≥ 8 rows for the JackpotHash
    // suffix alone). The exact `fold_end ≤ trace_len - 8` check is
    // enforced inside `schedule_layout`; this catches the obvious
    // degenerate cases.
    if trace_len < 16 || !trace_len.is_power_of_two() {
        return Err(format!(
            "canonical_program: trace_len {trace_len} must be a \
             power of two ≥ 16"
        ));
    }
    let strip_schedule = StripIndexSchedule::from_block_public(params, bp)?;
    canonical_program_for_strip_schedule(params, &strip_schedule, bp, trace_len)
}

pub fn canonical_program_for_strip_schedule(
    params: &ZkParams,
    strip_schedule: &StripIndexSchedule,
    bp: &BlockPublic,
    trace_len: usize,
) -> Result<RowMajorMatrix<Val>, String> {
    params.validate_base()?;
    if !params.noise_rank.is_multiple_of(16) {
        return Err(format!(
            "canonical_program requires 16 | noise_rank (Pearl §4.8 \
             always-16|r co-location path); got noise_rank={}",
            params.noise_rank
        ));
    }
    if trace_len < 16 || !trace_len.is_power_of_two() {
        return Err(format!(
            "canonical_program: trace_len {trace_len} must be a \
             power of two ≥ 16"
        ));
    }
    validate_strip_indices("A", &strip_schedule.a_indices, params.m)?;
    validate_strip_indices("B", &strip_schedule.b_indices, params.n)?;
    // §6(b)-R-b: `num_stripes > STRIPE_MAX` uses the stripe-major
    // schedule (the segmented sub-block-major layout has no prover —
    // it hit the per-sub-block CUMSUM-continuity wall). The choice is
    // params-pure (`k/r`), so prover and verifier agree; ≤STRIPE_MAX is
    // byte-identical to before (the validated production path).
    let num_stripes = params.k as usize / params.noise_rank as usize;
    let l = if num_stripes > crate::composite_layout::STRIPE_MAX {
        schedule_layout_rb(params, strip_schedule, trace_len)
    } else {
        schedule_layout_for_strip_schedule(params, strip_schedule, trace_len)
    };
    let sp = StripPlan::build_for_strip_schedule(params, strip_schedule);
    let program: Vec<RowDescriptor> = (0..trace_len)
        .map(|r| row_descriptor(r, l.class_of(r), &l, &sp, params, bp))
        .collect();
    let rows = build_preprocessed_columns(&program, trace_len);
    let w = rows.first().map(|r| r.len()).unwrap_or(0);
    let flat: Vec<Val> = rows.into_iter().flatten().collect();
    Ok(RowMajorMatrix::new(flat, w))
}

/// One 8-row BLAKE3 block of a tile's strip-opening — the
/// params-pure unit `place_matrix_strip_opening` emits (the
/// block *contents* are witness, but the PROGRAM_COLS — tweak +
/// selector schedule — are not).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum StripBlock {
    /// Block `b`∈0..16 of the leaf chunk at global index
    /// `chunk_index` (`place_leaf_chunk`). `single_chunk_root` ⇒
    /// the lone-chunk path (block 15 carries `F_ROOT` + the
    /// `IS_HASH_A/B` finalize selector).
    Leaf {
        chunk_index: u64,
        b: usize,
        single_chunk_root: bool,
    },
    /// An auth-fold parent compression (`place_parent`); `is_root`
    /// ⇒ `F_ROOT` + the `IS_HASH_A/B` finalize selector.
    Parent { is_root: bool },
}

/// Params-pure post-order block list for one tile's strip-opening
/// — mirrors `fold_strip` / `subtree_inside` / `place_leaf_chunk`
/// **exactly** (sibling subtrees consume 0 rows; each block = 8
/// rows). `8 * strip_blocks(..).len()` == `strip_opening_rows`.
#[cfg(test)]
fn strip_blocks(c0: usize, c1: usize, num_chunks: usize) -> Vec<StripBlock> {
    let mut out = Vec::new();
    if num_chunks == 1 {
        // Lone chunk (place_matrix_strip_opening's num_chunks==1
        // branch): place_leaf_chunk(chunk_index=0,
        // single_chunk_root=true).
        for b in 0..16 {
            out.push(StripBlock::Leaf {
                chunk_index: 0,
                b,
                single_chunk_root: true,
            });
        }
        return out;
    }
    fn subtree_inside(out: &mut Vec<StripBlock>, lo: usize, hi: usize, is_root: bool) {
        if hi - lo == 1 {
            // place_leaf_chunk(chunk_index=lo,
            // single_chunk_root=false) — a leaf is never root when
            // num_chunks>1.
            for b in 0..16 {
                out.push(StripBlock::Leaf {
                    chunk_index: lo as u64,
                    b,
                    single_chunk_root: false,
                });
            }
            return;
        }
        let mid = lo + left_len((hi - lo) as u64) as usize;
        subtree_inside(out, lo, mid, false);
        subtree_inside(out, mid, hi, false);
        out.push(StripBlock::Parent { is_root });
    }
    fn fold(out: &mut Vec<StripBlock>, lo: usize, hi: usize, c0: usize, c1: usize, is_root: bool) {
        if hi <= c0 || lo >= c1 {
            return; // auth sibling — 0 rows
        }
        if c0 <= lo && hi <= c1 {
            subtree_inside(out, lo, hi, is_root);
            return;
        }
        let mid = lo + left_len((hi - lo) as u64) as usize;
        fold(out, lo, mid, c0, c1, false);
        fold(out, mid, hi, c0, c1, false);
        out.push(StripBlock::Parent { is_root });
    }
    fold(&mut out, 0, num_chunks, c0, c1, true);
    out
}

/// Selective (disjoint-chunk) strip-opening block list — the in-circuit schedule
/// for a **multi-proof** over an arbitrary sorted, distinct chunk set `sel`,
/// generalizing [`strip_blocks`] from a contiguous range to a set-membership
/// predicate (mirrors `blake3_tree::collect_siblings_set` / `open_strip_set`).
/// Sibling subtrees disjoint from `sel` consume 0 rows, so the block count is
/// O(|sel|·log n) rather than the O(max−min) covering range — this is what keeps
/// the Layer-0 trace inside `PEARL_TRACE_BOUND` for scattered MoE routed tokens.
/// For a contiguous `sel == c0..c1` it is byte-identical to `strip_blocks`.
#[allow(dead_code)]
fn strip_blocks_set(sel: &[usize], num_chunks: usize) -> Vec<StripBlock> {
    let mut out = Vec::new();
    if num_chunks == 1 {
        debug_assert_eq!(sel, [0]);
        for b in 0..16 {
            out.push(StripBlock::Leaf {
                chunk_index: 0,
                b,
                single_chunk_root: true,
            });
        }
        return out;
    }
    // #{selected chunks in [lo,hi)} for the sorted, distinct `sel`.
    fn sel_count(sel: &[usize], lo: usize, hi: usize) -> usize {
        sel.partition_point(|&c| c < hi) - sel.partition_point(|&c| c < lo)
    }
    fn subtree_inside(out: &mut Vec<StripBlock>, lo: usize, hi: usize, is_root: bool) {
        if hi - lo == 1 {
            for b in 0..16 {
                out.push(StripBlock::Leaf {
                    chunk_index: lo as u64,
                    b,
                    single_chunk_root: false,
                });
            }
            return;
        }
        let mid = lo + left_len((hi - lo) as u64) as usize;
        subtree_inside(out, lo, mid, false);
        subtree_inside(out, mid, hi, false);
        out.push(StripBlock::Parent { is_root });
    }
    fn fold(out: &mut Vec<StripBlock>, lo: usize, hi: usize, sel: &[usize], is_root: bool) {
        let cnt = sel_count(sel, lo, hi);
        if cnt == 0 {
            return; // auth sibling — 0 rows
        }
        if cnt == hi - lo {
            subtree_inside(out, lo, hi, is_root);
            return;
        }
        let mid = lo + left_len((hi - lo) as u64) as usize;
        fold(out, lo, mid, sel, false);
        fold(out, mid, hi, sel, false);
        out.push(StripBlock::Parent { is_root });
    }
    fold(&mut out, 0, num_chunks, sel, true);
    out
}

/// Trace-row count for the selective strip opening of the sorted, distinct chunk
/// set `sel` (8 rows per `StripBlock`) — the multi-proof analogue of
/// [`crate::blake3_tree::strip_opening_rows`]. Used by `expected_layer0_rows` so
/// the row budget matches `place_matrix_strip_opening_set`'s placement.
pub fn strip_opening_rows_set(sel: &[usize], num_chunks: usize) -> usize {
    8 * strip_blocks_set(sel, num_chunks).len()
}

/// Per-tile strip-opening plan: the params-pure block list + the
/// region's `IS_HASH_A/B` finalize selector (4 = `IS_HASH_A`
/// A-side, 5 = `IS_HASH_B` B-side — `place_matrix_hash_a/b`).
struct StripPlan {
    ca0: usize,
    cb0: usize,
    w_tile: usize,
    a_id_base: u64,
    b_id_base: u64,
    blocks_a: Vec<StripBlock>,
    blocks_b: Vec<StripBlock>,
    // B5b: the opened row/column pattern (may be non-contiguous). The sweep must
    // index these, not tile geometry.
    a_indices: Vec<u32>,
    b_indices: Vec<u32>,
}

impl StripPlan {
    fn build_for_strip_schedule(params: &ZkParams, strip_schedule: &StripIndexSchedule) -> Self {
        let ((_ca0, _ca1, a_nc), (_cb0, _cb1, b_nc)) = strip_schedule
            .chunk_ranges(params)
            .expect("StripPlan requires valid strip chunk ranges");
        let k = params.k as usize;
        // B5b: selective opening — the schedule authenticates only the chunks the
        // opened rows/cols actually touch (a SET), not the covering range. For a
        // contiguous tile the set == the range, so `strip_blocks_set` reduces to
        // `strip_blocks` (byte-identical); for scattered MoE rows it is
        // O(|rows|·⌈k/1024⌉), keeping the trace inside `PEARL_TRACE_BOUND`.
        let (a_chunks, _) =
            crate::blake3_tree::indexed_strips_chunk_set(&strip_schedule.a_indices, k, a_nc * 1024);
        let (b_chunks, _) =
            crate::blake3_tree::indexed_strips_chunk_set(&strip_schedule.b_indices, k, b_nc * 1024);
        let ca0 = a_chunks[0];
        let cb0 = b_chunks[0];
        let h_tile = strip_schedule.a_indices.len();
        let w_tile = strip_schedule.b_indices.len();
        let a_id_base = crate::composite_trace::NOISED_CHUNK_ID_BASE;
        let b_id_base = a_id_base + ((h_tile * k).div_ceil(8)) as u64;
        StripPlan {
            ca0,
            cb0,
            w_tile,
            a_id_base,
            b_id_base,
            blocks_a: strip_blocks_set(&a_chunks, a_nc),
            blocks_b: strip_blocks_set(&b_chunks, b_nc),
            a_indices: strip_schedule.a_indices.clone(),
            b_indices: strip_schedule.b_indices.clone(),
        }
    }
}

/// BLAKE3 flag bits (mirror `place_leaf_chunk` / `place_parent`).
const F_CHUNK_START: u32 = 1 << 0;
const F_CHUNK_END: u32 = 1 << 1;
const F_PARENT: u32 = 1 << 2;
const F_ROOT: u32 = 1 << 3;
const F_KEYED_HASH: u32 = 1 << 4;

/// Params-pure PROGRAM_COL descriptor for offset `j`∈0..8 of a
/// strip-opening block. CR.4a: the pure-BLAKE3 schedule (tweak +
/// selectors). **CR.4b:** on the 16|r co-location path every
/// *leaf* block's round-0 row (`j==0`) is additionally the M-S1
/// `noised_packed` producer — `place_leaf_chunk` re-fills its
/// control cells with `{IS_NEW_BLAKE, IS_MSG_MAT}` (SELECTOR_COLS
/// idx 8 + 10), `mat_id = first addressed 8-byte sub-slice ID`,
/// `msg_pair=0` (the cx.1c pin). The 8
/// `NOISE_PACKED_PREP` pins on those rows are CR.4c. Parent
/// blocks are never co-located. The tweak/flags are params-pure
/// (`place_leaf_chunk`/`place_parent`): leaf → `counter =
/// chunk_index`, `flags = F_KEYED_HASH | F_CHUNK_START(b==0) |
/// F_CHUNK_END(b==15) | F_ROOT(single-chunk-root&&b==15)`; parent
/// → `F_KEYED_HASH | F_PARENT | F_ROOT(is_root)`; the root
/// block's finalize row gets the `IS_HASH_A/B` extra
/// (`selector_idx`).
fn strip_row_descriptor(spec: StripBlock, j: usize, selector_idx: usize) -> RowDescriptor {
    let (tweak, is_root) = match spec {
        StripBlock::Leaf {
            chunk_index,
            b,
            single_chunk_root,
        } => {
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
            (
                Blake3Tweak {
                    counter_low: chunk_index as u32,
                    counter_high: (chunk_index >> 32) as u16,
                    block_len: 64,
                    flags,
                },
                is_root,
            )
        }
        StripBlock::Parent { is_root } => {
            let mut flags = F_KEYED_HASH | F_PARENT;
            if is_root {
                flags |= F_ROOT;
            }
            (
                Blake3Tweak {
                    counter_low: 0,
                    counter_high: 0,
                    block_len: 64,
                    flags,
                },
                is_root,
            )
        }
    };
    let extra: &[usize] = if is_root {
        core::slice::from_ref(&selector_idx)
    } else {
        &[]
    };
    let mut desc = blake3_block_descriptor(j, pack_tweak(&tweak), extra);
    // CR.4b: co-located leaf round-0 producer row (16|r path —
    // `row_schedule` guarantees 16|r, and `place_leaf_chunk`
    // co-locates every leaf block's round-0 row when noise is
    // present). Adds IS_MSG_MAT (idx 10) on top of the round-0
    // IS_NEW_BLAKE (idx 8) already set by `blake3_block_descriptor`;
    // msg_pair stays 0; mat_id is layered in row_descriptor from
    // the strip side and chunk offset. Parent blocks are never
    // co-located.
    if matches!(spec, StripBlock::Leaf { .. }) && j == 0 {
        desc.selectors[10] = true; // IS_MSG_MAT ⇒ g = 1
    }
    desc
}

/// **CR.4c — the §4.C.2/b2 core.** Params-pure
/// `NOISE_PACKED_PREP[0..8]` for a co-located leaf round-0 row
/// (block `b` of leaf chunk `chunk_index`, A-side ⇒ `e_value`/
/// `s_a`/`|A|=m·k`; B-side ⇒ `f_value`/`s_b`/`|B|=n·k`). Mirrors
/// `place_leaf_chunk` exactly: for sub-slice `s`, `pin[s] =
/// Σ_{m<8} noise[s·8+m] · NOISE_PACKING_BASE^m`, where the strip
/// byte position is `p = chunk_index·1024 + b·64 + (s·8+m)`
/// (the bridge's `a_strip_lo + j` collapses to this since
/// `strip_lo = c0·1024` and `j` is chunk-`c0`-relative), and
/// `noise = noise_ref(seed, …) if p < |M| else 0` (chunk
/// padding). Witness-free: only `bp.s_a/s_b` + params.
fn coloc_leaf_noise_pins(
    side_a: bool,
    chunk_index: u64,
    b: usize,
    params: &ZkParams,
    bp: &BlockPublic,
) -> [i64; 8] {
    let k = params.k as usize;
    let r = params.noise_rank;
    let limit = if side_a {
        params.m as usize * k
    } else {
        params.n as usize * k
    };
    let mut pins = [0i64; 8];
    for (s, pin) in pins.iter_mut().enumerate() {
        let mut npp: i64 = 0;
        let mut pw: i64 = 1;
        for mm in 0..8 {
            let p = chunk_index as usize * 1024 + b * 64 + s * 8 + mm;
            let no: i8 = if p < limit {
                if side_a {
                    // A row-major m×k: row=p/k, col=p%k.
                    e_value(&bp.s_a, (p / k) as u32, (p % k) as u32, r)
                } else {
                    // B col-major n×k: col=p/k, k-idx=p%k ⇒
                    // f_value(s_b, k-idx, col).
                    f_value(&bp.s_b, (p % k) as u32, (p / k) as u32, r)
                }
            } else {
                0
            };
            npp += (no as i64) * pw;
            pw *= NOISE_PACKING_BASE as i64;
        }
        *pin = npp;
    }
    pins
}

/// Params-pure per-row descriptor for a row. CR.1 `Pad` +
/// CR.2 `KeyPin` are exact; not-yet-canonical classes return the
/// neutral placeholder — fenced by [`is_class_canonical`] / the
/// staged §5 KAT (NOT a soundness claim; see [`canonical_program`]).
/// CR.3–CR.5 replace each arm with its params-pure construction
/// (`StripOpen*` co-located leaf rows' 8 noise sub-slice pins from
/// `noise_ref(bp.s_a/s_b)`, the §4.C.2/b2 core; the §6(b)
/// `Sweep`/`Fold` schedule), each landed behind its own
/// `== extract` gate.
fn row_descriptor(
    row_idx: usize,
    class: RowClass,
    layout: &ScheduleLayout,
    sp: &StripPlan,
    params: &ZkParams,
    bp: &BlockPublic,
) -> RowDescriptor {
    match class {
        RowClass::Pad => RowDescriptor::padding(),
        RowClass::StripOpenA | RowClass::StripOpenB => {
            // CR.4a: pure-BLAKE3 schedule (flat 8-row blocks;
            // sibling subtrees → 0 rows). A selector_idx=4
            // (IS_HASH_A), B=5 (IS_HASH_B). CR.4b: co-located leaf
            // round-0 IS_MSG_MAT. CR.4c: the 8 NOISE_PACKED_PREP
            // pins = polyval(noise_ref(s_a/s_b at the leaf
            // (i,l)),129) — the §4.C.2/b2 core.
            let side_a = class == RowClass::StripOpenA;
            let (offset, blocks, selector_idx) = if side_a {
                (row_idx, &sp.blocks_a, 4usize)
            } else {
                (row_idx - layout.na, &sp.blocks_b, 5usize)
            };
            let block = offset / 8;
            let j = offset % 8;
            debug_assert!(
                block < blocks.len(),
                "strip row offset {offset} past block list"
            );
            let spec = blocks[block];
            let mut desc = strip_row_descriptor(spec, j, selector_idx);
            // CR.4c: layer the 8 noise sub-slice pins onto the
            // co-located leaf round-0 producer rows (16|r path).
            if let StripBlock::Leaf { chunk_index, b, .. } = spec {
                if j == 0 {
                    let pins = coloc_leaf_noise_pins(side_a, chunk_index, b, params, bp);
                    desc.noise_packed = pins[0];
                    desc.noise_packed_hi.copy_from_slice(&pins[1..8]);
                    let (base, c0) = if side_a {
                        (sp.a_id_base, sp.ca0)
                    } else {
                        (sp.b_id_base, sp.cb0)
                    };
                    desc.mat_id =
                        (base + ((chunk_index as usize - c0) * 128 + b * 8) as u64) as u32;
                }
            }
            desc
        }
        RowClass::KeyPin => {
            // CR.2: `place_key_pin_row` sets exactly one selector
            // and `mat_id=0`; row mh_end+1 = JOB_KEY (SELECTOR_COLS
            // idx 2), mh_end+2 = COMMITMENT_HASH slot (idx 3). The
            // pinned PI (κ / jackpot key) is written to `CV_IN` — a chip
            // column, not a PROGRAM_COL — so the canonical
            // descriptor is `bp`-independent.
            let mut selectors = [false; NUM_SELECTORS];
            if row_idx == layout.mh_end + 1 {
                selectors[2] = true; // IS_USE_JOB_KEY
            } else {
                debug_assert_eq!(row_idx, layout.mh_end + 2);
                selectors[3] = true; // IS_USE_COMMITMENT_HASH
            }
            RowDescriptor {
                selectors,
                ..RowDescriptor::padding()
            }
        }
        RowClass::JackpotHash => {
            // CR.3: the final 8-row keyed-BLAKE3 block at
            // `jpot_start`. Params-pure constant tweak; finalize
            // extra = IS_HASH_JACKPOT (SELECTOR_COLS idx 6, the
            // `place_jackpot_hash_block` `&[6]`).
            let j = row_idx - layout.jpot_start;
            debug_assert!(j < 8, "jackpot block is 8 rows");
            blake3_block_descriptor(j, jackpot_tweak_packed(), &[6])
        }
        RowClass::Sweep => {
            // CR.5: §6(b) sweep (`place_useful_work_chain` →
            // `place_matmul_step`). Row order is the nested
            // (sbi, sbj, step, chunk) loop; each row sets exactly
            // IS_RESET_CUMSUM (SELECTOR_COLS idx 0) on the
            // sub-block's first micro-step (step==0 && chunk==0)
            // else IS_UPDATE_CUMSUM (idx 1); mat_id=0, no
            // fold/msg_pair, no NOISE/CV/AB. num_stripes = k/r;
            // chunks = ⌈r/TILE_D⌉ (§6(b)-G1).
            let r = params.noise_rank as usize;
            let num_stripes = params.k as usize / r;
            let chunks = r.div_ceil(TILE_D).max(1);
            let n_sbj = sp.w_tile / TILE_H;
            // §6(b)-R-b: stripe-major sweep (`place_useful_work_chain_rb`).
            // The row → (stripe, sub-block, chunk) mapping and the
            // `IS_RESET_CUMSUM` rule differ from the segmented
            // sub-block-major path: R-b resets the matmul CUMSUM ONCE
            // (the very first sweep row, `stripe==0 && sb==0 && chunk==0`)
            // and accumulates as a harmless byproduct thereafter; the
            // per-stripe x_step is carried by the TileReduce lane, not
            // the matmul CUMSUM. `ids_for` (the positioned `noised_packed`
            // keys) is identical to the segmented path — same tile strips,
            // stripe-major order (LogUp is a multiset ⇒ order-independent).
            let (step, sbi, sbj, chunk, is_reset) = if layout.rb {
                let (stripe, sb, chunk) = layout
                    .rb_sweep_coords(row_idx)
                    .expect("R-b Sweep class ⇒ rb_sweep_coords is Some");
                let sbi = sb / n_sbj;
                let sbj = sb % n_sbj;
                let is_reset = stripe == 0 && sb == 0 && chunk == 0;
                (stripe, sbi, sbj, chunk, is_reset)
            } else {
                let per = num_stripes * chunks;
                let sweep_offset = row_idx - layout.sweep_start;
                let subblock = sweep_offset / per;
                let within = sweep_offset % per;
                let step = within / chunks;
                let chunk = within % chunks;
                let sbi = subblock / n_sbj;
                let sbj = subblock % n_sbj;
                let is_reset = step == 0 && chunk == 0;
                (step, sbi, sbj, chunk, is_reset)
            };
            let lo = step * r;
            let c0 = chunk * TILE_D;
            let w = (r - c0).min(TILE_D);
            let ids_for = |side_a: bool, sb_base: usize| -> [u64; 4] {
                // B5b: map the tile-local sub-block row to the actual opened
                // (possibly non-contiguous) matrix row, then to its covering-range
                // position (`row - c_base`) — the key the strip-opening producer
                // publishes. For a contiguous tile `indices[i] - c_base == i`, so
                // this is byte-identical to the previous tile-geometry path.
                let (indices, c_base) = if side_a {
                    (&sp.a_indices, sp.ca0)
                } else {
                    (&sp.b_indices, sp.cb0)
                };
                core::array::from_fn(|jc| {
                    let mut src = [None; 8];
                    for m in 0..8 {
                        let f = jc * 8 + m;
                        let (di, col) = (f / TILE_D, f % TILE_D);
                        if col < w {
                            let lane = indices[sb_base + di] as usize - c_base;
                            src[m] = Some((lane as u32, (lo + c0 + col) as u32));
                        }
                    }
                    crate::composite_trace::noised_chunk_id(
                        if side_a { sp.a_id_base } else { sp.b_id_base },
                        params.k as usize,
                        &src,
                    )
                })
            };
            let a_ids = ids_for(true, sbi * TILE_H);
            let b_ids = ids_for(false, sbj * TILE_H);
            let mut selectors = [false; NUM_SELECTORS];
            selectors[if is_reset { 0 } else { 1 }] = true;
            RowDescriptor {
                selectors,
                ab_id: crate::composite_preprocess::pack_ab_id(a_ids[0], b_ids[0]),
                a_ids,
                b_ids,
                ..RowDescriptor::padding()
            }
        }
        RowClass::Fold => {
            // CR.5: FoldChip row. CONTROL_PREP packs is_fold=1,
            // fold_slot, fold_stripe; mat_id=0; FOLD_* are chip columns.
            let (fold_slot, fold_stripe) = if layout.rb {
                // §6(b)-R-b: one interleaved fold row per stripe, folding
                // that stripe's TileReduce x_step into slot `stripe%16`.
                // `fold_stripe = 0` ALWAYS (FOLD_STRIPE_SEL[0]=1 satisfies
                // FoldChip's one-hot without activating the disabled SX
                // 64-lane keystone; sx_bound=false for R-b) — mirrors
                // `place_useful_work_chain_rb`'s fold row.
                let stripe = layout
                    .rb_fold_stripe(row_idx)
                    .expect("R-b Fold class ⇒ rb_fold_stripe is Some");
                ((stripe % 16) as u8, 0u8)
            } else {
                // Segmented §6(b)-G2: fold_slot = offset%16,
                // fold_stripe = offset (the SX_XR lane).
                let offset = row_idx - layout.fold_start;
                ((offset % 16) as u8, offset as u8)
            };
            RowDescriptor {
                is_fold: true,
                fold_slot,
                fold_stripe,
                ..RowDescriptor::padding()
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use p3_matrix::Matrix;

    use super::*;

    /// The selective (disjoint-chunk) strip-opening schedule strictly generalizes
    /// the contiguous-range schedule: for `sel == c0..c1` they are byte-identical.
    #[test]
    fn strip_blocks_set_generalizes_range() {
        for &nc in &[2usize, 3, 5, 8, 13, 17, 31, 64] {
            for c0 in 0..nc {
                for c1 in (c0 + 1)..=nc {
                    let sel: Vec<usize> = (c0..c1).collect();
                    assert_eq!(
                        strip_blocks_set(&sel, nc),
                        strip_blocks(c0, c1, nc),
                        "selective schedule != range schedule for [{c0},{c1}) of {nc}"
                    );
                }
            }
        }
    }

    /// A scattered selected set costs O(|sel|·log n) blocks (one 16-block leaf per
    /// selected chunk + fold parents), far fewer than the covering-range schedule —
    /// the production trace-size win that keeps the Layer-0 trace inside
    /// `PEARL_TRACE_BOUND` for scattered MoE routed tokens.
    #[test]
    fn strip_blocks_set_is_sublinear_for_scattered() {
        let nc = 4096usize;
        let sel: Vec<usize> = (0..64).map(|i| i * 63).collect();
        let blocks = strip_blocks_set(&sel, nc);
        let leaves = blocks
            .iter()
            .filter(|b| matches!(b, StripBlock::Leaf { .. }))
            .count();
        assert_eq!(
            leaves,
            sel.len() * 16,
            "one 16-block leaf per selected chunk"
        );
        let covering = strip_blocks(*sel.first().unwrap(), sel.last().unwrap() + 1, nc);
        assert!(
            blocks.len() < covering.len() / 4,
            "selective {} not << covering {}",
            blocks.len(),
            covering.len()
        );
        assert!(matches!(
            blocks.last(),
            Some(StripBlock::Parent { is_root: true })
        ));
        assert_eq!(
            blocks
                .iter()
                .filter(|b| matches!(b, StripBlock::Parent { is_root: true }))
                .count(),
            1
        );
    }
    use crate::blake3_tree::tile_chunk_range;

    fn p16() -> ZkParams {
        ZkParams {
            m: 16,
            k: 64,
            n: 16,
            noise_rank: 16,
            tile: 8,
            difficulty_bits: 0,
        }
    }

    #[test]
    fn row_schedule_regions_are_contiguous_and_cover_trace() {
        let p = p16();
        let len = 1 << 13; // MIN_STARK_LEN-class (P16 sub-envelope)
        let s = row_schedule(&p, 0, 0, len);
        assert_eq!(s.len(), len);
        // Region order: StripOpenA → StripOpenB → (Pad gap) →
        // KeyPin×2 → Sweep → (Pad) → Fold → (Pad) → JackpotHash.
        assert_eq!(s[0], RowClass::StripOpenA);
        assert_eq!(*s.last().unwrap(), RowClass::JackpotHash);
        assert_eq!(
            s.iter().filter(|&&c| c == RowClass::KeyPin).count(),
            2,
            "exactly two key-pin rows (JOB_KEY, COMMITMENT_HASH)"
        );
        assert_eq!(
            s.iter().filter(|&&c| c == RowClass::JackpotHash).count(),
            8,
            "jackpot-hash block is the last 8 rows"
        );
        let nsweep = s.iter().filter(|&&c| c == RowClass::Sweep).count();
        let nfold = s.iter().filter(|&&c| c == RowClass::Fold).count();
        assert_eq!(nfold, (p.k / p.noise_rank) as usize, "fold = num_stripes");
        assert!(nsweep > 0 && s.contains(&RowClass::StripOpenB));
    }

    fn bp0() -> BlockPublic {
        BlockPublic {
            tile_i: 0,
            tile_j: 0,
            kappa: [0u8; 32],
            s_a: [0u8; 32],
            s_b: [0u8; 32],
        }
    }

    /// §6(b)-R-b node-verify linchpin — the canonical (verifier-rebuilt,
    /// params-pure) program for `num_stripes > STRIPE_MAX` is
    /// BYTE-IDENTICAL to `extract_program` of an honest stripe-major
    /// trace (`place_useful_work_chain_rb`) on every sweep+fold row.
    /// The node commits to THIS canonical program, so it MUST match what
    /// the R-b prover builds — a divergence would reject a legitimate
    /// >64 certificate (or, if the divergence were in the prover's favour,
    /// > admit a forged schedule). Origin tile (0,0), 16|r ⇒ `ca0=0` ⇒ the
    /// > canonical `ids_for` (`indices[j]-ca0`) equals the trace's
    /// > tile-local lane. PROGRAM_COLS are position-derived (not
    /// > value-derived), so an all-zero matrix suffices to fix the schedule.
    #[test]
    fn cr_rb_canonical_program_eq_extract_on_sweep_fold() {
        use crate::composite_full_air::{extract_program, PROGRAM_COLS};
        use crate::composite_trace::CompositeTrace;

        let w = PROGRAM_COLS.len();
        for &num_stripes in &[65usize, 96, 128] {
            let r = 16u32;
            let k = num_stripes as u32 * r;
            let params = ZkParams {
                m: 8,
                k,
                n: 8,
                noise_rank: r,
                tile: 8,
                difficulty_bits: 0,
            };
            let bp = bp0();
            let sched = StripIndexSchedule::from_block_public(&params, &bp).expect("origin tile");
            // The R-b region size is trace_len-independent; probe it, then
            // size the trace to the next power of two that fits.
            let probe = schedule_layout_rb(&params, &sched, 1 << 22);
            let region_end = probe.sweep_start + probe.rb_num_stripes * probe.rb_per_stripe;
            let trace_len =
                ((region_end + 8).next_power_of_two()).max(crate::composite_layout::MIN_STARK_LEN);

            let canon =
                canonical_program(&params, &bp, trace_len).expect("R-b canonical program builds");
            assert_eq!(canon.width(), w, "canonical program width = PROGRAM_COLS");

            let l = schedule_layout_rb(&params, &sched, trace_len);
            assert!(l.rb, "params num_stripes>STRIPE_MAX ⇒ rb layout");
            let (h_tile, w_tile, kk) = (8usize, 8usize, k as usize);
            let a_prime = vec![0i8; h_tile * kk];
            let b_prime = vec![0i8; w_tile * kk];
            let mut trace = CompositeTrace::baseline(trace_len);
            trace.place_useful_work_chain_rb(
                l.sweep_start, &a_prime, &b_prime, h_tile, w_tile, r as usize, num_stripes,
            );
            let extracted = extract_program(&trace.matrix);

            let (mut nsweep, mut nfold) = (0usize, 0usize);
            for row in l.sweep_start..region_end {
                match l.class_of(row) {
                    RowClass::Sweep => nsweep += 1,
                    RowClass::Fold => nfold += 1,
                    other => panic!("row {row} in R-b region classified {other:?}"),
                }
                for c in 0..w {
                    assert_eq!(
                        canon.values[row * w + c],
                        extracted.values[row * w + c],
                        "R-b canonical != extract at row {row} col {c} \
                         (num_stripes={num_stripes}, class={:?})",
                        l.class_of(row),
                    );
                }
            }
            assert_eq!(nfold, num_stripes, "one fold row per stripe");
            assert_eq!(
                nsweep,
                num_stripes * l.rb_n_sb * l.rb_chunks,
                "sweep row count = num_stripes·n_sb·chunks"
            );
        }
    }

    /// §6(b)-R-b node-verify linchpin, NON-CONTIGUOUS pattern ticket — the
    /// Pearl-parity gap fix (2026-07-15). This is the production non-contiguous
    /// case: a `from_indices` opened schedule (a Pearl periodic pattern; MoE
    /// `outer_indices` gathers have the same non-contiguous shape). The opened
    /// lanes (`index − covering-range base`) are NOT tile-local, so the R-b
    /// prover MUST use the lane-indexed placement
    /// (`place_useful_work_chain_rb_indexed`) to match the canonical program's
    /// `ids_for` — the tile-local wrapper would MISMATCH (that was the bug).
    /// Asserts canonical == extract(indexed R-b trace) cell-for-cell on every
    /// sweep+fold row, at `num_stripes > STRIPE_MAX`.
    #[test]
    fn cr_rb_canonical_program_eq_extract_noncontiguous_indexed() {
        use crate::composite_full_air::{extract_program, PROGRAM_COLS};
        use crate::composite_trace::CompositeTrace;

        let w = PROGRAM_COLS.len();
        for &k in &[1536u32, 2048] {
            let r = 16u32;
            let num_stripes = (k / r) as usize; // 96, 128 — both > STRIPE_MAX
            let params = ZkParams {
                m: 64,
                k,
                n: 64,
                noise_rank: r,
                tile: 8,
                difficulty_bits: 0,
            };
            // Non-contiguous Pearl-pattern opened rows/cols (h=w=8, h·w=64).
            let a_indices = vec![0u32, 1, 8, 9, 16, 17, 24, 25];
            let b_indices = vec![0u32, 1, 8, 9, 32, 33, 40, 41];
            let sched = StripIndexSchedule::from_indices(&params, a_indices, b_indices)
                .expect("valid Pearl-pattern schedule");
            let bp = bp0();

            let probe = schedule_layout_rb(&params, &sched, 1 << 22);
            let region_end = probe.sweep_start + probe.rb_num_stripes * probe.rb_per_stripe;
            let trace_len =
                ((region_end + 8).next_power_of_two()).max(crate::composite_layout::MIN_STARK_LEN);

            let canon = canonical_program_for_strip_schedule(&params, &sched, &bp, trace_len)
                .expect("R-b canonical program builds for the pattern");

            let l = schedule_layout_rb(&params, &sched, trace_len);
            let (h_tile, w_tile, kk) = (8usize, 8usize, k as usize);
            // Opened covering-range lanes (index − chunk base) — the SAME mapping
            // the production scheduled prover passes to the indexed R-b chain.
            let ((ca0, _, _), (cb0, _, _)) = sched.chunk_ranges(&params).expect("chunk ranges");
            let a_lanes: Vec<usize> = sched.a_indices.iter().map(|&i| i as usize - ca0).collect();
            let b_lanes: Vec<usize> = sched.b_indices.iter().map(|&i| i as usize - cb0).collect();
            assert_ne!(
                a_lanes,
                (0..h_tile).collect::<Vec<_>>(),
                "lanes must be non-tile-local"
            );

            let a_prime = vec![0i8; h_tile * kk];
            let b_prime = vec![0i8; w_tile * kk];
            let mut trace = CompositeTrace::baseline(trace_len);
            trace.place_useful_work_chain_rb_indexed(
                l.sweep_start, &a_prime, &b_prime, h_tile, w_tile, r as usize, num_stripes,
                &a_lanes, &b_lanes,
            );
            let extracted = extract_program(&trace.matrix);

            for row in l.sweep_start..region_end {
                for c in 0..w {
                    assert_eq!(
                        canon.values[row * w + c],
                        extracted.values[row * w + c],
                        "R-b(indexed) canonical != extract at row {row} col {c} \
                         (non-contiguous pattern, num_stripes={num_stripes})",
                    );
                }
            }
        }
    }

    #[test]
    fn strip_index_schedule_from_tile_matches_legacy_tile_ranges() {
        let p = ZkParams {
            m: 64,
            k: 1536,
            n: 96,
            noise_rank: 16,
            tile: 8,
            difficulty_bits: 0,
        };
        let sched = StripIndexSchedule::from_tile(&p, 3, 7).expect("tile in grid");
        assert_eq!(sched.a_indices, vec![24, 25, 26, 27, 28, 29, 30, 31]);
        assert_eq!(sched.b_indices, vec![56, 57, 58, 59, 60, 61, 62, 63]);

        let (a_range, b_range) = sched.chunk_ranges(&p).expect("valid schedule");
        let t = p.tile as usize;
        let k = p.k as usize;
        assert_eq!(
            a_range,
            tile_chunk_range(3, t, k, (p.m as usize) * k),
            "contiguous A schedule must preserve legacy verifier range"
        );
        assert_eq!(
            b_range,
            tile_chunk_range(7, t, k, (p.n as usize) * k),
            "contiguous B schedule must preserve legacy verifier range"
        );
    }

    #[test]
    fn strip_index_schedule_accepts_pearl_noncontiguous_public_sets() {
        let p = ZkParams {
            m: 128,
            k: 2048,
            n: 128,
            noise_rank: 16,
            tile: 8,
            difficulty_bits: 0,
        };
        let sched = StripIndexSchedule::from_indices(
            &p,
            vec![0, 8, 64, 72],
            vec![0, 1, 8, 9, 32, 33, 40, 41],
        )
        .expect("Pearl periodic-pattern ticket sets are valid");
        let (a_range, b_range) = sched.chunk_ranges(&p).expect("valid ranges");
        assert_eq!(a_range, (0, 146, 256));
        assert_eq!(b_range, (0, 84, 256));
    }

    #[test]
    fn strip_index_schedule_rejects_invalid_public_sets_without_panic() {
        let p = p16();
        assert!(
            StripIndexSchedule::from_indices(&p, vec![], vec![0]).is_err(),
            "empty A schedule must be rejected"
        );
        assert!(
            StripIndexSchedule::from_indices(&p, vec![0], vec![0, 0]).is_err(),
            "duplicate B index must be rejected"
        );
        assert!(
            StripIndexSchedule::from_indices(&p, vec![2, 1], vec![0]).is_err(),
            "unsorted A schedule must be rejected"
        );
        assert!(
            StripIndexSchedule::from_indices(&p, vec![0], vec![p.n]).is_err(),
            "out-of-bounds B index must be rejected"
        );
        assert!(
            StripIndexSchedule::from_tile(&p, p.m / p.tile, 0).is_err(),
            "out-of-grid tile must be rejected"
        );
    }

    #[test]
    fn schedule_layout_for_strip_schedule_matches_contiguous_tile_layout() {
        let p = ZkParams {
            m: 64,
            k: 512,
            n: 96,
            noise_rank: 16,
            tile: 8,
            difficulty_bits: 0,
        };
        let len = 1 << 15;
        let sched = StripIndexSchedule::from_tile(&p, 3, 7).expect("tile schedule");
        let from_tile = schedule_layout(&p, 3, 7, len);
        let from_schedule = schedule_layout_for_strip_schedule(&p, &sched, len);
        assert_eq!(from_schedule.na, from_tile.na);
        assert_eq!(from_schedule.mh_end, from_tile.mh_end);
        assert_eq!(from_schedule.sweep_start, from_tile.sweep_start);
        assert_eq!(from_schedule.store_start, from_tile.store_start);
        assert_eq!(from_schedule.fold_start, from_tile.fold_start);
        assert_eq!(from_schedule.fold_end, from_tile.fold_end);
        assert_eq!(from_schedule.jpot_start, from_tile.jpot_start);

        let bp = BlockPublic {
            tile_i: 3,
            tile_j: 7,
            ..bp0()
        };
        let from_public = canonical_program(&p, &bp, len).expect("contiguous canonical program");
        let from_explicit = canonical_program_for_strip_schedule(&p, &sched, &bp, len)
            .expect("explicit schedule canonical program");
        assert_eq!(from_explicit.width, from_public.width);
        assert_eq!(from_explicit.height(), from_public.height());
        assert_eq!(from_explicit.values, from_public.values);
    }

    /// §6(b)-G3 — `num_stripes > STRIPE_MAX` splits into interleaved
    /// `[sweep, fold]` segments; single-field accessors mirror segment 0,
    /// and `class_of` / the local-index helpers resolve each segment.
    #[test]
    fn schedule_layout_segments_over_stripe_max() {
        let sm = crate::composite_layout::STRIPE_MAX; // 64
        let p = ZkParams {
            m: 512,
            k: 2048, // num_stripes = k/r = 128 = 2·STRIPE_MAX ⇒ 2 full segments
            n: 512,
            noise_rank: 16,
            tile: 8,
            difficulty_bits: 0,
        };
        let len = 1 << 13;
        let l = schedule_layout(&p, 0, 0, len);
        assert_eq!(l.num_segments, 2);
        // Single-field accessors mirror segment 0 (pre-G3 view).
        assert_eq!(l.sweep_start, l.segments[0].sweep_start);
        assert_eq!(l.store_start, l.segments[0].store_start);
        assert_eq!(l.fold_start, l.segments[0].fold_start);
        assert_eq!(l.fold_end, l.segments[0].fold_end);
        // Each full segment folds exactly STRIPE_MAX stripes.
        for g in 0..2 {
            assert_eq!(l.segments[g].fold_end - l.segments[g].fold_start, sm);
        }
        // Interleaved + contiguous: seg0.fold_end == seg1.sweep_start.
        assert_eq!(l.segments[0].fold_end, l.segments[1].sweep_start);
        // sweep-per-stripe = (h/2)(w/2)·⌈r/16⌉ = 4·4·1 = 16 (square tile=8).
        assert_eq!(
            l.segments[0].store_start - l.segments[0].sweep_start,
            16 * sm
        );
        // class_of + local-index helpers resolve segment 1.
        let r1 = l.segments[1].fold_start + 5;
        assert!(matches!(l.class_of(r1), RowClass::Fold));
        assert_eq!(l.fold_segment_and_local(r1), Some((1, 5)));
        assert!(matches!(
            l.class_of(l.segments[1].sweep_start),
            RowClass::Sweep
        ));
        assert_eq!(
            l.sweep_segment_and_offset(l.segments[1].sweep_start + 3),
            Some((1, 3))
        );
        // Fold rows keep GLOBAL stripe order: seg1 local 5 = global stripe 69,
        // and 69 % 16 == 5 == 69-64 (64 % 16 == 0 alignment the FoldChip needs).
        assert_eq!((sm + 5) % 16, 5 % 16);
    }

    /// §6(b)-G3 — a partial trailing segment (`num_stripes` not a multiple
    /// of `STRIPE_MAX`).
    #[test]
    fn schedule_layout_segments_partial_last() {
        let p = ZkParams {
            m: 512,
            k: 1600, // num_stripes = 100 = 64 + 36
            n: 512,
            noise_rank: 16,
            tile: 8,
            difficulty_bits: 0,
        };
        let len = 1 << 13;
        let l = schedule_layout(&p, 0, 0, len);
        assert_eq!(l.num_segments, 2);
        assert_eq!(l.segments[0].fold_end - l.segments[0].fold_start, 64);
        assert_eq!(l.segments[1].fold_end - l.segments[1].fold_start, 36);
        assert_eq!(l.segments[0].fold_end, l.segments[1].sweep_start);
        // Last fold row is the last stripe; the JackpotHash suffix follows.
        assert!(l.segments[1].fold_end <= l.jpot_start);
    }

    #[test]
    fn schedule_layout_for_strip_schedule_accounts_for_rectangular_shape() {
        let p = ZkParams {
            m: 128,
            k: 512,
            n: 128,
            noise_rank: 16,
            tile: 8,
            difficulty_bits: 0,
        };
        let len = 1 << 15;
        let sched = StripIndexSchedule::from_indices(
            &p,
            vec![0, 8, 64, 72],
            vec![0, 1, 8, 9, 32, 33, 40, 41],
        )
        .expect("rectangular Pearl schedule");
        let layout = schedule_layout_for_strip_schedule(&p, &sched, len);
        let ((ca0, ca1, a_nc), (_cb0, _cb1, b_nc)) = sched.chunk_ranges(&p).expect("chunk ranges");
        // B5b: schedule_layout now uses the SELECTIVE (disjoint-chunk) opening —
        // only the chunks the scattered rows/cols touch, not the covering range.
        let kk = p.k as usize;
        let (a_chunks, _) =
            crate::blake3_tree::indexed_strips_chunk_set(&sched.a_indices, kk, a_nc * 1024);
        let (b_chunks, _) =
            crate::blake3_tree::indexed_strips_chunk_set(&sched.b_indices, kk, b_nc * 1024);
        let na = strip_opening_rows_set(&a_chunks, a_nc);
        let nb = strip_opening_rows_set(&b_chunks, b_nc);
        assert_eq!(layout.na, na);
        assert_eq!(layout.mh_end, na + nb);
        // The scattered schedule's selective count is strictly smaller than the
        // covering-range count (the production trace-size win).
        assert!(
            na < strip_opening_rows(ca0, ca1, a_nc),
            "selective na {na} should be < covering-range na {}",
            strip_opening_rows(ca0, ca1, a_nc)
        );

        let num_stripes = (p.k / p.noise_rank) as usize;
        let chunks = (p.noise_rank as usize).div_ceil(TILE_D);
        let expected_sweep = (sched.a_indices.len() / TILE_H)
            * (sched.b_indices.len() / TILE_H)
            * num_stripes
            * chunks;
        assert_eq!(layout.store_start - layout.sweep_start, expected_sweep);
        assert_eq!(layout.fold_end - layout.fold_start, num_stripes);

        let bp = bp0();
        let program = canonical_program_for_strip_schedule(&p, &sched, &bp, len)
            .expect("rectangular schedule canonical program");
        assert_eq!(program.height(), len);
    }

    #[test]
    fn explicit_strip_schedule_does_not_require_native_tile_grid() {
        let p = ZkParams {
            m: 128,
            k: 512,
            n: 125,
            noise_rank: 16,
            tile: 6,
            difficulty_bits: 0,
        };
        assert!(
            p.validate().is_err(),
            "native square tile grid must reject this shape"
        );
        let sched = StripIndexSchedule::from_indices(
            &p,
            vec![0, 1, 6, 7, 12, 13],
            vec![0, 1, 8, 9, 16, 17, 24, 25],
        )
        .expect("explicit Pearl schedule is independent of native tile grid");
        let bp = bp0();
        let program = canonical_program_for_strip_schedule(&p, &sched, &bp, 1 << 15)
            .expect("explicit canonical program accepts non-native grid");
        assert_eq!(program.height(), 1 << 15);
    }

    #[test]
    fn cr1_canonical_program_pad_rows_are_exact_padding_pack() {
        use p3_field::PrimeField64;
        use p3_matrix::Matrix;

        use crate::composite_full_air::PROGRAM_COLS;

        // is_class_canonical fence: CR.0–CR.5 ⇒ EVERY class.
        for c in [
            RowClass::Pad,
            RowClass::KeyPin,
            RowClass::JackpotHash,
            RowClass::StripOpenA,
            RowClass::StripOpenB,
            RowClass::Sweep,
            RowClass::Fold,
        ] {
            assert!(is_class_canonical(c), "{c:?} canonical by CR.5");
        }

        let p = p16();
        let len = 1 << 13;
        let prog = canonical_program(&p, &bp0(), len).expect("test params valid");
        assert_eq!(prog.height(), len);
        assert_eq!(prog.width(), PROGRAM_COLS.len(), "program width");

        let sched = row_schedule(&p, 0, 0, len);
        let w = PROGRAM_COLS.len();
        let mut saw_pad = false;
        for (r, &class) in sched.iter().enumerate() {
            if class != RowClass::Pad {
                continue;
            }
            saw_pad = true;
            // Pad row: all PROGRAM_COLS zero except STARK_ROW_IDX
            // (the final program column, the monotonic row counter).
            for c in 0..w - 1 {
                assert_eq!(
                    prog.values[r * w + c].as_canonical_u64(),
                    0,
                    "Pad row {r} col {c} must be 0"
                );
            }
            assert_eq!(
                prog.values[r * w + (w - 1)].as_canonical_u64(),
                r as u64,
                "Pad row {r} STARK_ROW_IDX must be row_idx"
            );
        }
        assert!(saw_pad, "P16 schedule has Pad rows");
    }

    #[test]
    fn cr2_canonical_program_keypin_rows_are_exact() {
        use p3_field::PrimeField64;

        use crate::chips::control::ControlChip;
        use crate::composite_full_air::PROGRAM_COLS;

        let p = p16();
        let len = 1 << 13;
        let l = schedule_layout(&p, 0, 0, len);
        let prog = canonical_program(&p, &bp0(), len).expect("test params valid");
        let w = PROGRAM_COLS.len();

        // Expected CONTROL_PREP for each key-pin row: exactly one
        // selector set (JOB_KEY idx 2 at mh_end+1, COMMITMENT_HASH
        // idx 3 at mh_end+2), mat_id=0, no fold/msg_pair.
        for (row, sel_idx) in [(l.mh_end + 1, 2usize), (l.mh_end + 2, 3usize)] {
            assert_eq!(l.class_of(row), RowClass::KeyPin);
            let mut sel = [false; NUM_SELECTORS];
            sel[sel_idx] = true;
            let want_cp = ControlChip::pack_control_prep_full(&sel, 0, false, 0, 0, 0, false);
            // PROGRAM_COLS[0] = CONTROL_PREP.
            assert_eq!(
                prog.values[row * w].as_canonical_u64(),
                want_cp,
                "key-pin row {row} CONTROL_PREP must pack only \
                 SELECTOR_COLS idx {sel_idx}"
            );
            // Every non-control program column except STARK_ROW_IDX is zero.
            for c in 1..w - 1 {
                assert_eq!(
                    prog.values[row * w + c].as_canonical_u64(),
                    0,
                    "key-pin row {row} col {c} must be 0"
                );
            }
            assert_eq!(
                prog.values[row * w + (w - 1)].as_canonical_u64(),
                row as u64,
                "key-pin row {row} STARK_ROW_IDX"
            );
        }
    }

    #[test]
    fn cr3_canonical_program_jackpot_block_is_exact() {
        use p3_field::PrimeField64;

        use crate::chips::control::ControlChip;
        use crate::composite_full_air::PROGRAM_COLS;

        let p = p16();
        let len = 1 << 13;
        let l = schedule_layout(&p, 0, 0, len);
        let prog = canonical_program(&p, &bp0(), len).expect("test params valid");
        let w = PROGRAM_COLS.len();
        let tw = jackpot_tweak_packed();
        assert_ne!(tw, 0, "jackpot tweak packs non-zero (flags=0x1B)");

        // All 8 rows [jpot_start, len) are JackpotHash.
        for j in 0..8 {
            let row = l.jpot_start + j;
            assert_eq!(l.class_of(row), RowClass::JackpotHash);
            let mut sel = [false; NUM_SELECTORS];
            if j == 0 {
                sel[8] = true; // IS_NEW_BLAKE
            }
            if j == 7 {
                sel[9] = true; // IS_LAST_ROUND
                sel[6] = true; // IS_HASH_JACKPOT
            }
            let want_cp = ControlChip::pack_control_prep_full(&sel, 0, false, 0, 0, 0, false);
            // PROGRAM_COLS: [0]=CONTROL_PREP, [1..9]=NOISE×8,
            // [9]=CV_OR_TWEAK_PREP, [10]=AB_ID, then A/B IDs,
            // final=STARK_ROW_IDX.
            assert_eq!(
                prog.values[row * w].as_canonical_u64(),
                want_cp,
                "jackpot row j={j} CONTROL_PREP"
            );
            for c in 1..9 {
                assert_eq!(
                    prog.values[row * w + c].as_canonical_u64(),
                    0,
                    "jackpot row j={j} NOISE_PACKED_PREP[{}] must be 0",
                    c - 1
                );
            }
            assert_eq!(
                prog.values[row * w + 9].as_canonical_u64(),
                tw,
                "jackpot row j={j} CV_OR_TWEAK_PREP == jackpot tweak"
            );
            assert_eq!(
                prog.values[row * w + 10].as_canonical_u64(),
                0,
                "jackpot row j={j} AB_ID_PREP must be 0"
            );
            assert_eq!(
                prog.values[row * w + (w - 1)].as_canonical_u64(),
                row as u64,
                "jackpot row j={j} STARK_ROW_IDX"
            );
        }
    }

    #[test]
    fn cr4a_strip_blocks_row_count_matches_strip_opening_rows() {
        // The params-pure block walker must reproduce CR.0a's
        // row count exactly (8 rows/block; 0 for auth siblings).
        for nc in [1usize, 2, 3, 5, 8, 13, 16, 21] {
            for c0 in 0..nc {
                for c1 in (c0 + 1)..=nc {
                    let blocks = strip_blocks(c0, c1, nc);
                    assert_eq!(
                        blocks.len() * 8,
                        strip_opening_rows(c0, c1, nc),
                        "nc={nc} [{c0},{c1}) block*8 != strip_opening_rows"
                    );
                    // Lone chunk ⇒ 16 single-chunk-root leaf blocks.
                    if nc == 1 {
                        assert_eq!(blocks.len(), 16);
                        assert!(blocks.iter().all(|b| matches!(
                            b,
                            StripBlock::Leaf {
                                single_chunk_root: true,
                                ..
                            }
                        )));
                    }
                    // Exactly one root block (the post-order last).
                    if nc > 1 {
                        let roots = blocks
                            .iter()
                            .filter(|b| {
                                matches!(b, StripBlock::Parent { is_root: true })
                                    || matches!(
                                        b,
                                        StripBlock::Leaf {
                                            single_chunk_root: true,
                                            ..
                                        }
                                    )
                            })
                            .count();
                        assert_eq!(roots, 1, "nc={nc} [{c0},{c1}) must have exactly one root");
                    }
                }
            }
        }
    }

    #[test]
    #[should_panic(expected = "16|r")]
    fn row_schedule_rejects_non_16r() {
        // TEST_SMALL-shaped: r=4, 16∤4 — out of params-pure scope.
        let p = ZkParams {
            m: 64,
            k: 64,
            n: 64,
            noise_rank: 4,
            tile: 8,
            difficulty_bits: 0,
        };
        let _ = row_schedule(&p, 0, 0, 1 << 13);
    }

    /// M3 (DoS audit): `canonical_program` validates structurally-bad
    /// `ZkParams` / out-of-range tile_i/j / degenerate trace_len AT
    /// ENTRY — turning what were deep cryptic `assert!` panics in
    /// `schedule_layout` / `tile_chunk_range` into a typed `Err`.
    /// Reachable only on broken chain-pin trust (CRIT-1) in
    /// production; this is defense-in-depth.
    #[test]
    fn m3_canonical_program_rejects_invalid_inputs_without_panic() {
        let good = p16();
        let bp = bp0();
        let len = 1 << 13;
        // Baseline: valid 16|r ZkParams succeeds.
        canonical_program(&good, &bp, len).expect("baseline must succeed");

        // (a) Structural — m == 0 (caught by ZkParams::validate).
        let mut p = good;
        p.m = 0;
        let r = canonical_program(&p, &bp, len);
        assert!(r.is_err(), "m=0 must yield Err");

        // (b) noise_rank invalid (not a power of two ≥ 2) — caught by
        //     ZkParams::validate.
        let mut p = good;
        p.noise_rank = 0;
        assert!(
            canonical_program(&p, &bp, len).is_err(),
            "r=0 must yield Err"
        );

        // (c) Canonical-specific: 16 ∤ noise_rank. r=8 is a valid
        //     power-of-two ≥ 2 that divides k=64 — so passes
        //     ZkParams::validate, but the canonical 16|r co-location
        //     entry must catch it.
        let mut p = good;
        p.noise_rank = 8;
        let r = canonical_program(&p, &bp, len);
        assert!(r.is_err(), "r=8 (16∤8) must yield Err");
        let msg = r.unwrap_err();
        assert!(msg.contains("16"), "Err message should mention 16|r: {msg}");

        // (d) tile_j past the col-tile grid.
        let bad_bp = BlockPublic { tile_j: 999, ..bp };
        assert!(
            canonical_program(&good, &bad_bp, len).is_err(),
            "tile_j out of grid must yield Err"
        );

        // (e) trace_len degenerate.
        assert!(
            canonical_program(&good, &bp, 4).is_err(),
            "trace_len=4 must yield Err"
        );
        assert!(
            canonical_program(&good, &bp, 100).is_err(),
            "non-power-of-two trace_len must yield Err"
        );
    }
}

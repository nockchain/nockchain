//! P-B.2.0 — off-circuit BLAKE3 keyed chunk-tree walker + strip
//! opening (the **canonical reference** for the Pearl §4.6
//! matrix commitment).
//!
//! `crates/ai-pow/src/commit.rs::matrix_commitment` defines
//! `HASH_A / HASH_B = blake3::Hasher::new_keyed(κ)
//! .update(pad_to_chunk_boundary(M)).finalize()` — i.e. the root
//! of **BLAKE3's internal keyed chunk-Merkle**. BLAKE3's tree is
//! **not** a naïve left-leaning pairwise reduction: a subtree of
//! `n > 1` chunks splits with the **largest power of two number
//! of chunks strictly less than `n`** on the left
//! ([`left_len`]). The in-circuit `CompositeTrace::place_matrix_hash`
//! uses an equivalent bottom-up pairwise/promote-odd reduction; the
//! regression tests below compare both implementations against real
//! `blake3::Hasher` for power-of-two and non-power-of-two chunk counts.
//!
//! This module is **pure / off-circuit** (no AIR, no trace). It
//! provides the true tree, the full-matrix root, and an
//! authenticated **strip opening** (recompute the committed root
//! from only a contiguous chunk range + the off-range sibling
//! subtree-roots) — the primitive P-B.2.2's
//! `place_matrix_strip_opening` will mirror in-circuit. Every
//! function is KAT'd bit-identical to `blake3::Hasher::new_keyed`
//! for arbitrary (incl. non-power-of-two) chunk counts.
//!
//! Faithful to `place_matrix_hash`'s primitive: leaf = 16 keyed
//! compressions over the 1024-B chunk (`F_CHUNK_START` on block
//! 0, `F_CHUNK_END` on block 15, counter = chunk index); parent =
//! one keyed `F_PARENT` compression of `left‖right` with `κ` as
//! the chaining input; `F_ROOT` only on the final (root)
//! compression; all `F_KEYED_HASH`.

use crate::chips::blake3::compress::{blake3_compress, Blake3Tweak};

/// BLAKE3 flag bits (mirrors `place_matrix_hash`).
const F_CHUNK_START: u32 = 1 << 0;
const F_CHUNK_END: u32 = 1 << 1;
const F_PARENT: u32 = 1 << 2;
const F_ROOT: u32 = 1 << 3;
const F_KEYED_HASH: u32 = 1 << 4;

/// BLAKE3 chunk length in bytes (= one Merkle leaf).
pub const CHUNK_LEN: usize = 1024;
const BLOCK_LEN: usize = 64;
const BLOCKS_PER_CHUNK: usize = CHUNK_LEN / BLOCK_LEN; // 16

/// Zero-pad to a multiple of [`CHUNK_LEN`], min one chunk —
/// byte-identical to `ai-pow::commit::pad_to_chunk_boundary`
/// composed with `place_matrix_hash`'s `.max(CHUNK_LEN)`.
pub fn pad_to_chunk_boundary(data: &[u8]) -> Vec<u8> {
    let pad_to = data.len().div_ceil(CHUNK_LEN) * CHUNK_LEN;
    let mut v = data.to_vec();
    v.resize(pad_to.max(CHUNK_LEN), 0);
    v
}

/// `left_len(n)` — BLAKE3's split: the largest power of two
/// strictly less than `n` (number of chunks). `n >= 2`.
///
/// `n=2→1, 3→2, 4→2, 5→4, 6→4, 7→4, 8→4, 9→8, 16→8, 17→16`.
pub fn left_len(n: u64) -> u64 {
    debug_assert!(n >= 2);
    let mut l = 1u64;
    while (l << 1) < n {
        l <<= 1;
    }
    l
}

/// Keyed BLAKE3 **chunk** chaining value of one `CHUNK_LEN`-byte
/// chunk at `chunk_index`. `is_single_chunk_root` sets `F_ROOT`
/// on the last block (the lone-chunk case, where the chunk *is*
/// the tree root). Replicates `place_matrix_hash`'s chunk layer.
pub fn chunk_cv(
    chunk: &[u8; CHUNK_LEN],
    chunk_index: u64,
    kappa: &[u8; 32],
    is_single_chunk_root: bool,
) -> [u8; 32] {
    let mut cv = *kappa;
    for b in 0..BLOCKS_PER_CHUNK {
        let mut block = [0u8; BLOCK_LEN];
        block.copy_from_slice(&chunk[b * BLOCK_LEN..(b + 1) * BLOCK_LEN]);
        let mut flags = F_KEYED_HASH;
        if b == 0 {
            flags |= F_CHUNK_START;
        }
        if b == BLOCKS_PER_CHUNK - 1 {
            flags |= F_CHUNK_END;
            if is_single_chunk_root {
                flags |= F_ROOT;
            }
        }
        let tweak = Blake3Tweak {
            counter_low: chunk_index as u32,
            counter_high: (chunk_index >> 32) as u16,
            block_len: BLOCK_LEN as u32,
            flags,
        };
        cv = blake3_compress(&block, &cv, tweak);
    }
    cv
}

/// Keyed BLAKE3 **parent** compression of `left‖right`.
pub fn parent_cv(left: &[u8; 32], right: &[u8; 32], kappa: &[u8; 32], is_root: bool) -> [u8; 32] {
    let mut msg = [0u8; BLOCK_LEN];
    msg[..32].copy_from_slice(left);
    msg[32..].copy_from_slice(right);
    let mut flags = F_KEYED_HASH | F_PARENT;
    if is_root {
        flags |= F_ROOT;
    }
    let tweak = Blake3Tweak {
        counter_low: 0,
        counter_high: 0,
        block_len: BLOCK_LEN as u32,
        flags,
    };
    blake3_compress(&msg, kappa, tweak)
}

/// All per-chunk leaf CVs of `padded` (length a multiple of
/// `CHUNK_LEN`). For a single chunk the `F_ROOT` is **not** set
/// here (callers handle the lone-chunk root via [`merkle_root`]).
fn leaf_cvs(padded: &[u8], kappa: &[u8; 32]) -> Vec<[u8; 32]> {
    let n = padded.len() / CHUNK_LEN;
    (0..n)
        .map(|c| {
            let mut chunk = [0u8; CHUNK_LEN];
            chunk.copy_from_slice(&padded[c * CHUNK_LEN..(c + 1) * CHUNK_LEN]);
            chunk_cv(&chunk, c as u64, kappa, false)
        })
        .collect()
}

/// Root CV of the BLAKE3 chunk-subtree covering `leaf_cvs[lo..hi)`
/// (the true largest-pow2-left tree). `is_root` ⇒ `F_ROOT` on the
/// node's parent compression.
fn subtree_root(
    cvs: &[[u8; 32]],
    lo: usize,
    hi: usize,
    kappa: &[u8; 32],
    is_root: bool,
) -> [u8; 32] {
    debug_assert!(hi > lo);
    if hi - lo == 1 {
        return cvs[lo];
    }
    let mid = lo + left_len((hi - lo) as u64) as usize;
    let l = subtree_root(cvs, lo, mid, kappa, false);
    let r = subtree_root(cvs, mid, hi, kappa, false);
    parent_cv(&l, &r, kappa, is_root)
}

/// The Pearl §4.6 / C3 commitment of `matrix_bytes` under key
/// `kappa` — **bit-identical to**
/// `blake3::Hasher::new_keyed(kappa).update(pad(matrix_bytes))
/// .finalize()` (and to `ai-pow::commit::matrix_commitment`).
pub fn merkle_root(matrix_bytes: &[u8], kappa: &[u8; 32]) -> [u8; 32] {
    let padded = pad_to_chunk_boundary(matrix_bytes);
    let n = padded.len() / CHUNK_LEN;
    if n == 1 {
        let mut chunk = [0u8; CHUNK_LEN];
        chunk.copy_from_slice(&padded[..CHUNK_LEN]);
        return chunk_cv(&chunk, 0, kappa, true);
    }
    let cvs = leaf_cvs(&padded, kappa);
    subtree_root(&cvs, 0, n, kappa, true)
}

/// **P-B.2.3 — verifier-fixed opening schedule.** The
/// contiguous BLAKE3 chunk range `[c0, c1)` (and total
/// `num_chunks`) covering tile `tile_idx`'s `t` strips of a
/// `total_bytes`-byte row/col-major matrix, each strip `k`
/// bytes. **Pure deterministic function of public params**
/// (`tile_idx` is attested via §4.E/MED-3) ⇒ the verifier
/// recomputes it; the prover has no freedom to open a different
/// (cheaper) region (the strip-opening block's pinned
/// `CONTROL_PREP` layout is derived from this — CRIT-1/MED-3
/// discipline, D3-A, no new pinned column).
///
/// Whole-chunk cover (D2-A): a tile not aligned to the 1024-B
/// chunk grid pulls in the boundary chunks' adjacent bytes
/// (witness-only — they are still genuine committed bytes; the
/// HASH_A binding is unaffected; per-row sweep binding is
/// §4.C.2). `lo = tile_idx·t·k`, `hi = lo + t·k`:
/// `c0 = ⌊lo/1024⌋`, `c1 = ⌈hi/1024⌉`.
pub fn tile_chunk_range(
    tile_idx: usize,
    t: usize,
    k: usize,
    total_bytes: usize,
) -> (usize, usize, usize) {
    let lo = tile_idx * t * k;
    let hi = lo + t * k;
    assert!(
        hi <= total_bytes,
        "tile {tile_idx} strip span [{lo},{hi}) exceeds matrix {total_bytes}B"
    );
    let padded = (total_bytes.div_ceil(CHUNK_LEN) * CHUNK_LEN).max(CHUNK_LEN);
    let num_chunks = padded / CHUNK_LEN;
    let c0 = lo / CHUNK_LEN;
    let c1 = hi.div_ceil(CHUNK_LEN).min(num_chunks).max(c0 + 1);
    (c0, c1, num_chunks)
}

/// Pattern-aware variant of [`tile_chunk_range`].
///
/// Given explicit row/column indices into a row-major or column-major matrix
/// whose selected strips are each `k` bytes, return the minimal single
/// contiguous BLAKE3 chunk range that covers every selected strip. This is the
/// low-level schedule primitive needed for Pearl `PeriodicPattern` tickets:
/// non-contiguous Pearl row/column sets can still be authenticated to the one
/// committed matrix root by opening the chunk-covering interval.
///
/// This deliberately returns one contiguous range because
/// `place_matrix_strip_opening` proves one strip-opening root today. A future
/// multi-range opening can optimize this, but the single-range cover is already
/// verifier-recomputable and sound: it may reveal extra committed bytes, never
/// fewer than the selected ticket uses.
pub fn indexed_strips_chunk_range(
    indices: &[u32],
    k: usize,
    total_bytes: usize,
) -> (usize, usize, usize) {
    assert!(
        !indices.is_empty(),
        "indexed strip opening requires at least one index"
    );
    assert!(k > 0, "strip length k must be nonzero");
    let min_idx = indices
        .iter()
        .copied()
        .min()
        .expect("nonempty indices have min") as usize;
    let max_idx = indices
        .iter()
        .copied()
        .max()
        .expect("nonempty indices have max") as usize;
    let lo = min_idx
        .checked_mul(k)
        .expect("indexed strip opening lo overflow");
    let hi = max_idx
        .checked_add(1)
        .and_then(|idx| idx.checked_mul(k))
        .expect("indexed strip opening hi overflow");
    assert!(
        hi <= total_bytes,
        "indexed strip span [{lo},{hi}) exceeds matrix {total_bytes}B"
    );
    let padded = (total_bytes.div_ceil(CHUNK_LEN) * CHUNK_LEN).max(CHUNK_LEN);
    let num_chunks = padded / CHUNK_LEN;
    let c0 = lo / CHUNK_LEN;
    let c1 = hi.div_ceil(CHUNK_LEN).min(num_chunks).max(c0 + 1);
    (c0, c1, num_chunks)
}

/// The sorted, distinct SET of chunks covering the opened rows/columns `indices`
/// (each spanning `k` bytes). Selective-opening analogue of
/// [`indexed_strips_chunk_range`]: for **contiguous** `indices` it equals the
/// full range `c0..c1`, but for **scattered** `indices` (MoE routed tokens) it is
/// only the O(|indices|·⌈k/1024⌉) chunks actually touched — not the O(max−min)
/// covering range — which keeps the Layer-0 trace inside `PEARL_TRACE_BOUND` at
/// production scale. `chunks[0]` equals the range `c0` (the producer-key base).
pub fn indexed_strips_chunk_set(
    indices: &[u32],
    k: usize,
    total_bytes: usize,
) -> (Vec<usize>, usize) {
    assert!(
        !indices.is_empty(),
        "indexed strip opening requires >= 1 index"
    );
    assert!(k > 0, "strip length k must be nonzero");
    let padded = (total_bytes.div_ceil(CHUNK_LEN) * CHUNK_LEN).max(CHUNK_LEN);
    let num_chunks = padded / CHUNK_LEN;
    let mut chunks: Vec<usize> = Vec::new();
    for &idx in indices {
        let lo = (idx as usize)
            .checked_mul(k)
            .expect("indexed strip lo overflow");
        let hi = lo + k;
        assert!(
            hi <= total_bytes,
            "indexed strip [{lo},{hi}) exceeds {total_bytes}B"
        );
        let c_lo = lo / CHUNK_LEN;
        let c_hi = hi.div_ceil(CHUNK_LEN).min(num_chunks).max(c_lo + 1);
        for c in c_lo..c_hi {
            chunks.push(c);
        }
    }
    chunks.sort_unstable();
    chunks.dedup();
    (chunks, num_chunks)
}

/// **Phase A-CR (CR.0).** The number of trace rows
/// [`crate::composite_trace::CompositeTrace::place_matrix_strip_opening`]
/// consumes for the opening of `[c0,c1)` within `num_chunks`
/// total chunks — a **pure function of `(c0,c1,num_chunks)`**
/// (verifier-fixed from `(params, tile_i/j)` via
/// [`tile_chunk_range`]; no witness). It mirrors `place_matrix_
/// strip_opening`'s fold *exactly*: 8 rows per BLAKE3
/// compression; a fully-inside leaf chunk = 16 compressions; a
/// fully-outside subtree = an auth sibling (0 rows); a straddling
/// node = recurse L+R then +1 parent compression; the lone-chunk
/// case = 16 compressions. This is the params-pure row-count the
/// CR.0 schedule (and `canonical_program`) needs for the
/// strip-opening A/B regions. KAT'd `== place_matrix_strip_
/// opening`'s actual row advance.
pub fn strip_opening_rows(c0: usize, c1: usize, num_chunks: usize) -> usize {
    assert!(
        c0 < c1 && c1 <= num_chunks,
        "range [{c0},{c1}) out of 0..{num_chunks}"
    );
    // Fully-recomputed subtree [lo,hi) ⊆ [c0,c1): leaves = 16
    // compressions each; internal node = +1 parent compression.
    fn inside(lo: usize, hi: usize) -> usize {
        if hi - lo == 1 {
            return 16;
        }
        let mid = lo + left_len((hi - lo) as u64) as usize;
        inside(lo, mid) + inside(mid, hi) + 1
    }
    // The fold over the full tree [lo,hi) opening [c0,c1).
    fn fold(lo: usize, hi: usize, c0: usize, c1: usize) -> usize {
        if hi <= c0 || lo >= c1 {
            return 0; // auth sibling — no rows
        }
        if c0 <= lo && hi <= c1 {
            return inside(lo, hi);
        }
        let mid = lo + left_len((hi - lo) as u64) as usize;
        fold(lo, mid, c0, c1) + fold(mid, hi, c0, c1) + 1
    }
    let compressions = if num_chunks == 1 {
        16
    } else {
        fold(0, num_chunks, c0, c1)
    };
    compressions * 8
}

/// One off-range sibling on a strip's authentication path: the
/// subtree-root CV of a chunk range disjoint from the opening,
/// in the deterministic post-order the [`verify_strip_opening`]
/// fold consumes it (a contiguous opening + contiguous BLAKE3
/// subtrees ⇒ every node is fully-in, fully-out, or straddling).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AuthSibling {
    /// Inclusive-exclusive chunk range this sibling subtree covers.
    pub lo: usize,
    pub hi: usize,
    /// The committed subtree-root CV (prover-supplied in-circuit).
    pub cv: [u8; 32],
}

/// Honest prover side: given the full matrix and the opened
/// chunk range `[c0, c1)`, return (the opened leaf CVs, the
/// ordered off-range authentication siblings). The verifier /
/// circuit recomputes the opened leaf CVs from the revealed
/// strip bytes and folds with these siblings — see
/// [`verify_strip_opening`].
pub fn open_strip(
    matrix_bytes: &[u8],
    kappa: &[u8; 32],
    c0: usize,
    c1: usize,
) -> (Vec<[u8; 32]>, Vec<AuthSibling>) {
    let padded = pad_to_chunk_boundary(matrix_bytes);
    let n = padded.len() / CHUNK_LEN;
    assert!(c0 < c1 && c1 <= n, "range [{c0},{c1}) out of 0..{n}");
    let cvs = leaf_cvs(&padded, kappa);
    let opened = cvs[c0..c1].to_vec();
    let mut sibs = Vec::new();
    collect_siblings(&cvs, 0, n, c0, c1, kappa, &mut sibs);
    (opened, sibs)
}

/// Post-order walk mirroring [`fold_opening`]: record every
/// maximal subtree fully **outside** `[c0,c1)` as one sibling.
fn collect_siblings(
    cvs: &[[u8; 32]],
    lo: usize,
    hi: usize,
    c0: usize,
    c1: usize,
    kappa: &[u8; 32],
    out: &mut Vec<AuthSibling>,
) {
    if hi <= c0 || lo >= c1 {
        // Fully outside the opening ⇒ one authentication sibling.
        out.push(AuthSibling {
            lo,
            hi,
            cv: subtree_root(cvs, lo, hi, kappa, false),
        });
        return;
    }
    if c0 <= lo && hi <= c1 {
        return; // fully inside ⇒ recomputed from opened leaves
    }
    let mid = lo + left_len((hi - lo) as u64) as usize;
    collect_siblings(cvs, lo, mid, c0, c1, kappa, out);
    collect_siblings(cvs, mid, hi, c0, c1, kappa, out);
}

/// Verifier / circuit side: recompute the committed root from
/// the opened leaf CVs (`opened` = CVs of chunks `[c0,c1)`,
/// recomputed in-circuit from the revealed strip bytes) and the
/// ordered off-range `siblings`. Returns the recomputed root;
/// the caller asserts it `== PI_HASH_A/B`. Pure structural fold
/// — no full matrix needed.
pub fn verify_strip_opening(
    opened: &[[u8; 32]],
    siblings: &[AuthSibling],
    c0: usize,
    c1: usize,
    num_chunks: usize,
    kappa: &[u8; 32],
) -> [u8; 32] {
    assert_eq!(opened.len(), c1 - c0, "opened count != range width");
    if num_chunks == 1 {
        // Lone chunk: it IS the root; opened[0] must have been
        // computed with the single-chunk-root flag by the caller.
        assert!(c0 == 0 && c1 == 1 && siblings.is_empty());
        return opened[0];
    }
    let mut sib = siblings.iter();
    let root = fold_opening(0, num_chunks, c0, c1, opened, &mut sib, kappa, true);
    assert!(sib.next().is_none(), "unconsumed authentication siblings");
    root
}

#[allow(clippy::too_many_arguments)]
fn fold_opening<'a, I: Iterator<Item = &'a AuthSibling>>(
    lo: usize,
    hi: usize,
    c0: usize,
    c1: usize,
    opened: &[[u8; 32]],
    sibs: &mut I,
    kappa: &[u8; 32],
    is_root: bool,
) -> [u8; 32] {
    if hi <= c0 || lo >= c1 {
        let s = sibs.next().expect("missing authentication sibling");
        assert!(s.lo == lo && s.hi == hi, "sibling range mismatch");
        return s.cv;
    }
    if c0 <= lo && hi <= c1 {
        // Fully inside: recompute from the opened leaf CVs (the
        // true sub-tree over this range).
        return subtree_from_opened(lo, hi, c0, opened, kappa, is_root);
    }
    let mid = lo + left_len((hi - lo) as u64) as usize;
    let l = fold_opening(lo, mid, c0, c1, opened, sibs, kappa, false);
    let r = fold_opening(mid, hi, c0, c1, opened, sibs, kappa, false);
    parent_cv(&l, &r, kappa, is_root)
}

/// Subtree root over a fully-opened range, taking leaf CVs from
/// `opened` (indexed by `chunk_index - c0`).
fn subtree_from_opened(
    lo: usize,
    hi: usize,
    c0: usize,
    opened: &[[u8; 32]],
    kappa: &[u8; 32],
    is_root: bool,
) -> [u8; 32] {
    if hi - lo == 1 {
        return opened[lo - c0];
    }
    let mid = lo + left_len((hi - lo) as u64) as usize;
    let l = subtree_from_opened(lo, mid, c0, opened, kappa, false);
    let r = subtree_from_opened(mid, hi, c0, opened, kappa, false);
    parent_cv(&l, &r, kappa, is_root)
}

// ─────────────────────────────────────────────────────────────────────────
// Selective (disjoint-chunk) opening — the BLAKE3 keyed-tree MULTI-PROOF.
//
// [`open_strip`] opens a *contiguous* chunk range `[c0,c1)`; its recomputation
// cost is O(c1−c0). Production MoE opens an expert's routed-token rows, which
// scatter across ~m rows, so the covering range `[min,max)` is O(m) and breaches
// `PEARL_TRACE_BOUND` at scale. A *selective* opening authenticates an arbitrary
// SORTED SET of chunks in O(|set|·log n) recomputation instead. This is the
// off-circuit reference / de-risk foundation; the in-circuit selective schedule
// (`canonical.rs` strip-block generation + the AIR) is a separate staged change.
// ─────────────────────────────────────────────────────────────────────────

/// Number of selected chunks (from the sorted, distinct `sel`) that lie in
/// `[lo,hi)`.
fn sel_count(sel: &[usize], lo: usize, hi: usize) -> usize {
    sel.partition_point(|&c| c < hi) - sel.partition_point(|&c| c < lo)
}

/// Selective strip opening: authenticate the sorted, distinct chunk set `sel` to
/// the committed root. Returns the opened leaf CVs (one per `sel` entry, in
/// order) and the off-set authentication siblings (maximal subtrees disjoint from
/// `sel`). Reconstruct + check with [`verify_strip_opening_set`].
pub fn open_strip_set(
    matrix_bytes: &[u8],
    kappa: &[u8; 32],
    sel: &[usize],
) -> (Vec<[u8; 32]>, Vec<AuthSibling>) {
    let padded = pad_to_chunk_boundary(matrix_bytes);
    let n = padded.len() / CHUNK_LEN;
    assert!(!sel.is_empty(), "selective opening requires >= 1 chunk");
    assert!(
        sel.windows(2).all(|w| w[0] < w[1]),
        "selective opening chunks must be sorted + distinct"
    );
    assert!(*sel.last().unwrap() < n, "selective chunk out of 0..{n}");
    let cvs = leaf_cvs(&padded, kappa);
    let opened: Vec<[u8; 32]> = sel.iter().map(|&c| cvs[c]).collect();
    let mut sibs = Vec::new();
    collect_siblings_set(&cvs, 0, n, sel, kappa, &mut sibs);
    (opened, sibs)
}

fn collect_siblings_set(
    cvs: &[[u8; 32]],
    lo: usize,
    hi: usize,
    sel: &[usize],
    kappa: &[u8; 32],
    out: &mut Vec<AuthSibling>,
) {
    let cnt = sel_count(sel, lo, hi);
    if cnt == 0 {
        // Fully disjoint from the opening ⇒ one authentication sibling.
        out.push(AuthSibling {
            lo,
            hi,
            cv: subtree_root(cvs, lo, hi, kappa, false),
        });
        return;
    }
    if cnt == hi - lo {
        return; // fully selected ⇒ recomputed from opened leaves
    }
    let mid = lo + left_len((hi - lo) as u64) as usize;
    collect_siblings_set(cvs, lo, mid, sel, kappa, out);
    collect_siblings_set(cvs, mid, hi, sel, kappa, out);
}

/// Recompute the committed root from a selective opening; the caller asserts it
/// `== PI_HASH_A/B`. Pure structural fold — no full matrix needed.
pub fn verify_strip_opening_set(
    opened: &[[u8; 32]],
    siblings: &[AuthSibling],
    sel: &[usize],
    num_chunks: usize,
    kappa: &[u8; 32],
) -> [u8; 32] {
    assert_eq!(opened.len(), sel.len(), "opened count != selected count");
    if num_chunks == 1 {
        assert!(sel == [0] && siblings.is_empty());
        return opened[0];
    }
    let mut sib = siblings.iter();
    let root = fold_opening_set(0, num_chunks, sel, opened, &mut sib, kappa, true);
    assert!(sib.next().is_none(), "unconsumed authentication siblings");
    root
}

#[allow(clippy::too_many_arguments)]
fn fold_opening_set<'a, I: Iterator<Item = &'a AuthSibling>>(
    lo: usize,
    hi: usize,
    sel: &[usize],
    opened: &[[u8; 32]],
    sibs: &mut I,
    kappa: &[u8; 32],
    is_root: bool,
) -> [u8; 32] {
    let cnt = sel_count(sel, lo, hi);
    if cnt == 0 {
        let s = sibs.next().expect("missing authentication sibling");
        assert!(s.lo == lo && s.hi == hi, "sibling range mismatch");
        return s.cv;
    }
    if cnt == hi - lo {
        return subtree_from_opened_set(lo, hi, sel, opened, kappa, is_root);
    }
    let mid = lo + left_len((hi - lo) as u64) as usize;
    let l = fold_opening_set(lo, mid, sel, opened, sibs, kappa, false);
    let r = fold_opening_set(mid, hi, sel, opened, sibs, kappa, false);
    parent_cv(&l, &r, kappa, is_root)
}

fn subtree_from_opened_set(
    lo: usize,
    hi: usize,
    sel: &[usize],
    opened: &[[u8; 32]],
    kappa: &[u8; 32],
    is_root: bool,
) -> [u8; 32] {
    if hi - lo == 1 {
        // [lo,hi) fully selected ⇒ lo ∈ sel; its opened CV is at rank(lo).
        return opened[sel.partition_point(|&x| x < lo)];
    }
    let mid = lo + left_len((hi - lo) as u64) as usize;
    let l = subtree_from_opened_set(lo, mid, sel, opened, kappa, false);
    let r = subtree_from_opened_set(mid, hi, sel, opened, kappa, false);
    parent_cv(&l, &r, kappa, is_root)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kappa() -> [u8; 32] {
        core::array::from_fn(|i| (i as u8).wrapping_mul(37) ^ 0xA5)
    }

    fn bytes(n: usize) -> Vec<u8> {
        (0..n)
            .map(|i| ((i.wrapping_mul(2654435761)) ^ (i >> 5)) as u8)
            .collect()
    }

    #[test]
    fn left_len_blake3_split_values() {
        for (n, want) in [
            (2u64, 1u64),
            (3, 2),
            (4, 2),
            (5, 4),
            (6, 4),
            (7, 4),
            (8, 4),
            (9, 8),
            (16, 8),
            (17, 16),
            (1024, 512),
            (1025, 1024),
        ] {
            assert_eq!(left_len(n), want, "left_len({n})");
        }
    }

    /// **The P-B.2.0 honest-equivalence KAT.** The true-tree
    /// walker root is bit-identical to `blake3::Hasher::new_keyed`
    /// for *arbitrary* chunk counts — power-of-two **and**
    /// non-power-of-two (the GEMMA/QWEN-shaped case the in-circuit
    /// pairwise loop gets wrong; D1-A).
    #[test]
    fn merkle_root_matches_blake3_keyed_all_chunk_counts() {
        let k = kappa();
        // raw byte lengths chosen to land on these chunk counts:
        // 1..=17, 31, 32, 33, 100, 1000 — pow2 and very much not.
        let chunk_counts =
            [1usize, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 13, 16, 17, 31, 32, 33, 100, 1000];
        for &nc in &chunk_counts {
            // Use a non-chunk-multiple raw length so padding is
            // exercised too (last chunk partially zero-padded).
            let raw = bytes(nc * CHUNK_LEN - 257);
            let got = merkle_root(&raw, &k);
            let want: [u8; 32] = *blake3::Hasher::new_keyed(&k)
                .update(&pad_to_chunk_boundary(&raw))
                .finalize()
                .as_bytes();
            assert_eq!(
                got, want,
                "walker root != blake3::Hasher for {nc} chunks (non-pow2-correct)"
            );
        }
        // Empty input ⇒ one zero chunk.
        assert_eq!(
            merkle_root(&[], &k),
            *blake3::Hasher::new_keyed(&k)
                .update(&[0u8; CHUNK_LEN])
                .finalize()
                .as_bytes()
        );
    }

    /// **Real shipped-model scale.** `Llama-3.1-8B-Instruct-pearl`
    /// `up_proj`/`down_proj` weight = `4096·14336 = 58 720 256`
    /// int8 bytes ⇒ **57 344 BLAKE3 chunks** (= 0b1110…0, very
    /// much *not* a power of two — the actual production
    /// non-pow2 count, vs the ≤1000 swept above). The true-tree
    /// walker is still bit-identical to real `blake3::Hasher` at
    /// this scale ⇒ the structural identity (and hence
    /// `place_matrix_hash`'s correctness) holds for the genuine
    /// vLLM-plugin workload, not just toy sizes.
    #[test]
    fn walker_matches_blake3_at_llama_3_1_8b_weight_scale() {
        let k = kappa();
        let nc = (4096usize * 14336) / CHUNK_LEN; // 57_344
        assert_eq!(nc, 57_344);
        assert!(!nc.is_power_of_two());
        let raw = bytes(nc * CHUNK_LEN);
        let want: [u8; 32] = *blake3::Hasher::new_keyed(&k)
            .update(&raw) // already a chunk multiple
            .finalize()
            .as_bytes();
        assert_eq!(
            merkle_root(&raw, &k),
            want,
            "walker != blake3::Hasher at the real Llama-3.1-8B \
             weight scale (57 344 chunks)"
        );
    }

    /// The authenticated strip opening recomputes exactly the
    /// committed root for every contiguous chunk range — incl.
    /// boundary-straddling ranges and non-power-of-two trees
    /// (the §4.6 / P-B.2.2 invariant).
    #[test]
    fn strip_opening_recomputes_committed_root() {
        let k = kappa();
        for &nc in &[1usize, 2, 3, 5, 8, 13, 17, 31, 64, 100] {
            let raw = bytes(nc * CHUNK_LEN);
            let root = merkle_root(&raw, &k);
            for c0 in 0..nc {
                for c1 in (c0 + 1)..=nc {
                    if nc == 1 {
                        // lone-chunk root: opened[0] must carry the
                        // single-chunk-root flag.
                        let mut chunk = [0u8; CHUNK_LEN];
                        chunk.copy_from_slice(&pad_to_chunk_boundary(&raw));
                        let opened = [chunk_cv(&chunk, 0, &k, true)];
                        assert_eq!(verify_strip_opening(&opened, &[], 0, 1, 1, &k), root);
                        continue;
                    }
                    let (opened, sibs) = open_strip(&raw, &k, c0, c1);
                    let got = verify_strip_opening(&opened, &sibs, c0, c1, nc, &k);
                    assert_eq!(got, root, "open [{c0},{c1}) of {nc} chunks != root");
                }
            }
        }
    }

    /// Selective (disjoint-chunk) multi-proof authenticates to the committed root
    /// for singletons, endpoints, every-other, scattered handfuls, and the full
    /// set, across power- and non-power-of-two chunk counts.
    #[test]
    fn selective_opening_recomputes_committed_root() {
        let k = kappa();
        for &nc in &[2usize, 3, 5, 8, 13, 17, 31, 64, 100] {
            let raw = bytes(nc * CHUNK_LEN);
            let root = merkle_root(&raw, &k);
            let mut sets: Vec<Vec<usize>> = vec![
                vec![0],
                vec![nc / 2],
                vec![nc - 1],
                vec![0, nc - 1],
                (0..nc).step_by(2).collect(),
                (0..nc).step_by(3).collect(),
                (0..nc).collect(),
            ];
            if nc >= 4 {
                sets.push(vec![0, 1, nc - 2, nc - 1]);
                sets.push(vec![1, nc / 2, nc - 1]);
            }
            for sel in &sets {
                let (opened, sibs) = open_strip_set(&raw, &k, sel);
                let got = verify_strip_opening_set(&opened, &sibs, sel, nc, &k);
                assert_eq!(got, root, "selective open {sel:?} of {nc} chunks != root");
            }
        }
    }

    /// For a contiguous selected set `[c0,c1)` the multi-proof is byte-identical to
    /// the range opening — the set path strictly generalizes the range path.
    #[test]
    fn selective_matches_contiguous_range_opening() {
        let k = kappa();
        for &nc in &[2usize, 5, 8, 13, 31, 64] {
            let raw = bytes(nc * CHUNK_LEN);
            for c0 in 0..nc {
                for c1 in (c0 + 1)..=nc {
                    let sel: Vec<usize> = (c0..c1).collect();
                    let (o_set, s_set) = open_strip_set(&raw, &k, &sel);
                    let (o_rng, s_rng) = open_strip(&raw, &k, c0, c1);
                    assert_eq!(o_set, o_rng, "opened differ for [{c0},{c1}) of {nc}");
                    let key = |s: &[AuthSibling]| {
                        s.iter().map(|x| (x.lo, x.hi, x.cv)).collect::<Vec<_>>()
                    };
                    assert_eq!(
                        key(&s_set),
                        key(&s_rng),
                        "siblings differ for [{c0},{c1}) of {nc}"
                    );
                }
            }
        }
    }

    /// The production win: a scattered set of `h` chunks in a large tree costs
    /// O(h·log n) opened+siblings, NOT the O(max−min) covering range that
    /// [`open_strip`] (and the current in-circuit schedule) would pay.
    #[test]
    fn selective_opening_multiproof_is_sublinear() {
        let k = kappa();
        let nc = 4096usize; // e.g. m=4096 tokens (one chunk per row)
        let raw = bytes(nc * CHUNK_LEN);
        // 64 tokens spread evenly across all of [0,4096) — covering range 3970.
        let sel: Vec<usize> = (0..64).map(|i| i * 63).collect();
        let (opened, sibs) = open_strip_set(&raw, &k, &sel);
        assert_eq!(
            verify_strip_opening_set(&opened, &sibs, &sel, nc, &k),
            merkle_root(&raw, &k)
        );
        let logn = (usize::BITS - (nc - 1).leading_zeros()) as usize; // ceil(log2 nc)
        assert_eq!(opened.len(), sel.len());
        assert!(
            sibs.len() <= sel.len() * logn,
            "multi-proof siblings {} exceed |sel|·log2(nc)={}",
            sibs.len(),
            sel.len() * logn
        );
        let covering = sel.last().unwrap() - sel.first().unwrap() + 1;
        assert!(
            opened.len() + sibs.len() < covering,
            "selective proof ({}+{}) not smaller than covering range {}",
            opened.len(),
            sibs.len(),
            covering
        );
    }

    /// Tampering an opened leaf CV or an authentication sibling breaks the root.
    #[test]
    fn selective_opening_rejects_tampering() {
        let k = kappa();
        let nc = 64usize;
        let raw = bytes(nc * CHUNK_LEN);
        let sel = vec![0usize, 7, 8, 31, 63];
        let root = merkle_root(&raw, &k);
        let (mut opened, sibs) = open_strip_set(&raw, &k, &sel);
        assert_eq!(verify_strip_opening_set(&opened, &sibs, &sel, nc, &k), root);
        opened[2][0] ^= 1;
        assert_ne!(verify_strip_opening_set(&opened, &sibs, &sel, nc, &k), root);
        let (opened2, mut sibs2) = open_strip_set(&raw, &k, &sel);
        sibs2[0].cv[0] ^= 1;
        assert_ne!(
            verify_strip_opening_set(&opened2, &sibs2, &sel, nc, &k),
            root
        );
    }

    /// **§C adversarial audit (k > 1024).** At Llama scale a single strip (row)
    /// spans `k/1024` chunks, so `indexed_strips_chunk_set` must expand each
    /// scattered row to its full run of chunks; the resulting disjoint set must
    /// authenticate to the committed root and reject tampering. The k=1024
    /// fixtures (1 chunk/row) never exercise this multi-chunk-per-row expansion,
    /// which is exactly the production case (Pearl k = 4096 / 14336 / 28672).
    #[test]
    fn indexed_strips_chunk_set_authenticates_k_gt_1024_noncontiguous() {
        let k = kappa();
        for &kk in &[2048usize, 4096, 14336] {
            let chunks_per_row = kk / CHUNK_LEN;
            let total_rows = 128usize;
            let raw = bytes(total_rows * kk);
            let root = merkle_root(&raw, &k);
            let rows = [0u32, 5, 63, 127]; // scattered, far apart → disjoint runs
            let (chunk_set, nc) = indexed_strips_chunk_set(&rows, kk, raw.len());

            // Each row expands to exactly its contiguous run of `chunks_per_row`.
            assert_eq!(
                chunk_set.len(),
                rows.len() * chunks_per_row,
                "kk={kk}: expected {} chunks",
                rows.len() * chunks_per_row
            );
            for &r in &rows {
                for c in 0..chunks_per_row {
                    let want = (r as usize) * chunks_per_row + c;
                    assert!(
                        chunk_set.contains(&want),
                        "kk={kk}: row {r} missing spanning chunk {want}"
                    );
                }
            }
            // The disjoint multi-chunk-per-row set authenticates to the root.
            let (opened, sibs) = open_strip_set(&raw, &k, &chunk_set);
            assert_eq!(
                verify_strip_opening_set(&opened, &sibs, &chunk_set, nc, &k),
                root,
                "kk={kk}: k>1024 non-contiguous set must authenticate"
            );
            // Tamper a byte in row 5's first spanning chunk → root diverges.
            let (mut o2, s2) = open_strip_set(&raw, &k, &chunk_set);
            o2[chunks_per_row][0] ^= 1;
            assert_ne!(
                verify_strip_opening_set(&o2, &s2, &chunk_set, nc, &k),
                root,
                "kk={kk}: tampering a spanning chunk must break the root"
            );
        }
    }

    /// **P-B.2.3.** The opening schedule is a pure deterministic
    /// function of public params (verifier-recomputable, no
    /// prover freedom), every tile's schedule-derived range
    /// authenticates to the *one* committed root, the ranges
    /// **cover** the matrix, and `tile_idx` strictly orders `c0`.
    #[test]
    fn tile_chunk_range_schedule_is_deterministic_and_authenticates() {
        let k = kappa();
        // Llama-3.1-8B gate/up A side: m=4096,k=4096,tile=64 ⇒
        // a_bytes = m·k, each tile = t·k = 64·4096 = 256 chunks,
        // chunk-aligned (262144 % 1024 == 0).
        for &(m, kk, t) in &[
            (4096usize, 4096usize, 64usize), // Llama gate/up A
            (4096, 14336, 64),               // Llama down k=14336
            (64, 64, 8),                     // TEST_SMALL (sub-chunk tiles)
            (96, 64, 8),                     // rectangular
        ] {
            let total = m * kk;
            let raw: Vec<u8> = (0..total)
                .map(|i| (i.wrapping_mul(40503) ^ (i >> 7)) as u8)
                .collect();
            let root = merkle_root(&raw, &k);
            let n_tiles = m / t;
            let mut prev_c0 = None;
            for ti in 0..n_tiles {
                let (c0, c1, nc) = tile_chunk_range(ti, t, kk, total);
                // Determinism: recompute, identical.
                assert_eq!((c0, c1, nc), tile_chunk_range(ti, t, kk, total));
                assert!(c0 < c1 && c1 <= nc);
                // Monotone non-decreasing c0 in tile order.
                if let Some(p) = prev_c0 {
                    assert!(c0 >= p, "c0 must be monotone in tile_idx");
                }
                prev_c0 = Some(c0);
                // The schedule-derived opening authenticates to the
                // single committed root (any sub-range does).
                let (opened, sibs) = open_strip(&raw, &k, c0, c1);
                assert_eq!(opened.len(), c1 - c0);
                assert_eq!(
                    verify_strip_opening(&opened, &sibs, c0, c1, nc, &k),
                    root,
                    "tile {ti} schedule [{c0},{c1}) of {nc} must auth to root"
                );
            }
        }
    }

    #[test]
    fn indexed_strips_chunk_range_matches_contiguous_tile_schedule() {
        for &(m, kk, t) in
            &[(4096usize, 4096usize, 64usize), (4096, 14336, 64), (64, 64, 8), (96, 64, 8)]
        {
            let total = m * kk;
            let n_tiles = m / t;
            for ti in 0..n_tiles {
                let indices: Vec<u32> = (0..t).map(|d| (ti * t + d) as u32).collect();
                assert_eq!(
                    indexed_strips_chunk_range(&indices, kk, total),
                    tile_chunk_range(ti, t, kk, total),
                    "indexed contiguous range must match tile schedule for m={m} k={kk} t={t} tile={ti}"
                );
            }
        }
    }

    #[test]
    fn indexed_strips_chunk_range_authenticates_noncontiguous_patterns() {
        let kappa = kappa();
        let kk = 1024usize;
        let total_rows = 128usize;
        let raw: Vec<u8> = (0..total_rows * kk)
            .map(|i| (i.wrapping_mul(1103515245) ^ (i >> 9)) as u8)
            .collect();
        let root = merkle_root(&raw, &kappa);
        for indices in [
            vec![0u32, 8, 64, 72],
            vec![0u32, 1, 8, 9, 32, 33, 40, 41, 64, 65, 72, 73],
            vec![7u32, 15, 63, 95],
        ] {
            let (c0, c1, nc) = indexed_strips_chunk_range(&indices, kk, raw.len());
            assert!(c0 < c1 && c1 <= nc);
            for idx in &indices {
                let lo = (*idx as usize * kk) / CHUNK_LEN;
                let hi = ((*idx as usize + 1) * kk).div_ceil(CHUNK_LEN);
                assert!(
                    c0 <= lo && hi <= c1,
                    "range [{c0},{c1}) must cover selected strip index {idx}"
                );
            }
            let (opened, sibs) = open_strip(&raw, &kappa, c0, c1);
            assert_eq!(
                verify_strip_opening(&opened, &sibs, c0, c1, nc, &kappa),
                root,
                "non-contiguous indexed cover must authenticate to the committed root"
            );
        }
    }

    /// Adversarial: a tampered opened leaf CV, or a forged
    /// authentication sibling, makes the recomputed root diverge
    /// (so the in-circuit `== PI` check rejects).
    #[test]
    fn strip_opening_rejects_tampering() {
        let k = kappa();
        let nc = 13; // non-pow2 tree, non-trivial auth path
        let raw = bytes(nc * CHUNK_LEN);
        let root = merkle_root(&raw, &k);
        let (c0, c1) = (3, 9);

        // Tampered opened leaf.
        let (mut opened, sibs) = open_strip(&raw, &k, c0, c1);
        opened[2][0] ^= 1;
        assert_ne!(verify_strip_opening(&opened, &sibs, c0, c1, nc, &k), root);

        // Forged authentication sibling.
        let (opened, mut sibs) = open_strip(&raw, &k, c0, c1);
        sibs[0].cv[7] ^= 0x80;
        assert_ne!(verify_strip_opening(&opened, &sibs, c0, c1, nc, &k), root);
    }

    fn in_circuit_root(raw: &[u8], k: &[u8; 32], trace_len: usize) -> [u8; 32] {
        use crate::composite_trace::CompositeTrace;
        let mut trace = CompositeTrace::baseline(trace_len);
        let (_n, w) = trace.place_matrix_hash_a(0, raw, k);
        let mut b = [0u8; 32];
        for i in 0..8 {
            b[i * 4..i * 4 + 4].copy_from_slice(&w[i].to_le_bytes());
        }
        b
    }

    /// **D1 finding (the latent-gap hypothesis is DISPROVEN).**
    /// `place_matrix_hash`'s bottom-up *pair-adjacent /
    /// promote-odd* parent reduction is structurally identical to
    /// BLAKE3's top-down *largest-power-of-two-left* tree — for
    /// **every** chunk count, power-of-two AND non-power-of-two.
    /// Swept exhaustively over `1..=31` (incl. all the non-pow2
    /// counts the design feared); `place_matrix_hash` ==
    /// true-tree walker == real `blake3::Hasher`. ⇒ D1-A's
    /// "realign `place_matrix_hash`" is a **no-op**; P-B.2.1
    /// reduces to *this* equivalence verification.
    #[test]
    fn place_matrix_hash_equals_true_tree_and_blake3_all_counts() {
        let k = kappa();
        for nc in 1..=31usize {
            // -257 exercises the partial-final-chunk zero pad too.
            let raw = bytes(nc * CHUNK_LEN - 257);
            let blake = *blake3::Hasher::new_keyed(&k)
                .update(&pad_to_chunk_boundary(&raw))
                .finalize()
                .as_bytes();
            assert_eq!(merkle_root(&raw, &k), blake, "walker @ {nc} chunks");
            assert_eq!(
                in_circuit_root(&raw, &k, crate::composite_layout::MIN_STARK_LEN),
                blake,
                "place_matrix_hash != blake3 @ {nc} chunks \
                 (pairwise ≡ largest-pow2-left — no latent gap)"
            );
        }
    }

    /// The decisive **large non-power-of-two** case the D1
    /// concern was really about (GEMMA/QWEN-class chunk counts):
    /// 100 chunks (= 0b1100100, very much not a power of two).
    /// `place_matrix_hash` still equals real `blake3::Hasher` —
    /// definitively no scale-dependent fidelity gap.
    #[test]
    fn place_matrix_hash_equals_blake3_large_nonpow2() {
        let k = kappa();
        let nc = 100usize;
        let raw = bytes(nc * CHUNK_LEN);
        let blake = *blake3::Hasher::new_keyed(&k)
            .update(&pad_to_chunk_boundary(&raw))
            .finalize()
            .as_bytes();
        // 100 chunks ≈ 100·16·8 + parents·8 ≈ 13.6K rows ⇒ a
        // 16384-row trace (params-driven sizing, P-B).
        assert_eq!(in_circuit_root(&raw, &k, 1 << 14), blake);
        assert_eq!(merkle_root(&raw, &k), blake);
    }
}

//! M1 + H2 (DoS audit): the two memory-DoS paths in `ai-pow` whose
//! allocations were proportional to an *attacker-controlled* value
//! rather than to actual input size.
//!
//! * **M1** — `MatmulProof::decode` / `decode_path_list` did
//!   `Vec::with_capacity(untrusted_count)` after a *loose* cap check.
//!   A ~200-byte blob declaring `MAX_SPOT = 2^20` and truncating
//!   triggered ~100 MiB up-front allocation per decode (~500,000×
//!   amplification).
//! * **H2** — `fiat_shamir::challenge_indices` did
//!   `vec![false; range]` where `range = num_tiles`. After H1 capped
//!   `num_tiles` at `u32::MAX`, a call with `range = 2^32` would have
//!   burned ~4 GiB. The fix uses a `HashSet` sized to `O(count)`,
//!   regardless of `range`.
//!
//! This test installs a counting global allocator (scoped to this
//! test binary) and asserts both allocation paths are bounded.

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicUsize, Ordering};

use ai_pow::fiat_shamir::challenge_indices;
use ai_pow::params::MatmulParams;
use ai_pow::proof::MatmulProof;

struct CountingAlloc;
static ALLOCATED: AtomicUsize = AtomicUsize::new(0);
static PEAK: AtomicUsize = AtomicUsize::new(0);

unsafe impl GlobalAlloc for CountingAlloc {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let p = unsafe { System.alloc(layout) };
        if !p.is_null() {
            let cur = ALLOCATED.fetch_add(layout.size(), Ordering::Relaxed) + layout.size();
            PEAK.fetch_max(cur, Ordering::Relaxed);
        }
        p
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        ALLOCATED.fetch_sub(layout.size(), Ordering::Relaxed);
        unsafe { System.dealloc(ptr, layout) };
    }
}

#[global_allocator]
static A: CountingAlloc = CountingAlloc;

/// Build a tiny malicious blob: a valid header + empty `found`
/// opening + a `spot` length-prefix declaring `declared_n` entries
/// — and no actual spot bytes after. The pre-fix `decode` would
/// `Vec::with_capacity(declared_n)` of `TileOpening`s up-front.
fn malicious_spot_blob(declared_n: u32) -> Vec<u8> {
    let mut b = Vec::new();
    // 6 × 32-byte commitment fields (zeros).
    b.extend_from_slice(&[0u8; 32 * 6]);
    // `found` TileOpening — all variable-length fields empty.
    b.extend_from_slice(&0u32.to_le_bytes()); // i
    b.extend_from_slice(&0u32.to_le_bytes()); // j
    b.extend_from_slice(&0u32.to_le_bytes()); // m_path: len 0
    b.extend_from_slice(&0u32.to_le_bytes()); // a_rows: len 0
    b.extend_from_slice(&0u32.to_le_bytes()); // b_cols: len 0
    b.extend_from_slice(&0u32.to_le_bytes()); // a_row_paths: count 0
    b.extend_from_slice(&0u32.to_le_bytes()); // b_col_paths: count 0
    b.extend_from_slice(&declared_n.to_le_bytes()); // spot count
    b
}

/// Same idea, but the bomb is the path-list count *inside* the
/// `found` opening (`decode_path_list`'s `with_capacity`).
fn malicious_path_list_blob(declared_n: u32) -> Vec<u8> {
    let mut b = Vec::new();
    b.extend_from_slice(&[0u8; 32 * 6]);
    b.extend_from_slice(&0u32.to_le_bytes()); // i
    b.extend_from_slice(&0u32.to_le_bytes()); // j
    b.extend_from_slice(&0u32.to_le_bytes()); // m_path: len 0
    b.extend_from_slice(&0u32.to_le_bytes()); // a_rows: len 0
    b.extend_from_slice(&0u32.to_le_bytes()); // b_cols: len 0
    b.extend_from_slice(&declared_n.to_le_bytes()); // a_row_paths: count = bomb
    b
}

/// Parameter-aware decoder bomb: a syntactically plausible proof prefix whose
/// found opening declares an enormous A-strip length. The verifier-facing
/// decoder must compare that prefix against `params` and return before
/// allocating or copying any strip body.
fn malicious_param_strip_len_blob(declared_len: u32) -> Vec<u8> {
    let mut b = Vec::new();
    b.extend_from_slice(&[0u8; 32 * 6]);
    b.extend_from_slice(&0u32.to_le_bytes()); // i
    b.extend_from_slice(&0u32.to_le_bytes()); // j
    b.extend_from_slice(&0u32.to_le_bytes()); // m_path depth
    b.extend_from_slice(&declared_len.to_le_bytes()); // a_rows len = bomb
    b
}

fn shaped_params() -> MatmulParams {
    MatmulParams {
        m: 8,
        k: 64,
        n: 8,
        noise_rank: 4,
        tile: 8,
        spot_checks: 1,
        difficulty_bits: 0,
    }
}

/// Measure the peak allocation `f()` triggers, in bytes.
fn measure_alloc_peak<R>(f: impl FnOnce() -> R) -> (R, usize) {
    let before = ALLOCATED.load(Ordering::Relaxed);
    PEAK.store(before, Ordering::Relaxed);
    let r = f();
    let peak = PEAK.load(Ordering::Relaxed);
    (r, peak.saturating_sub(before))
}

/// Bomb threshold: with the bug, decode allocates ~100 MiB (spot)
/// or ~25 MiB (path-list) for a ~200-byte blob. The fix makes
/// allocation proportional to input — a few KiB at most for these
/// blobs. 4 MiB sits comfortably in the gap and tolerates parallel
/// test-harness noise.
const AMPLIFICATION_LIMIT: usize = 4 << 20;

#[test]
fn decode_does_not_amplify_attacker_declared_counts() {
    // (a) The `spot` count bomb.
    //
    // `MAX_SPOT = 1 << 20` (private const, mirrored here as the
    // worst-case policy-allowed value).
    let spot_blob = malicious_spot_blob(1 << 20);
    assert!(
        spot_blob.len() < 256,
        "blob is tiny ({} bytes), the bug amplifies it to ~100 MiB",
        spot_blob.len()
    );
    let (res, peak) = measure_alloc_peak(|| MatmulProof::decode(&spot_blob));
    assert!(res.is_err(), "malformed blob must be rejected");
    assert!(
        peak < AMPLIFICATION_LIMIT,
        "decode of a {}-byte blob declaring 2^20 spot entries allocated {} bytes — \
         must be proportional to input, not to the untrusted count \
         (pre-fix bomb was ~100 MiB)",
        spot_blob.len(),
        peak,
    );

    // (b) The `decode_path_list` count bomb (inside the `found`
    // opening; `MAX_STRIP_COUNT = 1 << 20`).
    let path_blob = malicious_path_list_blob(1 << 20);
    assert!(path_blob.len() < 256);
    let (res, peak) = measure_alloc_peak(|| MatmulProof::decode(&path_blob));
    assert!(res.is_err());
    assert!(
        peak < AMPLIFICATION_LIMIT,
        "decode_path_list of a {}-byte blob declaring 2^20 paths allocated {} bytes \
         (pre-fix bomb was ~25 MiB)",
        path_blob.len(),
        peak,
    );

    // (c) Sanity: an out-of-range count is rejected *before* any
    // allocation (the policy-cap check).
    let oversize = malicious_spot_blob(u32::MAX);
    let (res, peak) = measure_alloc_peak(|| MatmulProof::decode(&oversize));
    assert!(matches!(res, Err(_)));
    assert!(
        peak < AMPLIFICATION_LIMIT,
        "u32::MAX-count blob allocated {peak} bytes",
    );

    // (d) H2: `challenge_indices` allocation must be `O(count)`, not
    // `O(range)`. Pre-fix: `vec![false; range]` allocated ~4 GiB for
    // `range = 2^32`. Post-fix uses a `HashSet` sized to `count`.
    let seed = [0xA5u8; 32];
    let count: u32 = 80; // Pearl-class
    let range: u64 = 1u64 << 32; // == H1's tile-count ceiling
    let (out, peak) = measure_alloc_peak(|| challenge_indices(&seed, count, range));
    assert_eq!(
        out.len(),
        count as usize,
        "challenge_indices must return exactly `count` indices"
    );
    assert!(
        peak < AMPLIFICATION_LIMIT,
        "challenge_indices(count=80, range=2^32) allocated {peak} bytes — \
         must be O(count) not O(range) (pre-fix bomb was ~4 GiB)",
    );

    // (e) Verifier-facing parameter-aware decode must reject shape-impossible
    // strip lengths from the prefix alone, before allocating a declared
    // strip body.
    let params = shaped_params();
    let strip_blob = malicious_param_strip_len_blob(u32::MAX);
    let (res, peak) = measure_alloc_peak(|| MatmulProof::decode_for_params(&strip_blob, &params));
    assert!(res.is_err(), "shape-impossible strip length must reject");
    assert!(
        peak < AMPLIFICATION_LIMIT,
        "decode_for_params allocated {peak} bytes for a prefix-only strip-length bomb",
    );
}

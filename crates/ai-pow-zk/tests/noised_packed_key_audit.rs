//! Regression coverage for positioned matrix keys and their bounds.

use ai_pow_zk::canonical::{covering_id_lane_base, covering_id_span};
use ai_pow_zk::composite_trace::{
    noised_chunk_id, noised_id_bases, try_noised_id_bases, NOISED_CHUNK_ID_BASE,
};

fn single_src(lane: u32, col: u32) -> [Option<(u32, u32)>; 8] {
    let mut s = [None; 8];
    s[0] = Some((lane, col));
    s
}

/// Assert every A-side key lies strictly below `b_id_base` and every B-side
/// key at or above it, over every lane and every 8-byte sub-slice column.
fn assert_side_disjoint(a_lanes: &[u32], b_lanes: &[u32], k: usize, b_id_base: u64) {
    for &lane in a_lanes {
        for l in (0..k as u32).step_by(8) {
            let key = noised_chunk_id(NOISED_CHUNK_ID_BASE, k, &single_src(lane, l));
            assert!(
                key < b_id_base,
                "A-side key {key} (lane {lane}, col {l}) must stay below b_id_base {b_id_base}"
            );
        }
    }
    for &lane in b_lanes {
        for l in (0..k as u32).step_by(8) {
            let key = noised_chunk_id(b_id_base, k, &single_src(lane, l));
            assert!(
                key >= b_id_base,
                "B-side key {key} (lane {lane}, col {l}) must stay at or above b_id_base {b_id_base}"
            );
        }
    }
}

#[test]
fn noised_packed_scattered_keys_are_side_disjoint() {
    let k = 1024usize;
    // Non-contiguous coverage pattern with lanes beyond the tile height.
    let a_indices = [0u32, 1, 8, 9, 64, 65, 72, 73];
    let b_indices = [0u32, 1, 2, 3, 4, 5, 6, 7];
    let a_lanes: Vec<u32> = a_indices.map(|i| i - a_indices[0]).to_vec();
    let b_lanes: Vec<u32> = b_indices.map(|i| i - b_indices[0]).to_vec();
    let (a_id_base, b_id_base) = noised_id_bases(
        *a_lanes.iter().max().expect("A lanes are nonempty") as usize,
        *b_lanes.iter().max().expect("B lanes are nonempty") as usize,
        k,
    );
    assert_eq!(a_id_base, NOISED_CHUNK_ID_BASE);
    assert_side_disjoint(&a_lanes, &b_lanes, k, b_id_base);

    // Boundary keys remain on their respective sides of the namespace base.
    let key_a8 = noised_chunk_id(a_id_base, k, &single_src(8, 0));
    let key_b0 = noised_chunk_id(b_id_base, k, &single_src(0, 0));
    assert_ne!(
        key_a8, key_b0,
        "A and B boundary keys must use distinct noised_packed IDs"
    );
    assert!(key_a8 < b_id_base && key_b0 >= b_id_base);

    // A tile-height-only span is insufficient for this non-contiguous layout.
    let tile_height_base = NOISED_CHUNK_ID_BASE + ((a_indices.len() * k) / 8) as u64;
    assert_eq!(
        key_a8,
        noised_chunk_id(tile_height_base, k, &single_src(0, 0)),
        "non-contiguous schedules require the full covering-range span"
    );
}

#[test]
fn noised_packed_contiguous_tile_matches_legacy_derivation() {
    // A chunk-aligned contiguous tile has no producer tail, so the fixed base
    // is byte-identical to the legacy formula.
    for &(h_tile, w_tile, k) in &[(8usize, 8usize, 1024usize), (16, 16, 4096), (8, 8, 1152)] {
        let (a_id_base, b_id_base) = noised_id_bases(h_tile - 1, w_tile - 1, k);
        let legacy = NOISED_CHUNK_ID_BASE + ((h_tile * k) / 8) as u64;
        assert_eq!(a_id_base, NOISED_CHUNK_ID_BASE);
        assert_eq!(
            b_id_base, legacy,
            "contiguous (h_tile={h_tile}, k={k}) must keep the legacy b_id_base"
        );
        let a_lanes: Vec<u32> = (0..h_tile as u32).collect();
        let b_lanes: Vec<u32> = (0..w_tile as u32).collect();
        assert_side_disjoint(&a_lanes, &b_lanes, k, b_id_base);
    }
}

#[test]
fn noised_packed_sub_chunk_k_nonorigin_disjoint() {
    // For a k < 1024 non-origin tile, the chunk base is 0 and lanes are
    // absolute row indices. The namespace span must cover those indices.
    let k = 64usize;
    let lanes: Vec<u32> = (8u32..16).collect(); // tile rows 8..16
    let (a_id_base, b_id_base) = noised_id_bases(15, 15, k);
    assert_eq!(a_id_base, NOISED_CHUNK_ID_BASE);
    let legacy = NOISED_CHUNK_ID_BASE + ((8 * k) / 8) as u64; // 72
    let max_a_key = noised_chunk_id(a_id_base, k, &single_src(15, (k - 8) as u32));
    assert!(
        max_a_key >= legacy,
        "pins that the legacy base did not cover absolute-index lanes"
    );
    assert_side_disjoint(&lanes, &lanes, k, b_id_base);
}

#[test]
fn noised_packed_id_budget_guard() {
    // In-envelope production-scale spans fit with ample margin
    // (m=4096 rows A, n=28672 cols B, k=4096 ⇒ ~16.8M max id < 2^26).
    try_noised_id_bases(4095, 28671, 4096).expect("in-envelope spans must fit the id budget");
    // A span at the u32-index extreme overflows and is rejected, not panicked.
    assert!(
        try_noised_id_bases((1 << 24) - 1, 1, 1 << 16).is_err(),
        "span overflowing the 26-bit pack_ab_id budget must be rejected"
    );
    // Chunk 128 begins at matrix row 64 when k=2048.
    assert_eq!(
        covering_id_lane_base("A", 128, 2048).expect("aligned lane base"),
        64
    );
    assert_eq!(
        covering_id_span("A", &[64, 65], 128, 2048).expect("aligned wide span"),
        2
    );
    assert_eq!(
        covering_id_span("A", &[64, 65], 64, 1024).expect("aligned span"),
        2
    );
    assert_eq!(
        covering_id_span("A", &[0, 1, 8], 0, 64).expect("origin span"),
        9
    );
    // A selected chunk that starts inside a matrix row cannot use lane IDs.
    assert!(covering_id_span("A", &[1, 2], 1, 1536).is_err());
}

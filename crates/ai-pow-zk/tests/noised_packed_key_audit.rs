//! Adversarial-audit regression — `noised_packed` side-disjoint keys.
//!
//! Deriving `b_id_base` from the tile height (`a_id_base + h_tile·k/8`)
//! covers only tile-local lanes. For a SCATTERED opening the A-side lane
//! is the covering-range position (`row − ca0`), which reaches beyond
//! `h_tile` and collides with the B-side key space. The LogUp
//! fingerprint's packed value only binds a read to *some* committed
//! store entry, so the collision admits committed-B-value substitution
//! at colliding A positions — an XOR-delta jackpot-grinding amplifier.
//!
//! Deriving the bases from the full A covering-range span
//! (`max_a_lane + 1`) makes every A-side key `< b_id_base` and every
//! B-side key `>= b_id_base`, for any schedule. Contiguous tiles have
//! span == h_tile, so the tile-height derivation is byte-identical
//! there. This test pins the disjointness invariant, the contiguous
//! parity, the separation of the historical collision pair, and the
//! 26-bit id-budget guard.

use ai_pow_zk::canonical::covering_id_span;
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
    // The non-contiguous audit pattern (h_tile = 8): under the legacy base,
    // A-row-8's keys collided exactly with B-col-0's.
    let a_indices = [0u32, 1, 8, 9, 64, 65, 72, 73];
    let b_indices = [0u32, 1, 2, 3, 4, 5, 6, 7];
    let a_lanes: Vec<u32> = a_indices.map(|i| i - a_indices[0]).to_vec();
    let b_lanes: Vec<u32> = b_indices.map(|i| i - b_indices[0]).to_vec();
    let (a_id_base, b_id_base) = noised_id_bases(
        *a_lanes.iter().max().unwrap() as usize,
        *b_lanes.iter().max().unwrap() as usize,
        k,
    );
    assert_eq!(a_id_base, NOISED_CHUNK_ID_BASE);
    assert_side_disjoint(&a_lanes, &b_lanes, k, b_id_base);

    // The documented collision pair is now separated: A-row-8's key sits
    // below the B range, B-col-0's key inside it.
    let key_a8 = noised_chunk_id(a_id_base, k, &single_src(8, 0));
    let key_b0 = noised_chunk_id(b_id_base, k, &single_src(0, 0));
    assert_ne!(
        key_a8, key_b0,
        "the collision pair must not share a noised_packed chunk_id"
    );
    assert!(key_a8 < b_id_base && key_b0 >= b_id_base);

    // Pin the historical collision itself so the exploit class stays explicit:
    // under the LEGACY tile-height base the two keys were EQUAL.
    let legacy_b_id_base = NOISED_CHUNK_ID_BASE + ((a_indices.len() * k) / 8) as u64;
    assert_eq!(
        key_a8,
        noised_chunk_id(legacy_b_id_base, k, &single_src(0, 0)),
        "tile-height derivation collides A-row-8 with B-col-0 (the historical exploit)"
    );
}

#[test]
fn noised_packed_contiguous_tile_matches_legacy_derivation() {
    // Contiguous tiles have span == tile height, so the span-derived base is
    // byte-identical to the legacy formula — the native path is unchanged.
    for &(h_tile, w_tile, k) in &[(8usize, 8usize, 1024usize), (16, 16, 4096), (8, 8, 64)] {
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
    // k < 1024 non-origin tile: the chunk base is 0, so lanes are ABSOLUTE
    // row indices — every lane >= h_tile, the same collision class as the
    // scattered case (the legacy base could not cover them).
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
    // covering_id_span rejects k > 1024 non-origin schedules (the lane
    // arithmetic has no representation for them) instead of underflowing.
    assert!(covering_id_span("A", &[64, 65], 128).is_err());
    assert_eq!(covering_id_span("A", &[64, 65], 64).unwrap(), 2);
    assert_eq!(covering_id_span("A", &[0, 1, 8], 0).unwrap(), 9);
}

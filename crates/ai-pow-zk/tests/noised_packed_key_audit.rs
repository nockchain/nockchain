//! §A adversarial-audit regression — `noised_packed` key-collision documentation.
//!
//! For a SCATTERED opening the A-side `noised_chunk_id` key uses
//! `lane = row − ca0` (the covering-range position), which for scattered rows can
//! exceed `b_id_base = a_id_base + h_tile·k/8` and COLLIDE with the B-side key
//! space. This is BENIGN because the `noised_packed` LogUp fingerprint is the
//! 3-tuple `(chunk_id, packed_lo, packed_hi)` — the packed VALUE disambiguates,
//! and the values are bound to the committed matrices (BLAKE3 co-location →
//! `HASH_A/B`) with seed-derived noise. This test pins the collision so the
//! reliance on the value stays explicit: a future reduction to id-only keying
//! would reintroduce the exploit and must be caught here.

use ai_pow_zk::composite_trace::{noised_chunk_id, NOISED_CHUNK_ID_BASE};

fn single_src(lane: u32, col: u32) -> [Option<(u32, u32)>; 8] {
    let mut s = [None; 8];
    s[0] = Some((lane, col));
    s
}

#[test]
fn noised_packed_ab_key_collision_documented() {
    let k = 1024usize;
    // The non-contiguous audit pattern (h_tile = 8).
    let a_indices = [0u32, 1, 8, 9, 64, 65, 72, 73];
    let h_tile = a_indices.len();
    let ca0 = a_indices[0] as usize; // 0
    let a_id_base = NOISED_CHUNK_ID_BASE;
    let b_id_base = a_id_base + ((h_tile * k).div_ceil(8)) as u64;

    // A-side row 8 (index 2): covering-range lane = 8 − 0 = 8.
    let lane_a8 = a_indices[2] - (ca0 as u32);
    let key_a8 = noised_chunk_id(a_id_base, k, &single_src(lane_a8, 0));
    // B-side col 0 (index 0): lane = 0.
    let key_b0 = noised_chunk_id(b_id_base, k, &single_src(0, 0));

    // The A-side scattered key reaches into the B-side key range...
    assert!(
        key_a8 >= b_id_base,
        "A-side scattered key {key_a8} must reach the B-side space (>= {b_id_base})"
    );
    // ...and in fact collides exactly with B-col-0.
    assert_eq!(
        key_a8, key_b0,
        "documented §A collision: A-row-8 and B-col-0 share a noised_packed chunk_id"
    );

    // Contrast: a CONTIGUOUS tile (rows 0..8) never reaches b_id_base — the A-side
    // keys stay strictly below it, so the overlap is exclusively a scattered-open
    // phenomenon.
    let contig_max_lane = (h_tile - 1) as u32;
    let contig_key = noised_chunk_id(a_id_base, k, &single_src(contig_max_lane, (k - 1) as u32));
    assert!(
        contig_key < b_id_base,
        "contiguous A-side max key {contig_key} must stay below b_id_base {b_id_base}"
    );

    // SAFETY INVARIANT (enforced by the AIR, not this test): the noised_packed
    // interaction is a 3-tuple (chunk_id, packed_lo, packed_hi), so (id, val_A)
    // and (id, val_B) are DISTINCT fingerprints even when the ids collide. The
    // honest non-contiguous recursive round-trip exercises this collision and
    // verifies; `sec_4c10_noncontiguous_sweep_on_row_permuted_matrix_rejects`
    // exercises the adversarial wrong-value-at-position case and rejects.
}

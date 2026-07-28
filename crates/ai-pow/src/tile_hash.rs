//! Shape-aware difficulty threshold and 256-bit little-endian unsigned compare.
//!
//! Pearl §4.5 hardness rule:
//!
//!   BLAKE3(M, key = pow_key_for_nonce(s_a, nonce))  <=  2^(256 - b) · r · t_m · t_n
//!
//! `s_a` is derived from the nonce-bound attempt state before the noised
//! matmul is computed; `pow_key_for_nonce` is not the sole nonce binding.
//!
//! All 256-bit integers are encoded as little-endian byte arrays for parity
//! with Pearl, which interprets the BLAKE3 keyed hash via
//! `U256::from_little_endian` (Pearl zk-pow ffi/mine.rs:101). Byte 0 is
//! the LSB; byte 31 is the MSB. The comparison is the natural unsigned
//! ordering on these 256-bit integers.

use crate::params::MatmulParams;

/// 256-bit unsigned `hash <= target` with both operands encoded as
/// little-endian 32-byte arrays.
pub fn hash_le_target(hash: &[u8; 32], target: &[u8; 32]) -> bool {
    for k in (0..32).rev() {
        match hash[k].cmp(&target[k]) {
            core::cmp::Ordering::Less => return true,
            core::cmp::Ordering::Greater => return false,
            core::cmp::Ordering::Equal => continue,
        }
    }
    true
}

/// Compute the shape-aware target `2^(256 - b) · r · t^2`, saturating at the
/// max 256-bit value when the product would exceed it. Output is a 32-byte
/// little-endian unsigned integer.
pub fn difficulty_target(params: &MatmulParams) -> [u8; 32] {
    let r = params.noise_rank as u128;
    let t = params.tile as u128;
    let weight = r.saturating_mul(t).saturating_mul(t);
    target_from_weight(params.difficulty_bits, weight)
}

/// Build a 256-bit little-endian target representing `floor(weight · 2^(256-b))`,
/// saturated to `[0, 2^256 - 1]`. The 256-bit number is held as a high-128
/// half (bits 128..256) and a low-128 half (bits 0..128) and packed in
/// little-endian byte order: `out[0..16] = lo`, `out[16..32] = hi`.
fn target_from_weight(b: u32, weight: u128) -> [u8; 32] {
    if weight == 0 {
        return [0u8; 32];
    }
    if b == 0 {
        return [0xFFu8; 32];
    }
    if b >= 256 + 128 {
        return [0u8; 32];
    }
    let shift = 256i32 - b as i32;
    let (hi, lo): (u128, u128) = if shift >= 128 {
        let s = (shift - 128) as u32;
        if s > 0 && (weight >> (128 - s)) != 0 {
            return [0xFFu8; 32];
        }
        let hi = if s == 128 { 0 } else { weight << s };
        (hi, 0u128)
    } else if shift > 0 {
        let s = shift as u32;
        let lo = if s == 0 { weight } else { weight << s };
        let hi = if s == 0 { 0 } else { weight >> (128 - s) };
        (hi, lo)
    } else {
        let s = (-shift) as u32;
        if s >= 128 {
            return [0u8; 32];
        }
        (0u128, weight >> s)
    };
    let mut out = [0u8; 32];
    out[..16].copy_from_slice(&lo.to_le_bytes());
    out[16..].copy_from_slice(&hi.to_le_bytes());
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn target_compare_edges() {
        let zero = [0u8; 32];
        let max = [0xffu8; 32];
        assert!(hash_le_target(&zero, &zero));
        assert!(hash_le_target(&zero, &max));
        assert!(hash_le_target(&max, &max));
        assert!(!hash_le_target(&max, &zero));

        // Little-endian: byte 0 is least significant, byte 31 is most
        // significant. So setting byte 0 = 0x10/0x11 compares the LSBs.
        let mut a = [0u8; 32];
        a[0] = 0x10;
        let mut b = [0u8; 32];
        b[0] = 0x11;
        assert!(hash_le_target(&a, &b));
        assert!(!hash_le_target(&b, &a));

        // Most-significant byte (byte 31) dominates.
        let mut c = [0u8; 32];
        c[31] = 0x01;
        let mut d = [0u8; 32];
        d[31] = 0x00;
        d[0] = 0xff;
        assert!(!hash_le_target(&c, &d));
        assert!(hash_le_target(&d, &c));
    }

    #[test]
    fn difficulty_b_zero_is_max_target() {
        let params = MatmulParams::TEST_SMALL;
        let t = difficulty_target(&params);
        assert_eq!(t, [0xFF; 32]);
    }

    #[test]
    fn difficulty_b_huge_is_zero_target() {
        let mut params = MatmulParams::TEST_SMALL;
        params.difficulty_bits = 400;
        let t = difficulty_target(&params);
        assert_eq!(t, [0u8; 32]);
    }

    #[test]
    fn difficulty_target_monotonic_in_b() {
        let mut p = MatmulParams::TEST_SMALL;
        let mut prev: Option<[u8; 32]> = None;
        for b in [0u32, 1, 8, 16, 64, 128, 200, 256] {
            p.difficulty_bits = b;
            let t = difficulty_target(&p);
            if let Some(prev) = prev {
                assert!(
                    !hash_le_target(&prev, &t) || prev == t,
                    "target at b={b} ({t:?}) should be <= prev ({prev:?})"
                );
            }
            prev = Some(t);
        }
    }

    #[test]
    fn difficulty_target_shape_weighting() {
        let mut small = MatmulParams::TEST_SMALL;
        small.difficulty_bits = 128;
        let small_t = difficulty_target(&small);
        let mut big = small;
        big.tile = small.tile * 2;
        let big_t = difficulty_target(&big);
        assert!(!hash_le_target(&big_t, &small_t) || big_t == small_t);
    }

    #[test]
    fn difficulty_target_is_per_tile_not_per_grid() {
        let mut base = MatmulParams::TEST_SMALL;
        base.difficulty_bits = 128;
        let mut wider_grid = base;
        wider_grid.m *= 2;
        wider_grid.n *= 2;
        assert_eq!(wider_grid.noise_rank, base.noise_rank);
        assert_eq!(wider_grid.tile, base.tile);
        assert!(
            wider_grid.num_tiles() > base.num_tiles(),
            "test must increase the number of searchable tiles"
        );
        assert_eq!(
            difficulty_target(&base),
            difficulty_target(&wider_grid),
            "Pearl target prices one nonce-bound tile work unit; total grid \
             size increases available work units, not the per-tile target"
        );
    }

    #[test]
    fn difficulty_b_256_equals_weight_le() {
        let mut p = MatmulParams::TEST_SMALL;
        p.difficulty_bits = 256;
        // weight = r * t^2 = 4 * 64 = 256 = 0x100. As a 256-bit LE int, the
        // low byte is 0x00 and byte 1 is 0x01; everything else is zero.
        let weight = 256u128;
        let mut expected = [0u8; 32];
        expected[..16].copy_from_slice(&weight.to_le_bytes());
        assert_eq!(difficulty_target(&p), expected);
    }

    #[test]
    fn difficulty_matches_u256_arithmetic() {
        // Spot-check: at b = 256 the target value equals `weight`. The LE
        // encoding places the low byte of `weight` at index 0.
        let mut p = MatmulParams::TEST_SMALL;
        p.difficulty_bits = 256;
        let t = difficulty_target(&p);
        let weight_u128 = (p.noise_rank as u128) * (p.tile as u128) * (p.tile as u128);
        let mut expected = [0u8; 32];
        expected[..16].copy_from_slice(&weight_u128.to_le_bytes());
        assert_eq!(t, expected);
    }
}

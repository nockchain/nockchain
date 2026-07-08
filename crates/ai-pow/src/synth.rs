//! Deterministic synthesis of input matrices `(A, B)` from a seed.
//!
//! Used by tests and benches to construct Pearl-valid input matrices
//! without external data. **Not** for use as the actual miner input —
//! real miners supply their own `A` and `B`.

use crate::params::MatmulParams;
use crate::prng;

/// Deterministically build `(A, B)` of shapes matching `params`, with
/// every entry in `[-64, 63]`. Different `seed` bytes produce uncorrelated
/// matrix pairs.
pub fn synth_matrices(seed: &[u8], params: &MatmulParams) -> (Vec<i8>, Vec<i8>) {
    let m = params.m as usize;
    let k = params.k as usize;
    let n = params.n as usize;
    let mut a = vec![0i8; m * k];
    for i in 0..params.m {
        let off = (i as usize) * k;
        prng::expand_a_row(seed, i, params.k, &mut a[off..off + k]);
    }
    let mut b = vec![0i8; n * k];
    for j in 0..params.n {
        let off = (j as usize) * k;
        prng::expand_b_col(seed, j, params.k, &mut b[off..off + k]);
    }
    (a, b)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn synth_shapes_match_params() {
        let p = MatmulParams::TEST_SMALL;
        let (a, b) = synth_matrices(b"seed", &p);
        assert_eq!(a.len(), (p.m * p.k) as usize);
        assert_eq!(b.len(), (p.n * p.k) as usize);
    }

    #[test]
    fn synth_in_range() {
        let p = MatmulParams::TEST_SMALL;
        let (a, b) = synth_matrices(b"seed", &p);
        for x in a.iter().chain(b.iter()) {
            assert!(*x >= -64 && *x <= 63);
        }
    }

    #[test]
    fn synth_is_deterministic() {
        let p = MatmulParams::TEST_SMALL;
        let (a1, b1) = synth_matrices(b"seed", &p);
        let (a2, b2) = synth_matrices(b"seed", &p);
        assert_eq!(a1, a2);
        assert_eq!(b1, b2);
    }

    #[test]
    fn synth_seed_sensitive() {
        let p = MatmulParams::TEST_SMALL;
        let (a1, _) = synth_matrices(b"seed-1", &p);
        let (a2, _) = synth_matrices(b"seed-2", &p);
        assert_ne!(a1, a2);
    }
}

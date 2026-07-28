//! Deterministic synthesis of input matrices `(A, B)` from a seed.
//!
//! Used by tests/benches to construct Pearl-valid matrices without external
//! data, and as the `ai-pow-mine` binary's default matrix source:
//! `synth_matrices(AI_POW_PROD_SYNTH_SEED, ..)`.
//!
//! # Matrix-source policy
//!
//! The proof statement binds the matrix bytes through committed `HASH_A` and
//! `HASH_B` public inputs. Consensus verifies the proof against those committed
//! values and the block target; it does not require a globally canonical matrix
//! seed. Model provenance, economic usefulness, and uniqueness are network
//! policy concerns outside this deterministic test/default generator.

use crate::params::MatmulParams;
use crate::prng;

/// Default production synth seed used by `ai-pow-mine` when no external matrix
/// source is configured. Changing it changes the default miner workload, not the
/// consensus statement.
pub const AI_POW_PROD_SYNTH_SEED: &[u8] = b"ai-pow-prod-v1";

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

//! Deterministic synthesis of input matrices `(A, B)` from a seed.
//!
//! Used by tests/benches to construct Pearl-valid matrices without external
//! data, **and** as the production miner's default matrix source — the
//! `ai-pow-mine` binary defaults to `synth_matrices(AI_POW_PROD_SYNTH_SEED, ..)`.
//!
//! # Canonical-matrix soundness (audit)
//!
//! For AI-PoW to be sound, the matrices `A`/`B` a block's proof is built over
//! **must be canonically pinned** by the protocol — otherwise a miner grinds a
//! favorable `(A, B)` and the difficulty target loses meaning. The natural,
//! consensus-derivable pin is `synth_matrices(AI_POW_PROD_SYNTH_SEED, params)`:
//! every verifying node re-derives the identical matrices from the public seed +
//! the block's `params`, with no external model distribution. A verifier that
//! accepts a block MUST verify its proof against these canonical matrices (see
//! `ai-pow-miner::verify_ai_pow_block_artifact_jam`). Whether production instead
//! pins *external* weights is a protocol decision; the production binary's synth
//! default is the derivable, sound choice and the one consensus must enforce.

use crate::params::MatmulParams;
use crate::prng;

/// The canonical production synth seed. The `ai-pow-mine` binary defaults to it,
/// and a consensus verifier re-derives `(A, B)` from it (see the module docs) so
/// no external matrix distribution is needed. Changing it is a hard fork.
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

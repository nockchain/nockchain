//! Process-owned execution stacks for AI-PoW preprocessing, proving, and verification.
//!
//! Every production proof entry enters this reusable pool, even when called from
//! an existing global or embedding Rayon pool. Workers reserve 16 MiB of stack.
//! The worker count follows Rayon's normal `RAYON_NUM_THREADS`/CPU policy. The
//! pool also owns execution when Plonky3 parallelism is disabled; recursion
//! dependencies can independently enable Plonky3's parallel features.
//!
//! Applications must call [`prepare`] before admitting blocks or starting mining
//! and treat failure as a local resource error, never an invalid-proof verdict.
//! Library consumers can use [`try_install`] for the same fallible boundary.
//! Existing proof APIs retain their signatures and panic on resource failure if
//! called without successful preflight; do not convert that panic into rejection.

use std::sync::{Arc, LazyLock};

/// Stack reservation for each owned proof worker, independent of caller pools.
pub const WORKER_STACK_BYTES: usize = 16 * 1024 * 1024;

/// A process-local inability to construct the proof runtime.
#[derive(Clone, Debug, thiserror::Error)]
#[error(
    "could not initialize AI-PoW proof workers: {0}; check local thread/memory limits and restart"
)]
pub struct ProofRuntimeError(Arc<str>);

fn pool() -> Result<&'static rayon::ThreadPool, ProofRuntimeError> {
    static POOL: LazyLock<Result<rayon::ThreadPool, ProofRuntimeError>> = LazyLock::new(|| {
        rayon::ThreadPoolBuilder::new()
            .stack_size(WORKER_STACK_BYTES)
            .build()
            .map_err(|error| ProofRuntimeError(error.to_string().into()))
    });
    POOL.as_ref().map_err(Clone::clone)
}

/// Construct the shared workers before admission/mining. Failure is sticky for
/// this process: resource limits must be repaired before restarting.
pub fn prepare() -> Result<(), ProofRuntimeError> {
    pool().map(|_| ())
}

/// Run on the owned proof workers, propagating initialization failure separately
/// from the operation's result. Nested entry reuses the current owned worker.
/// No pool, thread, or heap allocation is performed per successful entry.
pub fn try_install<Op, R>(op: Op) -> Result<R, ProofRuntimeError>
where
    Op: FnOnce() -> R + Send,
    R: Send,
{
    let pool = pool()?;
    Ok(if pool.current_thread_index().is_some() {
        op()
    } else {
        pool.install(op)
    })
}

pub(crate) fn install<Op, R>(op: Op) -> R
where
    Op: FnOnce() -> R + Send,
    R: Send,
{
    try_install(op).unwrap_or_else(|error| panic!("{error}"))
}

#[cfg(test)]
mod tests {
    use p3_commit::Mmcs;
    use p3_field::PrimeCharacteristicRing;
    use p3_matrix::dense::RowMajorMatrix;

    use super::*;
    use crate::chips::matmul::chip::{build_trace, MatmulCumsumChip, Step};
    use crate::circuit::{
        build_stark_config, AiPowStarkConfig, CircuitConfig, Tip5Compress, Tip5Perm, Tip5Sponge,
        ValMmcs,
    };
    use crate::composite_layout::{TILE_D, TILE_H};
    use crate::{Val, ZkParams};

    fn params() -> ZkParams {
        ZkParams {
            m: 8,
            k: 16,
            n: 8,
            noise_rank: 2,
            tile: 2,
            difficulty_bits: 0,
        }
    }

    // Real native Merkle construction plus a real STARK over the production
    // Goldilocks/Tip5 stack. Tiny dimensions keep this a bounded regression.
    fn commitment_and_proof() -> (Vec<u8>, Vec<u8>) {
        let mmcs = ValMmcs::new(Tip5Sponge::new(Tip5Perm), Tip5Compress::new(Tip5Perm), 0);
        let matrix = RowMajorMatrix::new((0..1024).map(Val::from_u64).collect(), 8);
        let (commitment, _) = mmcs.commit(vec![matrix]);
        let config = build_stark_config(&params(), &CircuitConfig::TEST_PEARL);
        let mut steps = vec![
            Step {
                a: [[1; TILE_D]; TILE_H],
                b: [[-1; TILE_D]; TILE_H],
                is_reset: false,
                is_update: true,
            };
            4
        ];
        steps[0].is_reset = true;
        steps[0].is_update = false;
        let proof = p3_uni_stark::prove::<AiPowStarkConfig, _>(
            &config,
            &MatmulCumsumChip,
            build_trace(&steps),
            &[],
        );
        p3_uni_stark::verify::<AiPowStarkConfig, _>(&config, &MatmulCumsumChip, &proof, &[])
            .expect("owned-runtime proof verifies");
        let encoding = bincode::config::standard();
        (
            bincode::serde::encode_to_vec(&commitment, encoding).expect("encode commitment"),
            bincode::serde::encode_to_vec(&proof, encoding).expect("encode proof"),
        )
    }

    #[test]
    fn proof_runtime_preserves_commitments_and_proofs_from_foreign_pools() {
        const CHILD: &str = "AI_POW_STACK_REGRESSION_CHILD";
        if std::env::var_os(CHILD).is_none() {
            let status = std::process::Command::new(
                std::env::current_exe().expect("test executable"),
            )
            .args([
                "--exact",
                "runtime::tests::proof_runtime_preserves_commitments_and_proofs_from_foreign_pools",
                "--nocapture", "--test-threads=1",
            ])
            .env(CHILD, "1")
            .env("RAYON_NUM_THREADS", "2")
            .status()
            .expect("start isolated stack regression");
            assert!(
                status.success(),
                "isolated stack regression failed: {status}"
            );
            return;
        }

        // Explicit initialization is safe here because this is an isolated
        // process, not an assumption about the rest of the test suite.
        rayon::ThreadPoolBuilder::new()
            .num_threads(2)
            .stack_size(2 * 1024 * 1024)
            .build_global()
            .expect("preexisting small-stack global pool");
        rayon::join(|| (), || ());
        prepare().expect("initialize owned workers");
        prepare().expect("reuse owned workers");
        let ordinary = try_install(commitment_and_proof).expect("ordinary caller");
        let foreign = rayon::ThreadPoolBuilder::new()
            .num_threads(2)
            .stack_size(2 * 1024 * 1024)
            .build()
            .expect("foreign small-stack local pool");
        for _ in 0..2 {
            let actual = foreign.install(|| {
                try_install(|| {
                    assert!(pool()
                        .expect("initialized pool")
                        .current_thread_index()
                        .is_some());
                    // Nested entry must neither deadlock nor select the caller's
                    // global/local workers for real proof and commitment work.
                    try_install(commitment_and_proof).expect("nested proof entry")
                })
                .expect("foreign proof entry")
            });
            assert_eq!(actual, ordinary, "commitment and proof bytes are invariant");
        }

        #[cfg(feature = "recursion")]
        {
            use crate::canonical::{canonical_program, BlockPublic};
            use crate::recursion::canonical_l0_program_commitment_vals;

            let params = ZkParams {
                noise_rank: 16,
                tile: 8,
                ..params()
            };
            let public = BlockPublic {
                tile_i: 0,
                tile_j: 0,
                kappa: [0x42; 32],
                s_a: [0x43; 32],
                s_b: [0x44; 32],
            };
            let program = canonical_program(&params, &public, 8192).expect("canonical program");
            let profile = CircuitConfig::TEST_PEARL;
            // No explicit runtime entry: exercise the automatic production
            // admission/preprocessing boundary from ordinary and foreign callers.
            let ordinary = canonical_l0_program_commitment_vals(&params, &profile, &program);
            let actual = foreign
                .install(|| canonical_l0_program_commitment_vals(&params, &profile, &program));
            assert_eq!(
                actual, ordinary,
                "canonical admission commitment is invariant"
            );
            let nested =
                try_install(|| canonical_l0_program_commitment_vals(&params, &profile, &program))
                    .expect("nested canonical preprocessing");
            assert_eq!(nested, ordinary);
        }
    }
}

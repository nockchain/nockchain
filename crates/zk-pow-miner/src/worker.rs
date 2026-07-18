//! Mining worker — wraps one `SerfThread` loaded with the miner kernel
//! (`assets/miner.jam` via `kernels-open-miner::KERNEL`).
//!
//! Each [`SerfWorker`] runs one mining attempt at a time. The pool
//! ([`crate::pool::Pool`]) orchestrates dispatch + supersede across N
//! workers.
//!
//! The [`Worker`] trait exists so the pool + run-loop can be unit-tested
//! with a fast `StubWorker` (in `crate::pool`'s tests) without spinning
//! up a real Nock VM.

use async_trait::async_trait;
use kernels_open_miner::KERNEL;
use nockapp::kernel::form::SerfThread;
use nockapp::nockapp::wire::Wire;
use nockapp::noun::slab::NounSlab;
use nockapp::noun::AtomExt;
use nockapp::save::SaveableCheckpoint;
use nockapp::utils::NOCK_STACK_SIZE_TINY;
use nockchain_math::noun_ext::NounMathExtHandle;
use nockchain_math::structs::HoonList;
use nockchain_mining_common::MiningCandidate;
use nockvm::interpreter::NockCancelToken;
use nockvm::jets::hot::HotEntry;
use nockvm::noun::{Atom, D, T};
use rand::Rng;
use thiserror::Error;
use tracing::debug;
use zkvm_jetpack::form::belt::PRIME;

use crate::wire::ZkPowMinerWire;

pub type WorkerId = u64;

#[derive(Error, Debug)]
pub enum WorkerError {
    #[error("failed to spawn SerfThread: {0}")]
    SerfSpawn(String),
    #[error("serf poke failed: {0}")]
    Poke(String),
    #[error("decoding mine-result effect: {0}")]
    Decode(&'static str),
    #[error("miner kernel returned no mine-result effect")]
    NoMineResult,
}

/// Outcome of one mining attempt.
pub enum MineResult {
    /// The proof's digest cleared the target. `poke_slab` is the
    /// `[%command %pow %dumb-zkpow prf dig header nonce]` cause the caller
    /// pokes the *node*'s main kernel with (via `ZkPowMinerWire::Mined`).
    /// The `%dumb-zkpow` tag is the inner variant of the consensus kernel's
    /// `pow-variant` tagged union (see hoon/apps/dumbnet/lib/types.hoon).
    /// `hash_slab` is the digest as a tip5 5-tuple atom (the natural
    /// next-nonce seed if the caller wants to keep mining on the same
    /// candidate after submitting).
    Success {
        hash_slab: NounSlab,
        poke_slab: NounSlab,
    },
    /// Proof digest did not clear the target. `next_nonce` is the digest
    /// returned by the miner kernel — to be used as the nonce on the
    /// next attempt for the same candidate. This is how the old driver
    /// stepped through the nonce space; cribbed verbatim.
    Retry { next_nonce: NounSlab },
}

#[async_trait]
pub trait Worker: Send + Sync + 'static {
    fn id(&self) -> WorkerId;
    /// Signal the worker to abort its current `mine_attempt`. The
    /// attempt's future will resolve with `Err(WorkerError::Poke(...))`
    /// shortly after (the underlying Nock interpreter polls the
    /// cancel flag at branch / opcode boundaries).
    fn cancel(&self);
    /// Run one mining attempt. The caller pre-builds the `[version
    /// header nonce target pow-len]` poke slab via [`build_candidate_poke`]
    /// (the candidate isn't shared cross-thread because `NounSlab` is
    /// `Send` but not `Sync`). Returns `Ok(MineResult)` for success or
    /// retry; `Err` for a serf-level failure (e.g., cancellation,
    /// crash). Callers treat both success and retry as "this worker is
    /// now free; respawn it on the current candidate".
    async fn mine_attempt(&self, poke: NounSlab) -> Result<MineResult, WorkerError>;
}

/// Production worker: holds one `SerfThread` running the miner kernel.
pub struct SerfWorker {
    id: WorkerId,
    serf: SerfThread<SaveableCheckpoint>,
    cancel: NockCancelToken,
}

impl SerfWorker {
    pub async fn spawn(id: WorkerId, hot_state: Vec<HotEntry>) -> Result<Self, WorkerError> {
        let kernel = Vec::from(KERNEL);
        let test_jets_str = std::env::var("NOCK_TEST_JETS").unwrap_or_default();
        let test_jets = nockapp::kernel::boot::parse_test_jets(test_jets_str.as_str());
        let serf = SerfThread::<SaveableCheckpoint>::new(
            kernel,
            None,
            hot_state,
            NOCK_STACK_SIZE_TINY,
            None::<nockapp::kernel::form::PmaConfig>,
            test_jets,
            Default::default(),
        )
        .await
        .map_err(|e| WorkerError::SerfSpawn(format!("{e}")))?;
        let cancel = serf.cancel_token.clone();
        Ok(Self { id, serf, cancel })
    }
}

#[async_trait]
impl Worker for SerfWorker {
    fn id(&self) -> WorkerId {
        self.id
    }
    fn cancel(&self) {
        self.cancel.cancel();
    }
    async fn mine_attempt(&self, poke: NounSlab) -> Result<MineResult, WorkerError> {
        debug!(worker_id = self.id, "poking miner serf with candidate");
        let result_slab = self
            .serf
            .poke(ZkPowMinerWire::Candidate.to_wire(), poke)
            .await
            .map_err(|e| WorkerError::Poke(format!("{e}")))?;
        decode_mine_result(result_slab)
    }
}

/// Generate a fresh random nonce shaped as a Tip5 noun-digest (5-tuple
/// of Goldilocks belts). Mirrors the old in-tree driver's nonce gen
/// (`crates/nockchain/src/mining.rs::start_mining_attempt`).
pub fn random_nonce() -> NounSlab {
    let mut rng = rand::rng();
    let mut slab = NounSlab::new();
    let mut cell = <Atom as AtomExt>::from_value(&mut slab, rng.random::<u64>() % PRIME)
        .expect("u64 fits in atom")
        .as_noun();
    for _ in 1..5 {
        let atom = <Atom as AtomExt>::from_value(&mut slab, rng.random::<u64>() % PRIME)
            .expect("u64 fits in atom")
            .as_noun();
        cell = T(&mut slab, &[atom, cell]);
    }
    slab.set_root(cell);
    slab
}

/// Build the miner-kernel poke cause: `[version header nonce target pow-len]`.
/// Matches the `cause` schema in `hoon/apps/dumbnet/miner.hoon:19–23`.
/// Post-h-zoon: cross-slab noun reads must be bound to the source slab's
/// `NounSpace` via `copy_into(noun, &space)`.
pub fn build_candidate_poke(candidate: &MiningCandidate, nonce_slab: NounSlab) -> NounSlab {
    use nockvm::noun::NounAllocator;
    let version_space = candidate.version.noun_space();
    let header_space = candidate.block_header.noun_space();
    let nonce_space = nonce_slab.noun_space();
    let target_space = candidate.target.noun_space();
    let mut slab = NounSlab::new();
    let version = slab.copy_into(unsafe { *candidate.version.root() }, &version_space);
    let header = slab.copy_into(unsafe { *candidate.block_header.root() }, &header_space);
    let nonce = slab.copy_into(unsafe { *nonce_slab.root() }, &nonce_space);
    let target = slab.copy_into(unsafe { *candidate.target.root() }, &target_space);
    let cause = T(
        &mut slab,
        &[version, header, nonce, target, D(candidate.pow_len)],
    );
    slab.set_root(cause);
    slab
}

/// Decode the miner kernel's emitted effect list, looking for the first
/// `[%mine-result ?(%& %|) ...]` effect.
pub(crate) fn decode_mine_result(slab: NounSlab) -> Result<MineResult, WorkerError> {
    use nockvm::noun::NounAllocator;
    let space = slab.noun_space();
    let root = unsafe { *slab.root() };
    let effects = HoonList::try_from(root, &space)
        .map_err(|_| WorkerError::Decode("effect-list not a HoonList"))?;
    let mine_result_tail = effects
        .filter_map(|effect| {
            if effect.is_atom() {
                None
            } else {
                let effect_cell = effect.in_space(&space).as_cell().ok()?;
                if effect_cell.head().eq_bytes("mine-result") {
                    Some(effect_cell.tail().noun())
                } else {
                    None
                }
            }
        })
        .next()
        .ok_or(WorkerError::NoMineResult)?;
    let [res, tail] = mine_result_tail
        .in_space(&space)
        .uncell::<2>()
        .map_err(|_| WorkerError::Decode("mine-result tail not a 2-cell"))?
        .map(|h| h.noun());
    if unsafe { res.raw_equals(&D(0)) } {
        // success: tail = [hash poke]
        let [hash, poke] = tail
            .in_space(&space)
            .uncell::<2>()
            .map_err(|_| WorkerError::Decode("success tail not [hash poke]"))?
            .map(|h| h.noun());
        let mut hash_slab = NounSlab::new();
        let h = hash_slab.copy_into(hash, &space);
        hash_slab.set_root(h);
        let mut poke_slab = NounSlab::new();
        let p = poke_slab.copy_into(poke, &space);
        poke_slab.set_root(p);
        Ok(MineResult::Success {
            hash_slab,
            poke_slab,
        })
    } else {
        // retry: tail = the next nonce
        let mut next_nonce = NounSlab::new();
        let n = next_nonce.copy_into(tail, &space);
        next_nonce.set_root(n);
        Ok(MineResult::Retry { next_nonce })
    }
}

#[cfg(test)]
mod tests {
    use nockchain_mining_common::MiningCandidateKind;
    use nockvm_macros::tas;

    use super::*;

    /// Build a "mine-result" head atom — too long for the 8-char tas!
    /// macro, so use an indirect-atom string.
    fn mine_result_head(slab: &mut NounSlab) -> nockvm::noun::Noun {
        <Atom as AtomExt>::from_value(slab, "mine-result")
            .expect("mine-result atom")
            .as_noun()
    }

    /// Build a synthetic %mine-result success effect list shaped like
    /// what the production miner kernel emits post-Hoon-generalization:
    ///   `[[%mine-result %& [d1 d2 d3 d4 d5] [%command %pow %dumb-zkpow 101]] 0]`
    /// The decoder is shape-agnostic on the inner poke payload (it just
    /// copies the cell out) — `101` is a placeholder for what would be
    /// the prf/dig/header/nonce 4-tuple in production. The `%dumb-zkpow`
    /// tag is the inner pow-variant discriminator.
    fn synth_success_effect_list() -> NounSlab {
        let mut slab = NounSlab::new();
        let head = mine_result_head(&mut slab);
        let yes = D(0); // %&
        let digest = T(&mut slab, &[D(11), D(22), D(33), D(44), D(55)]);
        // `dumb-zkpow` is 10 bytes — too long for the 8-byte `tas!` macro.
        let dumb_zkpow_tag = <Atom as AtomExt>::from_value(&mut slab, "dumb-zkpow")
            .expect("dumb-zkpow atom")
            .as_noun();
        let poke = T(
            &mut slab,
            &[D(tas!(b"command")), D(tas!(b"pow")), dumb_zkpow_tag, D(101)],
        );
        let success_tail = T(&mut slab, &[yes, digest, poke]);
        let effect = T(&mut slab, &[head, success_tail]);
        let effect_list = T(&mut slab, &[effect, D(0)]);
        slab.set_root(effect_list);
        slab
    }

    /// Build a synthetic %mine-result retry effect list:
    ///   `[[%mine-result %| [n1 n2 n3 n4 n5]] 0]`
    fn synth_retry_effect_list() -> NounSlab {
        let mut slab = NounSlab::new();
        let head = mine_result_head(&mut slab);
        let no = D(1); // %|
        let next_nonce = T(&mut slab, &[D(101), D(102), D(103), D(104), D(105)]);
        let retry_tail = T(&mut slab, &[no, next_nonce]);
        let effect = T(&mut slab, &[head, retry_tail]);
        let effect_list = T(&mut slab, &[effect, D(0)]);
        slab.set_root(effect_list);
        slab
    }

    #[test]
    fn random_nonce_is_a_5_tuple_of_atoms() {
        use nockvm::noun::NounAllocator;
        let n = random_nonce();
        let space = n.noun_space();
        let root = unsafe { *n.root() };
        // Walk the right spine: 5 atoms on the left.
        let mut node = root;
        let mut count = 0;
        loop {
            match node.in_space(&space).as_cell() {
                Ok(cell) => {
                    assert!(cell.head().is_atom(), "spine head is an atom");
                    node = cell.tail().noun();
                    count += 1;
                }
                Err(_) => {
                    assert!(node.is_atom(), "rightmost is an atom");
                    count += 1;
                    break;
                }
            }
        }
        assert_eq!(count, 5, "Tip5 nonce is exactly 5 belts");
    }

    #[test]
    fn build_candidate_poke_has_correct_shape() {
        use nockvm::noun::NounAllocator;
        // Synthesise a minimal MiningCandidate (the type's fields are
        // pub, so we can construct directly in the test).
        let mut version = NounSlab::new();
        version.set_root(D(0));
        let mut block_header = NounSlab::new();
        let h = T(&mut block_header, &[D(0), D(0), D(0), D(0), D(0)]);
        block_header.set_root(h);
        let mut target = NounSlab::new();
        target.set_root(D(0xFFFF_FFFF));
        let candidate = MiningCandidate {
            kind: MiningCandidateKind::Zk,
            version,
            block_header,
            target,
            pow_len: 7,
        };
        let nonce = random_nonce();
        let poke = build_candidate_poke(&candidate, nonce);
        let space = poke.noun_space();

        // Should be [version=0 header=[0 0 0 0 0] nonce=[..] target=0xFFFFFFFF pow_len=7]
        let root = unsafe { *poke.root() };
        let cell = root.in_space(&space).as_cell().expect("poke is a cell");
        // head = version
        let v = cell
            .head()
            .as_atom()
            .expect("version atom")
            .as_u64()
            .unwrap();
        assert_eq!(v, 0, "version is %0");
        // Walk right spine 5 deep; rightmost is pow_len = 7.
        let mut node = cell.tail().noun();
        for _ in 0..3 {
            node = node
                .in_space(&space)
                .as_cell()
                .expect("spine cell")
                .tail()
                .noun();
        }
        // Now node should be the pow_len atom.
        let pl = node
            .in_space(&space)
            .as_atom()
            .expect("pow_len atom")
            .as_u64()
            .unwrap();
        assert_eq!(pl, 7, "pow_len is at the end of the cause spine");
    }

    #[test]
    fn decode_success_round_trips() {
        use nockvm::noun::NounAllocator;
        let slab = synth_success_effect_list();
        match decode_mine_result(slab).expect("decode") {
            MineResult::Success {
                hash_slab,
                poke_slab,
            } => {
                let hash_space = hash_slab.noun_space();
                let h = unsafe { *hash_slab.root() };
                let hc = h.in_space(&hash_space).as_cell().expect("hash is a cell");
                assert_eq!(hc.head().as_atom().unwrap().as_u64().unwrap(), 11);
                let poke_space = poke_slab.noun_space();
                let p = unsafe { *poke_slab.root() };
                let pc = p.in_space(&poke_space).as_cell().expect("poke is a cell");
                assert!(pc.head().eq_bytes("command"));
            }
            other => match other {
                MineResult::Retry { .. } => panic!("expected Success, got Retry"),
                _ => unreachable!(),
            },
        }
    }

    #[test]
    fn decode_retry_round_trips() {
        use nockvm::noun::NounAllocator;
        let slab = synth_retry_effect_list();
        match decode_mine_result(slab).expect("decode") {
            MineResult::Retry { next_nonce } => {
                let space = next_nonce.noun_space();
                let n = unsafe { *next_nonce.root() };
                let nc = n.in_space(&space).as_cell().expect("nonce is a cell");
                assert_eq!(nc.head().as_atom().unwrap().as_u64().unwrap(), 101);
            }
            MineResult::Success { .. } => panic!("expected Retry, got Success"),
        }
    }

    #[test]
    fn decode_errors_on_no_mine_result_effect() {
        let mut slab = NounSlab::new();
        let other = T(&mut slab, &[D(tas!(b"otherefx")), D(0)]);
        let list = T(&mut slab, &[other, D(0)]);
        slab.set_root(list);
        match decode_mine_result(slab) {
            Err(WorkerError::NoMineResult) => {}
            Err(e) => panic!("expected NoMineResult, got {e:?}"),
            Ok(_) => panic!("expected NoMineResult, got Ok"),
        }
    }

    /// Integration test: spawn a real SerfWorker, hand it a trivial-target
    /// candidate (target = max bignum so any digest passes), assert
    /// `MineResult::Success` on the first attempt.
    ///
    /// Marked `#[ignore]` because spawning a Nock VM + running the STARK
    /// is heavy (~10s+ wall clock). Run with:
    ///   cargo test -p zk-pow-miner --lib -- --ignored serf_worker
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    #[ignore]
    async fn serf_worker_mines_trivial_target() {
        use ibig::UBig;
        use zkvm_jetpack::hot::produce_prover_hot_state;

        let hot_state = produce_prover_hot_state();
        let worker = SerfWorker::spawn(0, hot_state).await.expect("spawn worker");

        // Target = 2^400 — comfortably above max-tip5-atom (the merged
        // 5-belt tip5 hash atom; the chain checks `(lte proof-hash
        // max-tip5-atom)` first, then `(lte proof-hash target)`). With
        // target this large, every proof passes.
        let candidate = synth_trivial_candidate(UBig::from(1u64) << 400, 2);
        let mut attempts = 0u32;
        let mut nonce = random_nonce();
        let started = std::time::Instant::now();
        // Allow a small handful of attempts as a guardrail — in
        // practice with target=2^400 the very first attempt should hit.
        let result = loop {
            attempts += 1;
            let poke = build_candidate_poke(&candidate, nonce);
            let r = worker.mine_attempt(poke).await.expect("mine_attempt");
            match r {
                MineResult::Success { .. } => break r,
                MineResult::Retry { next_nonce } => {
                    if attempts >= 4 {
                        panic!(
                            "trivial-target candidate should succeed within 4 attempts; \
                             got {attempts} retries"
                        );
                    }
                    nonce = next_nonce;
                }
            }
        };
        let elapsed = started.elapsed();
        eprintln!("serf_worker_mines_trivial_target: {attempts} attempt(s) in {elapsed:?}");
        assert!(matches!(result, MineResult::Success { .. }));
    }

    /// Helper: synth a trivial-difficulty candidate (max-bignum target).
    #[cfg(test)]
    fn synth_trivial_candidate(target_value: ibig::UBig, pow_len: u64) -> MiningCandidate {
        let mut version = NounSlab::new();
        version.set_root(D(0)); // %0
        let mut block_header = NounSlab::new();
        let h = T(&mut block_header, &[D(0), D(0), D(0), D(0), D(0)]);
        block_header.set_root(h);
        let mut target = NounSlab::new();
        let t = bignum_to_noun(&mut target, &target_value);
        target.set_root(t);
        MiningCandidate {
            kind: MiningCandidateKind::Zk,
            version,
            block_header,
            target,
            pow_len,
        }
    }

    /// Helper: serialise an `ibig::UBig` as `[%bn list-of-u32-belts]`.
    /// Mirrors the helper in `crates/nockchain/tests/open_prover_bench.rs`.
    #[cfg(test)]
    fn bignum_to_noun(slab: &mut NounSlab, value: &ibig::UBig) -> nockvm::noun::Noun {
        let mut list = D(0);
        let bytes = value.to_le_bytes();
        for chunk in bytes.chunks(4).rev() {
            let mut padded = [0u8; 4];
            padded[..chunk.len()].copy_from_slice(chunk);
            let chunk = u64::from(u32::from_le_bytes(padded));
            let atom = <Atom as AtomExt>::from_value(slab, chunk)
                .expect("atom")
                .as_noun();
            list = T(slab, &[atom, list]);
        }
        T(slab, &[D(tas!(b"bn")), list])
    }
}

use std::error::Error;
use std::time::Instant;

use ibig::UBig;
use kernels_open_miner::KERNEL;
use nockapp::kernel::boot::{parse_test_jets, TraceOpts};
use nockapp::kernel::form::{PmaConfig, SerfThread};
use nockapp::noun::slab::NounSlab;
use nockapp::save::SaveableCheckpoint;
use nockapp::utils::NOCK_STACK_SIZE_TINY;
use nockapp::wire::WireRepr;
use nockapp::AtomExt;
use nockchain_math::noun_ext::NounMathExtHandle;
use nockchain_math::structs::HoonList;
use nockvm::noun::{Atom, Noun, NounAllocator, D, T};
use nockvm_macros::tas;
use zkvm_jetpack::hot::produce_prover_hot_state;

fn tip5_to_noun(slab: &mut NounSlab, values: [u64; 5]) -> Result<Noun, Box<dyn Error>> {
    let mut tuple = Vec::with_capacity(values.len());
    for value in values {
        let atom = <Atom as AtomExt>::from_value(slab, value)
            .map_err(|e| Box::new(e) as Box<dyn Error>)?;
        tuple.push(atom.as_noun());
    }
    Ok(T(slab, &tuple))
}
fn bignum_to_noun(slab: &mut NounSlab, value: &UBig) -> Result<Noun, Box<dyn Error>> {
    let mut list = D(0);
    let bytes = value.to_le_bytes();
    for chunk in bytes.chunks(4).rev() {
        let mut padded = [0u8; 4];
        padded[..chunk.len()].copy_from_slice(chunk);
        let chunk = u64::from(u32::from_le_bytes(padded));
        let atom = <Atom as AtomExt>::from_value(slab, chunk)
            .map_err(|e| Box::new(e) as Box<dyn Error>)?;
        list = T(slab, &[atom.as_noun(), list]);
    }
    Ok(T(slab, &[D(tas!(b"bn")), list]))
}

#[tokio::test(flavor = "current_thread")]
async fn benchmark_open_prover_single_attempt() -> Result<(), Box<dyn Error>> {
    let kernel_bytes = Vec::from(KERNEL);
    let hot_state = produce_prover_hot_state();
    let test_jets = parse_test_jets("");

    let serf = SerfThread::<SaveableCheckpoint>::new(
        kernel_bytes,
        None,
        hot_state,
        NOCK_STACK_SIZE_TINY,
        None::<PmaConfig>,
        test_jets,
        TraceOpts::default(),
    )
    .await
    .map_err(|e| Box::new(e) as Box<dyn Error>)?;

    let mut poke_slab = NounSlab::new();
    let commitment = tip5_to_noun(&mut poke_slab, [1, 2, 3, 4, 5])?;
    let nonce = tip5_to_noun(&mut poke_slab, [0, 0, 0, 0, 0])?;
    let target = bignum_to_noun(&mut poke_slab, &(UBig::from(1u64) << 400))?;
    let poke_noun = T(&mut poke_slab, &[D(0), commitment, nonce, target, D(2)]);
    poke_slab.set_root(poke_noun);

    let start = Instant::now();
    let poke_result = serf
        .poke(
            WireRepr::new("zk-pow-miner", 1, vec!["candidate".into()]),
            poke_slab,
        )
        .await
        .map_err(|e| Box::new(e) as Box<dyn Error>)?;
    println!(
        "Open prover single proof attempt completed in {:.3?}",
        start.elapsed()
    );

    let poke_space = poke_result.noun_space();
    let root = unsafe { *poke_result.root() };
    let effects =
        HoonList::try_from(root, &poke_space).map_err(|e| Box::new(e) as Box<dyn Error>)?;
    let mut success = false;
    for effect in effects {
        let Ok(effect_cell) = effect.in_space(&poke_space).as_cell() else {
            continue;
        };
        if !effect_cell.head().eq_bytes("mine-result") {
            continue;
        }
        let [status, response] = effect_cell
            .tail()
            .uncell::<2>()
            .map_err(|e| Box::new(e) as Box<dyn Error>)?;
        let status = status
            .as_atom()
            .map_err(|e| Box::new(e) as Box<dyn Error>)?
            .as_u64()
            .map_err(|e| Box::new(e) as Box<dyn Error>)?;
        if status != 0 {
            continue;
        }
        let [_hash, command] = response
            .uncell::<2>()
            .map_err(|e| Box::new(e) as Box<dyn Error>)?;
        let [head, pow, variant, _proof, _digest, _commitment, _nonce] = command
            .uncell::<7>()
            .map_err(|e| Box::new(e) as Box<dyn Error>)?;
        assert!(head.eq_bytes("command"));
        assert!(pow.eq_bytes("pow"));
        assert!(variant.eq_bytes("dumb-zkpow"));
        success = true;
        break;
    }

    assert!(
        success,
        "open prover did not return a tagged ZK proof command"
    );
    serf.cancel_token.cancel();
    Ok(())
}

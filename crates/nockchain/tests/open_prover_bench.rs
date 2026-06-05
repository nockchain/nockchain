use std::error::Error;
use std::time::Instant;

use ibig::UBig;
use kernels_open_miner::KERNEL;
use nockapp::kernel::boot::{parse_test_jets, TraceOpts};
use nockapp::kernel::form::SerfThread;
use nockapp::noun::slab::NounSlab;
use nockapp::noun::NounExt;
use nockapp::save::SaveableCheckpoint;
use nockapp::utils::NOCK_STACK_SIZE_TINY;
use nockapp::wire::Wire;
use nockapp::AtomExt;
use nockchain::mining::MiningWire;
use nockchain_math::noun_ext::NounMathExt;
use nockchain_math::structs::HoonList;
use nockchain_types::BlockchainConstants;
use nockvm::noun::{Atom, Noun, D, T, YES};
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

async fn send_set_mining_key(serf: &SerfThread<SaveableCheckpoint>) -> Result<(), Box<dyn Error>> {
    let mut slab = NounSlab::new();
    let head = D(tas!(b"command"));
    let command = <Atom as AtomExt>::from_value(&mut slab, "set-mining-key")
        .map_err(|e| Box::new(e) as Box<dyn Error>)?
        .as_noun();
    let pubkey = <Atom as AtomExt>::from_value(&mut slab, "open-prover-test-pubkey")
        .map_err(|e| Box::new(e) as Box<dyn Error>)?
        .as_noun();
    let poke = T(&mut slab, &[head, command, pubkey]);
    slab.set_root(poke);

    serf.poke(MiningWire::SetPubKey.to_wire(), slab)
        .await
        .map_err(|e| Box::new(e) as Box<dyn Error>)?;
    Ok(())
}

async fn send_enable_mining(
    serf: &SerfThread<SaveableCheckpoint>,
) -> Result<NounSlab, Box<dyn Error>> {
    let mut slab = NounSlab::new();
    let head = D(tas!(b"command"));
    let command = <Atom as AtomExt>::from_value(&mut slab, "enable-mining")
        .map_err(|e| Box::new(e) as Box<dyn Error>)?
        .as_noun();
    let poke = T(&mut slab, &[head, command, YES]);
    slab.set_root(poke);

    serf.poke(MiningWire::Enable.to_wire(), slab)
        .await
        .map_err(|e| Box::new(e) as Box<dyn Error>)
}

fn extract_mine_start(slab: &NounSlab) -> Result<(Noun, Noun, Noun, Noun), Box<dyn Error>> {
    let root = unsafe { *slab.root() };
    let effects = HoonList::try_from(root).map_err(|e| Box::new(e) as Box<dyn Error>)?;
    for effect in effects {
        if let Ok(effect_cell) = effect.as_cell() {
            if effect_cell.head().eq_bytes("mine") {
                let mine_tail = effect_cell.tail();
                let mine_cell = mine_tail
                    .as_cell()
                    .map_err(|e| Box::new(e) as Box<dyn Error>)?;
                let mine_start = mine_cell.head();
                let [version, header, target, pow_len] = mine_start
                    .uncell::<4>()
                    .map_err(|e| Box::new(e) as Box<dyn Error>)?;
                return Ok((version, header, target, pow_len));
            }
        }
    }
    Err(Box::<dyn Error>::from(
        "kernel did not emit %mine start".to_owned(),
    ))
}

enum CandidateData {
    Kernel {
        version: Noun,
        header: Noun,
        target: Noun,
        pow_len: u64,
    },
    Synthetic,
}

#[tokio::test(flavor = "current_thread")]
async fn benchmark_open_prover_single_attempt() -> Result<(), Box<dyn Error>> {
    // Prepare a standalone miner kernel instance with the open-source prover hot state.
    let kernel_bytes = Vec::from(KERNEL);
    let hot_state = produce_prover_hot_state();
    let test_jets = parse_test_jets("");

    let serf = SerfThread::<SaveableCheckpoint>::new(
        kernel_bytes,
        None,
        hot_state,
        NOCK_STACK_SIZE_TINY,
        test_jets,
        TraceOpts::default(),
    )
    .await
    .map_err(|e| Box::new(e) as Box<dyn Error>)?;

    // Initialize mining state so the kernel provides candidate metadata.
    send_set_mining_key(&serf).await?;
    let enable_result = send_enable_mining(&serf).await?;
    let candidate_data = match extract_mine_start(&enable_result) {
        Ok((version_noun, header_noun, target_noun, pow_len_noun)) => {
            let pow_len_value = pow_len_noun
                .as_atom()
                .map_err(|e| Box::new(e) as Box<dyn Error>)?
                .as_u64()
                .map_err(|e| Box::new(e) as Box<dyn Error>)?;
            CandidateData::Kernel {
                version: version_noun,
                header: header_noun,
                target: target_noun,
                pow_len: pow_len_value,
            }
        }
        Err(err) => {
            println!("WARNING: falling back to synthetic mining candidate: {err}");
            CandidateData::Synthetic
        }
    };

    // Build a mining candidate poke: [version header nonce target pow_len].
    let mut poke_slab = NounSlab::new();
    let (version, header, target, pow_len_value) = match candidate_data {
        CandidateData::Kernel {
            version,
            header,
            target,
            pow_len,
        } => (
            poke_slab.copy_into(version),
            poke_slab.copy_into(header),
            poke_slab.copy_into(target),
            pow_len,
        ),
        CandidateData::Synthetic => {
            let max_target = BlockchainConstants::new().max_target_atom;
            (
                D(1),
                tip5_to_noun(&mut poke_slab, [1, 2, 3, 4, 5])?,
                bignum_to_noun(&mut poke_slab, &max_target)?,
                BlockchainConstants::DEFAULT_POW_LEN,
            )
        }
    };
    let nonce = tip5_to_noun(&mut poke_slab, [0, 0, 0, 0, 0])?;
    let pow_len = D(pow_len_value);
    let poke_noun = T(&mut poke_slab, &[version, header, nonce, target, pow_len]);
    poke_slab.set_root(poke_noun);

    // Execute a single proof attempt and record the elapsed time.
    let start = Instant::now();
    let poke_result = serf
        .poke(MiningWire::Candidate.to_wire(), poke_slab)
        .await
        .map_err(|e| Box::new(e) as Box<dyn Error>)?;
    let elapsed = start.elapsed();
    println!(
        "Open prover single proof attempt completed in {:.3?}",
        elapsed
    );

    // Verify we received a successful %mine-result effect.
    let root = unsafe { *poke_result.root() };
    let mut success = false;
    let effects = HoonList::try_from(root).map_err(|e| Box::new(e) as Box<dyn Error>)?;
    for effect in effects {
        if let Ok(effect_cell) = effect.as_cell() {
            if effect_cell.head().eq_bytes("mine-result") {
                if let Ok([status, _rest]) = effect_cell.tail().uncell() {
                    if let Ok(status_atom) = status.as_atom() {
                        if let Ok(value) = status_atom.as_u64() {
                            if value == 0 {
                                success = true;
                                break;
                            }
                        }
                    }
                }
            }
        }
    }

    assert!(success, "open prover mine-result did not succeed");
    serf.cancel_token.cancel();
    Ok(())
}

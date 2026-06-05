#![allow(clippy::result_large_err)]

use std::collections::{HashSet, VecDeque};
use std::error::Error;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use clap::{ColorChoice, Parser};
use kernels_open_dumb::KERNEL as NOCKCHAIN_KERNEL;
use kernels_open_miner::KERNEL as MINER_KERNEL;
use libp2p::PeerId;
use nockapp::kernel::boot::{self, NockStackSize, PmaSize, TraceOpts};
use nockapp::kernel::form::{PmaCopyDetail, PmaTimingSample, SerfThread};
use nockapp::noun::slab::NounSlab;
use nockapp::save::SaveableCheckpoint;
use nockapp::utils::{make_tas, NOCK_STACK_SIZE_TINY};
use nockapp::wire::{SystemWire, Wire};
use nockapp::{AtomExt, Bytes, NockApp, NockAppError};
use nockchain::mining::MiningWire;
use nockchain::setup::{self, fakenet_blockchain_constants, DEFAULT_GENESIS_BLOCK_HEIGHT};
use nockchain_libp2p_io::driver::Libp2pWire;
use nockchain_libp2p_io::tip5_util::tip5_hash_to_base58_stack;
use nockvm::noun::{Atom, Noun, NounAllocator, NounSpace, D, NO, T, YES};
use nockvm_macros::tas;
use noun_serde::NounEncode;
use rand::Rng;
use tempfile::TempDir;
use tracing::{info, warn};
use zkvm_jetpack::form::belt::PRIME;
use zkvm_jetpack::form::noun_ext::NounMathExt;
use zkvm_jetpack::form::structs::HoonList;
use zkvm_jetpack::hot::produce_prover_hot_state;

const DEFAULT_BLOCKS: usize = 100;
const DEFAULT_POW_LEN: u64 = 64;
const DEFAULT_LOG_DIFFICULTY: u64 = 2;
const DEFAULT_MINING_PKH: &str = "9yPePjfWAdUnzaQKyxcRXKRa5PpUzKKEwtpECBZsUYt9Jd7egSDEWoV";
const DEFAULT_V0_PUBKEY: &str = "2cPnE4Z9RevhTv9is9Hmc1amFubEFbUxzCV2Fxb9GxevJstV5VG92oYt6Sai3d3NjLFcsuVXSLx9hikMbD1agv9M267TVw3hV9MCpMfEnGo5LYtjJ7jPyHg8SERPjJRCWTgZ";

const GENESIS_POW_64_BEX_2: &[u8] = include_bytes!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/jams/fakenet-genesis-pow-64-bex-2.jam"
));
const GENESIS_POW_64_BEX_5: &[u8] = include_bytes!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/jams/fakenet-genesis-pow-64-bex-5.jam"
));

#[derive(Parser, Debug)]
#[command(
    name = "bench-nockchain-kernel",
    about = "Kernel-only nockchain peer benchmark (mining + catch-up)."
)]
struct BenchArgs {
    #[arg(long, default_value_t = DEFAULT_BLOCKS)]
    blocks: usize,
    #[arg(long, default_value_t = DEFAULT_POW_LEN)]
    pow_len: u64,
    #[arg(long, default_value_t = DEFAULT_LOG_DIFFICULTY)]
    log_difficulty: u64,
    #[arg(long, default_value_t = false)]
    skip_mining: bool,
    #[arg(long)]
    genesis_jam: Option<PathBuf>,
    #[arg(long, value_enum, default_value_t = NockStackSize::Medium)]
    stack_size: NockStackSize,
}

#[derive(Clone)]
struct MiningCandidate {
    version: NounSlab,
    header: NounSlab,
    target: NounSlab,
    pow_len: u64,
}

struct Miner {
    serf: SerfThread<SaveableCheckpoint>,
    next_nonce: Option<NounSlab>,
    total_attempts: u64,
}

impl Miner {
    async fn new() -> Result<Self, Box<dyn Error>> {
        let hot_state = produce_prover_hot_state();
        let test_jets_str = std::env::var("NOCK_TEST_JETS").unwrap_or_default();
        let test_jets = boot::parse_test_jets(test_jets_str.as_str());
        let serf = SerfThread::<SaveableCheckpoint>::new(
            Vec::from(MINER_KERNEL),
            None,
            hot_state,
            NOCK_STACK_SIZE_TINY,
            None,
            test_jets,
            TraceOpts::default(),
        )
        .await?;
        Ok(Self {
            serf,
            next_nonce: None,
            total_attempts: 0,
        })
    }

    async fn mine_candidate(
        &mut self,
        candidate: &MiningCandidate,
    ) -> Result<NounSlab, Box<dyn Error>> {
        let mut attempts = 0u64;
        let mut nonce = self.next_nonce.take();
        loop {
            attempts += 1;
            let nonce_slab = nonce.take().unwrap_or_else(random_nonce);
            let poke_slab = create_candidate_poke(candidate, &nonce_slab);
            let result = self
                .serf
                .poke(MiningWire::Candidate.to_wire(), poke_slab)
                .await?;
            match parse_mine_result(result)? {
                MineResult::Success { poke, next_nonce } => {
                    self.next_nonce = Some(next_nonce);
                    self.total_attempts += attempts;
                    return Ok(poke);
                }
                MineResult::Retry { next_nonce } => {
                    nonce = Some(next_nonce);
                }
            }
        }
    }
}

enum MineResult {
    Retry {
        next_nonce: NounSlab,
    },
    Success {
        poke: NounSlab,
        next_nonce: NounSlab,
    },
}

struct Poke {
    wire: nockapp::wire::WireRepr,
    noun: NounSlab,
}

struct MiningOutput {
    gossips: Vec<NounSlab>,
    duration: Duration,
    total_attempts: u64,
    poke_timestamps: Vec<Duration>,
}

struct CatchupOutput {
    duration: Duration,
    poke_timestamps: Vec<Duration>,
}

#[derive(Clone, Copy)]
struct TimedSample {
    idx: usize,
    value: Duration,
    ts: Duration,
}

#[derive(Clone, Copy)]
struct ValueSample {
    idx: usize,
    value_bytes: u64,
    ts: Duration,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    nockvm::check_endian();
    std::env::set_var("NOCKAPP_DISABLE_METRICS", "1");
    std::env::set_var("GNORT_DISABLE", "1");
    std::env::set_var("NOCK_PMA_TIMING", "1");
    std::env::set_var("NOCK_PMA_TIMING_DETAIL", "1");

    let args = BenchArgs::parse();

    let boot_cli = boot::Cli {
        new: true,
        trace_opts: TraceOpts::default(),
        gc_interval: None,
        rotating_snapshot_interval_event_time: None,
        ephemeral: false,
        color: ColorChoice::Auto,
        state_jam: None,
        bootstrap_from_chkjam: None,
        export_state_jam: None,
        stack_size: args.stack_size,
        pma_initial_size: Some(PmaSize::from_words(args.stack_size.stack_words())),
        pma_reserved_size: None,
        data_dir: None,
        event_log_path: None,
        disable_fsync: false,
    };
    boot::init_default_tracing(&boot_cli);

    let genesis_bytes = load_genesis_bytes(&args)?;
    let genesis_id = genesis_block_id(&genesis_bytes)?;

    let (peer1_dir, mut peer1) = build_nockapp("bench-peer-1", boot_cli.clone()).await?;
    let (_peer2_dir, mut peer2) = build_nockapp("bench-peer-2", boot_cli).await?;

    let mut constants = fakenet_blockchain_constants(args.pow_len, args.log_difficulty);
    if args.skip_mining {
        constants.check_pow_flag = false;
    }

    let mut peer1_init = build_init_pokes(&constants, &genesis_bytes, true)?;
    let mut peer2_init = build_init_pokes(&constants, &genesis_bytes, false)?;
    let peer1_init_count = peer1_init.len();

    apply_init_pokes(&mut peer2, &mut peer2_init).await?;
    let _ = peer2.take_pma_timing_samples_detailed();

    let mut miner = if args.skip_mining {
        None
    } else {
        Some(Miner::new().await?)
    };
    let mining_output = run_mining_peer(
        &mut peer1, &mut miner, &mut peer1_init, peer1_init_count, args.blocks, &genesis_id,
    )
    .await?;

    let peer1_id = PeerId::random();
    let catchup_output = run_catchup_peer(&mut peer2, &mining_output.gossips, peer1_id).await?;

    print_summary(&args, &mining_output, catchup_output.duration);

    let mining_samples = peer1
        .take_pma_timing_samples_detailed()
        .map(|mut samples| {
            if samples.len() < peer1_init_count {
                warn!(
                    "bench: mining timing samples ({}) less than init pokes ({}); timing output may be incomplete",
                    samples.len(),
                    peer1_init_count
                );
                samples.clear();
            } else {
                samples.drain(..peer1_init_count);
            }
            samples
        });
    report_phase_timings(
        "mining_pokes", mining_samples, &mining_output.poke_timestamps,
    );

    let catchup_samples = peer2.take_pma_timing_samples_detailed();
    report_phase_timings(
        "catchup_pokes", catchup_samples, &catchup_output.poke_timestamps,
    );

    drop(peer1_dir);
    Ok(())
}

async fn build_nockapp(name: &str, cli: boot::Cli) -> Result<(TempDir, NockApp), Box<dyn Error>> {
    let temp_dir = TempDir::new()?;
    let hot_state = produce_prover_hot_state();
    let app = boot::setup::<nockapp::noun::slab::NockJammer>(
        NOCKCHAIN_KERNEL,
        cli,
        hot_state.as_slice(),
        name,
        Some(temp_dir.path().to_path_buf()),
    )
    .await?;
    Ok((temp_dir, app))
}

fn load_genesis_bytes(args: &BenchArgs) -> Result<Vec<u8>, Box<dyn Error>> {
    if let Some(path) = &args.genesis_jam {
        return Ok(std::fs::read(path)?);
    }
    match (args.pow_len, args.log_difficulty) {
        (2, 1) => Ok(setup::FAKENET_GENESIS_BLOCK.to_vec()),
        (64, 2) => Ok(GENESIS_POW_64_BEX_2.to_vec()),
        (64, 5) => Ok(GENESIS_POW_64_BEX_5.to_vec()),
        _ => Err(format!(
            "No built-in genesis jam for pow_len={} log_difficulty={}; supply --genesis-jam",
            args.pow_len, args.log_difficulty
        )
        .into()),
    }
}

fn genesis_block_id(genesis_bytes: &[u8]) -> Result<String, Box<dyn Error>> {
    let mut slab: NounSlab = NounSlab::new();
    let noun = slab.cue_into(Bytes::from(genesis_bytes.to_vec()))?;
    let space = slab.noun_space();
    let block_id = block_id_from_page(noun, &space)?;
    Ok(tip5_hash_to_base58_stack(&mut slab, block_id, &space)?)
}

fn build_init_pokes(
    constants: &setup::BlockchainConstants,
    genesis_bytes: &[u8],
    enable_mining: bool,
) -> Result<VecDeque<Poke>, Box<dyn Error>> {
    let mut pokes = VecDeque::new();

    pokes.push_back(Poke {
        wire: SystemWire.to_wire(),
        noun: make_set_constants_poke(constants),
    });
    pokes.push_back(Poke {
        wire: SystemWire.to_wire(),
        noun: make_set_genesis_seal_poke(setup::FAKENET_GENESIS_MESSAGE),
    });
    pokes.push_back(Poke {
        wire: SystemWire.to_wire(),
        noun: make_set_btc_data_poke(),
    });
    if enable_mining {
        pokes.push_back(Poke {
            wire: MiningWire::SetPubKey.to_wire(),
            noun: make_set_mining_key_poke(DEFAULT_V0_PUBKEY, DEFAULT_MINING_PKH),
        });
        pokes.push_back(Poke {
            wire: MiningWire::Enable.to_wire(),
            noun: make_enable_mining_poke(true),
        });
    }
    pokes.push_back(Poke {
        wire: SystemWire.to_wire(),
        noun: make_born_poke(),
    });
    pokes.push_back(Poke {
        wire: SystemWire.to_wire(),
        noun: setup::heard_fake_genesis_block(Some(genesis_bytes.to_vec()))?,
    });

    Ok(pokes)
}

async fn apply_init_pokes(
    nockapp: &mut NockApp,
    pokes: &mut VecDeque<Poke>,
) -> Result<(), Box<dyn Error>> {
    while let Some(poke) = pokes.pop_front() {
        let _ = nockapp.poke(poke.wire, poke.noun).await?;
    }
    Ok(())
}

async fn run_mining_peer(
    nockapp: &mut NockApp,
    miner: &mut Option<Miner>,
    pending: &mut VecDeque<Poke>,
    skip_pokes: usize,
    target_blocks: usize,
    genesis_id: &str,
) -> Result<MiningOutput, Box<dyn Error>> {
    let mut gossips = Vec::new();
    let mut seen_blocks: HashSet<String> = HashSet::new();
    let mut mining_started = false;
    let mut start = None;
    let mut total_pokes = 0usize;
    let mut phase_start = if skip_pokes == 0 {
        Some(Instant::now())
    } else {
        None
    };
    let mut poke_timestamps = Vec::new();

    while let Some(poke) = pending.pop_front() {
        if phase_start.is_none() && total_pokes == skip_pokes {
            phase_start = Some(Instant::now());
        }
        let effects = nockapp.poke(poke.wire, poke.noun).await?;
        total_pokes += 1;
        if let Some(start) = phase_start {
            if total_pokes > skip_pokes {
                poke_timestamps.push(start.elapsed());
            }
        }
        for effect in effects {
            if let Some(candidate) = parse_mine_effect(&effect)? {
                if gossips.len() >= target_blocks {
                    continue;
                }
                if !mining_started {
                    mining_started = true;
                    start = Some(Instant::now());
                }
                let mined_poke = match miner.as_mut() {
                    Some(miner) => miner.mine_candidate(&candidate).await?,
                    None => create_pow_poke(&candidate, &random_nonce()),
                };
                pending.push_back(Poke {
                    wire: MiningWire::Mined.to_wire(),
                    noun: mined_poke,
                });
                continue;
            }

            if let Some(mut gossip) = extract_gossip_data(&effect)? {
                if !mining_started {
                    continue;
                }
                if let Some((block_id, fact_poke)) = heard_block_fact(&mut gossip)? {
                    if block_id == genesis_id {
                        continue;
                    }
                    if seen_blocks.insert(block_id) {
                        gossips.push(fact_poke);
                        if gossips.len() >= target_blocks {
                            break;
                        }
                    }
                }
            }
        }

        if gossips.len() >= target_blocks {
            break;
        }

        if pending.is_empty() && gossips.len() < target_blocks {
            return Err("No pending pokes while target not reached; mining stalled"
                .to_string()
                .into());
        }
    }

    if !mining_started {
        return Err("Mining never started (no %mine effect observed)"
            .to_string()
            .into());
    }
    if gossips.len() < target_blocks {
        return Err(format!(
            "Mined {} blocks but target is {}",
            gossips.len(),
            target_blocks
        )
        .into());
    }

    let duration = start.unwrap_or_else(Instant::now).elapsed();
    let total_attempts = miner.as_ref().map(|m| m.total_attempts).unwrap_or(0);
    Ok(MiningOutput {
        gossips,
        duration,
        total_attempts,
        poke_timestamps,
    })
}

async fn run_catchup_peer(
    nockapp: &mut NockApp,
    gossips: &[NounSlab],
    peer_id: PeerId,
) -> Result<CatchupOutput, Box<dyn Error>> {
    let start = Instant::now();
    let mut poke_timestamps = Vec::with_capacity(gossips.len());
    for gossip in gossips {
        let _ = nockapp
            .poke(Libp2pWire::Gossip(peer_id).to_wire(), gossip.clone())
            .await?;
        poke_timestamps.push(start.elapsed());
    }
    Ok(CatchupOutput {
        duration: start.elapsed(),
        poke_timestamps,
    })
}

fn parse_mine_effect(effect: &NounSlab) -> Result<Option<MiningCandidate>, NockAppError> {
    let Ok(effect_cell) = (unsafe { effect.root().as_cell() }) else {
        return Ok(None);
    };
    let space = effect.noun_space();
    let effect_cell = effect_cell.in_space(&space);
    if !effect_cell.head().eq_bytes("mine") {
        return Ok(None);
    }
    let Ok([version, commit, target, pow_len_noun]) = effect_cell.tail().noun().uncell(&space)
    else {
        return Err(NockAppError::OtherError(
            "Expected four elements in %mine effect".to_string(),
        ));
    };
    let pow_len = pow_len_noun
        .in_space(&space)
        .as_atom()?
        .as_u64()
        .map_err(|_| NockAppError::OtherError("pow-len was not a u64".to_string()))?;

    let mut version_slab = NounSlab::new();
    version_slab.copy_into(version, &space);
    let mut header_slab = NounSlab::new();
    header_slab.copy_into(commit, &space);
    let mut target_slab = NounSlab::new();
    target_slab.copy_into(target, &space);

    Ok(Some(MiningCandidate {
        version: version_slab,
        header: header_slab,
        target: target_slab,
        pow_len,
    }))
}

fn extract_gossip_data(effect: &NounSlab) -> Result<Option<NounSlab>, NockAppError> {
    let Ok(effect_cell) = (unsafe { effect.root().as_cell() }) else {
        return Ok(None);
    };
    let space = effect.noun_space();
    let effect_cell = effect_cell.in_space(&space);
    if !effect_cell.head().eq_bytes("gossip") {
        return Ok(None);
    }
    let gossip_cell = effect_cell.tail().noun();
    let data = gossip_cell.in_space(&space).as_cell()?.tail().noun();
    let mut data_slab = NounSlab::new();
    data_slab.copy_into(data, &space);
    Ok(Some(data_slab))
}

fn heard_block_fact(gossip: &mut NounSlab) -> Result<Option<(String, NounSlab)>, NockAppError> {
    let noun = unsafe { gossip.root() };
    let space = gossip.noun_space();
    let head = noun.in_space(&space).as_cell()?.head();
    if !head.eq_bytes(b"heard-block") {
        return Ok(None);
    }

    let page = noun.in_space(&space).as_cell()?.tail().noun();
    let block_id = block_id_from_page(page, &space)?;
    let block_id_str = tip5_hash_to_base58_stack(gossip, block_id, &space)?;
    let mut fact_poke = NounSlab::new();
    fact_poke.copy_from_slab(gossip);
    fact_poke.modify(|response_noun| vec![D(tas!(b"fact")), D(0), response_noun]);
    Ok(Some((block_id_str, fact_poke)))
}

fn block_id_from_page(page: Noun, space: &NounSpace) -> Result<Noun, NockAppError> {
    let page_cell = page.in_space(space).as_cell()?;
    match page_cell.head().as_atom() {
        Ok(version_atom) => {
            let version = version_atom.as_u64()?;
            if version == 1 {
                Ok(page_cell.tail().as_cell()?.head().noun())
            } else {
                Err(NockAppError::OtherError(format!(
                    "Unsupported page version {}",
                    version
                )))
            }
        }
        Err(_) => Ok(page_cell.head().noun()),
    }
}

fn parse_mine_result(result: NounSlab) -> Result<MineResult, NockAppError> {
    let result_noun = unsafe { result.root() };
    let space = result.noun_space();
    let Ok(effects) = HoonList::try_from(*result_noun, &space) else {
        return Err(NockAppError::OtherError(String::from(
            "Mining kernel result was not a list",
        )));
    };

    let mining_result = effects.filter_map(|effect| {
        if effect.is_atom() {
            return None;
        }
        let Ok(effect_cell) = effect.in_space(&space).as_cell() else {
            return None;
        };
        if effect_cell.head().eq_bytes("mine-result") {
            Some(effect_cell.tail().noun())
        } else {
            None
        }
    });

    let Some(mine_result) = mining_result.into_iter().next() else {
        return Err(NockAppError::OtherError(String::from(
            "Mining kernel result missing %mine-result",
        )));
    };

    let Ok([res, tail]) = mine_result.uncell(&space) else {
        return Err(NockAppError::OtherError(String::from(
            "Malformed %mine-result payload",
        )));
    };

    if unsafe { res.raw_equals(&D(0)) } {
        let Ok([hash, poke]) = tail.uncell(&space) else {
            return Err(NockAppError::OtherError(String::from(
                "Expected hash and poke in successful %mine-result",
            )));
        };
        let mut poke_slab = NounSlab::new();
        poke_slab.copy_into(poke, &space);
        let mut nonce_slab = NounSlab::new();
        nonce_slab.copy_into(hash, &space);
        Ok(MineResult::Success {
            poke: poke_slab,
            next_nonce: nonce_slab,
        })
    } else {
        let mut nonce_slab = NounSlab::new();
        nonce_slab.copy_into(tail, &space);
        Ok(MineResult::Retry {
            next_nonce: nonce_slab,
        })
    }
}

fn create_candidate_poke(candidate: &MiningCandidate, nonce: &NounSlab) -> NounSlab {
    let mut slab = NounSlab::new();
    let header_space = candidate.header.noun_space();
    let version_space = candidate.version.noun_space();
    let target_space = candidate.target.noun_space();
    let nonce_space = nonce.noun_space();
    let header = slab.copy_into(unsafe { *candidate.header.root() }, &header_space);
    let version = slab.copy_into(unsafe { *candidate.version.root() }, &version_space);
    let target = slab.copy_into(unsafe { *candidate.target.root() }, &target_space);
    let nonce = slab.copy_into(unsafe { *nonce.root() }, &nonce_space);
    let poke_noun = T(
        &mut slab,
        &[version, header, nonce, target, D(candidate.pow_len)],
    );
    slab.set_root(poke_noun);
    slab
}

fn create_pow_poke(candidate: &MiningCandidate, nonce: &NounSlab) -> NounSlab {
    let mut slab = NounSlab::new();
    let version_space = candidate.version.noun_space();
    let header_space = candidate.header.noun_space();
    let nonce_space = nonce.noun_space();
    let version = slab.copy_into(unsafe { *candidate.version.root() }, &version_space);
    let header = slab.copy_into(unsafe { *candidate.header.root() }, &header_space);
    let nonce = slab.copy_into(unsafe { *nonce.root() }, &nonce_space);
    // Dummy proof/digest: skip-mining disables pow checks; 0 passes check-target.
    let proof = T(&mut slab, &[version, D(0), D(0), D(0)]);
    let poke_noun = T(
        &mut slab,
        &[D(tas!(b"command")), D(tas!(b"pow")), proof, D(0), header, nonce],
    );
    slab.set_root(poke_noun);
    slab
}

fn random_nonce() -> NounSlab {
    let mut rng = rand::rng();
    let mut nonce_slab = NounSlab::new();
    let mut nonce_cell = Atom::from_value(&mut nonce_slab, rng.random::<u64>() % PRIME)
        .expect("Failed to create nonce atom")
        .as_noun();
    for _ in 1..5 {
        let nonce_atom = Atom::from_value(&mut nonce_slab, rng.random::<u64>() % PRIME)
            .expect("Failed to create nonce atom")
            .as_noun();
        nonce_cell = T(&mut nonce_slab, &[nonce_atom, nonce_cell]);
    }
    nonce_slab.set_root(nonce_cell);
    nonce_slab
}

fn make_set_constants_poke(constants: &setup::BlockchainConstants) -> NounSlab {
    let mut poke_slab = NounSlab::new();
    let tag = make_tas(&mut poke_slab, "set-constants").as_noun();
    let constants_noun = constants.to_noun(&mut poke_slab);
    let poke_noun = T(&mut poke_slab, &[D(tas!(b"command")), tag, constants_noun]);
    poke_slab.set_root(poke_noun);
    poke_slab
}

fn make_set_genesis_seal_poke(seal: &str) -> NounSlab {
    let mut poke_slab = NounSlab::new();
    let block_height_noun = Atom::new(&mut poke_slab, DEFAULT_GENESIS_BLOCK_HEIGHT).as_noun();
    let seal_byts = Bytes::from(seal.to_string().into_bytes());
    let seal_noun = Atom::from_bytes(&mut poke_slab, &seal_byts).as_noun();
    let tag = Bytes::from(b"set-genesis-seal".to_vec());
    let set_genesis_seal = Atom::from_bytes(&mut poke_slab, &tag).as_noun();
    let poke_noun = T(
        &mut poke_slab,
        &[D(tas!(b"command")), set_genesis_seal, block_height_noun, seal_noun],
    );
    poke_slab.set_root(poke_noun);
    poke_slab
}

fn make_set_btc_data_poke() -> NounSlab {
    let mut poke_slab = NounSlab::new();
    let poke_noun = T(
        &mut poke_slab,
        &[D(tas!(b"command")), D(tas!(b"btc-data")), D(0)],
    );
    poke_slab.set_root(poke_noun);
    poke_slab
}

fn make_born_poke() -> NounSlab {
    let mut poke_slab = NounSlab::new();
    let born = T(
        &mut poke_slab,
        &[D(tas!(b"command")), D(tas!(b"born")), D(0)],
    );
    poke_slab.set_root(born);
    poke_slab
}

fn make_enable_mining_poke(enable: bool) -> NounSlab {
    let mut slab = NounSlab::new();
    let enable_mining =
        Atom::from_value(&mut slab, "enable-mining").expect("Failed to create enable-mining atom");
    let enable_mining_poke = T(
        &mut slab,
        &[D(tas!(b"command")), enable_mining.as_noun(), if enable { YES } else { NO }],
    );
    slab.set_root(enable_mining_poke);
    slab
}

fn make_set_mining_key_poke(v0_pubkey: &str, pkh: &str) -> NounSlab {
    let mut slab = NounSlab::new();
    let set_mining_key_adv = Atom::from_value(&mut slab, "set-mining-key-advanced")
        .expect("Failed to create set-mining-key-advanced atom");

    let mut configs_list = D(0);
    let mut keys_noun = D(0);
    let key_atom = Atom::from_value(&mut slab, v0_pubkey)
        .expect("Failed to create key atom")
        .as_noun();
    keys_noun = T(&mut slab, &[key_atom, keys_noun]);
    let config_tuple = T(&mut slab, &[D(1), D(1), keys_noun]);
    configs_list = T(&mut slab, &[config_tuple, configs_list]);

    let mut pkh_configs_list = D(0);
    let pkh_noun = Atom::from_value(&mut slab, pkh)
        .expect("Failed to create pkh atom")
        .as_noun();
    let pkh_tuple = T(&mut slab, &[D(1), pkh_noun]);
    pkh_configs_list = T(&mut slab, &[pkh_tuple, pkh_configs_list]);

    let set_mining_key_poke = T(
        &mut slab,
        &[
            D(tas!(b"command")),
            set_mining_key_adv.as_noun(),
            configs_list,
            pkh_configs_list,
        ],
    );
    slab.set_root(set_mining_key_poke);
    slab
}

fn print_summary(args: &BenchArgs, mining: &MiningOutput, catchup: Duration) {
    let mined_blocks = mining.gossips.len() as f64;
    let mining_ms = duration_ms(mining.duration);
    let catchup_ms = duration_ms(catchup);
    let avg_mining_ms = if mined_blocks > 0.0 {
        mining_ms / mined_blocks
    } else {
        0.0
    };
    let avg_catchup_ms = if mined_blocks > 0.0 {
        catchup_ms / mined_blocks
    } else {
        0.0
    };
    let avg_attempts = if mined_blocks > 0.0 {
        (mining.total_attempts as f64) / mined_blocks
    } else {
        0.0
    };

    info!(
        "bench: blocks_target={} pow_len={} log_difficulty={} skip_mining={} stack_size={:?}",
        args.blocks, args.pow_len, args.log_difficulty, args.skip_mining, args.stack_size
    );
    info!(
        "bench: mined_blocks={} mining_ms={:.3} avg_ms_per_block={:.3} avg_attempts_per_block={:.2}",
        mining.gossips.len(),
        mining_ms,
        avg_mining_ms,
        avg_attempts
    );
    info!(
        "bench: catchup_blocks={} catchup_ms={:.3} avg_ms_per_block={:.3}",
        mining.gossips.len(),
        catchup_ms,
        avg_catchup_ms
    );
}

fn report_phase_timings(
    label: &str,
    samples: Option<Vec<PmaTimingSample>>,
    timestamps: &[Duration],
) {
    let Some(samples) = samples else {
        info!(
            "bench: {} timings unavailable (NOCK_PMA_TIMING not enabled at boot)",
            label
        );
        return;
    };
    if samples.is_empty() || timestamps.is_empty() {
        info!("bench: {} timings unavailable (no samples)", label);
        return;
    }

    let (total_samples, pma_samples, min_len) = build_timed_samples(&samples, timestamps, label);
    info!(
        "bench: {} timing timestamps are ms since phase start (post-init)",
        label
    );
    summarize_timed_samples(&format!("{label}_total_ms"), &total_samples);
    summarize_timed_samples(&format!("{label}_pma_ms"), &pma_samples);

    if let Some(detail_samples) = build_detail_samples(&samples, timestamps, label, min_len) {
        report_detail_samples(label, &detail_samples);
    }
}

struct PmaDetailSamples {
    warm_ms: Vec<TimedSample>,
    warm_alloc: Vec<ValueSample>,
    test_jets_ms: Vec<TimedSample>,
    test_jets_alloc: Vec<ValueSample>,
    hot_ms: Vec<TimedSample>,
    hot_alloc: Vec<ValueSample>,
    cache_ms: Vec<TimedSample>,
    cache_alloc: Vec<ValueSample>,
    cold_ms: Vec<TimedSample>,
    cold_alloc: Vec<ValueSample>,
    arvo_ms: Vec<TimedSample>,
    arvo_alloc: Vec<ValueSample>,
}

fn build_timed_samples(
    samples: &[PmaTimingSample],
    timestamps: &[Duration],
    label: &str,
) -> (Vec<TimedSample>, Vec<TimedSample>, usize) {
    let min_len = samples.len().min(timestamps.len());
    if samples.len() != timestamps.len() {
        warn!(
            "bench: {} timing count mismatch: samples={}, timestamps={}, truncating to {}",
            label,
            samples.len(),
            timestamps.len(),
            min_len
        );
    }
    let mut total = Vec::with_capacity(min_len);
    let mut pma = Vec::with_capacity(min_len);
    for (idx, (sample, ts)) in samples.iter().zip(timestamps).take(min_len).enumerate() {
        let total_value = sample.event + sample.pma_copy;
        total.push(TimedSample {
            idx,
            value: total_value,
            ts: *ts,
        });
        pma.push(TimedSample {
            idx,
            value: sample.pma_copy,
            ts: *ts,
        });
    }
    (total, pma, min_len)
}

fn build_detail_samples(
    samples: &[PmaTimingSample],
    timestamps: &[Duration],
    label: &str,
    min_len: usize,
) -> Option<PmaDetailSamples> {
    let mut detail = PmaDetailSamples {
        warm_ms: Vec::with_capacity(min_len),
        warm_alloc: Vec::with_capacity(min_len),
        test_jets_ms: Vec::with_capacity(min_len),
        test_jets_alloc: Vec::with_capacity(min_len),
        hot_ms: Vec::with_capacity(min_len),
        hot_alloc: Vec::with_capacity(min_len),
        cache_ms: Vec::with_capacity(min_len),
        cache_alloc: Vec::with_capacity(min_len),
        cold_ms: Vec::with_capacity(min_len),
        cold_alloc: Vec::with_capacity(min_len),
        arvo_ms: Vec::with_capacity(min_len),
        arvo_alloc: Vec::with_capacity(min_len),
    };

    for (idx, (sample, ts)) in samples.iter().zip(timestamps).take(min_len).enumerate() {
        let Some(copy) = sample.detail else {
            warn!(
                "bench: {} detail timings unavailable (NOCK_PMA_TIMING_DETAIL not enabled at boot)",
                label
            );
            return None;
        };
        push_detail_samples(&mut detail, idx, *ts, copy);
    }

    Some(detail)
}

fn push_detail_samples(
    detail: &mut PmaDetailSamples,
    idx: usize,
    ts: Duration,
    copy: PmaCopyDetail,
) {
    detail.warm_ms.push(TimedSample {
        idx,
        value: copy.warm.elapsed,
        ts,
    });
    detail.warm_alloc.push(ValueSample {
        idx,
        value_bytes: (copy.warm.alloc_words as u64) * 8,
        ts,
    });
    detail.test_jets_ms.push(TimedSample {
        idx,
        value: copy.test_jets.elapsed,
        ts,
    });
    detail.test_jets_alloc.push(ValueSample {
        idx,
        value_bytes: (copy.test_jets.alloc_words as u64) * 8,
        ts,
    });
    detail.hot_ms.push(TimedSample {
        idx,
        value: copy.hot.elapsed,
        ts,
    });
    detail.hot_alloc.push(ValueSample {
        idx,
        value_bytes: (copy.hot.alloc_words as u64) * 8,
        ts,
    });
    detail.cache_ms.push(TimedSample {
        idx,
        value: copy.cache.elapsed,
        ts,
    });
    detail.cache_alloc.push(ValueSample {
        idx,
        value_bytes: (copy.cache.alloc_words as u64) * 8,
        ts,
    });
    detail.cold_ms.push(TimedSample {
        idx,
        value: copy.cold.elapsed,
        ts,
    });
    detail.cold_alloc.push(ValueSample {
        idx,
        value_bytes: (copy.cold.alloc_words as u64) * 8,
        ts,
    });
    detail.arvo_ms.push(TimedSample {
        idx,
        value: copy.arvo.elapsed,
        ts,
    });
    detail.arvo_alloc.push(ValueSample {
        idx,
        value_bytes: (copy.arvo.alloc_words as u64) * 8,
        ts,
    });
}

fn report_detail_samples(label: &str, detail: &PmaDetailSamples) {
    summarize_timed_samples(&format!("{label}_pma_warm_ms"), &detail.warm_ms);
    summarize_value_samples(&format!("{label}_pma_warm_alloc_mib"), &detail.warm_alloc);
    summarize_timed_samples(&format!("{label}_pma_test_jets_ms"), &detail.test_jets_ms);
    summarize_value_samples(
        &format!("{label}_pma_test_jets_alloc_mib"),
        &detail.test_jets_alloc,
    );
    summarize_timed_samples(&format!("{label}_pma_hot_ms"), &detail.hot_ms);
    summarize_value_samples(&format!("{label}_pma_hot_alloc_mib"), &detail.hot_alloc);
    summarize_timed_samples(&format!("{label}_pma_cache_ms"), &detail.cache_ms);
    summarize_value_samples(&format!("{label}_pma_cache_alloc_mib"), &detail.cache_alloc);
    summarize_timed_samples(&format!("{label}_pma_cold_ms"), &detail.cold_ms);
    summarize_value_samples(&format!("{label}_pma_cold_alloc_mib"), &detail.cold_alloc);
    summarize_timed_samples(&format!("{label}_pma_arvo_ms"), &detail.arvo_ms);
    summarize_value_samples(&format!("{label}_pma_arvo_alloc_mib"), &detail.arvo_alloc);
}

fn summarize_timed_samples(label: &str, samples: &[TimedSample]) {
    if samples.is_empty() {
        info!("bench: {}: no samples", label);
        return;
    }
    let p50 = percentile_sample(samples, 50.0).expect("non-empty samples should have p50");
    let p95 = percentile_sample(samples, 95.0).expect("non-empty samples should have p95");
    let p99 = percentile_sample(samples, 99.0).expect("non-empty samples should have p99");
    let max = top_n_samples(samples, 1)[0];
    info!(
        "bench: {}: p50={} p95={} p99={} max={}",
        label,
        format_sample(p50),
        format_sample(p95),
        format_sample(p99),
        format_sample(max)
    );

    let top3 = top_n_samples(samples, 3);
    let top3_fmt = top3
        .iter()
        .map(|sample| format_sample(*sample))
        .collect::<Vec<_>>()
        .join(", ");
    info!("bench: {}: top3=[{}]", label, top3_fmt);
}

fn summarize_value_samples(label: &str, samples: &[ValueSample]) {
    if samples.is_empty() {
        info!("bench: {}: no samples", label);
        return;
    }
    let p50 =
        percentile_value_sample(samples, 50.0).expect("non-empty value samples should have p50");
    let p95 =
        percentile_value_sample(samples, 95.0).expect("non-empty value samples should have p95");
    let p99 =
        percentile_value_sample(samples, 99.0).expect("non-empty value samples should have p99");
    let max = top_n_value_samples(samples, 1)[0];
    info!(
        "bench: {}: p50={} p95={} p99={} max={}",
        label,
        format_value_sample(p50),
        format_value_sample(p95),
        format_value_sample(p99),
        format_value_sample(max)
    );

    let top3 = top_n_value_samples(samples, 3);
    let top3_fmt = top3
        .iter()
        .map(|sample| format_value_sample(*sample))
        .collect::<Vec<_>>()
        .join(", ");
    info!("bench: {}: top3=[{}]", label, top3_fmt);
}

fn percentile_sample(samples: &[TimedSample], pct: f64) -> Option<TimedSample> {
    if samples.is_empty() {
        return None;
    }
    let mut indices: Vec<usize> = (0..samples.len()).collect();
    indices.sort_by(|&a, &b| samples[a].value.cmp(&samples[b].value));
    let rank = ((pct / 100.0) * ((samples.len() - 1) as f64)).ceil() as usize;
    Some(samples[indices[rank]])
}

fn percentile_value_sample(samples: &[ValueSample], pct: f64) -> Option<ValueSample> {
    if samples.is_empty() {
        return None;
    }
    let mut indices: Vec<usize> = (0..samples.len()).collect();
    indices.sort_by(|&a, &b| samples[a].value_bytes.cmp(&samples[b].value_bytes));
    let rank = ((pct / 100.0) * ((samples.len() - 1) as f64)).ceil() as usize;
    Some(samples[indices[rank]])
}

fn top_n_samples(samples: &[TimedSample], count: usize) -> Vec<TimedSample> {
    let mut indices: Vec<usize> = (0..samples.len()).collect();
    indices.sort_by(|&a, &b| samples[b].value.cmp(&samples[a].value));
    indices
        .into_iter()
        .take(count.min(samples.len()))
        .map(|idx| samples[idx])
        .collect()
}

fn top_n_value_samples(samples: &[ValueSample], count: usize) -> Vec<ValueSample> {
    let mut indices: Vec<usize> = (0..samples.len()).collect();
    indices.sort_by(|&a, &b| samples[b].value_bytes.cmp(&samples[a].value_bytes));
    indices
        .into_iter()
        .take(count.min(samples.len()))
        .map(|idx| samples[idx])
        .collect()
}

fn format_sample(sample: TimedSample) -> String {
    format!(
        "{:.3}ms@t={:.3}ms(idx={})",
        duration_ms(sample.value),
        duration_ms(sample.ts),
        sample.idx + 1
    )
}

fn format_value_sample(sample: ValueSample) -> String {
    format!(
        "{:.3}MiB@t={:.3}ms(idx={})",
        bytes_to_mib(sample.value_bytes),
        duration_ms(sample.ts),
        sample.idx + 1
    )
}

fn duration_ms(d: Duration) -> f64 {
    (d.as_micros() as f64) / 1000.0
}

fn bytes_to_mib(bytes: u64) -> f64 {
    (bytes as f64) / (1024.0 * 1024.0)
}

use std::str::FromStr;

use kernels::miner::KERNEL;
use nockapp::kernel::form::SerfThread;
use nockapp::nockapp::driver::{IODriverFn, NockAppHandle, PokeResult};
use nockapp::nockapp::wire::Wire;
use nockapp::nockapp::NockAppError;
use nockapp::noun::slab::NounSlab;
use nockapp::noun::AtomExt;
use nockapp::save::SaveableCheckpoint;
use nockapp::utils::NOCK_STACK_SIZE_TINY;
use nockapp::CrownError;
use nockchain_libp2p_io::tip5_util::tip5_hash_to_base58;
use nockvm::ext::NounExt;
use nockvm::interpreter::NockCancelToken;
use nockvm::noun::{Atom, D, NO, T, YES};
use nockvm_macros::tas;
use rand::Rng;
use tokio::sync::Mutex;
use tracing::{debug, error, info, instrument, warn};
use zkvm_jetpack::form::belt::PRIME;
use zkvm_jetpack::form::noun_ext::NounMathExt;
use zkvm_jetpack::form::structs::HoonList;

pub enum MiningWire {
    Mined,
    Candidate,
    SetPubKey,
    Enable,
}

impl MiningWire {
    pub fn verb(&self) -> &'static str {
        match self {
            MiningWire::Mined => "mined",
            MiningWire::SetPubKey => "setpubkey",
            MiningWire::Candidate => "candidate",
            MiningWire::Enable => "enable",
        }
    }
}

impl Wire for MiningWire {
    const VERSION: u64 = 1;
    const SOURCE: &'static str = "miner";

    fn to_wire(&self) -> nockapp::wire::WireRepr {
        let tags = vec![self.verb().into()];
        nockapp::wire::WireRepr::new(MiningWire::SOURCE, MiningWire::VERSION, tags)
    }
}

#[derive(Debug, Clone)]
pub struct MiningKeyConfig {
    pub share: u64,
    pub m: u64,
    pub keys: Vec<String>,
}

impl FromStr for MiningKeyConfig {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        // Expected format: "share,m:key1,key2,key3"
        let parts: Vec<&str> = s.split(':').collect();
        if parts.len() != 2 {
            return Err("Invalid format. Expected 'share,m:key1,key2,key3'".to_string());
        }

        let share_m: Vec<&str> = parts[0].split(',').collect();
        if share_m.len() != 2 {
            return Err("Invalid share,m format".to_string());
        }

        let share = share_m[0].parse::<u64>().map_err(|e| e.to_string())?;
        let m = share_m[1].parse::<u64>().map_err(|e| e.to_string())?;
        let keys: Vec<String> = parts[1].split(',').map(String::from).collect();

        Ok(MiningKeyConfig { share, m, keys })
    }
}

#[derive(Debug, Clone)]
pub struct MiningPkhConfig {
    pub share: u64,
    pub pkh: String,
}

impl FromStr for MiningPkhConfig {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        // Expected format: "share,pkh"
        let parts: Vec<&str> = s.split(',').collect();
        if parts.len() != 2 {
            return Err("Invalid share,pkh format".to_string());
        }

        let share = parts[0].parse::<u64>().map_err(|e| e.to_string())?;
        let pkh = parts[1].parse::<String>().map_err(|e| e.to_string())?;

        Ok(MiningPkhConfig { share, pkh })
    }
}

struct MiningData {
    pub block_header: NounSlab,
    pub version: NounSlab,
    pub target: NounSlab,
    pub pow_len: u64,
}

pub fn create_mining_driver(
    mining_config: Option<Vec<MiningKeyConfig>>,
    mining_pkh_config: Option<Vec<MiningPkhConfig>>,
    mine: bool,
    num_threads: u64,
    init_complete_tx: Option<tokio::sync::oneshot::Sender<()>>,
) -> IODriverFn {
    Box::new(move |handle| {
        Box::pin(async move {
            // set up empty config for v0 keys (TODO remove when taking out pubkey infra from kernel)
            let mut configs = Vec::<MiningKeyConfig>::new();
            configs.push(MiningKeyConfig {
                share: 1,
                m: 1,
                // hardcoded key to satisfy pass-through for v0 pubkey mining infra
                keys: vec!["2qwq9dQRZfpFx8BDicghpMRnYGKZsZGxxhh9m362pzpM9aeo276pR1yHZPS41y3CW3vPKxeYM8p8fzZS8GXmDGzmNNCnVNekjrSYogqfEFMqwhHh5iCjaKPaDTwhupWqiXj6".to_string()],
            });

            let Some(pkh_configs) = mining_pkh_config else {
                enable_mining(&handle, false).await?;

                if let Some(tx) = init_complete_tx {
                    tx.send(()).map_err(|_| {
                        NockAppError::OtherError(String::from(
                            "Could not send driver initialization for mining driver.",
                        ))
                    })?;
                }

                return Ok(());
            };
            set_mining_key_advanced(&handle, configs, pkh_configs).await?;
            enable_mining(&handle, mine).await?;

            if let Some(tx) = init_complete_tx {
                tx.send(()).map_err(|_| {
                    NockAppError::OtherError(String::from(
                        "Could not send driver initialization for mining driver.",
                    ))
                })?;
            }

            if !mine {
                return Ok(());
            }

            info!("Starting mining driver with {} threads", num_threads);

            let mut mining_attempts = tokio::task::JoinSet::<(
                SerfThread<SaveableCheckpoint>,
                u64,
                Result<NounSlab, CrownError>,
            )>::new();
            let hot_state = zkvm_jetpack::hot::produce_prover_hot_state();
            let test_jets_str = std::env::var("NOCK_TEST_JETS").unwrap_or_default();
            let test_jets = nockapp::kernel::boot::parse_test_jets(test_jets_str.as_str());

            let mining_data: Mutex<Option<MiningData>> = Mutex::new(None);
            let mut cancel_tokens: Vec<NockCancelToken> = Vec::<NockCancelToken>::new();

            loop {
                tokio::select! {
                        mining_result = mining_attempts.join_next(), if !mining_attempts.is_empty() => {
                            let mining_result = mining_result.expect("Mining attempt failed");
                            let (serf, id, slab_res) = mining_result.expect("Mining attempt result failed");
                            let slab = slab_res.expect("Mining attempt result failed");
                            let result = unsafe { slab.root() };

                            match HoonList::try_from(*result) {
                                Err(_) => {
                                    start_mining_attempt(serf, mining_data.lock().await, &mut mining_attempts, None, id).await;
                                }
                                Ok(effects) => {
                                    let mining_result =
                                        effects.filter_map(|effect| {
                                            if effect.is_atom() {
                                                None
                                            } else {
                                                let Ok(effect_cell) = effect.as_cell() else {
                                                    error!("Expected effect to be a cell");
                                                    return None;
                                                };
                                                let hed = effect_cell.head();
                                                if hed.eq_bytes("mine-result") {
                                                    Some(effect_cell.tail())
                                                } else {
                                                    None
                                                }
                                            }
                                    }).next();
                                    match mining_result {
                                        None => {
                                            start_mining_attempt(serf, mining_data.lock().await, &mut mining_attempts, None, id).await;
                                        },
                                        Some(mine_result) => {
                                            let Ok([res, tail]) = mine_result.uncell() else {
                                                return Err(NockAppError::OtherError(String::from("Expected two elements in mining result")));
                                            };
                                            if unsafe { res.raw_equals(&D(0)) } {
                                                // success
                                                // poke main kernel with mined block and start a new attempt
                                                info!("Found block! thread={id}");
                                                let Ok([hash, poke]) = tail.uncell() else {
                                                    error!("Expected two elements in tail");
                                                    return Err(NockAppError::OtherError(String::from("Expected two elements in tail")));
                                                };
                                                let mut poke_slab = NounSlab::new();
                                                poke_slab.copy_into(poke);
                                                handle.poke(MiningWire::Mined.to_wire(), poke_slab).await.expect("Could not poke nockchain with mined PoW");

                                                // launch new attempt
                                                let mut nonce_slab = NounSlab::new();
                                                nonce_slab.copy_into(hash);
                                                start_mining_attempt(serf, mining_data.lock().await, &mut mining_attempts, Some(nonce_slab), id).await;
                                            } else {
                                                // failure
                                                //  launch new attempt, using hash as new nonce
                                                //  nonce is tail
                                                debug!("didn't find block, starting new attempt. thread={id}");
                                                let mut nonce_slab = NounSlab::new();
                                                nonce_slab.copy_into(tail);
                                                start_mining_attempt(serf, mining_data.lock().await, &mut mining_attempts, Some(nonce_slab), id).await;
                                            }
                                        }
                                    }
                                }
                            }
                        }

                    effect_res = handle.next_effect() => {
                        let Ok(effect) = effect_res else {
                            warn!("Error receiving effect in mining driver: {effect_res:?}");
                            continue;
                        };
                        let Ok(effect_cell) = (unsafe { effect.root().as_cell() }) else {
                            drop(effect);
                            continue;
                        };

                        if effect_cell.head().eq_bytes("mine") {
                            let (version_slab, header_slab, target_slab, pow_len) = {
                                let [version, commit, target, pow_len_noun] = effect_cell.tail().uncell().expect(
                                    "Expected three elements in %mine effect",
                                );
                                let mut version_slab = NounSlab::new();
                                version_slab.copy_into(version);
                                let mut header_slab = NounSlab::new();
                                header_slab.copy_into(commit);
                                let mut target_slab = NounSlab::new();
                                target_slab.copy_into(target);
                                let pow_len =
                                    pow_len_noun
                                        .as_atom()
                                        .expect("Expected pow-len to be an atom")
                                        .as_u64()
                                        .expect("Expected pow-len to be a u64");
                                (version_slab, header_slab, target_slab, pow_len)
                            };
                            debug!("received new candidate block header: {:?}",
                                tip5_hash_to_base58(*unsafe { header_slab.root() })
                                .expect("Failed to convert header to Base58")
                            );
                            *(mining_data.lock().await) = Some(MiningData {
                                block_header: header_slab,
                                version: version_slab,
                                target: target_slab,
                                pow_len: pow_len
                            });

                            // Mining hasn't started yet, so start it
                            if mining_attempts.is_empty() {
                                info!("starting mining threads");
                                for i in 0..num_threads {
                                    let kernel = Vec::from(KERNEL);
                                    let serf = SerfThread::<SaveableCheckpoint>::new(
                                        kernel,
                                        None,
                                        hot_state.clone(),
                                        NOCK_STACK_SIZE_TINY,
                                        test_jets.clone(),
                                        Default::default(),
                                    )
                                    .await
                                    .expect("Could not load mining kernel");

                                    cancel_tokens.push(serf.cancel_token.clone());

                                    start_mining_attempt(serf, mining_data.lock().await, &mut mining_attempts, None, i).await;
                                }
                                info!("mining threads started with {} threads", num_threads);
                            } else {
                                // Mining is already running so cancel all the running attemps
                                // which are mining on the old block.
                                debug!("restarting mining attempts with new block header.");
                                for token in &cancel_tokens {
                                    token.cancel();
                                }
                            }
                        }
                    }
                }
            }
        })
    })
}

fn create_poke(mining_data: &MiningData, nonce: &NounSlab) -> NounSlab {
    let mut slab = NounSlab::new();
    let header = slab.copy_into(unsafe { *(mining_data.block_header.root()) });
    let version = slab.copy_into(unsafe { *(mining_data.version.root()) });
    let target = slab.copy_into(unsafe { *(mining_data.target.root()) });
    let nonce = slab.copy_into(unsafe { *(nonce.root()) });
    let poke_noun = T(
        &mut slab,
        &[version, header, nonce, target, D(mining_data.pow_len)],
    );
    slab.set_root(poke_noun);
    slab
}

async fn set_mining_key_advanced(
    handle: &NockAppHandle,
    configs: Vec<MiningKeyConfig>,
    pkh_configs: Vec<MiningPkhConfig>,
) -> Result<PokeResult, NockAppError> {
    let mut set_mining_key_slab = NounSlab::new();
    let set_mining_key_adv = Atom::from_value(&mut set_mining_key_slab, "set-mining-key-advanced")
        .expect("Failed to create set-mining-key-advanced atom");

    // Create the list of v0 (pubkey) configs (TODO remove when taking out pubkey infra)
    let mut configs_list = D(0);
    for config in configs {
        // Create the list of keys
        let mut keys_noun = D(0);
        for key in config.keys {
            let key_atom =
                Atom::from_value(&mut set_mining_key_slab, key).expect("Failed to create key atom");
            keys_noun = T(&mut set_mining_key_slab, &[key_atom.as_noun(), keys_noun]);
        }

        // Create the config tuple [share m keys]
        let config_tuple = T(
            &mut set_mining_key_slab,
            &[D(config.share), D(config.m), keys_noun],
        );

        configs_list = T(&mut set_mining_key_slab, &[config_tuple, configs_list]);
    }

    // Create the list of v1 (pubkey hash) configs
    let mut pkh_configs_list = D(0);
    for config in pkh_configs {
        let pkh_noun = Atom::from_value(&mut set_mining_key_slab, config.pkh)
            .expect("Failed to create key atom")
            .as_noun();

        // Create the config tuple [share pkh]
        let config_tuple = T(&mut set_mining_key_slab, &[D(config.share), pkh_noun]);

        pkh_configs_list = T(&mut set_mining_key_slab, &[config_tuple, pkh_configs_list]);
    }

    let set_mining_key_poke = T(
        &mut set_mining_key_slab,
        &[
            D(tas!(b"command")),
            set_mining_key_adv.as_noun(),
            configs_list,
            pkh_configs_list,
        ],
    );
    set_mining_key_slab.set_root(set_mining_key_poke);

    handle
        .poke(MiningWire::SetPubKey.to_wire(), set_mining_key_slab)
        .await
}

//TODO add %set-mining-key-multisig poke
#[instrument(skip(handle))]
async fn enable_mining(handle: &NockAppHandle, enable: bool) -> Result<PokeResult, NockAppError> {
    let mut enable_mining_slab = NounSlab::new();
    let enable_mining = Atom::from_value(&mut enable_mining_slab, "enable-mining")
        .expect("Failed to create enable-mining atom");
    let enable_mining_poke = T(
        &mut enable_mining_slab,
        &[D(tas!(b"command")), enable_mining.as_noun(), if enable { YES } else { NO }],
    );
    enable_mining_slab.set_root(enable_mining_poke);
    handle
        .poke(MiningWire::Enable.to_wire(), enable_mining_slab)
        .await
}

async fn start_mining_attempt(
    serf: SerfThread<SaveableCheckpoint>,
    mining_data: tokio::sync::MutexGuard<'_, Option<MiningData>>,
    mining_attempts: &mut tokio::task::JoinSet<(
        SerfThread<SaveableCheckpoint>,
        u64,
        Result<NounSlab, CrownError>,
    )>,
    nonce: Option<NounSlab>,
    id: u64,
) {
    let nonce = nonce.unwrap_or_else(|| {
        let mut rng = rand::thread_rng();
        let mut nonce_slab = NounSlab::new();
        let mut nonce_cell = Atom::from_value(&mut nonce_slab, rng.gen::<u64>() % PRIME)
            .expect("Failed to create nonce atom")
            .as_noun();
        for _ in 1..5 {
            let nonce_atom = Atom::from_value(&mut nonce_slab, rng.gen::<u64>() % PRIME)
                .expect("Failed to create nonce atom")
                .as_noun();
            nonce_cell = T(&mut nonce_slab, &[nonce_atom, nonce_cell]);
        }
        nonce_slab.set_root(nonce_cell);
        nonce_slab
    });
    let mining_data_ref = mining_data
        .as_ref()
        .expect("Mining data should already be initialized");
    debug!(
        "starting mining attempt on thread {:?} on header {:?}with nonce: {:?}",
        id,
        tip5_hash_to_base58(*unsafe { mining_data_ref.block_header.root() })
            .expect("Failed to convert block header to Base58"),
        tip5_hash_to_base58(*unsafe { nonce.root() }).expect("Failed to convert nonce to Base58"),
    );
    let poke_slab = create_poke(mining_data_ref, &nonce);
    mining_attempts.spawn(async move {
        let result = serf.poke(MiningWire::Candidate.to_wire(), poke_slab).await;
        (serf, id, result)
    });
}

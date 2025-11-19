pub mod config;
pub mod mining;
pub mod setup;

use std::error::Error;
use std::fs;
use std::path::Path;

pub use config::NockchainCli;
use libp2p::identity::Keypair;
use libp2p::multiaddr::Multiaddr;
use libp2p::{allow_block_list, connection_limits, memory_connection_limits, PeerId};
use nockapp::kernel::boot;
use nockapp::utils::make_tas;
use nockapp::NockApp;
use termcolor::{ColorChoice, StandardStream};
pub mod colors;

use colors::*;
use nockapp::noun::slab::{Jammer, NounSlab};
use nockvm::jets::hot::HotEntry;
use nockvm::noun::{D, T, YES};
use nockvm_macros::tas;
use tracing::{debug, info, instrument};

use crate::mining::{MiningKeyConfig, MiningPkhConfig};
use crate::setup::fakenet_blockchain_constants;

/// Module for handling driver initialization signals
pub mod driver_init {
    use nockapp::driver::{make_driver, IODriverFn, PokeResult};
    use nockapp::noun::slab::NounSlab;
    use nockapp::wire::{SystemWire, Wire};
    use nockapp::NockAppError;
    use tokio::sync::oneshot;
    use tracing::{debug, error, info};

    /// A collection of initialization signals for drivers
    #[derive(Default)]
    pub struct DriverInitSignals {
        /// Sender for the born signal
        pub signal_tx: Option<oneshot::Sender<()>>,
        /// Receiver for the born signal
        pub signal_rx: Option<oneshot::Receiver<()>>,
        /// Map of driver names to their initialization signal senders
        pub driver_signals: std::collections::HashMap<String, oneshot::Receiver<()>>,
    }

    impl DriverInitSignals {
        /// Create a new DriverInitSignals instance
        pub fn new() -> Self {
            let (signal_tx, signal_rx) = oneshot::channel();
            Self {
                signal_tx: Some(signal_tx),
                signal_rx: Some(signal_rx),
                driver_signals: std::collections::HashMap::new(),
            }
        }

        /// Register a driver with an initialization signal
        pub fn register_driver(&mut self, name: &str) -> oneshot::Sender<()> {
            let (tx, rx) = oneshot::channel();
            self.driver_signals.insert(name.to_string(), rx);
            tx
        }

        /// Get the initialization signal sender for a driver
        pub fn get_signal_sender(&self, name: &str) -> Option<&oneshot::Receiver<()>> {
            self.driver_signals.get(name)
        }

        /// Create a task that waits for all registered drivers to initialize
        pub fn create_task(&mut self) -> tokio::task::JoinHandle<()> {
            let signal_tx = self.signal_tx.take().expect("Signal already used");
            let driver_signals = std::mem::take(&mut self.driver_signals);

            tokio::spawn(async move {
                // Wait for all registered drivers to initialize concurrently
                let mut join_set = tokio::task::JoinSet::new();
                for (name, rx) in driver_signals {
                    let name = name.clone();
                    join_set.spawn(async move {
                        let _ = rx.await;
                        info!("driver '{}' initialized", name);
                    });
                }

                // Wait for all tasks to complete
                while let Some(result) = join_set.join_next().await {
                    result.expect("Task panicked");
                }

                // Send the born poke signal
                let _ = signal_tx.send(());
                info!("all drivers initialized, born poke sent");
            })
        }

        /// Create the born driver that waits for signal before poking
        /// You can chain many of these together by passing 'init_complete_tx'.
        pub fn create_driver(
            &mut self,
            poke: NounSlab,
            init_complete_tx: Option<tokio::sync::oneshot::Sender<()>>,
        ) -> IODriverFn {
            let born_rx = self.signal_rx.take().expect("born signal already used");

            make_driver(move |handle| {
                Box::pin(async move {
                    // Wait for the born signal
                    let _ = born_rx.await;

                    let wire = SystemWire.to_wire();
                    let result = handle.poke(wire, poke).await?;

                    match result {
                        PokeResult::Ack => debug!("poke acknowledged"),
                        PokeResult::Nack => error!("poke nacked"),
                    }
                    if let Some(tx) = init_complete_tx {
                        tx.send(()).map_err(|_| {
                            NockAppError::OtherError(String::from(
                                "Could not send driver initialization for mining driver.",
                            ))
                        })?;
                    }

                    Ok(())
                })
            })
        }
    }
}

/// NockchainAPIConfig: toggles whether public server is enabled
/// can be expanded into a struct if necessary
#[derive(Debug, Clone)]
pub enum NockchainAPIConfig {
    EnablePublicServer(std::net::SocketAddr),
    DisablePublicServer,
}

impl NockchainAPIConfig {
    pub fn deploy_public(&self) -> bool {
        matches!(self, NockchainAPIConfig::EnablePublicServer(_))
    }

    pub fn addr(&self) -> Option<std::net::SocketAddr> {
        match self {
            NockchainAPIConfig::EnablePublicServer(addr) => Some(*addr),
            NockchainAPIConfig::DisablePublicServer => None,
        }
    }
}

/// # Load a keypair from a file or create a new one if the file doesn't exist
///
/// This function attempts to read a keypair from a specified file. If the file exists, it reads the keypair from the file.
/// If the file does not exist, it generates a new keypair, writes it to the file, and returns it.
///
/// # Arguments
/// * `keypair_path` - A reference to a Path object representing the file path where the keypair should be stored
/// * `force_new` - If true, generate a new keypair even if one already exists
///
/// # Returns
/// A Result containing the Keypair or an error if any operation fails
pub fn gen_keypair(keypair_path: &Path) -> Result<Keypair, Box<dyn Error>> {
    let new_keypair = libp2p::identity::Keypair::generate_ed25519();
    let new_keypair_bytes = new_keypair.to_protobuf_encoding()?;
    std::fs::write(keypair_path, new_keypair_bytes)?;
    let peer_id = new_keypair.public().to_peer_id();
    // write the peer_id encoded as base58 to a file
    std::fs::write(
        keypair_path.with_extension(config::PEER_ID_FILE_EXTENSION),
        peer_id.to_base58(),
    )?;
    info!("Generated new identity as peer {peer_id}");
    Ok(new_keypair)
}

fn load_keypair(keypair_path: &Path, force_old: bool) -> Result<Keypair, Box<dyn Error>> {
    if keypair_path.try_exists()? && force_old {
        let keypair_bytes = std::fs::read(keypair_path)?;
        let keypair = libp2p::identity::Keypair::from_protobuf_encoding(&keypair_bytes[..])?;
        let peer_id = keypair.public().to_peer_id();
        let pubkey_path = keypair_path.with_extension(config::PEER_ID_FILE_EXTENSION);
        if !pubkey_path.exists() {
            info!("Writing pubkey to {pubkey_path:?}");
            std::fs::write(pubkey_path, peer_id.to_base58())?;
        }
        info!("Loaded identity as peer {peer_id}");
        Ok(keypair)
    } else {
        if !force_old && keypair_path.try_exists()? {
            info!("Discarding existing peer ID and generating a new one");
            std::fs::remove_file(keypair_path)?;
        }
        gen_keypair(keypair_path)
    }
}

#[instrument(skip(kernel_jam, hot_state))]
pub async fn init_with_kernel<J: Jammer + Send + 'static>(
    cli: config::NockchainCli,
    kernel_jam: &[u8],
    hot_state: &[HotEntry],
    server_config: NockchainAPIConfig,
) -> Result<NockApp<J>, Box<dyn Error>> {
    welcome();

    cli.validate()?;

    let mut nockapp_cli = cli.nockapp_cli.clone();
    nockapp_cli.stack_size = nockapp::kernel::boot::NockStackSize::Medium;

    let mut nockapp =
        boot::setup::<J>(kernel_jam, nockapp_cli, hot_state, "nockchain", None).await?;

    let keypair = {
        let keypair_path = Path::new(config::IDENTITY_PATH);
        load_keypair(keypair_path, cli.no_new_peer_id)?
    };
    info!("allowed_peers_path: {:?}", cli.allowed_peers_path);
    let allowed = cli.allowed_peers_path.as_ref().map(|path| {
        let contents = fs::read_to_string(path).expect("failed to read allowed peers file: {}");
        let peer_ids: Vec<PeerId> = contents
            .lines()
            .map(|line| {
                let peer_id_bytes = bs58::decode(line)
                    .into_vec()
                    .expect("failed to decode peer ID bytes from base58");
                PeerId::from_bytes(&peer_id_bytes).expect("failed to decode peer ID from bytes")
            })
            .collect();
        let mut allow_behavior =
            allow_block_list::Behaviour::<allow_block_list::AllowedPeers>::default();
        for peer_id in peer_ids {
            allow_behavior.allow_peer(peer_id);
        }
        allow_behavior
    });

    let bind_multiaddrs = cli
        .bind
        .unwrap_or(vec!["/ip4/0.0.0.0/udp/0/quic-v1".parse()?])
        .clone()
        .into_iter()
        .map(|addr_str| addr_str.parse().expect("could not parse bind multiaddr"))
        .collect();

    let libp2p_config = nockchain_libp2p_io::config::LibP2PConfig::from_env()?;
    debug!("Using libp2p config: {:?}", libp2p_config);
    let limits = connection_limits::ConnectionLimits::default()
        .with_max_established_incoming(Some(
            cli.max_established_incoming
                .unwrap_or(libp2p_config.max_established_incoming_connections),
        ))
        .with_max_established_outgoing(Some(
            cli.max_established_outgoing
                .unwrap_or(libp2p_config.max_established_outgoing_connections),
        ))
        .with_max_pending_incoming(Some(
            cli.max_pending_incoming
                .unwrap_or(libp2p_config.max_pending_incoming_connections),
        ))
        .with_max_pending_outgoing(
            cli.max_pending_outgoing
                .or(Some(libp2p_config.max_pending_outgoing_connections)),
        )
        .with_max_established(
            cli.max_established
                .or(Some(libp2p_config.max_established_connections)),
        )
        .with_max_established_per_peer(
            cli.max_established_per_peer
                .or(Some(libp2p_config.max_established_connections_per_peer)),
        );
    let memory_limits = if cli.max_system_memory_bytes.is_some()
        && cli.max_system_memory_fraction.is_some()
    {
        panic!( "Must provide neither or one of --max-system-memory_bytes or --max-system-memory_percentage" )
    } else {
        if let Some(max_bytes) = cli.max_system_memory_bytes {
            Some(memory_connection_limits::Behaviour::with_max_bytes(
                max_bytes,
            ))
        } else {
            cli.max_system_memory_fraction
                .map(memory_connection_limits::Behaviour::with_max_percentage)
        }
    };

    let default_backbone_peers = if cli.fakenet {
        config::TESTNET_BACKBONE_NODES
    } else {
        config::REALNET_BACKBONE_NODES
    };

    let backbone_peers = default_backbone_peers
        .iter()
        .map(|multiaddr_str| {
            multiaddr_str
                .parse()
                .expect("could not parse multiaddr from built-in string")
        })
        .collect();

    // Set up initial peer addresses to connect to
    let mut initial_peer_multiaddrs: Vec<Multiaddr> = if cli.no_default_peers {
        Vec::new()
    } else {
        backbone_peers
    };

    let v: Vec<Multiaddr> = cli
        .peer
        .clone()
        .into_iter()
        .map(|multiaddr_str| {
            multiaddr_str
                .parse()
                .expect("could not parse multiaddr from string")
        })
        .collect();
    initial_peer_multiaddrs.extend(v);

    let force_peers: Vec<Multiaddr> = cli
        .force_peer
        .clone()
        .into_iter()
        .map(|multiaddr_str| {
            multiaddr_str
                .parse()
                .expect("could not parse multiaddr from string")
        })
        .collect();

    for multiaddr in &force_peers {
        initial_peer_multiaddrs.push(multiaddr.clone());
    }

    debug!("initial_peer_multiaddrs: {:?}", initial_peer_multiaddrs);
    debug!("force_peer_multiaddrs: {:?}", force_peers);

    let equix_builder = equix::EquiXBuilder::new();

    // Create driver initialization signals. the idea here is that we want to wait for
    // drivers that emit init pokes to complete before we send the born poke.
    let mut born_driver_signals = driver_init::DriverInitSignals::new();

    // Register drivers that need initialization signals
    let mining_init_tx = born_driver_signals.register_driver("mining");
    let libp2p_init_tx = born_driver_signals.register_driver("libp2p");

    // Create the born task that waits for all drivers to initialize
    let _born_task = born_driver_signals.create_task();

    let is_kernel_mainnet: Option<bool> = {
        let mut peek_slab = NounSlab::new();
        let peek_noun = T(&mut peek_slab, &[D(tas!(b"mainnet")), D(0)]);
        peek_slab.set_root(peek_noun);
        if let Some(peek_res) = nockapp.peek_handle(peek_slab).await? {
            let mainnet_flag = unsafe { peek_res.root() };
            if mainnet_flag.is_atom() {
                Some(unsafe { mainnet_flag.raw_equals(&YES) })
            } else {
                panic!("Invalid mainnet flag")
            }
        } else {
            None
        }
    };

    let genesis_seal_set: bool = {
        let mut peek_slab = NounSlab::new();
        let tag = make_tas(&mut peek_slab, "genesis-seal-set").as_noun();
        let peek_noun = T(&mut peek_slab, &[tag, D(0)]);
        peek_slab.set_root(peek_noun);
        if let Some(peek_res) = nockapp.peek_handle(peek_slab).await? {
            let genesis_seal = unsafe { peek_res.root() };
            if genesis_seal.is_atom() {
                unsafe { genesis_seal.raw_equals(&YES) }
            } else {
                panic!("Invalid genesis seal")
            }
        } else {
            panic!("Genesis seal peak failed")
        }
    };

    let born_init_tx = if cli.fakenet {
        // Set the require fakenet constants first, then handle the optional ones
        let mut fakenet_constants =
            fakenet_blockchain_constants(cli.fakenet_pow_len, cli.fakenet_log_difficulty);
        if let Some(coinbase_timelock_min) = cli.fakenet_coinbase_timelock_min {
            fakenet_constants = fakenet_constants.with_coinbase_timelock_min(coinbase_timelock_min);
        }
        if let Some(v1_phase) = cli.fakenet_v1_phase {
            fakenet_constants = fakenet_constants.with_v1_phase(v1_phase);
        }
        setup::poke(
            &mut nockapp,
            setup::SetupCommand::PokeFakenetConstants(fakenet_constants),
        )
        .await?;
        if let Some(true) = is_kernel_mainnet {
            panic!("Fatal: attemped to boot mainnet node with fakenet flag")
        } else if !genesis_seal_set {
            setup::poke(
                &mut nockapp,
                setup::SetupCommand::PokeSetGenesisSeal(setup::FAKENET_GENESIS_MESSAGE.to_string()),
            )
            .await?;
        }

        // Create driver initialization signals for fakenet
        let mut fake_genesis_signals = driver_init::DriverInitSignals::new();
        let born_init_tx = fake_genesis_signals.register_driver("born");
        let _ = fake_genesis_signals.create_task();

        // Check if custom genesis path is provided, read file if so
        let genesis_data = if let Some(genesis_path) = cli.fakenet_genesis_jam_path {
            Some(fs::read(genesis_path)?)
        } else {
            None
        };

        let poke = setup::heard_fake_genesis_block(genesis_data)?;
        let fakenet_driver = fake_genesis_signals.create_driver(poke, None);
        nockapp.add_io_driver(fakenet_driver).await;
        Some(born_init_tx)
    } else {
        if let Some(false) = is_kernel_mainnet {
            panic!("Fatal: attemped to boot fakenet kernel without fakenet flag!")
        } else if !genesis_seal_set {
            setup::poke(
                &mut nockapp,
                setup::SetupCommand::PokeSetGenesisSeal(setup::REALNET_GENESIS_MESSAGE.to_string()),
            )
            .await?;
        }
        None
    };
    setup::poke(&mut nockapp, setup::SetupCommand::PokeSetBtcData).await?;

    // Set up empty mining config by default (TODO remove when taking out pubkey infra)
    let mining_config: Option<Vec<MiningKeyConfig>> = { None };

    let mining_pkh_config = if let Some(pkh) = &cli.mining_pkh {
        Some(vec![MiningPkhConfig {
            share: 1,
            pkh: pkh.clone(),
        }])
    } else if let Some(mining_pkh_adv) = &cli.mining_pkh_adv {
        Some(mining_pkh_adv.clone())
    } else {
        None
    };

    let prune_inbound = cli.prune_inbound;

    let mine = cli.mine;

    let threads = if let Some(num_threads) = &cli.num_threads {
        *num_threads
    } else {
        1
    };

    let mining_driver = crate::mining::create_mining_driver(
        mining_config,
        mining_pkh_config,
        mine,
        threads,
        Some(mining_init_tx),
    );
    nockapp.add_io_driver(mining_driver).await;

    let libp2p_driver = nockchain_libp2p_io::driver::make_libp2p_driver(
        keypair,
        bind_multiaddrs,
        allowed,
        limits,
        memory_limits,
        &initial_peer_multiaddrs,
        &force_peers,
        prune_inbound,
        cli.fast_sync,
        equix_builder,
        config::CHAIN_INTERVAL,
        Some(libp2p_init_tx),
    );
    nockapp.add_io_driver(libp2p_driver).await;

    // Create the born driver that waits for the born signal
    // Make the born poke
    let mut born_slab = NounSlab::new();
    let born = T(
        &mut born_slab,
        &[D(tas!(b"command")), D(tas!(b"born")), D(0)],
    );
    born_slab.set_root(born);
    let born_driver = born_driver_signals.create_driver(born_slab, born_init_tx);

    // Add the born driver to the nockapp
    nockapp.add_io_driver(born_driver).await;

    if server_config.deploy_public() {
        let addr = server_config
            .addr()
            .expect("addr should be Some when deploy_public is true");
        nockapp
            .add_io_driver(nockapp_grpc::public_nockchain::grpc_server_driver(addr))
            .await;
    }
    nockapp
        .add_io_driver(nockapp_grpc::private_nockapp::grpc_server_driver(
            cli.bind_private_grpc_port,
        ))
        .await;

    nockapp.add_io_driver(nockapp::exit_driver()).await;

    Ok(nockapp)
}

fn welcome() {
    let mut stdout = StandardStream::stdout(ColorChoice::Auto);

    let banner = "
    _   _            _        _           _
   | \\ | | ___   ___| | _____| |__   __ _(_)_ __
   |  \\| |/ _ \\ / __| |/ / __| '_ \\ / _` | | '_ \\
   | |\\  | (_) | (__|   < (__| | | | (_| | | | | |
   |_| \\_|\\___/ \\___|_|\\_\\___|_| |_|\\__,_|_|_| |_|
   ";

    print_banner(&mut stdout, banner);

    //let info = [
    //("Build label", env!("BUILD_EMBED_LABEL")),
    //("Build host", env!("BUILD_HOST")),
    //("Build user", env!("BUILD_USER")),
    //("Build timestamp", env!("BUILD_TIMESTAMP")),
    //("Build date", env!("FORMATTED_DATE")),
    // ("Git commit", env!("BAZEL_GIT_COMMIT")),
    // ("Build timestamp", env!("VERGEN_BUILD_TIMESTAMP")),
    // ("Cargo debug", env!("VERGEN_CARGO_DEBUG")),
    // ("Cargo features", env!("VERGEN_CARGO_FEATURES")),
    // ("Cargo opt level", env!("VERGEN_CARGO_OPT_LEVEL")),
    // ("Cargo target", env!("VERGEN_CARGO_TARGET_TRIPLE")),
    // ("Git branch", env!("VERGEN_GIT_BRANCH")),
    // ("Git commit date", env!("VERGEN_GIT_COMMIT_DATE")),
    // ("Git commit author", env!("VERGEN_GIT_COMMIT_AUTHOR_NAME")),
    // ("Git commit message", env!("VERGEN_GIT_COMMIT_MESSAGE")),
    // ("Git commit timestamp", env!("VERGEN_GIT_COMMIT_TIMESTAMP")),
    // ("Git commit SHA", env!("VERGEN_GIT_SHA")),
    // ("Rustc channel", env!("VERGEN_RUSTC_CHANNEL")),
    // ("Rustc host", env!("VERGEN_RUSTC_HOST_TRIPLE")),
    // ("Rustc LLVM version", env!("VERGEN_RUSTC_LLVM_VERSION")),
    //];

    //print_version_info(&mut stdout, &info);
}

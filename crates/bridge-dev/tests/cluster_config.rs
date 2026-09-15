use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use alloy::primitives::{Address, B256};
use anyhow::{anyhow, Context, Result};
use bridge::shared::config::{BridgeConfigToml, SequencerConfigToml};
use bridge::shared::e2e_environment::{BaseSepoliaE2eManifest, BASE_SEPOLIA_E2E_CHAIN_ID};
use bridge_dev::anvil::{AnvilBackend, AnvilConfig};
use bridge_dev::anvil_fork::{PinnedAnvilFork, PinnedForkConfig};
use bridge_dev::artifacts::{ArtifactBuildMetadata, ArtifactFile, ArtifactRole, E2eArtifacts};
use bridge_dev::cluster_config::{
    deterministic_cluster_nodes, ClusterConfigError, ClusterConfigGenerator, ClusterConfigInput,
    ClusterDeploymentFacts,
};
use bridge_dev::environment::BaseE2eEnvironment;
use bridge_dev::fork_state::ForkState;
use bridge_dev::hermetic_deploy::{HermeticDeployConfig, HermeticDeployment};
use serde_json::{json, Value};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::task::JoinHandle;

const MANIFEST_JSON: &str = include_str!("../../bridge/e2e/environments/base-sepolia.json");
static RUN_SEQUENCE: AtomicU64 = AtomicU64::new(0);

#[tokio::test]
async fn generates_parseable_private_cluster_configs_and_redacted_manifest() -> Result<()> {
    let (backend, input) = deployed_input("parseable", 100).await?;
    let bundle = ClusterConfigGenerator::generate(&input)?;

    assert_eq!(bundle.paths.bridge_configs.len(), 5);
    for (node_id, path) in bundle.paths.bridge_configs.iter().enumerate() {
        let config = BridgeConfigToml::from_file(path)?;
        assert_eq!(config.node_id, node_id as u64);
        assert_eq!(config.base_chain_id()?, BASE_SEPOLIA_E2E_CHAIN_ID);
        assert!(config.base_ws_url().starts_with("ws://127.0.0.1:"));
        assert!(config
            .nockchain_sequencer_api_address()?
            .starts_with("http://127.0.0.1:"));
        assert_eq!(config.to_node_config()?.nodes.len(), 5);
        assert_eq!(
            config.bridge_constants()?.base_start_height,
            input.deployment.start_height
        );
        assert_eq!(config.withdrawal_activation_cutoff()?.nock_next_height, 1);
        assert_eq!(fs::metadata(path)?.permissions().mode() & 0o777, 0o600);
    }

    let sequencer = SequencerConfigToml::from_file(&bundle.paths.sequencer_config)?;
    assert_eq!(sequencer.base_chain_id()?, BASE_SEPOLIA_E2E_CHAIN_ID);
    assert_eq!(sequencer.validated_nodes()?.len(), 5);
    assert!(!sequencer.sequencer_journal.enabled);
    assert_eq!(
        fs::metadata(&bundle.paths.sequencer_config)?
            .permissions()
            .mode()
            & 0o777,
        0o600
    );

    ClusterConfigGenerator::verify_backend(&backend, &input).await?;
    let redacted = fs::read_to_string(&bundle.paths.redacted_manifest)?;
    for node in &input.nodes {
        assert!(!redacted.contains(&node.eth_private_key));
        assert!(!redacted.contains(&node.nock_private_key));
    }
    assert!(!redacted.contains("PRIVATE KEY"));
    assert_eq!(bundle.manifest.schema_version, 1);
    assert_eq!(bundle.manifest.chain_id, BASE_SEPOLIA_E2E_CHAIN_ID);
    assert_eq!(
        bundle.manifest.bridge_nodes,
        input.nodes.clone().map(|node| node.eth_address)
    );
    assert_eq!(
        bundle.manifest.start_block_hash,
        format!("{:#x}", input.deployment.start_block_hash)
    );
    assert_eq!(
        bundle.manifest.proxy_runtime_keccak256,
        format!("{:#x}", input.deployment.proxy_runtime_keccak256)
    );
    assert_eq!(
        bundle.manifest.implementation_runtime_keccak256,
        format!("{:#x}", input.deployment.implementation_runtime_keccak256)
    );
    assert_eq!(
        bundle.manifest.nock_runtime_keccak256,
        format!("{:#x}", input.deployment.nock_runtime_keccak256)
    );

    let mut wrong_frontier = input.clone();
    wrong_frontier.deployment.start_block_hash = B256::repeat_byte(0x55);
    assert!(matches!(
        ClusterConfigGenerator::verify_backend(&backend, &wrong_frontier).await,
        Err(ClusterConfigError::FrontierMismatch { .. })
    ));

    backend.shutdown().await?;
    Ok(())
}

#[tokio::test]
async fn isolates_concurrent_runs_and_rejects_remote_or_stale_configuration() -> Result<()> {
    let (backend, input) = deployed_input("isolated", 300).await?;
    let mut left = input.clone();
    left.run_root = preserved_run_root("left");
    left.run_id = "concurrent-left".to_owned();
    let mut right = input.clone();
    right.run_root = preserved_run_root("right");
    right.run_id = "concurrent-right".to_owned();
    right.port_offset = 500;

    let left_task = tokio::task::spawn_blocking(move || ClusterConfigGenerator::generate(&left));
    let right_task = tokio::task::spawn_blocking(move || ClusterConfigGenerator::generate(&right));
    let (left_bundle, right_bundle) = tokio::try_join!(left_task, right_task)?;
    let left_bundle = left_bundle?;
    let right_bundle = right_bundle?;
    assert_ne!(left_bundle.paths.config_dir, right_bundle.paths.config_dir);
    assert_ne!(left_bundle.manifest.run_id, right_bundle.manifest.run_id);
    assert_ne!(
        BridgeConfigToml::from_file(&left_bundle.paths.bridge_configs[0])?.ingress_listen_address(),
        BridgeConfigToml::from_file(&right_bundle.paths.bridge_configs[0])?
            .ingress_listen_address()
    );

    let mut remote = input.clone();
    remote.run_root = preserved_run_root("remote");
    remote.base_http_url = "https://sepolia.base.org".to_owned();
    remote.base_ws_url = "wss://sepolia.base.org".to_owned();
    assert!(matches!(
        ClusterConfigGenerator::generate(&remote),
        Err(ClusterConfigError::InvalidBaseUrl)
    ));

    let stale = ClusterConfigGenerator::generate(&input)?;
    assert!(matches!(
        ClusterConfigGenerator::generate(&input),
        Err(ClusterConfigError::StaleConfig(path)) if path == stale.paths.config_dir
    ));

    let mut wrong_signer = input.clone();
    wrong_signer.run_root = preserved_run_root("wrong-signer");
    wrong_signer.nodes[0].eth_address = wrong_signer.nodes[1].eth_address.clone();
    assert!(matches!(
        ClusterConfigGenerator::generate(&wrong_signer),
        Err(ClusterConfigError::InvalidNodeIdentity(0))
    ));

    backend.shutdown().await?;
    Ok(())
}

#[tokio::test]
async fn generates_and_verifies_configs_from_a_pinned_fork_baseline() -> Result<()> {
    let upstream = AnvilBackend::start(AnvilConfig::empty(), &checked_environment()).await?;
    let nodes = deterministic_cluster_nodes();
    let signers: [Address; 5] = nodes
        .iter()
        .map(|node| node.eth_address.parse::<Address>())
        .collect::<Result<Vec<_>, _>>()?
        .try_into()
        .map_err(|_| anyhow!("expected five deterministic signers"))?;
    let deployment = HermeticDeployment::deploy(
        &upstream,
        HermeticDeployConfig::discover(&workspace_root()?, signers),
    )
    .await?;
    let fork_environment =
        BaseE2eEnvironment::from_manifest(manifest_for_deployment(deployment.facts())?)?;
    let source_proxy = SourceChainProxy::start(upstream.http_url().as_url().to_string()).await?;
    let fork = PinnedAnvilFork::start(
        PinnedForkConfig::new(source_proxy.endpoint.clone()),
        &fork_environment,
    )
    .await?;
    let fork_state = ForkState::capture(
        fork.backend(),
        &deployment.facts().bridge_state,
        deployment.facts().addresses.deployer,
        signers.to_vec(),
    )
    .await?;
    let deployment_facts = ClusterDeploymentFacts::from_seeded_fork(
        fork.evidence(),
        &fork_state,
        BASE_SEPOLIA_E2E_CHAIN_ID,
    )?;
    let run_root = preserved_run_root("pinned-fork");
    let input = ClusterConfigInput {
        artifacts: fake_artifacts(&run_root.join("artifacts"))?,
        run_root,
        run_id: "pinned-fork-run".to_owned(),
        deployment: deployment_facts,
        nodes,
        base_http_url: fork.backend().http_url().as_url().to_string(),
        base_ws_url: fork.backend().ws_url().to_owned(),
        port_offset: 700,
        base_confirmation_depth: 1,
        nockchain_confirmation_depth: 1,
        withdrawal_activation_nock_next_height: 1,
        base_blocks_chunk: 10,
        fakenet_pow_len: 64,
        fakenet_log_difficulty: 2,
    };

    let bundle = ClusterConfigGenerator::generate(&input)?;
    assert_eq!(bundle.manifest.environment_id, "base-sepolia-fork");
    assert_eq!(
        bundle.manifest.start_height,
        fork_state.baseline().block_number
    );
    ClusterConfigGenerator::verify_backend(fork.backend(), &input).await?;

    fork.shutdown().await?;
    upstream.shutdown().await?;
    Ok(())
}

async fn deployed_input(
    label: &str,
    port_offset: u16,
) -> Result<(AnvilBackend, ClusterConfigInput)> {
    let backend = AnvilBackend::start(AnvilConfig::empty(), &checked_environment()).await?;
    let nodes = deterministic_cluster_nodes();
    let signers = nodes
        .iter()
        .map(|node| node.eth_address.parse::<Address>())
        .collect::<Result<Vec<_>, _>>()?
        .try_into()
        .map_err(|_| anyhow!("expected five deterministic signers"))?;
    let workspace = workspace_root()?;
    let deployment = HermeticDeployment::deploy(
        &backend,
        HermeticDeployConfig::discover(&workspace, signers),
    )
    .await?;
    let facts =
        ClusterDeploymentFacts::from_hermetic(deployment.facts(), BASE_SEPOLIA_E2E_CHAIN_ID)?;
    let run_root = preserved_run_root(label);
    let artifacts = fake_artifacts(&run_root.join("artifacts"))?;
    let input = ClusterConfigInput {
        run_root,
        run_id: "golden-hermetic-run".to_owned(),
        deployment: facts,
        artifacts,
        nodes,
        base_http_url: backend.http_url().as_url().to_string(),
        base_ws_url: backend.ws_url().to_owned(),
        port_offset,
        base_confirmation_depth: 1,
        nockchain_confirmation_depth: 1,
        withdrawal_activation_nock_next_height: 1,
        base_blocks_chunk: 10,
        fakenet_pow_len: 64,
        fakenet_log_difficulty: 2,
    };
    Ok((backend, input))
}

fn manifest_for_deployment(
    deployment: &bridge_dev::hermetic_deploy::DeploymentFacts,
) -> Result<BaseSepoliaE2eManifest> {
    let mut manifest = BaseSepoliaE2eManifest::from_json(MANIFEST_JSON)?;
    let runtime_hash = |name: &str| -> Result<String> {
        deployment
            .runtime_artifacts
            .iter()
            .find(|artifact| artifact.contract_name == name)
            .map(|artifact| format!("{:#x}", artifact.runtime_keccak256))
            .with_context(|| format!("missing {name} runtime fact"))
    };
    manifest.source_chain.fork_block.number = deployment.block_number;
    manifest.source_chain.fork_block.hash = format!("{:#x}", deployment.block_hash);
    manifest.source_chain.fork_block.explorer_url = format!(
        "https://base-sepolia.blockscout.com/block/{}",
        deployment.block_number
    );
    manifest.contracts.message_inbox.proxy.address =
        format!("{:#x}", deployment.addresses.message_inbox_proxy);
    manifest.contracts.message_inbox.implementation.address =
        format!("{:#x}", deployment.addresses.message_inbox_implementation);
    manifest.contracts.nock.address = format!("{:#x}", deployment.addresses.nock);
    manifest
        .contracts
        .message_inbox
        .proxy
        .runtime_code_keccak256 = runtime_hash("ERC1967Proxy")?;
    manifest
        .contracts
        .message_inbox
        .implementation
        .runtime_code_keccak256 = runtime_hash("MessageInbox")?;
    manifest.contracts.nock.runtime_code_keccak256 = runtime_hash("Nock")?;
    manifest
        .contracts
        .message_inbox
        .proxy
        .deployment
        .block_number = 1;
    manifest
        .contracts
        .message_inbox
        .implementation
        .deployment
        .block_number = 1;
    manifest.contracts.nock.deployment.block_number = 1;
    manifest.pristine_state.message_inbox_owner = deployment.bridge_state.owner.clone();
    manifest.pristine_state.nock_owner = deployment.bridge_state.nock_owner.clone();
    manifest.pristine_state.bridge_nodes = deployment.bridge_state.bridge_nodes.clone();
    manifest.pristine_state.threshold = deployment.bridge_state.threshold;
    manifest.pristine_state.withdrawals_enabled = deployment.bridge_state.withdrawals_enabled;
    manifest
        .pristine_state
        .reciprocal_pairing
        .message_inbox_nock = deployment.bridge_state.message_inbox_nock.clone();
    manifest.pristine_state.reciprocal_pairing.nock_inbox =
        deployment.bridge_state.nock_inbox.clone();
    manifest.artifacts.erc1967_proxy.verification_url = format!(
        "https://base-sepolia.blockscout.com/api/v2/smart-contracts/{:#x}",
        deployment.addresses.message_inbox_proxy
    );
    manifest.artifacts.message_inbox.verification_url = format!(
        "https://base-sepolia.blockscout.com/api/v2/smart-contracts/{:#x}",
        deployment.addresses.message_inbox_implementation
    );
    manifest.artifacts.nock.verification_url = format!(
        "https://base-sepolia.blockscout.com/api/v2/smart-contracts/{:#x}",
        deployment.addresses.nock
    );
    manifest.validate()?;
    Ok(manifest)
}

fn fake_artifacts(root: &Path) -> Result<E2eArtifacts> {
    fs::create_dir_all(root)?;
    let artifact = |role: ArtifactRole, name: &str| -> Result<ArtifactFile> {
        let path = root.join(name);
        fs::write(&path, format!("bridge-e2e-{name}"))?;
        Ok(ArtifactFile {
            role,
            path,
            sha256: format!("sha256-{name}"),
            size_bytes: name.len() as u64,
            modified_unix_seconds: Some(1_700_000_000),
            architecture: None,
        })
    };
    Ok(E2eArtifacts {
        bridge: artifact(ArtifactRole::BridgeBinary, "bridge-bin")?,
        node: artifact(ArtifactRole::NodeBinary, "node-bin")?,
        miner: artifact(ArtifactRole::MinerBinary, "zk-pow-mine")?,
        wallet: artifact(ArtifactRole::WalletBinary, "nockchain-wallet")?,
        sequencer_ctl: Some(artifact(ArtifactRole::SequencerCtlBinary, "sequencer-ctl")?),
        bridge_jam: artifact(ArtifactRole::BridgeJam, "bridge.jam")?,
        roswell_jam: artifact(ArtifactRole::RoswellJam, "roswell.jam")?,
        fakenet_genesis_jam: artifact(ArtifactRole::FakenetGenesisJam, "fakenet-genesis.jam")?,
        build: ArtifactBuildMetadata {
            package_version: "test".to_owned(),
            git_revision: Some("0123456789abcdef".to_owned()),
            target_arch: "aarch64".to_owned(),
            target_os: "macos".to_owned(),
        },
    })
}

fn checked_environment() -> BaseE2eEnvironment {
    BaseE2eEnvironment::from_json(MANIFEST_JSON)
        .expect("checked-in Base Sepolia environment must validate")
}

fn workspace_root() -> Result<PathBuf> {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .map(Path::to_path_buf)
        .context("bridge-dev is not under workspace crates directory")
}

struct SourceChainProxy {
    endpoint: String,
    task: JoinHandle<()>,
}

impl SourceChainProxy {
    async fn start(upstream: String) -> Result<Self> {
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let endpoint = format!("http://{}", listener.local_addr()?);
        let client = reqwest::Client::new();
        let task = tokio::spawn(async move {
            while let Ok((stream, _)) = listener.accept().await {
                let upstream = upstream.clone();
                let client = client.clone();
                tokio::spawn(async move {
                    let _ = proxy_source_request(stream, &client, &upstream).await;
                });
            }
        });
        Ok(Self { endpoint, task })
    }
}

impl Drop for SourceChainProxy {
    fn drop(&mut self) {
        self.task.abort();
    }
}

async fn proxy_source_request(
    mut stream: TcpStream,
    client: &reqwest::Client,
    upstream: &str,
) -> Result<()> {
    let request = read_http_json(&mut stream).await?;
    let response = if request.get("method").and_then(Value::as_str) == Some("eth_chainId") {
        json!({
            "jsonrpc": "2.0",
            "id": request.get("id").cloned().unwrap_or(Value::Null),
            "result": "0x14a34",
        })
    } else {
        client
            .post(upstream)
            .json(&request)
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?
    };
    let body = response.to_string();
    let headers = format!(
        "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
        body.len()
    );
    stream.write_all(headers.as_bytes()).await?;
    stream.write_all(body.as_bytes()).await?;
    Ok(())
}

async fn read_http_json(stream: &mut TcpStream) -> Result<Value> {
    let mut buffer = Vec::with_capacity(2_048);
    let header_end = loop {
        let mut chunk = [0_u8; 1_024];
        let read = stream.read(&mut chunk).await?;
        if read == 0 {
            return Err(anyhow!("source proxy connection closed before headers"));
        }
        buffer.extend_from_slice(&chunk[..read]);
        if let Some(position) = find_bytes(&buffer, b"\r\n\r\n") {
            break position + 4;
        }
    };
    let headers = std::str::from_utf8(&buffer[..header_end])?;
    let content_length = headers
        .lines()
        .find_map(|line| {
            let (name, value) = line.split_once(':')?;
            name.eq_ignore_ascii_case("content-length")
                .then(|| value.trim().parse::<usize>().ok())
                .flatten()
        })
        .context("source proxy request missing content-length")?;
    while buffer.len() < header_end + content_length {
        let mut chunk = [0_u8; 1_024];
        let read = stream.read(&mut chunk).await?;
        if read == 0 {
            return Err(anyhow!("source proxy connection closed before body"));
        }
        buffer.extend_from_slice(&chunk[..read]);
    }
    Ok(serde_json::from_slice(
        &buffer[header_end..header_end + content_length],
    )?)
}

fn find_bytes(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

fn preserved_run_root(label: &str) -> PathBuf {
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock must follow Unix epoch")
        .as_nanos();
    let sequence = RUN_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    std::env::temp_dir().join(format!(
        "nockbridge-cluster-config-{label}-{}-{timestamp}-{sequence}",
        std::process::id()
    ))
}

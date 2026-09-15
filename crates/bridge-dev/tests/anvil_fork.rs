use std::fs;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::atomic::{AtomicUsize, Ordering};

use anyhow::{anyhow, Context, Result};
use bridge::shared::e2e_environment::BaseSepoliaE2eManifest;
use bridge_dev::anvil::{AnvilBackend, AnvilConfig, AnvilStartError};
use bridge_dev::anvil_fork::{PinnedAnvilFork, PinnedForkConfig, PinnedForkError};
use bridge_dev::environment::BaseE2eEnvironment;
use bridge_dev::fork_preflight::{ForkPreflight, PristineDeploymentFacts};
use serde::Deserialize;
use serde_json::{json, Value};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener as TokioTcpListener, TcpStream};
use tokio::process::Command;
use tokio::task::JoinHandle;

const MANIFEST_JSON: &str = include_str!("../../bridge/e2e/environments/base-sepolia.json");
const ARCHIVE_RPC_ENV: &str = "BASE_SEPOLIA_ARCHIVE_RPC_URL";

#[tokio::test]
async fn controlled_upstream_proves_pin_preflight_and_failure_boundaries() -> Result<()> {
    let upstream = AnvilBackend::start(AnvilConfig::empty(), &checked_environment()).await?;
    let checked_manifest = BaseSepoliaE2eManifest::from_json(MANIFEST_JSON)?;
    let deployment = deploy_fixture(&upstream, &checked_manifest).await?;
    let latest = read_block(upstream.http_url().as_url().as_str(), "latest").await?;
    let provisional_manifest = fixture_manifest(checked_manifest, &deployment, &latest)?;
    let provisional = BaseE2eEnvironment::from_manifest(provisional_manifest)?;
    let observed = ForkPreflight::observe(upstream.http_url(), &provisional).await?;
    let exact_manifest = manifest_from_facts(provisional.manifest().clone(), &observed)?;
    let exact_environment = BaseE2eEnvironment::from_manifest(exact_manifest.clone())?;
    let source_proxy = SourceChainProxy::start(upstream.http_url().as_url().to_string()).await?;
    let source_url = source_proxy.endpoint.clone();

    assert!(matches!(
        AnvilBackend::start(
            AnvilConfig::fork(source_url.clone(), latest.number),
            &exact_environment
        )
        .await,
        Err(AnvilStartError::ForkRequiresPinnedPreflight)
    ));

    let fork = PinnedAnvilFork::start(
        PinnedForkConfig::new(source_url.clone()),
        &exact_environment,
    )
    .await?;
    assert_eq!(fork.evidence().source_block_number, latest.number);
    assert_eq!(fork.evidence().source_block_hash, latest.hash);
    assert_eq!(fork.evidence().pristine, *fork.pristine().facts());
    assert_eq!(fork.backend().block_number().await?, latest.number);
    assert_eq!(fork.evidence().source_rpc.scheme, "http");
    let evidence_json = serde_json::to_string(fork.evidence())?;
    assert!(!evidence_json.contains(&source_url));
    assert!(!evidence_json.contains("rpc_url"));
    fork.shutdown().await?;

    let mut wrong_hash_manifest = exact_manifest.clone();
    wrong_hash_manifest.source_chain.fork_block.hash = nonzero_hash(9);
    let wrong_hash_environment = BaseE2eEnvironment::from_manifest(wrong_hash_manifest)?;
    let error = PinnedAnvilFork::start(
        PinnedForkConfig::new(source_url.clone()),
        &wrong_hash_environment,
    )
    .await
    .err()
    .ok_or_else(|| anyhow!("wrong source block hash unexpectedly passed"))?;
    assert!(matches!(
        error,
        PinnedForkError::SourceBlockHashMismatch { .. }
    ));

    let mut wrong_code_manifest = exact_manifest.clone();
    wrong_code_manifest
        .contracts
        .message_inbox
        .proxy
        .runtime_code_keccak256 = nonzero_hash(10);
    let wrong_code_environment = BaseE2eEnvironment::from_manifest(wrong_code_manifest)?;
    let error = PinnedAnvilFork::start(
        PinnedForkConfig::new(source_url.clone()),
        &wrong_code_environment,
    )
    .await
    .err()
    .ok_or_else(|| anyhow!("wrong source code hash unexpectedly passed"))?;
    assert!(matches!(error, PinnedForkError::SourceCodeMismatch { .. }));

    let block_one = read_block(upstream.http_url().as_url().as_str(), "0x1").await?;
    let mut predeployment_manifest = exact_manifest.clone();
    predeployment_manifest.source_chain.fork_block.number = 1;
    predeployment_manifest.source_chain.fork_block.hash = block_one.hash;
    predeployment_manifest.source_chain.fork_block.explorer_url =
        "https://base-sepolia.blockscout.com/block/1".to_owned();
    let predeployment_environment = BaseE2eEnvironment::from_manifest(predeployment_manifest)?;
    let error = PinnedAnvilFork::start(
        PinnedForkConfig::new(source_url.clone()),
        &predeployment_environment,
    )
    .await
    .err()
    .ok_or_else(|| anyhow!("predeployment fork unexpectedly passed"))?;
    assert!(matches!(
        error,
        PinnedForkError::SourceArchiveUnavailable { .. }
    ));

    let authority = ["fixture-user", "fixture-value"].join(":");
    let sensitive_source = format!("http://{authority}@127.0.0.1:9/?mode=test");
    let error = PinnedAnvilFork::start(
        PinnedForkConfig::new(sensitive_source.clone()),
        &exact_environment,
    )
    .await
    .err()
    .ok_or_else(|| anyhow!("unreachable credentialed source unexpectedly passed"))?;
    let rendered = format!("{error:#}");
    assert!(!rendered.contains(&sensitive_source));
    assert!(!rendered.contains(&authority));

    let seed_calls = AtomicUsize::new(0);
    if PinnedAnvilFork::start(PinnedForkConfig::new(source_url), &wrong_hash_environment)
        .await
        .is_ok()
    {
        seed_calls.fetch_add(1, Ordering::SeqCst);
    }
    assert_eq!(seed_calls.load(Ordering::SeqCst), 0);

    upstream.shutdown().await?;
    Ok(())
}

#[tokio::test]
async fn base_sepolia_archive_fork_when_explicitly_supplied() -> Result<()> {
    let Some(source_rpc_url) = std::env::var_os(ARCHIVE_RPC_ENV) else {
        return Ok(());
    };
    let environment = checked_environment();
    let fork = PinnedAnvilFork::start(
        PinnedForkConfig::new(source_rpc_url.to_string_lossy().into_owned()),
        &environment,
    )
    .await?;
    assert_eq!(
        fork.evidence().source_block_hash,
        environment.manifest().source_chain.fork_block.hash
    );
    fork.shutdown().await?;
    Ok(())
}

fn checked_environment() -> BaseE2eEnvironment {
    BaseE2eEnvironment::from_json(MANIFEST_JSON)
        .expect("checked-in Base Sepolia environment must validate")
}

async fn deploy_fixture(
    upstream: &AnvilBackend,
    manifest: &BaseSepoliaE2eManifest,
) -> Result<FixtureDeployment> {
    let workspace = workspace_root()?;
    let contracts = workspace.join("crates/bridge/contracts");
    let deployment_dir = contracts.join("deployments");
    fs::create_dir_all(&deployment_dir)?;
    let deployment_path = deployment_dir.join(format!(
        "anvil-fork-{}-{}.json",
        std::process::id(),
        upstream.facts().port
    ));
    let deployer = first_anvil_account(upstream.http_url().as_url().as_str()).await?;
    let forge = std::env::var_os("FORGE_BIN").unwrap_or_else(|| "forge".into());
    let mut command = Command::new(forge);
    command
        .current_dir(&contracts)
        .args([
            "script",
            "forge/Deploy.s.sol:Deploy",
            "--rpc-url",
            upstream.http_url().as_url().as_str(),
            "--broadcast",
            "--unlocked",
            "--sender",
            &deployer,
            "--non-interactive",
            "--quiet",
        ])
        .env("DEPLOYMENTS_PATH", &deployment_path)
        .env("DEPLOY_TARGET_NETWORK", "bridge-e2e-fork-fixture")
        .env("DEPLOYER_ADDRESS", &deployer)
        .env("NOCK_NAME", "Nock")
        .env("NOCK_SYMBOL", "NOCK")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    for (index, node) in manifest.pristine_state.bridge_nodes.iter().enumerate() {
        command.env(format!("BRIDGE_NODE_{index}"), node);
    }
    let output = command.output().await?;
    if !output.status.success() {
        return Err(anyhow!(
            "fixture deployment failed: {}",
            String::from_utf8_lossy(&output.stderr)
        ));
    }
    let deployed: DeploymentFile = serde_json::from_str(&fs::read_to_string(deployment_path)?)?;
    Ok(FixtureDeployment {
        message_inbox_implementation: deployed.message_inbox_implementation.to_ascii_lowercase(),
        message_inbox_proxy: deployed.message_inbox_proxy.to_ascii_lowercase(),
        nock: deployed.nock.to_ascii_lowercase(),
        deployer,
    })
}

async fn first_anvil_account(endpoint: &str) -> Result<String> {
    let cast = std::env::var_os("CAST_BIN").unwrap_or_else(|| "cast".into());
    let output = Command::new(cast)
        .args(["rpc", "--rpc-url", endpoint, "eth_accounts"])
        .output()
        .await?;
    if !output.status.success() {
        return Err(anyhow!("cast eth_accounts failed"));
    }
    let accounts: Vec<String> = serde_json::from_slice(&output.stdout)?;
    accounts
        .first()
        .map(|account| account.to_ascii_lowercase())
        .context("Anvil returned no accounts")
}

async fn read_block(endpoint: &str, block: &str) -> Result<LatestBlock> {
    let cast = std::env::var_os("CAST_BIN").unwrap_or_else(|| "cast".into());
    let output = Command::new(cast)
        .args(["block", block, "--rpc-url", endpoint, "--json"])
        .output()
        .await?;
    if !output.status.success() {
        return Err(anyhow!("cast block failed"));
    }
    let value: Value = serde_json::from_slice(&output.stdout)?;
    let number = value["number"].as_str().context("missing block number")?;
    let hash = value["hash"].as_str().context("missing block hash")?;
    Ok(LatestBlock {
        number: u64::from_str_radix(number.trim_start_matches("0x"), 16)?,
        hash: hash.to_ascii_lowercase(),
    })
}

fn fixture_manifest(
    mut manifest: BaseSepoliaE2eManifest,
    deployment: &FixtureDeployment,
    block: &LatestBlock,
) -> Result<BaseSepoliaE2eManifest> {
    manifest.source_chain.fork_block.number = block.number;
    manifest.source_chain.fork_block.hash = block.hash.clone();
    manifest.source_chain.fork_block.explorer_url =
        format!("https://base-sepolia.blockscout.com/block/{}", block.number);
    manifest.contracts.message_inbox.proxy.address = deployment.message_inbox_proxy.clone();
    manifest.contracts.message_inbox.implementation.address =
        deployment.message_inbox_implementation.clone();
    manifest.contracts.nock.address = deployment.nock.clone();
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
    manifest.pristine_state.message_inbox_owner = deployment.deployer.clone();
    manifest.pristine_state.nock_owner = deployment.deployer.clone();
    manifest
        .pristine_state
        .reciprocal_pairing
        .message_inbox_nock = deployment.nock.clone();
    manifest.pristine_state.reciprocal_pairing.nock_inbox = deployment.message_inbox_proxy.clone();
    manifest.artifacts.erc1967_proxy.verification_url =
        verification_url(&deployment.message_inbox_proxy);
    manifest.artifacts.message_inbox.verification_url =
        verification_url(&deployment.message_inbox_implementation);
    manifest.artifacts.nock.verification_url = verification_url(&deployment.nock);
    manifest.validate()?;
    Ok(manifest)
}

fn manifest_from_facts(
    mut manifest: BaseSepoliaE2eManifest,
    facts: &PristineDeploymentFacts,
) -> Result<BaseSepoliaE2eManifest> {
    manifest.source_chain.fork_block.number = facts.source_block.number;
    manifest.source_chain.fork_block.hash = facts.source_block.hash.clone();
    manifest
        .contracts
        .message_inbox
        .proxy
        .runtime_code_keccak256 = facts.message_inbox_proxy.keccak256.clone();
    manifest
        .contracts
        .message_inbox
        .implementation
        .runtime_code_keccak256 = facts.message_inbox_implementation.keccak256.clone();
    manifest.contracts.nock.runtime_code_keccak256 = facts.nock.keccak256.clone();
    manifest.pristine_state.message_inbox_owner = facts.message_inbox_owner.clone();
    manifest.pristine_state.nock_owner = facts.nock_owner.clone();
    manifest.pristine_state.bridge_nodes = facts.bridge_nodes.clone();
    manifest.pristine_state.threshold = facts.threshold;
    manifest.pristine_state.withdrawals_enabled = facts.withdrawals_enabled;
    manifest.pristine_state.reciprocal_pairing = facts.reciprocal_pairing.clone();
    manifest.validate()?;
    Ok(manifest)
}

fn verification_url(address: &str) -> String {
    format!("https://base-sepolia.blockscout.com/api/v2/smart-contracts/{address}")
}

fn nonzero_hash(value: u8) -> String {
    format!("0x{:064x}", value)
}

fn workspace_root() -> Result<PathBuf> {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .map(Path::to_path_buf)
        .context("bridge-dev is not under workspace crates directory")
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct DeploymentFile {
    message_inbox_implementation: String,
    message_inbox_proxy: String,
    nock: String,
}

struct FixtureDeployment {
    message_inbox_implementation: String,
    message_inbox_proxy: String,
    nock: String,
    deployer: String,
}

struct LatestBlock {
    number: u64,
    hash: String,
}

struct SourceChainProxy {
    endpoint: String,
    task: JoinHandle<()>,
}

impl SourceChainProxy {
    async fn start(upstream: String) -> Result<Self> {
        let listener = TokioTcpListener::bind("127.0.0.1:0").await?;
        let endpoint = format!("http://{}", listener.local_addr()?);
        let client = reqwest::Client::new();
        let task = tokio::spawn(async move {
            loop {
                let Ok((stream, _)) = listener.accept().await else {
                    break;
                };
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
            "result": format!("0x{:x}", 84_532u64),
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
    let mut buffer = Vec::with_capacity(2048);
    let header_end = loop {
        let mut chunk = [0u8; 1024];
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
        let mut chunk = [0u8; 1024];
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

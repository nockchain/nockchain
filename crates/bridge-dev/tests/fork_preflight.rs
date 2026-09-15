use std::fs;
use std::net::TcpListener as StdTcpListener;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::time::Duration;

use anyhow::{anyhow, Context, Result};
use bridge::shared::e2e_environment::{
    BaseSepoliaE2eManifest, BASE_SEPOLIA_E2E_CHAIN_ID, BASE_SEPOLIA_E2E_ENVIRONMENT_ID,
};
use bridge_dev::base_backend::BaseBackend;
use bridge_dev::environment::BaseE2eEnvironment;
use bridge_dev::fork_preflight::{
    DeploymentContract, DeploymentMismatch, ForkPreflight, PristineDeploymentFacts,
};
use bridge_dev::nonproduction_guard::{LoopbackBaseRpcUrl, NonproductionGuard};
use serde::Deserialize;
use serde_json::Value;
use tokio::net::TcpStream;
use tokio::process::{Child, Command};
use tokio::time::{sleep, Instant};

const MANIFEST_JSON: &str = include_str!("../../bridge/e2e/environments/base-sepolia.json");
const STARTUP_TIMEOUT: Duration = Duration::from_secs(10);
static RUN_ID: AtomicU64 = AtomicU64::new(1);

#[tokio::test]
async fn deployed_fixture_passes_and_every_independent_drift_fails() -> Result<()> {
    let mut anvil = TestAnvil::start().await?;
    let checked_in = BaseSepoliaE2eManifest::from_json(MANIFEST_JSON)?;
    let guarded = NonproductionGuard::acquire(
        &anvil.endpoint, BASE_SEPOLIA_E2E_ENVIRONMENT_ID, &checked_in,
    )
    .await?;
    let _backend = BaseBackend::new(guarded)?;

    let deployment = deploy_fixture(&anvil, &checked_in).await?;
    let block = read_latest_block(&anvil.endpoint).await?;
    let provisional_manifest = fixture_manifest(checked_in, &deployment, &block)?;
    let provisional = BaseE2eEnvironment::from_manifest(provisional_manifest)?;
    let observed = ForkPreflight::observe(&anvil.rpc, &provisional).await?;
    let exact_manifest = manifest_from_facts(provisional.manifest().clone(), &observed)?;
    let environment = BaseE2eEnvironment::from_manifest(exact_manifest)?;

    let verified = ForkPreflight::verify(&anvil.rpc, &environment).await?;
    let facts = verified.into_facts();
    assert_eq!(ForkPreflight::compare(&environment, &facts), Vec::new());

    assert_drift(
        &environment,
        &facts,
        |candidate| {
            candidate.source_block.number += 1;
        },
        |mismatch| matches!(mismatch, DeploymentMismatch::SourceBlockNumber { .. }),
    );
    assert_drift(
        &environment,
        &facts,
        |candidate| {
            candidate.source_block.hash = nonzero_hash(1);
        },
        |mismatch| matches!(mismatch, DeploymentMismatch::SourceBlockHash { .. }),
    );
    assert_drift(
        &environment,
        &facts,
        |candidate| {
            candidate.message_inbox_proxy.keccak256 = nonzero_hash(2);
        },
        |mismatch| {
            matches!(
                mismatch,
                DeploymentMismatch::RuntimeCodeHash {
                    contract: DeploymentContract::MessageInboxProxy,
                    ..
                }
            )
        },
    );
    assert_drift(
        &environment,
        &facts,
        |candidate| {
            candidate.message_inbox_implementation.keccak256 = nonzero_hash(3);
        },
        |mismatch| {
            matches!(
                mismatch,
                DeploymentMismatch::RuntimeCodeHash {
                    contract: DeploymentContract::MessageInboxImplementation,
                    ..
                }
            )
        },
    );
    assert_drift(
        &environment,
        &facts,
        |candidate| {
            candidate.nock.keccak256 = nonzero_hash(4);
        },
        |mismatch| {
            matches!(
                mismatch,
                DeploymentMismatch::RuntimeCodeHash {
                    contract: DeploymentContract::Nock,
                    ..
                }
            )
        },
    );
    assert_drift(
        &environment,
        &facts,
        |candidate| {
            candidate.message_inbox_proxy.byte_len = 0;
        },
        |mismatch| {
            matches!(
                mismatch,
                DeploymentMismatch::EmptyRuntimeCode {
                    contract: DeploymentContract::MessageInboxProxy,
                    ..
                }
            )
        },
    );
    assert_drift(
        &environment,
        &facts,
        |candidate| {
            candidate.proxy_implementation = different_address(&candidate.proxy_implementation);
        },
        |mismatch| matches!(mismatch, DeploymentMismatch::ProxyImplementation { .. }),
    );
    assert_drift(
        &environment,
        &facts,
        |candidate| {
            candidate.proxy_implementation =
                "0x0000000000000000000000000000000000000000".to_owned();
        },
        |mismatch| matches!(mismatch, DeploymentMismatch::EmptyProxyImplementationSlot),
    );
    assert_drift(
        &environment,
        &facts,
        |candidate| {
            candidate.message_inbox_owner = different_address(&candidate.message_inbox_owner);
        },
        |mismatch| matches!(mismatch, DeploymentMismatch::MessageInboxOwner { .. }),
    );
    assert_drift(
        &environment,
        &facts,
        |candidate| {
            candidate.nock_owner = different_address(&candidate.nock_owner);
        },
        |mismatch| matches!(mismatch, DeploymentMismatch::NockOwner { .. }),
    );
    for index in 0..5 {
        assert_drift(
            &environment,
            &facts,
            |candidate| {
                candidate.bridge_nodes[index] = different_address(&candidate.bridge_nodes[index]);
            },
            |mismatch| matches!(mismatch, DeploymentMismatch::BridgeNode { index: observed, .. } if *observed == index),
        );
    }
    assert_drift(
        &environment,
        &facts,
        |candidate| {
            candidate.bridge_nodes[1] = candidate.bridge_nodes[0].clone();
        },
        |mismatch| matches!(mismatch, DeploymentMismatch::DuplicateBridgeNode { .. }),
    );
    assert_drift(
        &environment,
        &facts,
        |candidate| {
            candidate.threshold += 1;
        },
        |mismatch| matches!(mismatch, DeploymentMismatch::Threshold { .. }),
    );
    assert_drift(
        &environment,
        &facts,
        |candidate| {
            candidate.withdrawals_enabled = !candidate.withdrawals_enabled;
        },
        |mismatch| matches!(mismatch, DeploymentMismatch::WithdrawalsEnabled { .. }),
    );
    assert_drift(
        &environment,
        &facts,
        |candidate| {
            candidate.reciprocal_pairing.message_inbox_nock =
                different_address(&candidate.reciprocal_pairing.message_inbox_nock);
        },
        |mismatch| matches!(mismatch, DeploymentMismatch::MessageInboxNock { .. }),
    );
    assert_drift(
        &environment,
        &facts,
        |candidate| {
            candidate.reciprocal_pairing.nock_inbox =
                different_address(&candidate.reciprocal_pairing.nock_inbox);
        },
        |mismatch| matches!(mismatch, DeploymentMismatch::NockInbox { .. }),
    );
    assert_drift(
        &environment,
        &facts,
        |candidate| {
            candidate.protocol.withdrawal_wire_id.push_str("-drift");
        },
        |mismatch| matches!(mismatch, DeploymentMismatch::LocalMetadata { field_path, .. } if field_path == "protocol.withdrawal_wire_id"),
    );
    assert_drift(
        &environment,
        &facts,
        |candidate| {
            candidate.protocol.withdrawal_policy_id.push_str("-drift");
        },
        |mismatch| matches!(mismatch, DeploymentMismatch::LocalMetadata { field_path, .. } if field_path == "protocol.withdrawal_policy_id"),
    );
    assert_drift(
        &environment,
        &facts,
        |candidate| {
            candidate.artifacts.message_inbox.abi_sha256 = nonzero_hash(5);
        },
        |mismatch| matches!(mismatch, DeploymentMismatch::LocalMetadata { field_path, .. } if field_path == "artifacts.message_inbox.abi_sha256"),
    );
    assert_drift(
        &environment,
        &facts,
        |candidate| {
            candidate.artifacts.nock.verified_artifact_sha256 = nonzero_hash(6);
        },
        |mismatch| matches!(mismatch, DeploymentMismatch::LocalMetadata { field_path, .. } if field_path == "artifacts.nock.verified_artifact_sha256"),
    );

    let evidence = serde_json::to_value(&facts)?;
    let serialized = evidence.to_string().to_ascii_lowercase();
    assert!(!serialized.contains("rpc_url"));
    assert!(!serialized.contains("private_key"));
    assert!(!serialized.contains("credential"));

    let mut mismatch = facts.clone();
    mismatch.threshold += 1;
    let seed_calls = AtomicUsize::new(0);
    if ForkPreflight::compare(&environment, &mismatch).is_empty() {
        seed_calls.fetch_add(1, Ordering::SeqCst);
    }
    assert_eq!(seed_calls.load(Ordering::SeqCst), 0);

    anvil.shutdown().await
}

fn assert_drift<M, P>(
    environment: &BaseE2eEnvironment,
    facts: &PristineDeploymentFacts,
    mutate: M,
    predicate: P,
) where
    M: FnOnce(&mut PristineDeploymentFacts),
    P: Fn(&DeploymentMismatch) -> bool,
{
    let mut candidate = facts.clone();
    mutate(&mut candidate);
    let mismatches = ForkPreflight::compare(environment, &candidate);
    assert!(
        mismatches.iter().any(predicate),
        "expected mismatch not found in {mismatches:?}"
    );
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

fn different_address(address: &str) -> String {
    let Some((last_index, last)) = address.char_indices().next_back() else {
        return "0x0000000000000000000000000000000000000001".to_owned();
    };
    let replacement = if last == '1' { '2' } else { '1' };
    format!("{}{replacement}", &address[..last_index])
}

async fn deploy_fixture(
    anvil: &TestAnvil,
    manifest: &BaseSepoliaE2eManifest,
) -> Result<FixtureDeployment> {
    let workspace = workspace_root()?;
    let contracts = workspace.join("crates/bridge/contracts");
    let deployment_dir = contracts.join("deployments");
    fs::create_dir_all(&deployment_dir)?;
    let run_name = anvil
        .run_dir
        .file_name()
        .and_then(|name| name.to_str())
        .context("non-UTF-8 fixture run directory")?;
    let deployment_path = deployment_dir.join(format!("{run_name}.json"));
    let forge = std::env::var_os("FORGE_BIN").unwrap_or_else(|| "forge".into());
    let mut command = Command::new(forge);
    command
        .current_dir(&contracts)
        .args([
            "script", "forge/Deploy.s.sol:Deploy", "--rpc-url", &anvil.endpoint, "--broadcast",
            "--unlocked", "--sender", &anvil.deployer, "--non-interactive", "--quiet",
        ])
        .env("DEPLOYMENTS_PATH", &deployment_path)
        .env("DEPLOY_TARGET_NETWORK", "bridge-e2e-fixture")
        .env("DEPLOYER_ADDRESS", &anvil.deployer)
        .env("NOCK_NAME", "Nock")
        .env("NOCK_SYMBOL", "NOCK")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    for (index, node) in manifest.pristine_state.bridge_nodes.iter().enumerate() {
        command.env(format!("BRIDGE_NODE_{index}"), node);
    }
    let output = command
        .output()
        .await
        .context("running fixture deployment")?;
    if !output.status.success() {
        return Err(anyhow!(
            "fixture deployment failed: {}",
            String::from_utf8_lossy(&output.stderr)
        ));
    }
    let deployed: DeploymentFile = serde_json::from_str(&fs::read_to_string(&deployment_path)?)?;
    Ok(FixtureDeployment {
        message_inbox_proxy: deployed.message_inbox_proxy.to_ascii_lowercase(),
        message_inbox_implementation: deployed.message_inbox_implementation.to_ascii_lowercase(),
        nock: deployed.nock.to_ascii_lowercase(),
        deployer: anvil.deployer.clone(),
    })
}

async fn read_latest_block(endpoint: &str) -> Result<LatestBlock> {
    let cast = std::env::var_os("CAST_BIN").unwrap_or_else(|| "cast".into());
    let output = Command::new(cast)
        .args(["block", "latest", "--rpc-url", endpoint, "--json"])
        .output()
        .await?;
    if !output.status.success() {
        return Err(anyhow!("cast block latest failed"));
    }
    let value: Value = serde_json::from_slice(&output.stdout)?;
    let number = value["number"]
        .as_str()
        .context("missing latest block number")?;
    let hash = value["hash"]
        .as_str()
        .context("missing latest block hash")?;
    Ok(LatestBlock {
        number: u64::from_str_radix(number.trim_start_matches("0x"), 16)?,
        hash: hash.to_ascii_lowercase(),
    })
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

#[derive(Deserialize)]
struct AnvilConfig {
    available_accounts: Vec<String>,
}

struct TestAnvil {
    endpoint: String,
    rpc: LoopbackBaseRpcUrl,
    deployer: String,
    run_dir: PathBuf,
    child: Child,
}

impl TestAnvil {
    async fn start() -> Result<Self> {
        let port = reserve_port()?;
        let run_id = RUN_ID.fetch_add(1, Ordering::Relaxed);
        let run_dir = std::env::temp_dir().join(format!(
            "nockbridge-fork-preflight-{}-{run_id}",
            std::process::id()
        ));
        fs::create_dir_all(&run_dir)?;
        let config_path = run_dir.join("anvil.json");
        let anvil_bin = std::env::var_os("ANVIL_BIN").unwrap_or_else(|| "anvil".into());
        let mut child = Command::new(anvil_bin)
            .args([
                "--silent",
                "--port",
                &port.to_string(),
                "--chain-id",
                &BASE_SEPOLIA_E2E_CHAIN_ID.to_string(),
                "--config-out",
                config_path.to_str().context("non-UTF-8 config path")?,
            ])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .spawn()
            .context("starting real Anvil process")?;
        let endpoint = format!("http://127.0.0.1:{port}");
        let deadline = Instant::now() + STARTUP_TIMEOUT;
        loop {
            if TcpStream::connect(("127.0.0.1", port)).await.is_ok() && config_path.is_file() {
                break;
            }
            if let Some(status) = child.try_wait()? {
                return Err(anyhow!("Anvil exited before readiness: {status}"));
            }
            if Instant::now() >= deadline {
                return Err(anyhow!("Anvil did not listen before timeout"));
            }
            sleep(Duration::from_millis(50)).await;
        }
        let config: AnvilConfig = serde_json::from_str(&fs::read_to_string(config_path)?)?;
        let deployer = config
            .available_accounts
            .first()
            .context("Anvil emitted no accounts")?
            .to_ascii_lowercase();
        Ok(Self {
            rpc: LoopbackBaseRpcUrl::parse(&endpoint)?,
            endpoint,
            deployer,
            run_dir,
            child,
        })
    }

    async fn shutdown(&mut self) -> Result<()> {
        self.child.kill().await.context("stopping Anvil")?;
        let _ = self.child.wait().await?;
        Ok(())
    }
}

impl Drop for TestAnvil {
    fn drop(&mut self) {
        let _ = self.child.start_kill();
    }
}

fn workspace_root() -> Result<PathBuf> {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .map(Path::to_path_buf)
        .context("bridge-dev is not under workspace crates directory")
}

fn reserve_port() -> Result<u16> {
    let listener = StdTcpListener::bind("127.0.0.1:0")?;
    Ok(listener.local_addr()?.port())
}

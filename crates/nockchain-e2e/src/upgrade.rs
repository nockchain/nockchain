use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{anyhow, Result};
use nockchain_testkit::NodeSpec;
use tokio::process::Command;

use crate::node::{NodeManager, NodeMode};

pub struct UpgradeTestConfig {
    pub activation_height: u64,
    pub v1_phase: u64,
    pub pow_len: u64,
    pub log_difficulty: u64,
    pub base_grpc_port: u16,
    pub base_private_grpc_port: u16,
    pub base_p2p_port: u16,
    pub mining_pkh: Option<String>,
    pub update_candidate_interval_secs: Option<u64>,
    pub nockchain_bin: PathBuf,
    pub work_dir: PathBuf,
}

impl UpgradeTestConfig {
    pub fn new(activation_height: u64, nockchain_bin: PathBuf, work_dir: PathBuf) -> Self {
        Self {
            activation_height,
            v1_phase: 1,
            pow_len: 2,
            log_difficulty: 1,
            base_grpc_port: 6300,
            base_private_grpc_port: 7300,
            base_p2p_port: 4300,
            mining_pkh: None,
            update_candidate_interval_secs: None,
            nockchain_bin,
            work_dir,
        }
    }
}

pub struct NockchainCluster {
    node_manager: NodeManager,
    run_dir: PathBuf,
    node_id: String,
    fakenet_v1_phase: Option<u64>,
    fakenet_bythos_phase: Option<u64>,
}

impl NockchainCluster {
    pub async fn with_activation_height(config: UpgradeTestConfig) -> Result<Self> {
        let node_id = "node-a".to_string();
        let spec = NodeSpec {
            id: node_id.clone(),
            grpc_public_addr: None,
            grpc_private_port: None,
            grpc_enabled: true,
            data_dir: None,
            fakenet: true,
            mine: config.mining_pkh.is_some(),
            mining_pkh: config.mining_pkh.clone(),
            peers: Vec::new(),
            peer_from: Vec::new(),
            restart_peer_from: Vec::new(),
            force_peers: Vec::new(),
            bind: vec![format!("/ip4/127.0.0.1/udp/{}/quic-v1", config.base_p2p_port)],
            new_state: true,
            no_default_peers: true,
            allowed_peers_path: None,
            fakenet_pow_len: Some(config.pow_len),
            fakenet_log_difficulty: Some(config.log_difficulty),
            fakenet_v1_phase: Some(config.v1_phase),
            fakenet_bythos_phase: Some(config.activation_height),
            fakenet_update_candidate_interval_secs: config.update_candidate_interval_secs,
            fakenet_genesis_jam_path: None,
            extra_args: Vec::new(),
            env: BTreeMap::new(),
            binary: Some(config.nockchain_bin.clone()),
        };

        let node_manager = NodeManager::new(
            &[spec],
            &config.work_dir,
            Some(config.nockchain_bin),
            config.base_grpc_port,
            config.base_private_grpc_port,
            config.base_p2p_port,
            NodeMode::Process,
        )?;

        Ok(Self {
            node_manager,
            run_dir: config.work_dir,
            node_id,
            fakenet_v1_phase: Some(config.v1_phase),
            fakenet_bythos_phase: Some(config.activation_height),
        })
    }

    pub async fn start(&mut self) -> Result<()> {
        let ids = vec![self.node_id.clone()];
        self.node_manager.start_nodes(&ids).await
    }

    pub async fn shutdown(mut self) -> Result<()> {
        self.node_manager.shutdown_all().await
    }

    pub async fn wait_for_grpc(&self, timeout: Duration) -> Result<()> {
        self.node_manager
            .wait_for_grpc(&self.node_id, timeout)
            .await
    }

    pub async fn mine_to_height(&self, height: u64, timeout: Duration) -> Result<()> {
        self.node_manager
            .wait_for_height(&self.node_id, height, timeout)
            .await
            .map(|_| ())
    }

    pub async fn current_height(&self) -> Result<u64> {
        let head = self.node_manager.fetch_private_head(&self.node_id).await?;
        Ok(head.height)
    }

    pub async fn fetch_constants(&self) -> Result<Vec<u8>> {
        self.node_manager.fetch_constants(&self.node_id).await
    }

    pub async fn set_mining_pkh(&mut self, mining_pkh: String) -> Result<()> {
        self.node_manager
            .set_mining_pkh(&self.node_id, mining_pkh)
            .await
    }

    pub async fn stop_mining(&mut self) -> Result<()> {
        self.node_manager.disable_mining(&self.node_id).await
    }

    pub fn grpc_public_addr(&self) -> Result<&str> {
        self.node_manager.grpc_public_addr(&self.node_id)
    }

    pub fn grpc_private_addr(&self) -> Result<&str> {
        self.node_manager.grpc_private_addr(&self.node_id)
    }

    pub fn wallet(&self, name: &str, wallet_bin: PathBuf) -> Result<WalletClient> {
        if is_explicit_path(&wallet_bin) && !wallet_bin.exists() {
            return Err(anyhow!(
                "wallet binary not found at {}",
                wallet_bin.display()
            ));
        }

        let wallet_dir = self.run_dir.join("wallets").join(name);
        std::fs::create_dir_all(&wallet_dir)?;
        let private_port = self.node_manager.grpc_private_port(&self.node_id)?;
        let fakenet = self.node_manager.is_fakenet(&self.node_id)?;

        Ok(WalletClient {
            wallet_bin,
            wallet_dir,
            private_port,
            fakenet,
            fakenet_v1_phase: self.fakenet_v1_phase,
            fakenet_bythos_phase: self.fakenet_bythos_phase,
        })
    }
}

pub struct WalletClient {
    wallet_bin: PathBuf,
    wallet_dir: PathBuf,
    private_port: u16,
    fakenet: bool,
    fakenet_v1_phase: Option<u64>,
    fakenet_bythos_phase: Option<u64>,
}

pub struct WalletCommandOutput {
    pub stdout: String,
    pub stderr: String,
}

impl WalletClient {
    pub async fn run(&self, command: &str, args: &[String]) -> Result<WalletCommandOutput> {
        self.run_expect(command, args, 0).await
    }

    pub async fn run_expect(
        &self,
        command: &str,
        args: &[String],
        expected_exit_code: i32,
    ) -> Result<WalletCommandOutput> {
        let mut cmd = Command::new(&self.wallet_bin);
        cmd.current_dir(&self.wallet_dir)
            .env("NOCKAPP_HOME", &self.wallet_dir)
            .arg("--client")
            .arg("private")
            .arg("--private-grpc-server-port")
            .arg(self.private_port.to_string());
        if self.fakenet {
            cmd.arg("--fakenet");
            if let Some(phase) = self.fakenet_v1_phase {
                cmd.arg("--fakenet-v1-phase").arg(phase.to_string());
            }
            if let Some(phase) = self.fakenet_bythos_phase {
                cmd.arg("--fakenet-bythos-phase").arg(phase.to_string());
            }
        }
        cmd.arg(command).args(args);

        let output = cmd.output().await?;
        let stdout = String::from_utf8_lossy(&output.stdout).to_string();
        let stderr = String::from_utf8_lossy(&output.stderr).to_string();
        let exit_code = output
            .status
            .code()
            .ok_or_else(|| anyhow!("wallet command terminated by signal"))?;
        if exit_code != expected_exit_code {
            return Err(anyhow!(
                "wallet '{}' exited with {} (expected {}): {}",
                command,
                exit_code,
                expected_exit_code,
                stderr.trim()
            ));
        }

        Ok(WalletCommandOutput { stdout, stderr })
    }

    pub fn dir(&self) -> &Path {
        &self.wallet_dir
    }
}

fn is_explicit_path(path: &Path) -> bool {
    path.components().count() > 1 || path.is_absolute()
}

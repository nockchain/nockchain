use std::collections::{HashMap, HashSet};
use std::future::Future;
use std::net::{SocketAddr, TcpListener as StdTcpListener, UdpSocket as StdUdpSocket};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use anyhow::{anyhow, Context, Result};
use nockapp_grpc_proto::pb::common::v1::Hash as PbHash;
use nockchain_testkit::{NodeSpec, PeerFrom};
use testcontainers::core::{AccessMode, IntoContainerPort, Mount, WaitFor};
use testcontainers::runners::AsyncRunner;
use testcontainers::{GenericImage, ImageExt, TestcontainersError};
use tokio::process::{Child, Command};
use tokio::time::{sleep, timeout, Instant};
use tracing::info;

use crate::grpc::{
    configure_mining_payout, fetch_constants_private, fetch_head, fetch_heaviest_private,
    set_mining_enabled, wait_for_height, wait_for_height_private, wait_for_ready, HeadInfo,
    PrivateHeadInfo,
};

const PEER_ID_FILENAME: &str = ".nockchain_identity.peerid";
const DOCKER_WORKDIR: &str = "/var/nockchain";
const PEER_ID_WAIT_TIMEOUT: Duration = Duration::from_secs(120);
const PROCESS_SHUTDOWN_GRACE: Duration = Duration::from_secs(10);
const MINER_SHUTDOWN_GRACE: Duration = Duration::from_secs(20);
// Mandatory verifier setup can take several minutes on a fresh node. Match
// the established standalone CPU smoke's boot bound, not post-start RPC waits.
const NODE_STARTUP_TIMEOUT: Duration = Duration::from_secs(1800);

#[derive(Clone, Debug)]
pub enum NodeMode {
    Process,
    Docker {
        image: String,
        network: String,
        miner_image: String,
        miner_binary: PathBuf,
    },
}

pub struct NodeManager {
    mode: NodeMode,
    configs: HashMap<String, NodeConfig>,
    handles: HashMap<String, NodeHandle>,
}

impl NodeManager {
    pub fn new(
        specs: &[NodeSpec],
        run_dir: &Path,
        nockchain_bin: Option<PathBuf>,
        miner_bin: Option<PathBuf>,
        base_grpc_port: u16,
        base_private_grpc_port: u16,
        base_p2p_port: u16,
        mode: NodeMode,
    ) -> Result<Self> {
        let p2p_ports = resolve_p2p_ports(specs, base_p2p_port)?;
        let mut configs = HashMap::new();
        for (index, spec) in specs.iter().enumerate() {
            let grpc_public_addr = match &spec.grpc_public_addr {
                Some(addr) => addr.clone(),
                None => format!("127.0.0.1:{}", base_grpc_port + index as u16),
            };
            let _addr: SocketAddr = grpc_public_addr
                .parse()
                .with_context(|| format!("invalid grpc_public_addr for {}", spec.id))?;
            let grpc_public_port = parse_socket_port(&grpc_public_addr)?;

            let grpc_private_port = spec
                .grpc_private_port
                .unwrap_or(base_private_grpc_port + index as u16);
            let grpc_private_addr = format!("127.0.0.1:{grpc_private_port}");
            let p2p_port = *p2p_ports
                .get(&spec.id)
                .ok_or_else(|| anyhow!("missing p2p port assignment for {}", spec.id))?;

            let work_dir = resolve_work_dir(run_dir, spec)?;
            let binary = absolutize_binary(expand_env_path(resolve_binary(
                spec,
                nockchain_bin.as_ref(),
            ))?)?;
            let miner_binary = match &mode {
                NodeMode::Process => absolutize_binary(expand_env_path(
                    spec.miner_binary
                        .as_ref()
                        .or(miner_bin.as_ref())
                        .cloned()
                        .unwrap_or_else(|| PathBuf::from("target/release/zk-pow-mine")),
                )?)?,
                NodeMode::Docker { miner_binary, .. } => {
                    expand_env_path(spec.miner_binary.as_ref().unwrap_or(miner_binary).clone())?
                }
            };

            let supports_bythos_phase = match &mode {
                NodeMode::Process => binary_supports_flag(&binary, "--fakenet-bythos-phase")?,
                NodeMode::Docker { .. } => true,
            };

            let config = NodeConfig {
                base_spec: spec.clone(),
                spec: spec.clone(),
                grpc_public_addr,
                grpc_public_port,
                grpc_private_addr,
                grpc_private_port,
                grpc_private_host_port: None,
                p2p_port,
                miner_work_dir: work_dir.join("miner"),
                work_dir,
                binary,
                miner_binary,
                miner_supports_cpu_flag: None,
                operator_token: effective_auth_env(spec, PRIVATE_GRPC_AUTH_TOKEN_ENV),
                miner_token: effective_auth_env(spec, PRIVATE_GRPC_MINER_TOKEN_ENV),
                supports_bythos_phase,
            };
            configs.insert(spec.id.clone(), config);
        }

        if let NodeMode::Docker { network, .. } = &mode {
            create_docker_network(network)?;
        }

        Ok(Self {
            mode,
            configs,
            handles: HashMap::new(),
        })
    }

    pub async fn start_nodes(&mut self, ids: &[String]) -> Result<()> {
        self.ensure_ports_available(ids)?;
        let mut started = Vec::new();
        if let Err(error) = self.start_nodes_inner(ids, &mut started).await {
            return match self.stop_nodes(&started).await {
                Ok(()) => Err(error),
                Err(cleanup) => {
                    Err(error.context(format!("startup cleanup also failed: {cleanup:#}")))
                }
            };
        }
        Ok(())
    }

    async fn start_nodes_inner(&mut self, ids: &[String], started: &mut Vec<String>) -> Result<()> {
        let port_map = match &self.mode {
            NodeMode::Docker { .. } => Some(build_p2p_port_map(&self.configs)?),
            NodeMode::Process => None,
        };
        for id in ids {
            if self.handles.contains_key(id) {
                self.ensure_node_running(id).await?;
                continue;
            }
            let config = self
                .configs
                .get_mut(id)
                .ok_or_else(|| anyhow!("unknown node id '{id}'"))?;
            config.refresh_credentials();
            let config = self.configs.get(id).expect("known node config");
            let grpc_enabled = config.spec.grpc_enabled;
            let mine = config.spec.mine;
            let public_port = config.grpc_public_port;
            let private_port = config.grpc_private_port;
            let peer_from = self.resolve_peer_from(&config.spec).await?;
            let node = match &self.mode {
                NodeMode::Process => spawn_process_node(config, &peer_from).await?,
                NodeMode::Docker { image, network, .. } => {
                    let peers = self
                        .rewrite_peers_for_docker(
                            &config.spec,
                            port_map.as_ref().expect("Docker port map"),
                            network,
                        )
                        .await?;
                    ChildHandle::Docker(
                        spawn_docker_node(config, image, network, &peer_from, &peers).await?,
                    )
                }
            };
            // Register ownership before any fallible port, relay, or RPC setup.
            self.handles
                .insert(id.clone(), NodeHandle { node, miner: None });
            started.push(id.clone());
            self.configs
                .get_mut(id)
                .expect("known node config")
                .spec
                .new_state = false;
            if grpc_enabled {
                if let ChildHandle::Docker(container) =
                    &self.handles.get(id).expect("owned node").node
                {
                    let host = container.get_host().await?;
                    let public_host_port = container.get_host_port_ipv4(public_port.tcp()).await?;
                    let private_host_port =
                        container.get_host_port_ipv4(private_port.tcp()).await?;
                    if let NodeMode::Docker { network, .. } = &self.mode {
                        start_private_grpc_forwarder(
                            &docker_container_name(network, id),
                            network,
                            private_port,
                        )?;
                    }
                    let config = self.configs.get_mut(id).expect("known node config");
                    config.grpc_public_addr = format!("{host}:{public_host_port}");
                    config.grpc_private_addr = format!("{host}:{private_host_port}");
                    config.grpc_private_host_port = Some(private_host_port);
                }
                self.while_node_running(
                    id,
                    wait_for_height_private(
                        self.grpc_private_addr(id)?,
                        self.operator_token(id)?,
                        0,
                        NODE_STARTUP_TIMEOUT,
                    ),
                )
                .await?;
            }
            if mine {
                self.start_miner(id).await?;
            }
        }
        self.ensure_running().await
    }

    pub async fn stop_nodes(&mut self, ids: &[String]) -> Result<()> {
        let mut errors = Vec::new();
        // Stop all selected proof workers while their nodes are still available.
        for id in ids {
            if let Err(error) = self.stop_miner(id).await {
                errors.push(format!("stop miner '{id}': {error:#}"));
            }
        }
        for id in ids {
            if let Some(handle) = self.handles.remove(id) {
                let config = self.configs.get(id).expect("owned node config");
                if let NodeMode::Docker { network, .. } = &self.mode {
                    if config.spec.grpc_enabled {
                        let forwarder =
                            private_grpc_forwarder_name(&docker_container_name(network, id));
                        if let Err(error) = persist_forwarder_logs(&config.work_dir, &forwarder) {
                            errors.push(format!("capture relay '{id}': {error:#}"));
                        }
                        if let Err(error) = remove_stale_docker_container(&forwarder) {
                            errors.push(format!("remove relay '{id}': {error:#}"));
                        }
                    }
                }
                if let Err(error) = stop_owned_child(handle.node, &config.work_dir, false).await {
                    errors.push(format!("stop node '{id}': {error:#}"));
                }
            }
        }
        if errors.is_empty() {
            Ok(())
        } else {
            Err(anyhow!(errors.join("; ")))
        }
    }

    pub async fn shutdown_all(&mut self) -> Result<()> {
        let ids: Vec<String> = self.handles.keys().cloned().collect();
        let stop = self.stop_nodes(&ids).await;
        let network = match &self.mode {
            NodeMode::Docker { network, .. } => remove_docker_network(network),
            NodeMode::Process => Ok(()),
        };
        match (stop, network) {
            (Err(error), Err(network)) => {
                Err(error.context(format!("network cleanup also failed: {network:#}")))
            }
            (Err(error), Ok(())) | (Ok(()), Err(error)) => Err(error),
            (Ok(()), Ok(())) => Ok(()),
        }
    }

    pub async fn restart_nodes(&mut self, ids: &[String]) -> Result<()> {
        self.stop_nodes(ids).await?;
        self.start_nodes(ids).await
    }

    pub fn set_node_env(&mut self, id: &str, key: String, value: String) -> Result<()> {
        let config = self
            .configs
            .get_mut(id)
            .ok_or_else(|| anyhow!("unknown node id '{id}'"))?;
        config.spec.env.insert(key, value);
        Ok(())
    }

    pub async fn wait_for_grpc(&self, id: &str, timeout: std::time::Duration) -> Result<()> {
        let addr = self.grpc_public_addr(id)?;
        match self
            .while_node_running(id, wait_for_ready(addr, timeout))
            .await
        {
            Ok(()) => Ok(()),
            Err(err) => {
                if let Some(diagnostics) = self.capture_docker_diagnostics(id).await? {
                    return Err(err.context(diagnostics));
                }
                Err(err)
            }
        }
    }

    pub async fn ensure_running(&self) -> Result<()> {
        for id in self.handles.keys() {
            self.ensure_node_running(id).await?;
        }
        Ok(())
    }

    pub async fn wait_for_duration(&self, duration: Duration) -> Result<()> {
        if self.handles.is_empty() {
            sleep(duration).await;
            return Ok(());
        }
        self.ensure_running().await?;
        tokio::select! {
            _ = sleep(duration) => self.ensure_running().await,
            error = self.wait_for_owned_exit() => Err(error),
        }
    }

    async fn ensure_node_running(&self, id: &str) -> Result<()> {
        let config = self
            .configs
            .get(id)
            .ok_or_else(|| anyhow!("unknown node id '{id}'"))?;
        let handle = self
            .handles
            .get(id)
            .ok_or_else(|| anyhow!("node '{id}' is not running"))?;
        ensure_handle_running(&handle.node, id, "node", &config.work_dir).await?;
        if let Some(miner) = &handle.miner {
            ensure_handle_running(miner, id, "miner", &config.miner_work_dir).await?;
        }
        Ok(())
    }

    async fn wait_for_owned_exit(&self) -> anyhow::Error {
        loop {
            if let Err(error) = self.ensure_running().await {
                return error;
            }
            sleep(Duration::from_millis(200)).await;
        }
    }

    async fn while_node_running<T>(
        &self,
        id: &str,
        future: impl Future<Output = Result<T>>,
    ) -> Result<T> {
        self.ensure_node_running(id).await?;
        let result = tokio::select! {
            result = future => result,
            error = self.wait_for_owned_exit() => Err(error),
        };
        self.ensure_running().await?;
        result
    }

    pub async fn wait_for_height(
        &self,
        id: &str,
        height: u64,
        timeout: std::time::Duration,
    ) -> Result<HeadInfo> {
        let private_addr = self.grpc_private_addr(id)?;
        let public_addr = self.grpc_public_addr(id)?;
        self.while_node_running(id, async {
            let start = Instant::now();
            match wait_for_height_private(private_addr, self.operator_token(id)?, height, timeout)
                .await
            {
                Ok(private_head) => match self.fetch_head(id).await {
                    Ok(head) => Ok(head),
                    Err(_) => Ok(head_info_from_private(private_head)),
                },
                Err(error) => {
                    let remaining = timeout.saturating_sub(start.elapsed());
                    if remaining.is_zero() {
                        return Err(error);
                    }
                    wait_for_height(public_addr, height, remaining).await
                }
            }
        })
        .await
    }

    pub async fn fetch_head(&self, id: &str) -> Result<HeadInfo> {
        let addr = self.grpc_public_addr(id)?;
        self.while_node_running(id, fetch_head(addr)).await
    }

    pub async fn fetch_private_head(&self, id: &str) -> Result<PrivateHeadInfo> {
        let addr = self.grpc_private_addr(id)?;
        self.while_node_running(id, fetch_heaviest_private(addr, self.operator_token(id)?))
            .await
    }

    pub async fn fetch_private_head_if_current(
        &mut self,
        id: &str,
    ) -> Result<Option<PrivateHeadInfo>> {
        if !self.node_handle_is_live(id).await? {
            return Ok(None);
        }
        self.fetch_private_head(id).await.map(Some)
    }

    pub async fn fetch_constants(&self, id: &str) -> Result<Vec<u8>> {
        let addr = self.grpc_private_addr(id)?;
        self.while_node_running(id, fetch_constants_private(addr, self.operator_token(id)?))
            .await
    }

    pub fn grpc_private_port(&self, id: &str) -> Result<u16> {
        let config = self
            .configs
            .get(id)
            .ok_or_else(|| anyhow!("unknown node id '{id}'"))?;
        Ok(config
            .grpc_private_host_port
            .unwrap_or(config.grpc_private_port))
    }

    pub async fn upgrade_node(&mut self, id: &str, binary: PathBuf) -> Result<()> {
        let config = self
            .configs
            .get_mut(id)
            .ok_or_else(|| anyhow!("unknown node id '{id}'"))?;
        match &self.mode {
            NodeMode::Process => {
                let binary = absolutize_binary(expand_env_path(binary)?)?;
                let supports = binary_supports_flag(&binary, "--fakenet-bythos-phase")?;
                config.binary = binary;
                config.supports_bythos_phase = supports;
            }
            NodeMode::Docker { .. } => {
                config.spec.binary = Some(binary);
                config.supports_bythos_phase = true;
            }
        }
        let ids = vec![id.to_string()];
        self.restart_nodes(&ids).await
    }

    pub async fn set_mining_pkh(&mut self, id: &str, mining_pkh: String) -> Result<()> {
        self.configs
            .get_mut(id)
            .ok_or_else(|| anyhow!("unknown node id '{id}'"))?
            .spec
            .mine = false;
        self.stop_miner(id).await?;
        let config = self
            .configs
            .get_mut(id)
            .ok_or_else(|| anyhow!("unknown node id '{id}'"))?;
        config.spec.mining_pkh = Some(mining_pkh);
        config.spec.mine = true;
        if self.handles.contains_key(id) {
            self.start_miner(id).await?;
        }
        Ok(())
    }

    pub async fn disable_mining(&mut self, id: &str) -> Result<()> {
        self.set_mining_enabled(id, false).await
    }

    pub async fn set_mining_enabled(&mut self, id: &str, enabled: bool) -> Result<()> {
        let config = self
            .configs
            .get_mut(id)
            .ok_or_else(|| anyhow!("unknown node id '{id}'"))?;
        config.spec.mine = enabled;
        if !self.handles.contains_key(id) {
            return Ok(());
        }
        if enabled {
            if self
                .handles
                .get(id)
                .is_some_and(|handle| handle.miner.is_some())
            {
                self.ensure_node_running(id).await?;
                return set_mining_enabled(
                    self.grpc_private_addr(id)?,
                    self.operator_token(id)?,
                    true,
                )
                .await;
            }
            self.start_miner(id).await
        } else {
            let stop = self.stop_miner(id).await;
            let disable =
                set_mining_enabled(self.grpc_private_addr(id)?, self.operator_token(id)?, false)
                    .await;
            match (stop, disable) {
                (Err(error), Err(disable)) => {
                    Err(error.context(format!("disable mining also failed: {disable:#}")))
                }
                (Err(error), Ok(())) | (Ok(()), Err(error)) => Err(error),
                (Ok(()), Ok(())) => Ok(()),
            }
        }
    }

    pub async fn upgrade_miner(&mut self, id: &str, binary: PathBuf) -> Result<()> {
        let config = self
            .configs
            .get_mut(id)
            .ok_or_else(|| anyhow!("unknown node id '{id}'"))?;
        let enabled = config.spec.mine;
        config.spec.mine = false;
        self.stop_miner(id).await?;
        let config = self
            .configs
            .get_mut(id)
            .ok_or_else(|| anyhow!("unknown node id '{id}'"))?;
        match &self.mode {
            NodeMode::Process => config.miner_binary = absolutize_binary(expand_env_path(binary)?)?,
            NodeMode::Docker { .. } => {
                config.spec.miner_image = Some(expand_env_vars(&binary.to_string_lossy())?);
            }
        }
        config.miner_supports_cpu_flag = None;
        config.spec.mine = enabled;
        if config.spec.mine && self.handles.contains_key(id) {
            self.start_miner(id).await?;
        }
        Ok(())
    }

    async fn start_miner(&mut self, id: &str) -> Result<()> {
        if let Err(error) = self.start_miner_inner(id).await {
            self.configs
                .get_mut(id)
                .expect("known mining node")
                .spec
                .mine = false;
            return match self.stop_miner(id).await {
                Ok(()) => Err(error),
                Err(cleanup) => {
                    Err(error.context(format!("miner startup cleanup also failed: {cleanup:#}")))
                }
            };
        }
        Ok(())
    }

    async fn start_miner_inner(&mut self, id: &str) -> Result<()> {
        self.ensure_node_running(id).await?;
        let config = self
            .configs
            .get(id)
            .ok_or_else(|| anyhow!("unknown node id '{id}'"))?;
        if self
            .handles
            .get(id)
            .is_some_and(|handle| handle.miner.is_some())
        {
            return Ok(());
        }
        if config.spec.mining_pkh.is_none() {
            return Err(anyhow!("node '{id}' requests mining without mining_pkh"));
        }
        if config.miner_supports_cpu_flag.is_none() {
            let supports = match &self.mode {
                NodeMode::Process => binary_supports_flag(&config.miner_binary, "--cpu")?,
                NodeMode::Docker { miner_image, .. } => {
                    docker_miner_supports_cpu_flag(config, miner_image).await?
                }
            };
            self.configs
                .get_mut(id)
                .expect("known mining node")
                .miner_supports_cpu_flag = Some(supports);
        }
        let config = self.configs.get(id).expect("known mining node");
        configure_mining_payout(
            self.grpc_private_addr(id)?,
            self.operator_token(id)?,
            config.spec.mining_pkh.as_deref().expect("mining payout"),
        )
        .await?;
        let miner = match &self.mode {
            NodeMode::Process => spawn_process_miner(config).await?,
            NodeMode::Docker {
                miner_image,
                network,
                ..
            } => ChildHandle::Docker(spawn_docker_miner(config, miner_image, network).await?),
        };
        // The real child is reachable by every failure-cleanup path immediately.
        self.handles.get_mut(id).expect("owned mining node").miner = Some(miner);
        if self
            .configs
            .get(id)
            .expect("known mining node")
            .miner_token
            .is_some()
        {
            // Restricted miners deliberately leave payout/enable to the operator.
            set_mining_enabled(self.grpc_private_addr(id)?, self.operator_token(id)?, true).await?;
        }
        sleep(Duration::from_millis(200)).await;
        self.ensure_node_running(id).await
    }

    async fn stop_miner(&mut self, id: &str) -> Result<()> {
        let miner = self
            .handles
            .get_mut(id)
            .and_then(|handle| handle.miner.take());
        if let Some(miner) = miner {
            let config = self.configs.get(id).expect("owned mining node config");
            stop_owned_child(miner, &config.miner_work_dir, true).await?;
        }
        Ok(())
    }

    pub fn operator_token(&self, id: &str) -> Result<Option<&str>> {
        let config = self
            .configs
            .get(id)
            .ok_or_else(|| anyhow!("unknown node id '{id}'"))?;
        Ok(config.operator_token.as_deref())
    }

    fn ensure_ports_available(&self, ids: &[String]) -> Result<()> {
        if matches!(&self.mode, NodeMode::Docker { .. }) {
            return Ok(());
        }
        for id in ids {
            if self.handles.contains_key(id) {
                continue;
            }
            let config = self
                .configs
                .get(id)
                .ok_or_else(|| anyhow!("unknown node id '{id}'"))?;
            if config.spec.grpc_enabled {
                ensure_tcp_port_available(
                    &config.spec.id, "public gRPC", &config.grpc_public_addr,
                    "choose a different --base-grpc-port block or stop the stale process",
                )?;
                ensure_tcp_port_available(
                    &config.spec.id, "private gRPC", &config.grpc_private_addr,
                    "choose a different --base-private-grpc-port block or stop the stale process",
                )?;
            }
            ensure_udp_port_available(
                &config.spec.id, "p2p", config.p2p_port,
                "choose a different --base-p2p-port block or stop the stale process",
            )?;
        }
        Ok(())
    }

    async fn node_handle_is_live(&mut self, id: &str) -> Result<bool> {
        let Some(handle) = self.handles.get(id) else {
            return Ok(false);
        };
        match &handle.node {
            ChildHandle::Process(handle) => Ok(handle.child.lock().await.try_wait()?.is_none()),
            ChildHandle::Docker(container) => container.is_running().await.map_err(Into::into),
        }
    }

    pub async fn apply_partition(&mut self, groups: &[Vec<String>], _run_dir: &Path) -> Result<()> {
        if groups.is_empty() {
            return Err(anyhow!("partition requires at least one group"));
        }

        let all_nodes: Vec<String> = self.configs.keys().cloned().collect();
        let mut group_index: HashMap<String, usize> = HashMap::new();
        for (idx, group) in groups.iter().enumerate() {
            for node in group {
                if group_index.insert(node.clone(), idx).is_some() {
                    return Err(anyhow!(
                        "node '{node}' appears in multiple partition groups"
                    ));
                }
            }
        }

        let mut unknown_nodes = Vec::new();
        for node in group_index.keys() {
            if !self.configs.contains_key(node) {
                unknown_nodes.push(node.clone());
            }
        }
        if !unknown_nodes.is_empty() {
            return Err(anyhow!(
                "partition groups include unknown nodes: {:?}", unknown_nodes
            ));
        }

        let mut target_nodes: Vec<String> = group_index.keys().cloned().collect();
        target_nodes.sort();
        target_nodes.dedup();

        let clear_partition = groups.len() == 1 && {
            let group = &groups[0];
            let mut group_sorted = group.clone();
            group_sorted.sort();
            let mut all_sorted = all_nodes.clone();
            all_sorted.sort();
            group_sorted == all_sorted
        };

        if clear_partition {
            for node in &target_nodes {
                let config = self
                    .configs
                    .get_mut(node)
                    .ok_or_else(|| anyhow!("unknown node id '{node}'"))?;
                config.spec.no_default_peers = config.base_spec.no_default_peers;
                config.spec.allowed_peers_path = config.base_spec.allowed_peers_path.clone();
            }
            return self.restart_nodes(&target_nodes).await;
        }

        let mut peer_ids = HashMap::new();
        for node in &target_nodes {
            let config = self
                .configs
                .get(node)
                .ok_or_else(|| anyhow!("unknown node id '{node}'"))?;
            let peer_id = wait_for_peer_id(&config.work_dir).await?;
            peer_ids.insert(node.clone(), peer_id);
        }

        for node in &target_nodes {
            let idx = group_index
                .get(node)
                .ok_or_else(|| anyhow!("missing group for node '{node}'"))?;
            let allowlist = groups
                .get(*idx)
                .ok_or_else(|| anyhow!("missing partition group {idx}"))?;
            let mut entries = Vec::new();
            for peer in allowlist {
                if let Some(peer_id) = peer_ids.get(peer) {
                    entries.push(peer_id.clone());
                }
            }

            let peers_dir = self
                .configs
                .get(node)
                .ok_or_else(|| anyhow!("unknown node id '{node}'"))?
                .work_dir
                .join("peers");
            std::fs::create_dir_all(&peers_dir)?;
            let path = peers_dir.join("allowed-peers.txt");
            std::fs::write(&path, entries.join("\n"))?;
            let config = self
                .configs
                .get_mut(node)
                .ok_or_else(|| anyhow!("unknown node id '{node}'"))?;
            config.spec.no_default_peers = true;
            config.spec.allowed_peers_path = Some(path);
        }

        self.restart_nodes(&target_nodes).await
    }

    pub fn grpc_public_addr(&self, id: &str) -> Result<&str> {
        if !self.handles.contains_key(id) {
            return Err(anyhow!("node '{id}' is not running"));
        }
        let config = self
            .configs
            .get(id)
            .ok_or_else(|| anyhow!("unknown node id '{id}'"))?;
        if !config.spec.grpc_enabled {
            return Err(anyhow!("node '{id}' has gRPC disabled"));
        }
        Ok(config.grpc_public_addr.as_str())
    }

    pub fn grpc_private_addr(&self, id: &str) -> Result<&str> {
        if !self.handles.contains_key(id) {
            return Err(anyhow!("node '{id}' is not running"));
        }
        let config = self
            .configs
            .get(id)
            .ok_or_else(|| anyhow!("unknown node id '{id}'"))?;
        if !config.spec.grpc_enabled {
            return Err(anyhow!("node '{id}' has gRPC disabled"));
        }
        Ok(config.grpc_private_addr.as_str())
    }

    pub fn is_fakenet(&self, id: &str) -> Result<bool> {
        let config = self
            .configs
            .get(id)
            .ok_or_else(|| anyhow!("unknown node id '{id}'"))?;
        Ok(config.spec.fakenet)
    }

    pub fn fakenet_phase_overrides(&self, id: &str) -> Result<(Option<u64>, Option<u64>)> {
        let config = self
            .configs
            .get(id)
            .ok_or_else(|| anyhow!("unknown node id '{id}'"))?;
        if !config.spec.fakenet {
            return Ok((None, None));
        }
        let bythos_phase = if config.supports_bythos_phase {
            config.spec.fakenet_bythos_phase
        } else {
            None
        };
        Ok((config.spec.fakenet_v1_phase, bythos_phase))
    }

    pub async fn wait_for_peer_id(&self, id: &str) -> Result<String> {
        let config = self
            .configs
            .get(id)
            .ok_or_else(|| anyhow!("unknown node id '{id}'"))?;
        wait_for_peer_id(&config.work_dir).await
    }

    pub async fn combined_logs(&self, id: &str) -> Result<String> {
        let config = self
            .configs
            .get(id)
            .ok_or_else(|| anyhow!("unknown node id '{id}'"))?;

        let (stdout, stderr) = match self.handles.get(id).map(|handle| &handle.node) {
            Some(ChildHandle::Process(_)) => (
                read_log_file(&config.work_dir.join("stdout.log"))?,
                read_log_file(&config.work_dir.join("stderr.log"))?,
            ),
            Some(ChildHandle::Docker(container)) => {
                let (_, _, stdout, stderr) = persist_docker_logs(config, container).await?;
                (stdout, stderr)
            }
            None => return Err(anyhow!("node '{id}' is not running")),
        };

        Ok(format!("{stdout}\n{stderr}"))
    }

    async fn capture_docker_diagnostics(&self, id: &str) -> Result<Option<String>> {
        let Some(NodeHandle {
            node: ChildHandle::Docker(container),
            ..
        }) = self.handles.get(id)
        else {
            return Ok(None);
        };
        let config = self
            .configs
            .get(id)
            .ok_or_else(|| anyhow!("unknown node id '{id}'"))?;

        std::fs::create_dir_all(&config.work_dir)?;

        let (stdout_path, stderr_path, stdout, stderr) =
            persist_docker_logs(config, container).await?;

        let running = match container.is_running().await {
            Ok(value) => value.to_string(),
            Err(err) => format!("unknown ({err})"),
        };
        let exit_code = match container.exit_code().await {
            Ok(value) => format!("{value:?}"),
            Err(err) => format!("unknown ({err})"),
        };

        let stderr_tail = log_tail_preview(&stderr, 12);
        let stdout_tail = log_tail_preview(&stdout, 12);
        let preview = if stderr_tail.is_empty() {
            stdout_tail
        } else {
            stderr_tail
        };
        let preview = if preview.is_empty() {
            "<no container logs captured>".to_string()
        } else {
            preview
        };

        Ok(Some(format!(
            "docker node '{}' status running={} exit_code={} stdout_log={} stderr_log={} log_tail:\n{}",
            id,
            running,
            exit_code,
            stdout_path.display(),
            stderr_path.display(),
            preview
        )))
    }
}

struct NodeConfig {
    base_spec: NodeSpec,
    spec: NodeSpec,
    grpc_public_addr: String,
    grpc_public_port: u16,
    grpc_private_addr: String,
    grpc_private_port: u16,
    grpc_private_host_port: Option<u16>,
    p2p_port: u16,
    work_dir: PathBuf,
    miner_work_dir: PathBuf,
    binary: PathBuf,
    miner_binary: PathBuf,
    miner_supports_cpu_flag: Option<bool>,
    operator_token: Option<String>,
    miner_token: Option<String>,
    supports_bythos_phase: bool,
}

impl NodeConfig {
    fn refresh_credentials(&mut self) {
        self.operator_token = effective_auth_env(&self.spec, PRIVATE_GRPC_AUTH_TOKEN_ENV);
        self.miner_token = effective_auth_env(&self.spec, PRIVATE_GRPC_MINER_TOKEN_ENV);
    }
}

struct ProcessHandle {
    child: tokio::sync::Mutex<Child>,
}

struct NodeHandle {
    node: ChildHandle,
    miner: Option<ChildHandle>,
}

#[allow(clippy::large_enum_variant)]
enum ChildHandle {
    Process(ProcessHandle),
    Docker(testcontainers::ContainerAsync<GenericImage>),
}

struct DockerPeers {
    peers: Vec<String>,
    force_peers: Vec<String>,
}

async fn ensure_handle_running(
    handle: &ChildHandle,
    id: &str,
    role: &str,
    work_dir: &Path,
) -> Result<()> {
    match handle {
        ChildHandle::Process(handle) => {
            if let Some(status) = handle.child.lock().await.try_wait()? {
                return Err(anyhow!(
                    "{role} '{id}' exited with status {status} (see {})",
                    work_dir.join("stderr.log").display(),
                ));
            }
        }
        ChildHandle::Docker(container) => {
            if !container.is_running().await? {
                let status = container.exit_code().await?;
                let error = anyhow!(
                    "docker {role} '{id}' exited with status {status:?} (see {})",
                    work_dir.join("stderr.log").display(),
                );
                return match persist_docker_child_logs(work_dir, container).await {
                    Ok(_) => Err(error),
                    Err(logs) => Err(error.context(format!("log capture also failed: {logs:#}"))),
                };
            }
        }
    }
    Ok(())
}

async fn stop_owned_child(handle: ChildHandle, work_dir: &Path, miner: bool) -> Result<()> {
    match handle {
        ChildHandle::Process(handle) => {
            stop_process_child(&mut handle.child.into_inner(), miner).await
        }
        ChildHandle::Docker(container) => {
            let mut errors = Vec::new();
            if miner {
                // The standalone CPU executable watches SIGINT, not SIGTERM.
                let stop = Command::new("docker")
                    .args(["stop", "--signal", "SIGINT", "--timeout"])
                    .arg(MINER_SHUTDOWN_GRACE.as_secs().to_string())
                    .arg(container.id())
                    .output()
                    .await;
                match stop {
                    Ok(output) if output.status.success() => {}
                    Ok(output) => errors.push(format!(
                        "stop miner container: {}",
                        String::from_utf8_lossy(&output.stderr).trim(),
                    )),
                    Err(error) => errors.push(format!("stop miner container: {error}")),
                }
            } else if let Err(error) = container
                .stop_with_timeout(Some(PROCESS_SHUTDOWN_GRACE.as_secs() as i32))
                .await
            {
                errors.push(format!("stop node container: {error}"));
            }
            if let Err(error) = persist_docker_child_logs(work_dir, &container).await {
                errors.push(format!("capture container logs: {error:#}"));
            }
            // Await removal rather than relying on asynchronous Drop before
            // releasing the node namespace or the fixture network.
            if let Err(error) = container.rm().await {
                errors.push(format!("remove owned container: {error}"));
            }
            if errors.is_empty() {
                Ok(())
            } else {
                Err(anyhow!(errors.join("; ")))
            }
        }
    }
}

async fn stop_process_child(child: &mut Child, miner: bool) -> Result<()> {
    let mut earlier_error: Option<anyhow::Error> = match child.try_wait() {
        Ok(Some(_)) => return Ok(()),
        Ok(None) => None,
        Err(error) => Some(error.into()),
    };
    let grace = if miner {
        MINER_SHUTDOWN_GRACE
    } else {
        PROCESS_SHUTDOWN_GRACE
    };
    if request_process_shutdown(child, miner) {
        match timeout(grace, child.wait()).await {
            Ok(Ok(_)) => return earlier_error.map_or(Ok(()), Err),
            Ok(Err(error)) => {
                earlier_error = Some(match earlier_error {
                    Some(earlier) => earlier.context(format!("graceful wait also failed: {error}")),
                    None => error.into(),
                });
            }
            Err(_) => {}
        }
    }
    // A status/wait error must not bypass forced termination and reaping.
    let kill = child.start_kill().context("kill owned child");
    let reap = child.wait().await.context("reap owned child");
    let forced = match (kill, reap) {
        (Err(error), Err(reap)) => Err(error.context(format!("reap also failed: {reap:#}"))),
        (Err(error), Ok(_)) | (Ok(_), Err(error)) => Err(error),
        (Ok(()), Ok(_)) => Ok(()),
    };
    match (earlier_error, forced) {
        (Some(error), Err(forced)) => {
            Err(error.context(format!("forced cleanup also failed: {forced:#}")))
        }
        (Some(error), Ok(())) | (None, Err(error)) => Err(error),
        (None, Ok(())) => Ok(()),
    }
}

#[cfg(unix)]
fn request_process_shutdown(child: &Child, miner: bool) -> bool {
    let Some(pid) = child.id() else {
        return false;
    };
    let signal = if miner { libc::SIGINT } else { libc::SIGTERM };
    // SAFETY: pid belongs to the owned child and both signals are valid.
    unsafe { libc::kill(pid as libc::pid_t, signal) == 0 }
}

#[cfg(not(unix))]
fn request_process_shutdown(_: &Child, _: bool) -> bool {
    false
}

async fn spawn_process_node(config: &NodeConfig, peer_from: &[String]) -> Result<ChildHandle> {
    std::fs::create_dir_all(&config.work_dir)?;
    archive_previous_logs(&config.work_dir)?;

    let stdout_path = config.work_dir.join("stdout.log");
    let stderr_path = config.work_dir.join("stderr.log");
    let stdout = std::fs::File::create(stdout_path)?;
    let stderr = std::fs::File::create(stderr_path)?;

    let args = build_node_args(config, peer_from)?;
    info!(
        "spawning node {}: {} {}",
        config.spec.id,
        config.binary.display(),
        args.join(" ")
    );

    let mut command = Command::new(&config.binary);
    command
        .current_dir(&config.work_dir)
        .stdout(Stdio::from(stdout))
        .stderr(Stdio::from(stderr))
        .args(&args);
    command.kill_on_drop(true);
    if let Some((key, value)) = private_grpc_auth_env(&config.spec) {
        command.env(key, value);
    }
    for (key, value) in &config.spec.env {
        command.env(key, value);
    }
    apply_process_credentials(&mut command, config, false);

    let child = command.spawn().context("spawn nockchain")?;

    Ok(ChildHandle::Process(ProcessHandle {
        child: tokio::sync::Mutex::new(child),
    }))
}

async fn spawn_docker_node(
    config: &NodeConfig,
    default_image: &str,
    network: &str,
    peer_from: &[String],
    docker_peers: &DockerPeers,
) -> Result<testcontainers::ContainerAsync<GenericImage>> {
    std::fs::create_dir_all(&config.work_dir)?;
    archive_previous_logs(&config.work_dir)?;

    let image_ref = resolve_docker_image(config, default_image)?;
    let image = parse_image_ref(&image_ref);
    let args = build_node_args_docker(config, peer_from, docker_peers)?;
    let container_name = docker_container_name(network, &config.spec.id);
    remove_stale_docker_container(&container_name)?;

    info!(
        "spawning docker node {}: {}:{} {}",
        config.spec.id,
        image.name,
        image.tag,
        args.join(" ")
    );

    let mount = Mount::bind_mount(config.work_dir.display().to_string(), DOCKER_WORKDIR)
        .with_access_mode(AccessMode::ReadWrite);

    let mut image = GenericImage::new(image.name, image.tag)
        .with_wait_for(WaitFor::Nothing)
        .with_exposed_port(config.p2p_port.udp());

    if config.spec.grpc_enabled {
        image = image
            .with_exposed_port(config.grpc_public_port.tcp())
            .with_exposed_port(config.grpc_private_port.tcp());
    }

    let mut request = image
        .with_entrypoint("/usr/local/bin/nockchain")
        .with_cmd(args)
        .with_network(network)
        .with_container_name(&container_name)
        .with_working_dir(DOCKER_WORKDIR)
        .with_mount(mount);
    if config.spec.grpc_enabled {
        let private_port = config.grpc_private_port;
        request = request.with_host_config_modifier(move |host_config| {
            // Override only private publication; public gRPC/P2P retain their
            // existing publication. The operator relay must be host-loopback-only.
            host_config
                .port_bindings
                .get_or_insert_with(HashMap::new)
                .insert(
                    format!("{private_port}/tcp"),
                    Some(vec![testcontainers::bollard::models::PortBinding {
                        host_ip: Some("127.0.0.1".to_string()),
                        host_port: Some("0".to_string()),
                    }]),
                );
        });
    }

    #[cfg(unix)]
    {
        request = request.with_user(current_host_user());
    }

    if let Some((key, value)) = private_grpc_auth_env(&config.spec) {
        request = request.with_env_var(key, value);
    }
    for (key, value) in &config.spec.env {
        request = request.with_env_var(key, value);
    }
    if let Some(token) = &config.operator_token {
        request = request.with_env_var(PRIVATE_GRPC_AUTH_TOKEN_ENV, token);
    }
    if let Some(token) = &config.miner_token {
        request = request.with_env_var(PRIVATE_GRPC_MINER_TOKEN_ENV, token);
    }

    request.start().await.map_err(Into::into)
}

fn effective_auth_env(spec: &NodeSpec, key: &str) -> Option<String> {
    spec.env
        .get(key)
        .cloned()
        .or_else(|| std::env::var(key).ok())
        .filter(|token| !token.is_empty())
}

fn apply_process_credentials(command: &mut Command, config: &NodeConfig, miner: bool) {
    if miner && config.miner_token.is_some() {
        command.env_remove(PRIVATE_GRPC_AUTH_TOKEN_ENV);
    } else if let Some(token) = &config.operator_token {
        command.env(PRIVATE_GRPC_AUTH_TOKEN_ENV, token);
    } else {
        command.env_remove(PRIVATE_GRPC_AUTH_TOKEN_ENV);
    }
    if let Some(token) = &config.miner_token {
        command.env(PRIVATE_GRPC_MINER_TOKEN_ENV, token);
    } else {
        command.env_remove(PRIVATE_GRPC_MINER_TOKEN_ENV);
    }
}

fn build_miner_args(config: &NodeConfig, node_addr: String) -> Result<Vec<String>> {
    let pkh =
        config.spec.mining_pkh.as_ref().ok_or_else(|| {
            anyhow!("node '{}' requests mining without mining_pkh", config.spec.id)
        })?;
    let mut args = vec![
        "--node-addr".to_string(),
        node_addr,
        "--mining-pkh".to_string(),
        pkh.clone(),
        "--num-threads".to_string(),
        config.spec.mining_threads.to_string(),
    ];
    // CPU-only builds reject --cpu; CUDA-capable builds need it to select CPU.
    if config.miner_supports_cpu_flag == Some(true) {
        args.push("--cpu".to_string());
    }
    Ok(args)
}

async fn spawn_process_miner(config: &NodeConfig) -> Result<ChildHandle> {
    std::fs::create_dir_all(&config.miner_work_dir)?;
    archive_previous_logs(&config.miner_work_dir)?;
    let stdout = std::fs::File::create(config.miner_work_dir.join("stdout.log"))?;
    let stderr = std::fs::File::create(config.miner_work_dir.join("stderr.log"))?;
    let args = build_miner_args(config, format!("http://{}", config.grpc_private_addr))?;
    info!(
        "spawning miner {}: {} {}",
        config.spec.id,
        config.miner_binary.display(),
        args.join(" ")
    );
    let mut command = Command::new(&config.miner_binary);
    command
        .current_dir(&config.miner_work_dir)
        .args(args)
        .stdout(Stdio::from(stdout))
        .stderr(Stdio::from(stderr))
        .kill_on_drop(true);
    for (key, value) in &config.spec.env {
        command.env(key, value);
    }
    apply_process_credentials(&mut command, config, true);
    let child = command.spawn().context("spawn standalone CPU miner")?;
    Ok(ChildHandle::Process(ProcessHandle {
        child: tokio::sync::Mutex::new(child),
    }))
}

fn miner_image<'a>(config: &'a NodeConfig, default_image: &'a str) -> &'a str {
    config.spec.miner_image.as_deref().unwrap_or(default_image)
}

async fn docker_miner_supports_cpu_flag(config: &NodeConfig, default_image: &str) -> Result<bool> {
    let output = Command::new("docker")
        .args(["run", "--rm", "--entrypoint"])
        .arg(&config.miner_binary)
        .arg(miner_image(config, default_image))
        .arg("--help")
        .output()
        .await
        .context("read standalone miner CLI capabilities")?;
    if !output.status.success() {
        return Err(anyhow!(
            "miner image '{}' executable '{}' --help failed: {}",
            miner_image(config, default_image),
            config.miner_binary.display(),
            String::from_utf8_lossy(&output.stderr).trim(),
        ));
    }
    Ok(String::from_utf8_lossy(&output.stdout).contains("--cpu")
        || String::from_utf8_lossy(&output.stderr).contains("--cpu"))
}

async fn spawn_docker_miner(
    config: &NodeConfig,
    default_image: &str,
    network: &str,
) -> Result<testcontainers::ContainerAsync<GenericImage>> {
    std::fs::create_dir_all(&config.miner_work_dir)?;
    archive_previous_logs(&config.miner_work_dir)?;
    let image_ref = miner_image(config, default_image);
    let image = parse_image_ref(image_ref);
    let args = build_miner_args(
        config,
        format!("http://127.0.0.1:{}", config.grpc_private_port),
    )?;
    let name = format!("{}-{}-miner", network, config.spec.id);
    remove_stale_docker_container(&name)?;
    info!(
        "spawning docker miner {}: {} {} {}",
        config.spec.id,
        image_ref,
        config.miner_binary.display(),
        args.join(" ")
    );
    let mount = Mount::bind_mount(
        config.miner_work_dir.display().to_string(),
        "/var/nockchain-miner",
    )
    .with_access_mode(AccessMode::ReadWrite);
    let mut request = GenericImage::new(image.name, image.tag)
        .with_wait_for(WaitFor::Nothing)
        .with_entrypoint(&config.miner_binary.to_string_lossy())
        .with_cmd(args)
        .with_network(format!(
            "container:{}",
            docker_container_name(network, &config.spec.id)
        ))
        .with_container_name(name)
        .with_working_dir("/var/nockchain-miner")
        .with_mount(mount);
    #[cfg(unix)]
    {
        request = request.with_user(current_host_user());
    }
    for (key, value) in &config.spec.env {
        if key == PRIVATE_GRPC_AUTH_TOKEN_ENV || key == PRIVATE_GRPC_MINER_TOKEN_ENV {
            continue;
        }
        request = request.with_env_var(key, value);
    }
    if let Some(token) = &config.miner_token {
        request = request.with_env_var(PRIVATE_GRPC_MINER_TOKEN_ENV, token);
    } else if let Some(token) = &config.operator_token {
        request = request.with_env_var(PRIVATE_GRPC_AUTH_TOKEN_ENV, token);
    }
    request.start().await.map_err(Into::into)
}

fn archive_previous_logs(work_dir: &Path) -> Result<()> {
    let mut previous = Vec::new();
    for name in [
        "stdout.log", "stderr.log", "private-grpc-forwarder.stdout.log",
        "private-grpc-forwarder.stderr.log",
    ] {
        let path = work_dir.join(name);
        if path.try_exists()? {
            previous.push((name, path));
        }
    }
    if previous.is_empty() {
        return Ok(());
    }
    let logs_dir = work_dir.join("logs");
    std::fs::create_dir_all(&logs_dir)?;
    let mut generation = 1u64;
    let archive = loop {
        let archive = logs_dir.join(format!("launch-{generation:03}"));
        match std::fs::create_dir(&archive) {
            Ok(()) => break archive,
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => generation += 1,
            Err(error) => return Err(error.into()),
        }
    };
    for (name, path) in previous {
        std::fs::rename(path, archive.join(name))?;
    }
    Ok(())
}

fn persist_forwarder_logs(work_dir: &Path, name: &str) -> Result<()> {
    let output = std::process::Command::new("docker")
        .args(["logs", name])
        .output()?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        if stderr.contains("No such container") {
            return Ok(());
        }
        return Err(anyhow!("read private gRPC relay logs: {}", stderr.trim()));
    }
    std::fs::write(
        work_dir.join("private-grpc-forwarder.stdout.log"),
        output.stdout,
    )?;
    std::fs::write(
        work_dir.join("private-grpc-forwarder.stderr.log"),
        output.stderr,
    )?;
    Ok(())
}

const PRIVATE_GRPC_AUTH_TOKEN_ENV: &str = "NOCKCHAIN_PRIVATE_GRPC_AUTH_TOKEN";
const PRIVATE_GRPC_MINER_TOKEN_ENV: &str = "NOCKCHAIN_PRIVATE_GRPC_MINER_TOKEN";
const PRIVATE_GRPC_ALLOW_UNAUTHENTICATED_ENV: &str = "NOCKCHAIN_ALLOW_UNAUTHENTICATED_PRIVATE_GRPC";

/// Harness nodes run without private gRPC bearer tokens, and the node refuses
/// to start in that state unless the unauthenticated opt-out is set. Use the
/// environment form rather than the CLI flag so older binaries and images that
/// predate the flag still start (they ignore the variable). A scenario that
/// provisions an operator token or sets the opt-out itself is left alone.
fn private_grpc_auth_env(spec: &NodeSpec) -> Option<(&'static str, &'static str)> {
    if spec.env.contains_key(PRIVATE_GRPC_AUTH_TOKEN_ENV)
        || spec
            .env
            .contains_key(PRIVATE_GRPC_ALLOW_UNAUTHENTICATED_ENV)
    {
        return None;
    }
    Some((PRIVATE_GRPC_ALLOW_UNAUTHENTICATED_ENV, "1"))
}

const PRIVATE_GRPC_FORWARDER_IMAGE_ENV: &str = "NOCKCHAIN_E2E_FORWARDER_IMAGE";
const DEFAULT_PRIVATE_GRPC_FORWARDER_IMAGE: &str = "alpine/socat:1.8.0.3";

fn private_grpc_forwarder_name(container_name: &str) -> String {
    format!("{container_name}-private-grpc-fwd")
}

/// Relay the node's loopback-only private gRPC listener to its published port.
///
/// The node binds the private listener to 127.0.0.1 inside the container and
/// refuses any other address. Docker's published-port proxy targets the
/// container's network address, not its loopback, so the published port alone
/// cannot reach the listener. A socat sidecar joined to the node's network
/// namespace (`--network container:<node>`) listens on the container's address
/// at the same port and relays to 127.0.0.1. Only the harness-controlled
/// sidecar is reachable from the published port; the node's own listener stays
/// loopback-only, matching the production requirement. The image can be
/// overridden with `NOCKCHAIN_E2E_FORWARDER_IMAGE`.
fn start_private_grpc_forwarder(container_name: &str, network: &str, port: u16) -> Result<()> {
    let ip = docker_container_network_ip(container_name, network)?.ok_or_else(|| {
        anyhow!("docker node {container_name} has no address on network {network}")
    })?;
    let forwarder_name = private_grpc_forwarder_name(container_name);
    remove_stale_docker_container(&forwarder_name)?;
    let image = std::env::var(PRIVATE_GRPC_FORWARDER_IMAGE_ENV)
        .ok()
        .filter(|image| !image.is_empty())
        .unwrap_or_else(|| DEFAULT_PRIVATE_GRPC_FORWARDER_IMAGE.to_string());
    let output = std::process::Command::new("docker")
        .args([
            "run",
            "-d",
            "--rm",
            "--name",
            &forwarder_name,
            "--network",
            &format!("container:{container_name}"),
            &image,
            &format!("TCP-LISTEN:{port},bind={ip},fork,reuseaddr"),
            &format!("TCP:127.0.0.1:{port}"),
        ])
        .output()
        .context("start private gRPC forwarder")?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(anyhow!(
            "failed to start private gRPC forwarder {} ({}): {}",
            forwarder_name,
            image,
            stderr.trim()
        ));
    }
    Ok(())
}

fn docker_container_network_ip(container_name: &str, network: &str) -> Result<Option<String>> {
    let format = format!(
        "{{{{with index .NetworkSettings.Networks {:?}}}}}{{{{.IPAddress}}}}{{{{end}}}}",
        network
    );
    let output = std::process::Command::new("docker")
        .args(["inspect", "--format", &format, container_name])
        .output()?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        if stderr.contains("No such object") {
            return Ok(None);
        }
        return Err(anyhow!(
            "docker inspect for {} failed: {}",
            container_name,
            stderr.trim()
        ));
    }

    let ip = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if ip.is_empty() {
        return Ok(None);
    }
    Ok(Some(ip))
}

#[cfg(unix)]
fn current_host_user() -> String {
    // SAFETY: geteuid/getegid read process credentials and have no preconditions.
    let uid = unsafe { libc::geteuid() };
    // SAFETY: geteuid/getegid read process credentials and have no preconditions.
    let gid = unsafe { libc::getegid() };
    format!("{uid}:{gid}")
}

#[allow(clippy::vec_init_then_push)]
fn build_node_args(config: &NodeConfig, peer_from: &[String]) -> Result<Vec<String>> {
    let spec = &config.spec;
    let mut args = Vec::new();

    if spec.grpc_enabled {
        args.push("--bind-public-grpc-addr".to_string());
        args.push(config.grpc_public_addr.clone());
        args.push("--bind-private-grpc-port".to_string());
        args.push(config.grpc_private_port.to_string());
    }

    if spec.fakenet {
        args.push("--fakenet".to_string());
    }

    if spec.new_state {
        args.push("--new".to_string());
    }

    if spec.no_default_peers {
        args.push("--no-default-peers".to_string());
    }

    args.push("--no-new-peer-id".to_string());

    if let Some(path) = &spec.allowed_peers_path {
        args.push("--allowed-peers-path".to_string());
        args.push(absolutize_path(path)?.display().to_string());
    }

    let topology_peer_arg = topology_peer_arg(spec);
    push_peer_args(&mut args, topology_peer_arg, peer_from);
    push_peer_args(&mut args, topology_peer_arg, &spec.peers);
    push_peer_args(&mut args, "--force-peer", &spec.force_peers);

    if spec.bind.is_empty() {
        args.push("--bind".to_string());
        args.push(format!("/ip4/127.0.0.1/udp/{}/quic-v1", config.p2p_port));
    } else {
        for bind in &spec.bind {
            args.push("--bind".to_string());
            args.push(bind.clone());
        }
    }

    if let Some(pow_len) = spec.fakenet_pow_len {
        args.push("--fakenet-pow-len".to_string());
        args.push(pow_len.to_string());
    }

    if let Some(log_difficulty) = spec.fakenet_log_difficulty {
        args.push("--fakenet-log-difficulty".to_string());
        args.push(log_difficulty.to_string());
    }

    if let Some(phase) = spec.fakenet_v1_phase {
        args.push("--fakenet-v1-phase".to_string());
        args.push(phase.to_string());
    }

    if let Some(phase) = spec.fakenet_bythos_phase {
        if config.supports_bythos_phase {
            args.push("--fakenet-bythos-phase".to_string());
            args.push(phase.to_string());
        }
    }

    if let Some(interval_secs) = spec.fakenet_update_candidate_interval_secs {
        args.push("--fakenet-update-candidate-interval-secs".to_string());
        args.push(interval_secs.to_string());
    }

    if let Some(path) = &spec.fakenet_genesis_jam_path {
        args.push("--fakenet-genesis-jam-path".to_string());
        args.push(absolutize_path(path)?.display().to_string());
    }

    for arg in &spec.extra_args {
        args.push(arg.clone());
    }

    Ok(args)
}

fn build_node_args_docker(
    config: &NodeConfig,
    peer_from: &[String],
    docker_peers: &DockerPeers,
) -> Result<Vec<String>> {
    let spec = &config.spec;
    let mut args = Vec::new();

    if spec.grpc_enabled {
        args.push("--bind-public-grpc-addr".to_string());
        args.push(format!("0.0.0.0:{}", config.grpc_public_port));
        // The node refuses any non-loopback private gRPC bind, so the private
        // listener stays on 127.0.0.1 inside the container. The published
        // port is served by the socat sidecar started in
        // `start_private_grpc_forwarder`, which relays the container's network
        // address to this loopback port.
        args.push("--bind-private-grpc-port".to_string());
        args.push(config.grpc_private_port.to_string());
    }

    if spec.fakenet {
        args.push("--fakenet".to_string());
    }

    if spec.new_state {
        args.push("--new".to_string());
    }

    if spec.no_default_peers {
        args.push("--no-default-peers".to_string());
    }

    args.push("--no-new-peer-id".to_string());

    if let Some(path) = &spec.allowed_peers_path {
        args.push("--allowed-peers-path".to_string());
        args.push(container_path_for(config, path)?);
    }

    let topology_peer_arg = topology_peer_arg(spec);
    push_peer_args(&mut args, topology_peer_arg, peer_from);
    push_peer_args(&mut args, topology_peer_arg, &docker_peers.peers);
    push_peer_args(&mut args, "--force-peer", &docker_peers.force_peers);

    if spec.bind.is_empty() {
        args.push("--bind".to_string());
        args.push(format!("/ip4/0.0.0.0/udp/{}/quic-v1", config.p2p_port));
    } else {
        for bind in &spec.bind {
            args.push("--bind".to_string());
            args.push(rewrite_bind_for_docker(bind)?);
        }
    }

    if let Some(pow_len) = spec.fakenet_pow_len {
        args.push("--fakenet-pow-len".to_string());
        args.push(pow_len.to_string());
    }

    if let Some(log_difficulty) = spec.fakenet_log_difficulty {
        args.push("--fakenet-log-difficulty".to_string());
        args.push(log_difficulty.to_string());
    }

    if let Some(phase) = spec.fakenet_v1_phase {
        args.push("--fakenet-v1-phase".to_string());
        args.push(phase.to_string());
    }

    if let Some(phase) = spec.fakenet_bythos_phase {
        if config.supports_bythos_phase {
            args.push("--fakenet-bythos-phase".to_string());
            args.push(phase.to_string());
        }
    }

    if let Some(interval_secs) = spec.fakenet_update_candidate_interval_secs {
        args.push("--fakenet-update-candidate-interval-secs".to_string());
        args.push(interval_secs.to_string());
    }

    if let Some(path) = &spec.fakenet_genesis_jam_path {
        args.push("--fakenet-genesis-jam-path".to_string());
        args.push(container_path_for(config, path)?);
    }

    for arg in &spec.extra_args {
        args.push(arg.clone());
    }

    Ok(args)
}

fn topology_peer_arg(spec: &NodeSpec) -> &'static str {
    if spec.new_state {
        "--peer"
    } else {
        "--force-peer"
    }
}

fn push_peer_args(args: &mut Vec<String>, flag: &str, peers: &[String]) {
    for peer in peers {
        args.push(flag.to_string());
        args.push(peer.clone());
    }
}

fn resolve_work_dir(run_dir: &Path, spec: &NodeSpec) -> Result<PathBuf> {
    if let Some(dir) = &spec.data_dir {
        if dir.is_absolute() {
            return Ok(dir.clone());
        }
        return Ok(run_dir.join(dir));
    }

    Ok(run_dir.join("nodes").join(&spec.id))
}

fn resolve_binary(spec: &NodeSpec, override_bin: Option<&PathBuf>) -> PathBuf {
    if let Some(path) = &spec.binary {
        return path.clone();
    }
    if let Some(path) = override_bin {
        return path.clone();
    }
    PathBuf::from("nockchain")
}

fn expand_env_path(path: PathBuf) -> Result<PathBuf> {
    let raw = path.to_string_lossy();
    if !raw.contains("${") {
        return Ok(path);
    }
    let expanded = expand_env_vars(&raw)?;
    Ok(PathBuf::from(expanded))
}

#[allow(clippy::while_let_on_iterator)]
fn expand_env_vars(input: &str) -> Result<String> {
    if !input.contains("${") {
        return Ok(input.to_string());
    }

    let mut out = String::with_capacity(input.len());
    let mut chars = input.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch == '$' && matches!(chars.peek(), Some('{')) {
            chars.next();
            let mut key = String::new();
            let mut closed = false;
            while let Some(next) = chars.next() {
                if next == '}' {
                    closed = true;
                    break;
                }
                key.push(next);
            }
            if !closed {
                return Err(anyhow!("unterminated variable reference in '{}'", input));
            }
            if key.is_empty() {
                return Err(anyhow!("empty variable reference in '{}'", input));
            }
            let value = std::env::var(&key)
                .map_err(|_| anyhow!("missing variable '{}' in '{}'", key, input))?;
            out.push_str(&value);
        } else {
            out.push(ch);
        }
    }

    Ok(out)
}

fn binary_supports_flag(binary: &Path, flag: &str) -> Result<bool> {
    let output = std::process::Command::new(binary)
        .arg("--help")
        .output()
        .with_context(|| format!("failed to run {} --help", binary.display()))?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    Ok(stdout.contains(flag) || stderr.contains(flag))
}

fn absolutize_binary(path: PathBuf) -> Result<PathBuf> {
    if path.is_absolute() {
        return Ok(path);
    }
    let cwd = std::env::current_dir().context("resolve binary cwd")?;
    Ok(cwd.join(path))
}

fn absolutize_path(path: &Path) -> Result<PathBuf> {
    if path.is_absolute() {
        return Ok(path.to_path_buf());
    }
    let cwd = std::env::current_dir().context("resolve path cwd")?;
    Ok(cwd.join(path))
}

fn read_docker_log_bytes(result: Result<Vec<u8>, TestcontainersError>) -> Result<String> {
    let bytes = result?;
    Ok(match String::from_utf8(bytes) {
        Ok(text) => text,
        Err(error) => String::from_utf8_lossy(error.as_bytes()).into_owned(),
    })
}

async fn persist_docker_logs(
    config: &NodeConfig,
    container: &testcontainers::ContainerAsync<GenericImage>,
) -> Result<(PathBuf, PathBuf, String, String)> {
    persist_docker_child_logs(&config.work_dir, container).await
}

async fn persist_docker_child_logs(
    work_dir: &Path,
    container: &testcontainers::ContainerAsync<GenericImage>,
) -> Result<(PathBuf, PathBuf, String, String)> {
    std::fs::create_dir_all(work_dir)?;
    let stdout = read_docker_log_bytes(container.stdout_to_vec().await);
    let stderr = read_docker_log_bytes(container.stderr_to_vec().await);
    let stdout_path = work_dir.join("stdout.log");
    let stderr_path = work_dir.join("stderr.log");
    if let (Ok(stdout), Ok(stderr)) = (&stdout, &stderr) {
        persist_log_bytes(&stdout_path, &stderr_path, stdout, stderr)?;
    } else {
        if let Ok(stdout) = &stdout {
            std::fs::write(&stdout_path, stdout.as_bytes())?;
        }
        if let Ok(stderr) = &stderr {
            std::fs::write(&stderr_path, stderr.as_bytes())?;
        }
    }
    Ok((stdout_path, stderr_path, stdout?, stderr?))
}

fn persist_log_bytes(
    stdout_path: &Path,
    stderr_path: &Path,
    stdout: &str,
    stderr: &str,
) -> Result<()> {
    std::fs::write(stdout_path, stdout.as_bytes())?;
    std::fs::write(stderr_path, stderr.as_bytes())?;
    Ok(())
}

fn log_tail_preview(logs: &str, max_lines: usize) -> String {
    if max_lines == 0 {
        return String::new();
    }

    let lines: Vec<&str> = logs
        .lines()
        .map(str::trim_end)
        .filter(|line| !line.is_empty())
        .collect();
    if lines.is_empty() {
        return String::new();
    }

    let start = lines.len().saturating_sub(max_lines);
    lines[start..].join("\n")
}

fn parse_socket_port(addr: &str) -> Result<u16> {
    let parsed: SocketAddr = addr
        .parse()
        .map_err(|err| anyhow!("invalid socket address '{addr}': {err}"))?;
    Ok(parsed.port())
}

fn extract_udp_port(value: &str) -> Option<u16> {
    let (_, rest) = value.split_once("/udp/")?;
    let port_str = rest.split('/').next()?;
    port_str.parse().ok()
}

fn resolve_p2p_ports(specs: &[NodeSpec], base_p2p_port: u16) -> Result<HashMap<String, u16>> {
    let mut assigned = HashMap::new();
    let mut used_ports = HashSet::new();

    for spec in specs {
        if let Some(bind) = spec.bind.first() {
            let port = extract_udp_port(bind)
                .ok_or_else(|| anyhow!("failed to parse udp port from bind '{bind}'"))?;
            if let Some(existing) = assigned.insert(spec.id.clone(), port) {
                return Err(anyhow!(
                    "node '{}' received conflicting p2p ports {} and {}", spec.id, existing, port
                ));
            }
            if !used_ports.insert(port) {
                let owner = specs
                    .iter()
                    .find(|candidate| {
                        candidate.id != spec.id && assigned.get(&candidate.id) == Some(&port)
                    })
                    .map(|candidate| candidate.id.as_str())
                    .unwrap_or("<unknown>");
                return Err(anyhow!(
                    "duplicate p2p port {} for nodes '{}' and '{}'", port, owner, spec.id
                ));
            }
        }
    }

    for (index, spec) in specs.iter().enumerate() {
        if assigned.contains_key(&spec.id) {
            continue;
        }

        let mut candidate = base_p2p_port + index as u16;
        while used_ports.contains(&candidate) {
            candidate = candidate
                .checked_add(1)
                .ok_or_else(|| anyhow!("ran out of p2p ports while assigning {}", spec.id))?;
        }

        used_ports.insert(candidate);
        assigned.insert(spec.id.clone(), candidate);
    }

    Ok(assigned)
}

fn build_p2p_port_map(configs: &HashMap<String, NodeConfig>) -> Result<HashMap<u16, String>> {
    let mut map = HashMap::new();
    for config in configs.values() {
        if let Some(existing) = map.insert(config.p2p_port, config.spec.id.clone()) {
            return Err(anyhow!(
                "duplicate p2p port {} for nodes '{}' and '{}'", config.p2p_port, existing,
                config.spec.id
            ));
        }
    }
    Ok(map)
}

fn format_docker_peer_addr(
    host_proto: &str,
    host: &str,
    port: u16,
    peer_id: Option<&str>,
) -> String {
    let mut addr = format!("/{host_proto}/{host}/udp/{port}/quic-v1");
    if let Some(peer_id) = peer_id {
        addr.push_str("/p2p/");
        addr.push_str(peer_id);
    }
    addr
}

fn rewrite_bind_for_docker(bind: &str) -> Result<String> {
    if let Some(rest) = bind.strip_prefix("/ip4/") {
        let mut parts = rest.splitn(2, '/');
        let _ = parts.next();
        let tail = parts.next().unwrap_or("");
        return Ok(format!("/ip4/0.0.0.0/{}", tail));
    }
    Ok(bind.to_string())
}

fn container_path_for(config: &NodeConfig, host_path: &Path) -> Result<String> {
    let rel = host_path.strip_prefix(&config.work_dir).map_err(|_| {
        anyhow!(
            "path {} is outside node data dir {}",
            host_path.display(),
            config.work_dir.display()
        )
    })?;
    Ok(Path::new(DOCKER_WORKDIR).join(rel).display().to_string())
}

struct ImageRef {
    name: String,
    tag: String,
}

fn head_info_from_private(head: PrivateHeadInfo) -> HeadInfo {
    HeadInfo {
        height: head.height,
        block_id: head.block_id.map(PbHash::from),
    }
}

fn resolve_docker_image(config: &NodeConfig, default_image: &str) -> Result<String> {
    if let Some(path) = &config.spec.binary {
        let expanded = expand_env_vars(&path.to_string_lossy())?;
        return Ok(expanded);
    }
    Ok(default_image.to_string())
}

fn parse_image_ref(image: &str) -> ImageRef {
    let last_slash = image.rfind('/').unwrap_or(0);
    let tag_split = image[last_slash..].rfind(':');
    if let Some(idx) = tag_split {
        let split_at = last_slash + idx;
        let (name, tag) = image.split_at(split_at);
        return ImageRef {
            name: name.to_string(),
            tag: tag.trim_start_matches(':').to_string(),
        };
    }
    ImageRef {
        name: image.to_string(),
        tag: "latest".to_string(),
    }
}

fn docker_container_name(network: &str, node_id: &str) -> String {
    format!("{}-{}", network, node_id)
}

fn create_docker_network(name: &str) -> Result<()> {
    let output = std::process::Command::new("docker")
        .args(["network", "create", name])
        .output()?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        if stderr.contains("already exists") {
            return Ok(());
        }
        return Err(anyhow!("docker network create failed: {}", stderr.trim()));
    }
    Ok(())
}

fn remove_docker_network(name: &str) -> Result<()> {
    let output = std::process::Command::new("docker")
        .args(["network", "rm", name])
        .output()?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        if !stderr.contains("No such network") {
            return Err(anyhow!("docker network rm failed: {}", stderr.trim()));
        }
    }
    Ok(())
}

fn remove_stale_docker_container(name: &str) -> Result<()> {
    let output = std::process::Command::new("docker")
        .args(["rm", "-f", name])
        .output()?;
    if output.status.success() {
        return Ok(());
    }

    let stderr = String::from_utf8_lossy(&output.stderr);
    if stderr.contains("No such container") {
        return Ok(());
    }

    Err(anyhow!("docker rm -f {} failed: {}", name, stderr.trim()))
}

impl NodeManager {
    async fn rewrite_peers_for_docker(
        &self,
        spec: &NodeSpec,
        port_map: &HashMap<u16, String>,
        network: &str,
    ) -> Result<DockerPeers> {
        let mut peers = Vec::new();
        for peer in &spec.peers {
            peers.push(
                self.rewrite_peer_addr_for_docker(peer, port_map, network)
                    .await?,
            );
        }
        let mut force_peers = Vec::new();
        for peer in &spec.force_peers {
            force_peers.push(
                self.rewrite_peer_addr_for_docker(peer, port_map, network)
                    .await?,
            );
        }
        Ok(DockerPeers { peers, force_peers })
    }

    async fn rewrite_peer_addr_for_docker(
        &self,
        addr: &str,
        port_map: &HashMap<u16, String>,
        network: &str,
    ) -> Result<String> {
        let port = extract_udp_port(addr)
            .ok_or_else(|| anyhow!("failed to parse udp port from peer '{addr}'"))?;
        let node_id = port_map
            .get(&port)
            .ok_or_else(|| anyhow!("peer port {} not found in scenario", port))?;
        let peer_id = self.try_resolve_peer_id(node_id).await?;
        self.docker_peer_addr_for_node(node_id, port, network, peer_id.as_deref())
    }

    fn docker_peer_addr_for_node(
        &self,
        node_id: &str,
        port: u16,
        network: &str,
        peer_id: Option<&str>,
    ) -> Result<String> {
        if let Some(ip) = self.docker_node_ip(node_id, network)? {
            return Ok(format_docker_peer_addr("ip4", &ip, port, peer_id));
        }
        let container_name = docker_container_name(network, node_id);
        Ok(format_docker_peer_addr(
            "dns4", &container_name, port, peer_id,
        ))
    }

    fn docker_node_ip(&self, node_id: &str, network: &str) -> Result<Option<String>> {
        let Some(NodeHandle {
            node: ChildHandle::Docker(_),
            ..
        }) = self.handles.get(node_id)
        else {
            return Ok(None);
        };

        docker_container_network_ip(&docker_container_name(network, node_id), network)
    }

    async fn try_resolve_peer_id(&self, node_id: &str) -> Result<Option<String>> {
        let config = self
            .configs
            .get(node_id)
            .ok_or_else(|| anyhow!("unknown node id '{node_id}'"))?;
        if let Some(peer_id) = read_peer_id_if_present(&config.work_dir)? {
            return Ok(Some(peer_id));
        }
        if self.handles.contains_key(node_id) {
            if let Ok(peer_id) = wait_for_peer_id(&config.work_dir).await {
                return Ok(Some(peer_id));
            }
        }
        Ok(None)
    }

    async fn resolve_peer_from(&self, spec: &NodeSpec) -> Result<Vec<String>> {
        let mut resolved = Vec::new();
        for peer_from in &spec.peer_from {
            resolved.push(self.resolve_peer_entry(peer_from).await?);
        }
        if !spec.new_state {
            for peer_from in &spec.restart_peer_from {
                if let Some(peer) = self.try_resolve_peer_entry(peer_from).await? {
                    resolved.push(peer);
                }
            }
        }
        Ok(resolved)
    }

    async fn try_resolve_peer_entry(&self, peer_from: &PeerFrom) -> Result<Option<String>> {
        let Some(peer_id) = self.try_resolve_peer_id(&peer_from.node).await? else {
            return Ok(None);
        };
        if let NodeMode::Docker { network, .. } = &self.mode {
            let port = extract_udp_port(&peer_from.listen).ok_or_else(|| {
                anyhow!("failed to parse udp port from peer_from listen '{}'", peer_from.listen)
            })?;
            return self
                .docker_peer_addr_for_node(&peer_from.node, port, network, Some(&peer_id))
                .map(Some);
        }
        Ok(Some(format!("{}/p2p/{}", peer_from.listen, peer_id)))
    }

    async fn resolve_peer_entry(&self, peer_from: &PeerFrom) -> Result<String> {
        let config = self
            .configs
            .get(&peer_from.node)
            .ok_or_else(|| anyhow!("unknown peer_from node '{}'", peer_from.node))?;
        let peer_id = wait_for_peer_id(&config.work_dir).await?;
        if let NodeMode::Docker { network, .. } = &self.mode {
            let port = extract_udp_port(&peer_from.listen).ok_or_else(|| {
                anyhow!("failed to parse udp port from peer_from listen '{}'", peer_from.listen)
            })?;
            return self.docker_peer_addr_for_node(&peer_from.node, port, network, Some(&peer_id));
        }
        Ok(format!("{}/p2p/{}", peer_from.listen, peer_id))
    }
}

async fn wait_for_peer_id(work_dir: &Path) -> Result<String> {
    let path = work_dir.join(PEER_ID_FILENAME);
    let deadline = Instant::now() + PEER_ID_WAIT_TIMEOUT;
    loop {
        match std::fs::read_to_string(&path) {
            Ok(contents) => return Ok(contents.trim().to_string()),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                if Instant::now() >= deadline {
                    return Err(anyhow!("peer id file not found at {}", path.display()));
                }
            }
            Err(err) => return Err(err.into()),
        }
        sleep(Duration::from_millis(100)).await;
    }
}

fn read_peer_id_if_present(work_dir: &Path) -> Result<Option<String>> {
    let path = work_dir.join(PEER_ID_FILENAME);
    match std::fs::read_to_string(path) {
        Ok(peer_id) => Ok(Some(peer_id.trim().to_string())),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(err) => Err(err.into()),
    }
}

fn read_log_file(path: &Path) -> Result<String> {
    match std::fs::read_to_string(path) {
        Ok(contents) => Ok(contents),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(String::new()),
        Err(err) => Err(err.into()),
    }
}

fn ensure_tcp_port_available(
    node_id: &str,
    port_kind: &str,
    addr: &str,
    guidance: &str,
) -> Result<()> {
    let listener = StdTcpListener::bind(addr).map_err(|err| {
        anyhow!(
            "node '{}' cannot start because {} port {} is unavailable: {} ({})", node_id,
            port_kind, addr, err, guidance
        )
    })?;
    drop(listener);
    Ok(())
}

fn ensure_udp_port_available(
    node_id: &str,
    port_kind: &str,
    port: u16,
    guidance: &str,
) -> Result<()> {
    let addr = ("127.0.0.1", port);
    let socket = StdUdpSocket::bind(addr).map_err(|err| {
        anyhow!(
            "node '{}' cannot start because {} port {} is unavailable: {} ({})", node_id,
            port_kind, port, err, guidance
        )
    })?;
    drop(socket);
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::net::{TcpListener, UdpSocket};
    use std::path::PathBuf;
    use std::time::{SystemTime, UNIX_EPOCH};

    use anyhow::{anyhow, Context, Result};
    use tokio::time::{timeout, Duration};

    #[test]
    fn persist_log_bytes_writes_stdout_and_stderr() {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time")
            .as_nanos();
        let temp_dir = std::env::temp_dir().join(format!("nockchain-e2e-node-tests-{nonce}"));
        std::fs::create_dir_all(&temp_dir).expect("create temp dir");
        let stdout_path = temp_dir.join("stdout.log");
        let stderr_path = temp_dir.join("stderr.log");

        crate::node::persist_log_bytes(&stdout_path, &stderr_path, "stdout body", "stderr body")
            .expect("persist log bytes");

        assert_eq!(
            std::fs::read_to_string(stdout_path).expect("read stdout"),
            "stdout body"
        );
        assert_eq!(
            std::fs::read_to_string(stderr_path).expect("read stderr"),
            "stderr body"
        );
        std::fs::remove_dir_all(temp_dir).expect("cleanup temp dir");
    }

    #[test]
    fn log_tail_preview_keeps_last_non_empty_lines() {
        let logs = "\nfirst\n\nsecond\nthird\n";

        let preview = crate::node::log_tail_preview(logs, 2);

        assert_eq!(preview, "second\nthird");
    }

    #[test]
    fn log_tail_preview_handles_empty_logs() {
        assert_eq!(crate::node::log_tail_preview("", 4), "");
    }

    #[test]
    fn format_docker_peer_addr_appends_peer_id_when_present() {
        let addr =
            crate::node::format_docker_peer_addr("ip4", "172.18.0.2", 4801, Some("peer-123"));

        assert_eq!(addr, "/ip4/172.18.0.2/udp/4801/quic-v1/p2p/peer-123");
    }

    #[test]
    fn format_docker_peer_addr_omits_peer_id_when_absent() {
        let addr = crate::node::format_docker_peer_addr("dns4", "nockchain-e2e-node-a", 4801, None);

        assert_eq!(addr, "/dns4/nockchain-e2e-node-a/udp/4801/quic-v1");
    }

    #[test]
    fn resolve_p2p_ports_skips_explicit_bind_ports_for_fallback_nodes() {
        let specs = vec![
            nockchain_testkit::NodeSpec {
                id: "node-a".to_string(),
                bind: vec!["/ip4/127.0.0.1/udp/4101/quic-v1".to_string()],
                ..dummy_node_spec("node-a")
            },
            nockchain_testkit::NodeSpec {
                id: "node-b".to_string(),
                ..dummy_node_spec("node-b")
            },
        ];

        let ports = crate::node::resolve_p2p_ports(&specs, 4100).expect("resolve p2p ports");

        assert_eq!(ports.get("node-a"), Some(&4101));
        assert_eq!(ports.get("node-b"), Some(&4102));
    }

    #[test]
    fn resolve_p2p_ports_rejects_duplicate_explicit_bind_ports() {
        let specs = vec![
            nockchain_testkit::NodeSpec {
                id: "node-a".to_string(),
                bind: vec!["/ip4/127.0.0.1/udp/4101/quic-v1".to_string()],
                ..dummy_node_spec("node-a")
            },
            nockchain_testkit::NodeSpec {
                id: "node-b".to_string(),
                bind: vec!["/ip4/127.0.0.1/udp/4101/quic-v1".to_string()],
                ..dummy_node_spec("node-b")
            },
        ];

        let err =
            crate::node::resolve_p2p_ports(&specs, 4100).expect_err("duplicate bind should fail");

        assert!(err
            .to_string()
            .contains("duplicate p2p port 4101 for nodes 'node-a' and 'node-b'"));
    }

    #[test]
    fn set_node_env_updates_stopped_node_config() {
        let config = dummy_node_config();
        let mut manager = crate::node::NodeManager {
            mode: crate::node::NodeMode::Process,
            configs: HashMap::from([(String::from("node-a"), config)]),
            handles: HashMap::new(),
        };

        manager
            .set_node_env(
                "node-a",
                String::from("NOCKCHAIN_LIBP2P_REQ_RES_GEN2_BUNDLE_ENABLED"),
                String::from("true"),
            )
            .expect("set env should succeed");

        let stored = manager
            .configs
            .get("node-a")
            .and_then(|entry| {
                entry
                    .spec
                    .env
                    .get("NOCKCHAIN_LIBP2P_REQ_RES_GEN2_BUNDLE_ENABLED")
            })
            .map(String::as_str);
        assert_eq!(stored, Some("true"));
    }

    #[tokio::test]
    async fn start_nodes_fails_when_public_grpc_port_is_occupied() -> Result<()> {
        let busy_public = TcpListener::bind("127.0.0.1:0")?;
        let addr = busy_public.local_addr()?.to_string();

        let err = crate::node::ensure_tcp_port_available(
            "node-a", "public gRPC", &addr,
            "choose a different --base-grpc-port block or stop the stale process",
        )
        .expect_err("occupied public gRPC port should fail before spawn");

        assert!(err.to_string().contains("public gRPC port"));
        assert!(err.to_string().contains("base-grpc-port"));
        Ok(())
    }

    #[tokio::test]
    async fn start_nodes_fails_when_private_grpc_port_is_occupied() -> Result<()> {
        let busy_private = TcpListener::bind("127.0.0.1:0")?;
        let addr = busy_private.local_addr()?.to_string();

        let err = crate::node::ensure_tcp_port_available(
            "node-a", "private gRPC", &addr,
            "choose a different --base-private-grpc-port block or stop the stale process",
        )
        .expect_err("occupied private gRPC port should fail before spawn");

        assert!(err.to_string().contains("private gRPC port"));
        assert!(err.to_string().contains("base-private-grpc-port"));
        Ok(())
    }

    #[tokio::test]
    async fn start_nodes_fails_when_p2p_port_is_occupied() -> Result<()> {
        let busy_p2p = UdpSocket::bind("127.0.0.1:0")?;
        let port = busy_p2p.local_addr()?.port();

        let err = crate::node::ensure_udp_port_available(
            "node-a", "p2p", port,
            "choose a different --base-p2p-port block or stop the stale process",
        )
        .expect_err("occupied p2p port should fail before spawn");

        assert!(err.to_string().contains("p2p port"));
        assert!(err.to_string().contains("base-p2p-port"));
        Ok(())
    }

    #[tokio::test]
    async fn fetch_private_head_if_current_skips_unowned_node_ports() -> Result<()> {
        let busy_private = TcpListener::bind("127.0.0.1:0")?;
        let mut config = dummy_node_config();
        config.grpc_private_port = busy_private.local_addr()?.port();
        config.grpc_private_addr = format!("127.0.0.1:{}", config.grpc_private_port);
        let mut manager = crate::node::NodeManager {
            mode: crate::node::NodeMode::Process,
            configs: HashMap::from([(String::from("node-a"), config)]),
            handles: HashMap::new(),
        };

        let head = manager.fetch_private_head_if_current("node-a").await?;
        assert!(head.is_none(), "unowned node ports should not be queried");
        Ok(())
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn fetch_private_head_if_current_skips_dead_process_handles() -> Result<()> {
        let busy_private = TcpListener::bind("127.0.0.1:0")?;
        let mut config = dummy_node_config();
        config.grpc_private_port = busy_private.local_addr()?.port();
        config.grpc_private_addr = format!("127.0.0.1:{}", config.grpc_private_port);
        let mut child = tokio::process::Command::new("sh")
            .arg("-c")
            .arg("exit 0")
            .spawn()?;
        let status = timeout(Duration::from_secs(2), child.wait())
            .await
            .context("dead-process test child did not exit")??;
        assert!(
            status.success(),
            "dead-process test child should exit cleanly"
        );
        let mut manager = crate::node::NodeManager {
            mode: crate::node::NodeMode::Process,
            configs: HashMap::from([(String::from("node-a"), config)]),
            handles: HashMap::from([(
                String::from("node-a"),
                crate::node::NodeHandle {
                    node: crate::node::ChildHandle::Process(crate::node::ProcessHandle {
                        child: tokio::sync::Mutex::new(child),
                    }),
                    miner: None,
                },
            )]),
        };

        let head = manager.fetch_private_head_if_current("node-a").await?;
        assert!(
            head.is_none(),
            "dead process handles should suppress final summaries"
        );
        Ok(())
    }

    #[tokio::test]
    async fn resolve_peer_from_adds_restart_peers_after_peer_id_exists() -> Result<()> {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time")
            .as_nanos();
        let temp_dir = std::env::temp_dir().join(format!("nockchain-e2e-peer-from-{nonce}"));
        let initial_dir = temp_dir.join("initial");
        let restart_dir = temp_dir.join("restart");
        std::fs::create_dir_all(&initial_dir)?;
        std::fs::create_dir_all(&restart_dir)?;
        std::fs::write(
            initial_dir.join(crate::node::PEER_ID_FILENAME),
            "initial-peer\n",
        )?;

        let mut initial = dummy_node_config();
        initial.spec.id = String::from("initial");
        initial.base_spec.id = String::from("initial");
        initial.work_dir = initial_dir;
        let mut restart = dummy_node_config();
        restart.spec.id = String::from("restart");
        restart.base_spec.id = String::from("restart");
        restart.work_dir = restart_dir.clone();

        let manager = crate::node::NodeManager {
            mode: crate::node::NodeMode::Process,
            configs: HashMap::from([
                (String::from("initial"), initial),
                (String::from("restart"), restart),
            ]),
            handles: HashMap::new(),
        };
        let mut spec = dummy_node_spec("node-a");
        spec.peer_from = vec![nockchain_testkit::PeerFrom {
            node: String::from("initial"),
            listen: String::from("/ip4/127.0.0.1/udp/4100/quic-v1"),
        }];
        spec.restart_peer_from = vec![nockchain_testkit::PeerFrom {
            node: String::from("restart"),
            listen: String::from("/ip4/127.0.0.1/udp/4101/quic-v1"),
        }];

        let first_start = manager.resolve_peer_from(&spec).await?;
        spec.new_state = false;
        let restart_before_peer_id = manager.resolve_peer_from(&spec).await?;
        std::fs::write(
            restart_dir.join(crate::node::PEER_ID_FILENAME),
            "restart-peer\n",
        )?;
        let restarted = manager.resolve_peer_from(&spec).await?;

        assert_eq!(
            first_start,
            vec!["/ip4/127.0.0.1/udp/4100/quic-v1/p2p/initial-peer"]
        );
        assert_eq!(
            restart_before_peer_id,
            vec!["/ip4/127.0.0.1/udp/4100/quic-v1/p2p/initial-peer"]
        );
        assert_eq!(
            restarted,
            vec![
                "/ip4/127.0.0.1/udp/4100/quic-v1/p2p/initial-peer",
                "/ip4/127.0.0.1/udp/4101/quic-v1/p2p/restart-peer",
            ]
        );

        std::fs::remove_dir_all(temp_dir).expect("cleanup temp dir");
        Ok(())
    }

    #[test]
    fn restarting_preserves_previous_child_logs() -> Result<()> {
        let run_dir = actual_run_dir("log-retention")?;
        std::fs::write(run_dir.join("stdout.log"), "first launch stdout")?;
        std::fs::write(run_dir.join("stderr.log"), "first launch stderr")?;
        crate::node::archive_previous_logs(&run_dir)?;
        assert!(!run_dir.join("stdout.log").try_exists()?);
        assert!(!run_dir.join("stderr.log").try_exists()?);
        std::fs::write(run_dir.join("stdout.log"), "second launch")?;
        crate::node::archive_previous_logs(&run_dir)?;
        std::fs::write(run_dir.join("stdout.log"), "current launch")?;
        assert_eq!(
            std::fs::read_to_string(run_dir.join("stdout.log"))?,
            "current launch"
        );
        assert_eq!(
            std::fs::read_to_string(run_dir.join("logs/launch-001/stdout.log"))?,
            "first launch stdout",
        );
        assert_eq!(
            std::fs::read_to_string(run_dir.join("logs/launch-001/stderr.log"))?,
            "first launch stderr",
        );
        assert_eq!(
            std::fs::read_to_string(run_dir.join("logs/launch-002/stdout.log"))?,
            "second launch",
        );
        std::fs::remove_dir_all(run_dir)?;
        Ok(())
    }

    #[tokio::test]
    #[ignore = "requires real NOCKCHAIN_BIN_NEW and NOCKCHAIN_MINER_BIN binaries"]
    async fn genuine_process_miner_lifecycle() -> Result<()> {
        let node_bin = actual_binary("NOCKCHAIN_BIN_NEW")?;
        let miner_bin = actual_binary("NOCKCHAIN_MINER_BIN")?;
        exercise_actual_miner(
            crate::node::NodeMode::Process,
            Some(node_bin),
            Some(miner_bin),
        )
        .await
    }

    #[tokio::test]
    #[ignore = "requires local Docker and real NOCKCHAIN_E2E_IMAGE/NOCKCHAIN_E2E_MINER_IMAGE CPU images"]
    async fn genuine_docker_miner_lifecycle() -> Result<()> {
        let mode = crate::node::NodeMode::Docker {
            image: std::env::var("NOCKCHAIN_E2E_IMAGE").context("set NOCKCHAIN_E2E_IMAGE")?,
            network: format!("nockchain-e2e-miner-{}", actual_nonce()?),
            miner_image: std::env::var("NOCKCHAIN_E2E_MINER_IMAGE")
                .context("set NOCKCHAIN_E2E_MINER_IMAGE")?,
            miner_binary: PathBuf::from("/usr/local/bin/zk-pow-mine"),
        };
        exercise_actual_miner(mode, None, None).await
    }

    #[tokio::test]
    #[ignore = "requires a real NOCKCHAIN_BIN_NEW binary"]
    async fn genuine_missing_miner_startup_releases_owned_children() -> Result<()> {
        let node_bin = actual_binary("NOCKCHAIN_BIN_NEW")?;
        let config = dummy_node_config();
        let mut spec = actual_node_spec();
        spec.mine = true;
        let run_dir = actual_run_dir("missing-miner-startup")?;
        let miner_bin = run_dir.join("missing-zk-pow-mine");
        eprintln!("missing-miner startup artifacts: {}", run_dir.display());
        let mut manager = crate::node::NodeManager::new(
            &[spec],
            &run_dir,
            Some(node_bin),
            Some(miner_bin),
            config.grpc_public_port,
            config.grpc_private_port,
            config.p2p_port,
            crate::node::NodeMode::Process,
        )?;
        let start = manager.start_nodes(&["node-a".to_string()]).await;
        let released = manager.handles.is_empty();
        let public = TcpListener::bind(("127.0.0.1", config.grpc_public_port));
        let private = TcpListener::bind(("127.0.0.1", config.grpc_private_port));
        let p2p = UdpSocket::bind(("127.0.0.1", config.p2p_port));
        manager.shutdown_all().await?;
        let failure = start
            .err()
            .context("startup must fail when its miner executable is absent")?;
        assert!(
            matches!(
                failure.downcast_ref::<std::io::Error>(),
                Some(error) if error.kind() == std::io::ErrorKind::NotFound,
            ),
            "unexpected startup failure: {failure:#}"
        );
        assert!(
            released,
            "failed startup must release its children before shutdown"
        );
        let (_public, _private, _p2p) = (public?, private?, p2p?);
        Ok(())
    }

    async fn exercise_actual_miner(
        mode: crate::node::NodeMode,
        node_bin: Option<PathBuf>,
        miner_bin: Option<PathBuf>,
    ) -> Result<()> {
        let config = dummy_node_config();
        let upgrade = match &mode {
            crate::node::NodeMode::Process => miner_bin.clone().context("actual miner binary")?,
            crate::node::NodeMode::Docker { miner_image, .. } => PathBuf::from(miner_image),
        };
        let run_dir = actual_run_dir("real-miner-lifecycle")?;
        eprintln!("real-miner lifecycle artifacts: {}", run_dir.display());
        let mut manager = crate::node::NodeManager::new(
            &[actual_node_spec()],
            &run_dir,
            node_bin,
            miner_bin,
            config.grpc_public_port,
            config.grpc_private_port,
            config.p2p_port,
            mode,
        )?;
        let result = exercise_actual_miner_steps(&mut manager, upgrade).await;
        let shutdown = manager.shutdown_all().await;
        match (result, shutdown) {
            (Err(error), Err(shutdown)) => {
                return Err(error.context(format!("shutdown also failed: {shutdown:#}")))
            }
            (Err(error), Ok(())) | (Ok(_), Err(error)) => return Err(error),
            (Ok((public_port, private_port)), Ok(())) => {
                let _public = TcpListener::bind(("127.0.0.1", public_port))?;
                let _private = TcpListener::bind(("127.0.0.1", private_port))?;
            }
        }
        assert!(manager.handles.is_empty());
        let config = manager
            .configs
            .get("node-a")
            .context("actual node config")?;
        assert_actual_logs(&config.work_dir)?;
        assert_actual_logs(&config.miner_work_dir)?;
        assert_actual_logs(&config.work_dir.join("logs/launch-001"))?;
        assert_actual_logs(&config.miner_work_dir.join("logs/launch-001"))?;
        if matches!(&manager.mode, crate::node::NodeMode::Process) {
            let _p2p = UdpSocket::bind(("127.0.0.1", config.p2p_port))?;
        }
        Ok(())
    }

    async fn exercise_actual_miner_steps(
        manager: &mut crate::node::NodeManager,
        upgrade: PathBuf,
    ) -> Result<(u16, u16)> {
        let id = "node-a";
        manager.start_nodes(&[id.to_string()]).await?;
        assert!(manager
            .handles
            .get(id)
            .context("owned node")?
            .miner
            .is_none());
        let private_addr = manager.grpc_private_addr(id)?.to_string();
        let denied = crate::grpc::fetch_heaviest_private(&private_addr, None)
            .await
            .err()
            .context("private RPC without credentials must fail")?;
        assert!(
            matches!(
                denied.downcast_ref::<crate::grpc::PrivateRpcError>(),
                Some(crate::grpc::PrivateRpcError::Status(status))
                    if status.code() == tonic::Code::Unauthenticated,
            ),
            "unexpected unauthenticated RPC failure: {denied:#}"
        );
        let initial = manager.fetch_private_head(id).await?;
        assert_eq!(initial.height, 0);
        if let crate::node::NodeMode::Docker { network, .. } = &manager.mode {
            let output = tokio::process::Command::new("docker")
                .args(["inspect", &crate::node::docker_container_name(network, id)])
                .output()
                .await?;
            if !output.status.success() {
                return Err(anyhow!(
                    "inspect actual node: {}",
                    String::from_utf8_lossy(&output.stderr)
                ));
            }
            let details: serde_json::Value = serde_json::from_slice(&output.stdout)?;
            let internal_port = manager
                .configs
                .get(id)
                .context("node config")?
                .grpc_private_port;
            let key = format!("{internal_port}/tcp");
            let bindings = details[0]["NetworkSettings"]["Ports"][&key]
                .as_array()
                .context("actual private port bindings")?;
            assert!(!bindings.is_empty());
            assert!(bindings
                .iter()
                .all(|binding| binding["HostIp"].as_str() == Some("127.0.0.1")));
        }
        manager.set_mining_enabled(id, true).await?;
        // Phase three requires genuine v0 proofs before the v1 reward transition.
        manager
            .wait_for_height(id, 2, Duration::from_secs(300))
            .await?;
        manager.set_mining_enabled(id, false).await?;
        assert!(manager
            .handles
            .get(id)
            .context("owned node")?
            .miner
            .is_none());
        let paused = manager.fetch_private_head(id).await?.height;
        tokio::time::sleep(Duration::from_secs(2)).await;
        assert_eq!(manager.fetch_private_head(id).await?.height, paused);
        manager.restart_nodes(&[id.to_string()]).await?;
        assert_eq!(manager.fetch_private_head(id).await?.height, paused);
        assert!(manager
            .handles
            .get(id)
            .context("restarted node")?
            .miner
            .is_none());
        let payout = manager
            .configs
            .get(id)
            .context("node config")?
            .spec
            .mining_pkh
            .clone()
            .context("retained actual payout")?;
        manager.set_mining_pkh(id, payout).await?;
        manager
            .wait_for_height(id, paused + 2, Duration::from_secs(300))
            .await?;
        manager.upgrade_miner(id, upgrade).await?;
        let before_upgrade = manager.fetch_private_head(id).await?.height;
        manager
            .wait_for_height(id, before_upgrade + 1, Duration::from_secs(300))
            .await?;
        let miner = manager
            .handles
            .get(id)
            .context("owned node")?
            .miner
            .as_ref()
            .context("owned real miner")?;
        let wait = timeout(
            Duration::from_secs(5),
            manager.wait_for_height(id, u64::MAX, Duration::from_secs(30)),
        );
        let crash = async {
            tokio::time::sleep(Duration::from_millis(200)).await;
            match miner {
                crate::node::ChildHandle::Process(handle) => {
                    handle.child.lock().await.kill().await?
                }
                crate::node::ChildHandle::Docker(container) => {
                    container.stop_with_timeout(Some(0)).await?
                }
            }
            Ok::<(), anyhow::Error>(())
        };
        let (wait, crash) = tokio::join!(wait, crash);
        crash?;
        assert!(wait
            .context("owned miner crash did not promptly interrupt height wait")?
            .is_err());
        assert!(manager.node_handle_is_live(id).await?);
        let public_port = manager
            .grpc_public_addr(id)?
            .rsplit(':')
            .next()
            .context("actual public port")?
            .parse()?;
        Ok((public_port, manager.grpc_private_port(id)?))
    }

    fn actual_node_spec() -> nockchain_testkit::NodeSpec {
        let mut spec = dummy_node_spec("node-a");
        spec.mining_pkh =
            Some("9yPePjfWAdUnzaQKyxcRXKRa5PpUzKKEwtpECBZsUYt9Jd7egSDEWoV".to_string());
        spec.no_default_peers = true;
        spec.fakenet_pow_len = Some(2);
        spec.fakenet_log_difficulty = Some(1);
        spec.fakenet_v1_phase = Some(3);
        spec.fakenet_update_candidate_interval_secs = Some(1);
        spec.env.insert(
            "NOCKCHAIN_PRIVATE_GRPC_AUTH_TOKEN".to_string(),
            "0123456789abcdef0123456789abcdef".to_string(),
        );
        spec.env.insert(
            "NOCKCHAIN_PRIVATE_GRPC_MINER_TOKEN".to_string(),
            "abcdef0123456789abcdef0123456789".to_string(),
        );
        spec
    }

    fn actual_binary(key: &str) -> Result<PathBuf> {
        std::env::var_os(key)
            .map(PathBuf::from)
            .with_context(|| format!("set {key} to a real binary"))
    }

    fn actual_nonce() -> Result<u128> {
        Ok(SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos())
    }

    fn actual_run_dir(name: &str) -> Result<PathBuf> {
        let dir = std::env::temp_dir().join(format!("nockchain-e2e-{name}-{}", actual_nonce()?));
        std::fs::create_dir_all(&dir)?;
        Ok(dir)
    }

    fn assert_actual_logs(work_dir: &std::path::Path) -> Result<()> {
        let stdout = std::fs::read(work_dir.join("stdout.log"))?;
        let stderr = std::fs::read(work_dir.join("stderr.log"))?;
        assert!(
            !stdout.is_empty() || !stderr.is_empty(),
            "real child logs must be retained"
        );
        Ok(())
    }

    fn dummy_node_spec(id: &str) -> nockchain_testkit::NodeSpec {
        nockchain_testkit::NodeSpec {
            id: id.to_string(),
            grpc_public_addr: None,
            grpc_private_port: None,
            grpc_enabled: true,
            data_dir: None,
            fakenet: true,
            mine: false,
            mining_pkh: None,
            mining_threads: 1,
            miner_binary: None,
            miner_image: None,
            peers: Vec::new(),
            peer_from: Vec::new(),
            restart_peer_from: Vec::new(),
            force_peers: Vec::new(),
            bind: Vec::new(),
            new_state: true,
            no_default_peers: false,
            allowed_peers_path: None,
            fakenet_pow_len: None,
            fakenet_log_difficulty: None,
            fakenet_v1_phase: None,
            fakenet_bythos_phase: None,
            fakenet_update_candidate_interval_secs: None,
            fakenet_genesis_jam_path: None,
            extra_args: Vec::new(),
            env: Default::default(),
            binary: None,
        }
    }

    fn dummy_node_config() -> crate::node::NodeConfig {
        let spec = dummy_node_spec("node-a");
        let grpc_public_port = allocate_tcp_port();
        let mut grpc_private_port = allocate_tcp_port();
        while grpc_private_port == grpc_public_port {
            grpc_private_port = allocate_tcp_port();
        }
        let mut p2p_port = allocate_udp_port();
        while p2p_port == grpc_public_port || p2p_port == grpc_private_port {
            p2p_port = allocate_udp_port();
        }
        crate::node::NodeConfig {
            base_spec: spec.clone(),
            spec,
            grpc_public_addr: format!("127.0.0.1:{grpc_public_port}"),
            grpc_public_port,
            grpc_private_addr: format!("127.0.0.1:{grpc_private_port}"),
            grpc_private_port,
            grpc_private_host_port: None,
            p2p_port,
            work_dir: std::env::temp_dir().join("nockchain-e2e-node-args-test"),
            miner_work_dir: std::env::temp_dir().join("nockchain-e2e-miner-tests"),
            binary: PathBuf::from("nockchain"),
            miner_binary: PathBuf::from("zk-pow-mine"),
            miner_supports_cpu_flag: None,
            operator_token: None,
            miner_token: None,
            supports_bythos_phase: true,
        }
    }

    fn allocate_tcp_port() -> u16 {
        TcpListener::bind("127.0.0.1:0")
            .expect("allocate test tcp port")
            .local_addr()
            .expect("read test tcp addr")
            .port()
    }

    fn allocate_udp_port() -> u16 {
        UdpSocket::bind("127.0.0.1:0")
            .expect("allocate test udp port")
            .local_addr()
            .expect("read test udp addr")
            .port()
    }
}

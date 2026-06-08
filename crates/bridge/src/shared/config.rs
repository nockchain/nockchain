use std::collections::HashSet;
use std::convert::TryInto;
use std::fs;
use std::path::{Path, PathBuf};
use std::str::FromStr;

use alloy::primitives::Address;
use nockchain_math::belt::{Belt, PRIME};
use nockchain_types::tx_engine::common::Hash as NockPkh;
use nockchain_types::v1::{Lock, LockPrimitive, Pkh, SpendCondition};
use num_bigint::BigUint;
use serde::{Deserialize, Serialize};

use crate::shared::errors::BridgeError;
use crate::shared::types::{
    AtomBytes, BridgeConstants, NodeConfig, NodeInfo, SchnorrSecretKey, Tip5Hash,
};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BridgeConfigToml {
    pub node_id: u64,
    pub base_ws_url: String,
    pub bridge_lock_root: String,
    #[serde(default)]
    pub inbox_contract_address: Option<String>,
    #[serde(default)]
    pub nock_contract_address: Option<String>,
    pub my_eth_key: String,
    pub my_nock_key: String,
    pub grpc_address: String,
    #[serde(default)]
    pub nockchain_sequencer_api_address: Option<String>,
    /// Number of confirmations required on Base before sending a batch to the kernel.
    /// Zero means the latest observed tip is eligible immediately.
    pub base_confirmation_depth: u64,
    /// Number of confirmations required on nockchain before sending a block to the kernel.
    /// Zero means the latest observed tip is eligible immediately.
    pub nockchain_confirmation_depth: u64,
    /// Base contract lastDepositNonce at the time the deposit nonce epoch is activated.
    ///
    /// When non-zero, this must equal the nonce of the anchor deposit identified by
    /// deposit_nonce_epoch_start_height + deposit_nonce_epoch_start_tx_id_base58.
    /// When zero, the start height/tx-id may be omitted to start from the first deposit
    /// at/after the default start height.
    ///
    /// The explicit deposit nonce epoch exists because deposit nonce semantics were
    /// corrected by an ad-hoc update about a month after launch; the epoch anchors
    /// runtime signing to the first deposit governed by those corrected semantics.
    #[serde(default, alias = "nonce_epoch_base")]
    pub deposit_nonce_epoch_base: Option<u64>,
    /// Nockchain height at which the deposit nonce epoch starts.
    ///
    /// Deposits with `block_height < deposit_nonce_epoch_start_height` will not be signed under the
    /// epoch scheme, and are expected to have been handled prior to activation.
    ///
    /// Required when deposit_nonce_epoch_base is non-zero.
    #[serde(default, alias = "nonce_epoch_start_height")]
    pub deposit_nonce_epoch_start_height: Option<u64>,
    /// Nockchain tx-id (base58) for the first deposit in the nonce epoch.
    ///
    /// This tx-id is included as the first entry in the deposit log and its nonce is
    /// `deposit_nonce_epoch_base`. Deposits in the same block with smaller tx-ids are ignored.
    ///
    /// Required when deposit_nonce_epoch_base is non-zero.
    #[serde(default, alias = "nonce_epoch_start_tx_id_base58")]
    pub deposit_nonce_epoch_start_tx_id_base58: Option<String>,
    /// Nock hashchain next height that must be reached before withdrawal
    /// processing activates.
    #[serde(default)]
    pub withdrawal_activation_nock_next_height: Option<u64>,
    #[serde(default)]
    pub ingress_listen_address: Option<String>,
    pub nodes: Vec<NodeInfoToml>,
    /// Optional bridge constants (defaults applied if omitted)
    #[serde(default)]
    pub constants: Option<BridgeConstantsToml>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NodeInfoToml {
    pub ip: String,
    pub eth_pubkey: String, // TODO: this should be eth_address
    /// Nockchain public key hash (PKH) - base58 encoded ~52 chars
    pub nock_pkh: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SequencerConfigToml {
    pub nock_contract_address: String,
    pub nockchain_confirmation_depth: u64,
    pub nodes: Vec<SequencerNodeInfoToml>,
    #[serde(default)]
    pub sequencer_journal: SequencerJournalConfigToml,
    /// Optional bridge constants (defaults applied if omitted)
    #[serde(default)]
    pub constants: Option<BridgeConstantsToml>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SequencerNodeInfoToml {
    pub eth_pubkey: String,
    /// Nockchain public key hash (PKH) - base58 encoded ~52 chars
    pub nock_pkh: String,
}

#[derive(Debug, Clone)]
pub struct SequencerNodeInfo {
    pub eth_address: Address,
    pub nock_pkh: NockPkh,
}

#[derive(Debug, Clone)]
pub struct NonceEpochConfig {
    pub base: u64,
    pub start_height: u64,
    pub start_tx_id: Option<Tip5Hash>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WithdrawalActivationCutoff {
    pub nock_next_height: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SequencerJournalConfigToml {
    /// Enables the sequencer's remote write-ahead journal mirror.
    ///
    /// This defaults to true so production configs fail closed unless durable
    /// object storage is configured or the operator explicitly disables it.
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// Public Ethereum address expected to verify every signed journal record.
    ///
    /// The corresponding private key is intentionally not accepted from this
    /// TOML file; the standalone sequencer reads it from vault-backed env only.
    #[serde(default)]
    pub verifier_address: Option<String>,
    #[serde(default)]
    pub object_store: SequencerJournalObjectStoreToml,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SequencerJournalObjectStoreToml {
    #[serde(default)]
    pub endpoint: Option<String>,
    #[serde(default)]
    pub bucket: Option<String>,
    #[serde(default = "default_sequencer_journal_region")]
    pub region: String,
    #[serde(default = "default_sequencer_journal_prefix")]
    pub prefix: String,
    #[serde(default = "default_sequencer_journal_id")]
    pub journal_id: String,
    #[serde(default)]
    pub access_key_id: Option<String>,
    #[serde(default)]
    pub secret_access_key: Option<String>,
}

impl Default for SequencerJournalConfigToml {
    fn default() -> Self {
        Self {
            enabled: true,
            verifier_address: None,
            object_store: SequencerJournalObjectStoreToml::default(),
        }
    }
}

impl Default for SequencerJournalObjectStoreToml {
    fn default() -> Self {
        Self {
            endpoint: None,
            bucket: None,
            region: default_sequencer_journal_region(),
            prefix: default_sequencer_journal_prefix(),
            journal_id: default_sequencer_journal_id(),
            access_key_id: None,
            secret_access_key: None,
        }
    }
}

/// Optional bridge constants configuration.
/// All fields are optional - defaults match Hoon type defaults.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BridgeConstantsToml {
    /// Minimum signatures required (default: 3)
    #[serde(default = "default_min_signers")]
    pub min_signers: u64,

    /// Total number of bridge nodes (default: 5)
    #[serde(default = "default_total_signers")]
    pub total_signers: u64,

    /// Minimum nocks for a bridge event (default: 1_000_000)
    #[serde(default = "default_minimum_event_nocks")]
    pub minimum_event_nocks: u64,

    /// Fee per nock in nicks (default: 195)
    #[serde(default = "default_nicks_fee_per_nock")]
    pub nicks_fee_per_nock: u64,

    /// Base blocks per chunk (default: 100)
    #[serde(default = "default_base_blocks_chunk")]
    pub base_blocks_chunk: u64,

    /// Base chain start height (default: 33_387_036)
    #[serde(default = "default_base_start_height")]
    pub base_start_height: u64,

    /// Nockchain start height (default: 25)
    #[serde(default = "default_nockchain_start_height")]
    pub nockchain_start_height: u64,
}

// Default functions for serde
fn default_min_signers() -> u64 {
    3
}
fn default_total_signers() -> u64 {
    5
}
fn default_minimum_event_nocks() -> u64 {
    1_000_000
}
fn default_nicks_fee_per_nock() -> u64 {
    195
}
fn default_base_blocks_chunk() -> u64 {
    100
}
fn default_base_start_height() -> u64 {
    33_387_036
}
fn default_nockchain_start_height() -> u64 {
    25
}
fn default_true() -> bool {
    true
}
fn default_sequencer_journal_region() -> String {
    "auto".to_string()
}
fn default_sequencer_journal_prefix() -> String {
    "withdrawal-sequencer".to_string()
}
fn default_sequencer_journal_id() -> String {
    "default".to_string()
}

impl Default for BridgeConstantsToml {
    fn default() -> Self {
        Self {
            min_signers: default_min_signers(),
            total_signers: default_total_signers(),
            minimum_event_nocks: default_minimum_event_nocks(),
            nicks_fee_per_nock: default_nicks_fee_per_nock(),
            base_blocks_chunk: default_base_blocks_chunk(),
            base_start_height: default_base_start_height(),
            nockchain_start_height: default_nockchain_start_height(),
        }
    }
}

impl BridgeConstantsToml {
    /// Convert to BridgeConstants with validation.
    pub fn to_bridge_constants(&self) -> Result<BridgeConstants, BridgeError> {
        // Validation
        if self.min_signers > self.total_signers {
            return Err(BridgeError::Config(format!(
                "min_signers ({}) cannot exceed total_signers ({})",
                self.min_signers, self.total_signers
            )));
        }
        if self.min_signers == 0 {
            return Err(BridgeError::Config("min_signers must be at least 1".into()));
        }
        if self.minimum_event_nocks == 0 {
            return Err(BridgeError::Config(
                "minimum_event_nocks must be greater than 0".into(),
            ));
        }
        if self.base_blocks_chunk == 0 {
            return Err(BridgeError::Config(
                "base_blocks_chunk must be greater than 0".into(),
            ));
        }

        // Warn if base_start_height is not aligned to batch boundaries
        // This is allowed but unusual - the driver now handles misalignment correctly
        let offset = self.base_start_height % self.base_blocks_chunk;
        if offset != 0 && offset != 1 {
            tracing::warn!(
                base_start_height = self.base_start_height,
                base_blocks_chunk = self.base_blocks_chunk,
                offset = offset,
                "base_start_height is not aligned to batch boundary (this is supported but unusual)"
            );
        }

        Ok(BridgeConstants {
            version: 0,
            min_signers: self.min_signers,
            total_signers: self.total_signers,
            minimum_event_nocks: self.minimum_event_nocks,
            nicks_fee_per_nock: self.nicks_fee_per_nock,
            base_blocks_chunk: self.base_blocks_chunk,
            base_start_height: self.base_start_height,
            nockchain_start_height: self.nockchain_start_height,
        })
    }
}

#[derive(Debug, Deserialize)]
struct DeploymentsAddresses {
    #[serde(rename = "messageInboxProxy")]
    message_inbox_proxy: Option<String>,
    #[serde(rename = "nock")]
    nock: Option<String>,
}

impl BridgeConfigToml {
    /// Loads and parses a bridge config TOML file from disk.
    pub fn from_file<P: AsRef<Path>>(path: P) -> Result<Self, BridgeError> {
        let contents = fs::read_to_string(path.as_ref()).map_err(|e| {
            BridgeError::Config(format!(
                "Failed to read config file at {}: {}",
                path.as_ref().display(),
                e
            ))
        })?;

        toml::from_str(&contents).map_err(|e| {
            BridgeError::Config(format!(
                "Failed to parse TOML config at {}: {}",
                path.as_ref().display(),
                e
            ))
        })
    }

    /// Converts the raw TOML config into the validated runtime node config used
    /// by the bridge.
    pub fn to_node_config(&self) -> Result<NodeConfig, BridgeError> {
        let my_eth_key = parse_hex_key(&self.my_eth_key, "my_eth_key")?;
        let my_nock_key_limbs = base58_to_schnorr_t8(&self.my_nock_key, "my_nock_key")?;
        let bridge_lock_root = Tip5Hash::from_base58(&self.bridge_lock_root).map_err(|err| {
            BridgeError::Config(format!(
                "Invalid bridge_lock_root '{}': {}",
                self.bridge_lock_root, err
            ))
        })?;

        let nodes = self
            .nodes
            .iter()
            .map(|n| n.to_node_info())
            .collect::<Result<Vec<_>, _>>()?;

        if nodes.len() != 5 {
            return Err(BridgeError::Config(format!(
                "expected exactly 5 nodes, found {}",
                nodes.len()
            )));
        }

        let mut seen_ips = HashSet::new();
        let mut seen_eth = HashSet::new();
        let mut seen_nock = HashSet::new();
        for node in &nodes {
            if !seen_ips.insert(node.ip.clone()) {
                return Err(BridgeError::Config(format!(
                    "duplicate node ip detected: {}",
                    node.ip
                )));
            }
            if !seen_eth.insert(node.eth_pubkey.as_slice().to_vec()) {
                return Err(BridgeError::Config(
                    "duplicate ethereum pubkey detected".into(),
                ));
            }
            if !seen_nock.insert(node.nock_pkh.clone()) {
                return Err(BridgeError::Config(
                    "duplicate nockchain pkh detected".into(),
                ));
            }
        }

        Ok(NodeConfig {
            node_id: self.node_id,
            nodes,
            bridge_lock_root,
            my_eth_key: AtomBytes::from(my_eth_key),
            my_nock_key: SchnorrSecretKey::from(my_nock_key_limbs),
        })
    }

    /// Resolves the MessageInbox contract address, preferring the explicit
    /// config value and falling back to `deployments.json`.
    pub fn inbox_contract_address(&self) -> Result<Address, BridgeError> {
        if let Some(address) = self.inbox_contract_address.as_ref().and_then(|value| {
            if value.trim().is_empty() {
                None
            } else {
                Some(value)
            }
        }) {
            return Address::from_str(address).map_err(|e| {
                BridgeError::Config(format!("Invalid inbox_contract_address: {}", e))
            });
        }
        if let Some(deployments) = load_deployments_addresses()? {
            if let Some(address) = deployments.message_inbox_proxy {
                return Address::from_str(&address).map_err(|e| {
                    BridgeError::Config(format!(
                        "Invalid messageInboxProxy in deployments.json: {}",
                        e
                    ))
                });
            }
        }
        Err(BridgeError::Config(
            "Missing MessageInbox contract address. Set inbox_contract_address in bridge-conf.toml or ensure deployments.json provides messageInboxProxy."
                .into(),
        ))
    }

    /// Resolves the NOCK token contract address, preferring the explicit
    /// config value and falling back to `deployments.json`.
    pub fn nock_contract_address(&self) -> Result<Address, BridgeError> {
        if let Some(address) = self.nock_contract_address.as_ref().and_then(|value| {
            if value.trim().is_empty() {
                None
            } else {
                Some(value)
            }
        }) {
            return Address::from_str(address)
                .map_err(|e| BridgeError::Config(format!("Invalid nock_contract_address: {}", e)));
        }
        if let Some(deployments) = load_deployments_addresses()? {
            if let Some(address) = deployments.nock {
                return Address::from_str(&address).map_err(|e| {
                    BridgeError::Config(format!("Invalid nock address in deployments.json: {}", e))
                });
            }
        }
        Err(BridgeError::Config(
            "Missing Nock token contract address. Set nock_contract_address in bridge-conf.toml or ensure deployments.json provides nock."
                .into(),
        ))
    }

    /// Returns the configured Base websocket endpoint.
    pub fn base_ws_url(&self) -> &str {
        &self.base_ws_url
    }

    /// Returns the configured private Nockchain gRPC endpoint.
    pub fn grpc_address(&self) -> &str {
        &self.grpc_address
    }

    /// Returns the withdrawal sequencer RPC endpoint that this bridge should
    /// use.
    ///
    /// If the operator does not configure an explicit
    /// `nockchain_sequencer_api_address`, the bridge derives the colocated API
    /// node address from `grpc_address`.
    pub fn nockchain_sequencer_api_address(&self) -> Result<String, BridgeError> {
        if let Some(address) = self
            .nockchain_sequencer_api_address
            .as_ref()
            .map(|value| value.trim())
            .filter(|value| !value.is_empty())
        {
            return Ok(address.to_string());
        }
        derive_grpc_sibling_address(&self.grpc_address, 100)
    }

    /// Returns the configured Ethereum signing key hex string.
    pub fn my_eth_key_hex(&self) -> &str {
        &self.my_eth_key
    }

    /// Returns the explicitly configured ingress listen address, if any.
    pub fn ingress_listen_address(&self) -> Option<&str> {
        self.ingress_listen_address.as_deref()
    }

    /// Get bridge constants, using defaults if not configured.
    pub fn bridge_constants(&self) -> Result<BridgeConstants, BridgeError> {
        match &self.constants {
            Some(c) => c.to_bridge_constants(),
            None => Ok(BridgeConstants::default()),
        }
    }

    /// Parses the optional nonce-epoch anchor tx id from base58.
    pub fn deposit_nonce_epoch_start_tx_id(&self) -> Result<Option<Tip5Hash>, BridgeError> {
        let Some(value) = self.deposit_nonce_epoch_start_tx_id_base58.as_deref() else {
            return Ok(None);
        };
        let belts = base58_to_belts::<5>(value, "deposit_nonce_epoch_start_tx_id_base58")?;
        Ok(Some(Tip5Hash(belts)))
    }

    /// Resolves the withdrawal activation cutoff. A fresh withdrawal projection
    /// waits for this Nock kernel frontier before initializing its cursor at
    /// the current full Base/Nock kernel position.
    pub fn withdrawal_activation_cutoff(&self) -> Result<WithdrawalActivationCutoff, BridgeError> {
        let nock_next_height = self.withdrawal_activation_nock_next_height.ok_or_else(|| {
            BridgeError::Config("withdrawal_activation_nock_next_height is required".into())
        })?;
        Ok(WithdrawalActivationCutoff { nock_next_height })
    }
}

impl SequencerConfigToml {
    /// Loads and parses the public standalone withdrawal sequencer config.
    pub fn from_file<P: AsRef<Path>>(path: P) -> Result<Self, BridgeError> {
        let contents = fs::read_to_string(path.as_ref()).map_err(|e| {
            BridgeError::Config(format!(
                "Failed to read sequencer config file at {}: {}",
                path.as_ref().display(),
                e
            ))
        })?;

        toml::from_str(&contents).map_err(|e| {
            BridgeError::Config(format!(
                "Failed to parse sequencer TOML config at {}: {}",
                path.as_ref().display(),
                e
            ))
        })
    }

    /// Resolves the NOCK token contract address required for Base burn verification.
    pub fn nock_contract_address(&self) -> Result<Address, BridgeError> {
        Address::from_str(&self.nock_contract_address)
            .map_err(|e| BridgeError::Config(format!("Invalid nock_contract_address: {}", e)))
    }

    /// Get bridge constants, using defaults if not configured.
    pub fn bridge_constants(&self) -> Result<BridgeConstants, BridgeError> {
        match &self.constants {
            Some(c) => c.to_bridge_constants(),
            None => Ok(BridgeConstants::default()),
        }
    }

    /// Converts public operator facts into the validated node set required by
    /// the standalone withdrawal sequencer.
    pub fn validated_nodes(&self) -> Result<Vec<SequencerNodeInfo>, BridgeError> {
        if self.nodes.len() != 5 {
            return Err(BridgeError::Config(format!(
                "expected exactly 5 sequencer nodes, found {}",
                self.nodes.len()
            )));
        }

        let nodes = self
            .nodes
            .iter()
            .map(|node| node.to_node_info())
            .collect::<Result<Vec<_>, _>>()?;

        let mut seen_eth = HashSet::new();
        let mut seen_nock = HashSet::new();
        for node in &nodes {
            if !seen_eth.insert(node.eth_address) {
                return Err(BridgeError::Config(
                    "duplicate ethereum address detected in sequencer config".into(),
                ));
            }
            if !seen_nock.insert(node.nock_pkh.clone()) {
                return Err(BridgeError::Config(
                    "duplicate nockchain pkh detected in sequencer config".into(),
                ));
            }
        }

        Ok(nodes)
    }
}

pub const CANONICAL_TESTING_BRIDGE_NODE_PKHS_B58: [&str; 5] = [
    "A47ZMEQ2U2x1h3bVMUNdkutKYNiyXFWMVTQZC8BWgXBmS5mc6ysAhLZ",
    "BYp766x6Zhu7DHbewMHu7ajsAenRMm1M7rgmpxUwY83BJy4RGMAG2z8",
    "2f7BtZpaaKVb9mCUFgMuYjcQXhrexfqCJs4h1es5t9jQrqdmhVgYLU6",
    "BLCg8KPPKDJPJ8hhdHSGsurxgKwBorqpF1qrHsCiojsPf96GEzwsFQ",
    "AeZ1jsSHoAg7bjBr2k4kMeRERsx85Bp68tfTMiiYZtjFRCtc4gexNWc",
];
pub const CANONICAL_TESTING_BRIDGE_LOCK_ROOT_B58: &str =
    "79FBgCdsSVSJ8RtWtRbndRZ8Qg7WjhKgYPY8K722ntd3crFWnWXFZf4";

pub fn derive_bridge_spend_authority_from_pkhs<I>(
    min_signers: u64,
    pkhs: I,
) -> Result<(SpendCondition, Tip5Hash), BridgeError>
where
    I: IntoIterator<Item = NockPkh>,
{
    let spend_condition =
        SpendCondition::new(vec![LockPrimitive::Pkh(Pkh::new(min_signers, pkhs))]);
    let lock_root = Lock::SpendCondition(spend_condition.clone())
        .hash()
        .map_err(|err| {
            BridgeError::Config(format!(
                "failed to derive bridge withdrawal lock root from signer set: {err}"
            ))
        })?;
    Ok((spend_condition, lock_root))
}

pub fn derive_bridge_spend_authority_from_nodes(
    nodes: &[NodeInfo],
    min_signers: u64,
) -> Result<(SpendCondition, Tip5Hash), BridgeError> {
    derive_bridge_spend_authority_from_pkhs(
        min_signers,
        nodes.iter().map(|node| node.nock_pkh.clone()),
    )
}

pub fn canonical_testing_bridge_lock_root() -> Result<Tip5Hash, BridgeError> {
    Tip5Hash::from_base58(CANONICAL_TESTING_BRIDGE_LOCK_ROOT_B58).map_err(|err| {
        BridgeError::Config(format!(
            "invalid canonical testing bridge lock root {}: {err}",
            CANONICAL_TESTING_BRIDGE_LOCK_ROOT_B58
        ))
    })
}

/// Applies a fixed port offset to an existing gRPC endpoint while preserving
/// the endpoint's original scheme and host formatting.
fn derive_grpc_sibling_address(endpoint: &str, port_offset: u16) -> Result<String, BridgeError> {
    let (scheme, authority) = match endpoint.split_once("://") {
        Some((scheme, authority)) => (Some(scheme), authority),
        None => (None, endpoint),
    };
    let (host, port) = authority.rsplit_once(':').ok_or_else(|| {
        BridgeError::Config(format!(
            "could not derive sibling gRPC address from endpoint without host:port: {endpoint}"
        ))
    })?;
    let port: u16 = port.parse().map_err(|err| {
        BridgeError::Config(format!(
            "could not parse gRPC endpoint port in {endpoint}: {err}"
        ))
    })?;
    let derived_port = port.checked_add(port_offset).ok_or_else(|| {
        BridgeError::Config(format!(
            "gRPC endpoint port overflow while deriving sibling address from {endpoint}"
        ))
    })?;
    Ok(match scheme {
        Some(scheme) => format!("{scheme}://{host}:{derived_port}"),
        None => format!("{host}:{derived_port}"),
    })
}

impl NonceEpochConfig {
    /// Returns the first nonce assigned inside the configured nonce epoch.
    pub fn first_epoch_nonce(&self) -> u64 {
        if self.start_tx_id.is_some() {
            self.base
        } else {
            self.base.saturating_add(1)
        }
    }

    /// Returns whether a deposit `(block_height, tx_id)` precedes the nonce
    /// epoch activation anchor.
    pub fn is_before_start_key(&self, block_height: u64, tx_id: &Tip5Hash) -> bool {
        if block_height < self.start_height {
            return true;
        }
        if block_height > self.start_height {
            return false;
        }
        let Some(start_tx_id) = self.start_tx_id.as_ref() else {
            return false;
        };
        tx_id.to_be_limb_bytes() < start_tx_id.to_be_limb_bytes()
    }
}

impl NodeInfoToml {
    /// Converts one TOML node entry into the validated runtime node-info
    /// structure.
    pub fn to_node_info(&self) -> Result<NodeInfo, BridgeError> {
        let eth_pubkey = parse_hex_key(&self.eth_pubkey, "eth_pubkey")?;
        let nock_pkh = NockPkh::from_base58(&self.nock_pkh)
            .map_err(|err| BridgeError::Config(format!("invalid pkh for nock_pkh: {}", err)))?;

        Ok(NodeInfo {
            ip: self.ip.clone(),
            eth_pubkey: AtomBytes::from(eth_pubkey),
            nock_pkh,
        })
    }
}

impl SequencerNodeInfoToml {
    /// Converts one public sequencer TOML node entry into validated node facts.
    pub fn to_node_info(&self) -> Result<SequencerNodeInfo, BridgeError> {
        let eth_pubkey = parse_hex_key(&self.eth_pubkey, "eth_pubkey")?;
        if eth_pubkey.len() != 20 {
            return Err(BridgeError::Config(format!(
                "sequencer eth_pubkey must be a 20-byte Ethereum address, found {} bytes",
                eth_pubkey.len()
            )));
        }
        let nock_pkh = NockPkh::from_base58(&self.nock_pkh)
            .map_err(|err| BridgeError::Config(format!("invalid pkh for nock_pkh: {}", err)))?;

        Ok(SequencerNodeInfo {
            eth_address: Address::from_slice(&eth_pubkey),
            nock_pkh,
        })
    }
}

/// Parses a required hex-encoded config key, accepting an optional `0x`
/// prefix.
fn parse_hex_key(hex_str: &str, field_name: &str) -> Result<Vec<u8>, BridgeError> {
    let hex_str = hex_str.strip_prefix("0x").unwrap_or(hex_str);
    let bytes = hex::decode(hex_str).map_err(|e| {
        BridgeError::Config(format!("Invalid hex encoding for {}: {}", field_name, e))
    })?;

    if bytes.is_empty() {
        return Err(BridgeError::Config(format!(
            "{} cannot be empty",
            field_name
        )));
    }

    Ok(bytes)
}

/// Decodes a base58 value into a fixed number of Belt limbs.
fn base58_to_belts<const N: usize>(value: &str, field: &str) -> Result<[Belt; N], BridgeError> {
    let bytes = bs58::decode(value).into_vec().map_err(|e| {
        BridgeError::Config(format!("Invalid base58 encoding for {}: {}", field, e))
    })?;
    if bytes.is_empty() {
        return Err(BridgeError::Config(format!("{} cannot be empty", field)));
    }

    let mut big = BigUint::from_bytes_be(&bytes);
    let prime = BigUint::from(PRIME);
    let mut belts = [Belt(0); N];
    for belt in belts.iter_mut() {
        let rem = (&big % &prime)
            .try_into()
            .map_err(|_| BridgeError::Config(format!("{} limb did not fit in field", field)))?;
        *belt = Belt(rem);
        big /= &prime;
    }

    if big > BigUint::from(0u8) {
        return Err(BridgeError::Config(format!(
            "{} exceeds {} Belt limbs",
            field, N
        )));
    }

    Ok(belts)
}

/// Decodes a base58-encoded atom into a Schnorr t8 limb array.
///
/// Hoon `schnorr-seckey` is represented as `atom-to-t8:belt-schnorr:cheetah`,
/// which chunks the atom into 8 little-endian 32-bit limbs (`rip-correct 5`).
/// This is not the same encoding as the field-prime Belt arrays used for tip5
/// hashes and other based values.
fn base58_to_schnorr_t8(value: &str, field: &str) -> Result<[Belt; 8], BridgeError> {
    let bytes = bs58::decode(value).into_vec().map_err(|e| {
        BridgeError::Config(format!("Invalid base58 encoding for {}: {}", field, e))
    })?;
    if bytes.is_empty() {
        return Err(BridgeError::Config(format!("{} cannot be empty", field)));
    }

    let mut big = BigUint::from_bytes_be(&bytes);
    let radix = BigUint::from(1u64 << 32);
    let mut belts = [Belt(0); 8];
    for belt in belts.iter_mut() {
        let rem = (&big % &radix)
            .try_into()
            .map_err(|_| BridgeError::Config(format!("{} limb did not fit in u32", field)))?;
        *belt = Belt(rem);
        big /= &radix;
    }

    if big > BigUint::from(0u8) {
        return Err(BridgeError::Config(format!(
            "{} exceeds 8 Schnorr t8 limbs",
            field
        )));
    }

    Ok(belts)
}

/// Loads contract addresses from the repo-local `deployments.json`, if it
/// exists.
fn load_deployments_addresses() -> Result<Option<DeploymentsAddresses>, BridgeError> {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("contracts")
        .join("deployments.json");
    if !path.exists() {
        return Ok(None);
    }
    let contents = fs::read_to_string(&path).map_err(|e| {
        BridgeError::Config(format!(
            "Failed to read deployments.json at {}: {}",
            path.display(),
            e
        ))
    })?;
    if contents.trim().is_empty() {
        return Ok(None);
    }
    let addresses: DeploymentsAddresses = serde_json::from_str(&contents)?;
    Ok(Some(addresses))
}

/// Returns the default config path under the system bridge data directory.
pub fn default_config_path() -> Result<PathBuf, BridgeError> {
    let bridge_data_dir = nockapp::system_data_dir().join("bridge");
    Ok(bridge_data_dir.join("bridge-conf.toml"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sequencer_journal_defaults_enabled() {
        let config = SequencerJournalConfigToml::default();

        assert!(config.enabled);
        assert_eq!(config.verifier_address, None);
        assert_eq!(config.object_store.region, "auto");
        assert_eq!(config.object_store.prefix, "withdrawal-sequencer");
        assert_eq!(config.object_store.journal_id, "default");
    }

    #[test]
    fn canonical_testing_bridge_lock_root_constant_matches_testing_signer_set() {
        let pkhs = CANONICAL_TESTING_BRIDGE_NODE_PKHS_B58
            .iter()
            .map(|raw| NockPkh::from_base58(raw).expect("valid canonical testing bridge pkh"))
            .collect::<Vec<_>>();
        let (_, derived_root) =
            derive_bridge_spend_authority_from_pkhs(3, pkhs).expect("derived testing root");

        assert_eq!(
            canonical_testing_bridge_lock_root().expect("canonical testing root"),
            derived_root
        );
    }
}

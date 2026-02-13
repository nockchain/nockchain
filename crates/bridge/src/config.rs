use std::collections::HashSet;
use std::convert::TryInto;
use std::fs;
use std::path::{Path, PathBuf};
use std::str::FromStr;

use alloy::primitives::Address;
use nockchain_math::belt::{Belt, PRIME};
use nockchain_types::tx_engine::common::Hash as NockPkh;
use num_bigint::BigUint;
use serde::{Deserialize, Serialize};

use crate::errors::BridgeError;
use crate::types::{AtomBytes, BridgeConstants, NodeConfig, NodeInfo, SchnorrSecretKey, Tip5Hash};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BridgeConfigToml {
    pub node_id: u64,
    pub base_ws_url: String,
    #[serde(default)]
    pub inbox_contract_address: Option<String>,
    #[serde(default)]
    pub nock_contract_address: Option<String>,
    pub my_eth_key: String,
    pub my_nock_key: String,
    pub grpc_address: String,
    /// Number of confirmations required on Base before sending a batch to the kernel.
    pub base_confirmation_depth: u64,
    /// Number of confirmations required on nockchain before sending a block to the kernel.
    pub nockchain_confirmation_depth: u64,
    /// Base contract lastDepositNonce at the time the runtime nonce epoch is activated.
    ///
    /// When non-zero, this must equal the nonce of the anchor deposit identified by
    /// nonce_epoch_start_height + nonce_epoch_start_tx_id_base58.
    /// When zero, the start height/tx-id may be omitted to start from the first deposit
    /// at/after the default start height.
    #[serde(default)]
    pub nonce_epoch_base: Option<u64>,
    /// Nockchain height at which the runtime nonce epoch starts.
    ///
    /// Deposits with `block_height < nonce_epoch_start_height` will not be signed under the
    /// epoch scheme, and are expected to have been handled prior to activation.
    ///
    /// Required when nonce_epoch_base is non-zero.
    #[serde(default)]
    pub nonce_epoch_start_height: Option<u64>,
    /// Nockchain tx-id (base58) for the first deposit in the nonce epoch.
    ///
    /// This tx-id is included as the first entry in the deposit log and its nonce is
    /// `nonce_epoch_base`. Deposits in the same block with smaller tx-ids are ignored.
    ///
    /// Required when nonce_epoch_base is non-zero.
    #[serde(default)]
    pub nonce_epoch_start_tx_id_base58: Option<String>,
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

#[derive(Debug, Clone)]
pub struct NonceEpochConfig {
    pub base: u64,
    pub start_height: u64,
    pub start_tx_id: Option<Tip5Hash>,
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

    pub fn to_node_config(&self) -> Result<NodeConfig, BridgeError> {
        let my_eth_key = parse_hex_key(&self.my_eth_key, "my_eth_key")?;
        let my_nock_key_limbs = base58_to_belts::<8>(&self.my_nock_key, "my_nock_key")?;

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
            my_eth_key: AtomBytes::from(my_eth_key),
            my_nock_key: SchnorrSecretKey::from(my_nock_key_limbs),
        })
    }

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

    pub fn base_ws_url(&self) -> &str {
        &self.base_ws_url
    }

    pub fn grpc_address(&self) -> &str {
        &self.grpc_address
    }

    pub fn my_eth_key_hex(&self) -> &str {
        &self.my_eth_key
    }

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

    pub fn nonce_epoch_start_tx_id(&self) -> Result<Option<Tip5Hash>, BridgeError> {
        let Some(value) = self.nonce_epoch_start_tx_id_base58.as_deref() else {
            return Ok(None);
        };
        let belts = base58_to_belts::<5>(value, "nonce_epoch_start_tx_id_base58")?;
        Ok(Some(Tip5Hash(belts)))
    }
}

impl NonceEpochConfig {
    pub fn first_epoch_nonce(&self) -> u64 {
        if self.start_tx_id.is_some() {
            self.base
        } else {
            self.base.saturating_add(1)
        }
    }

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

pub fn default_config_path() -> Result<PathBuf, BridgeError> {
    let bridge_data_dir = nockapp::system_data_dir().join("bridge");
    Ok(bridge_data_dir.join("bridge-conf.toml"))
}

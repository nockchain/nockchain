use std::collections::{HashMap, HashSet};
use std::str::FromStr;

use alloy::primitives::{Address, Signature};
use alloy::signers::local::PrivateKeySigner;
use alloy::signers::Signer;
use nockapp::nockapp::wire::{Wire, WireRepr};
use nockvm::noun::{Noun, NounSpace};
use noun_serde::NounDecode;

use crate::shared::errors::BridgeError;
use crate::shared::types::{NodeConfig, NounDigest};

/// Ethereum signature handler for bridge operations
pub struct EthereumSigner {
    wallet: PrivateKeySigner,
}

impl EthereumSigner {
    /// Create a new Ethereum signer from a private key
    pub fn new(private_key: String) -> Result<Self, BridgeError> {
        let key = private_key.strip_prefix("0x").unwrap_or(&private_key);
        let wallet = PrivateKeySigner::from_str(key)?;

        Ok(Self { wallet })
    }

    /// Sign a proposal hash with Ethereum secp256k1
    ///
    /// The proposal_hash is the hash of a bundle proposal that needs to be signed.
    /// This uses EIP-191 message prefix for Ethereum compatibility.
    pub async fn sign_proposal(
        &self,
        proposal_hash_noun: Noun,
        space: &NounSpace,
    ) -> Result<Signature, BridgeError> {
        let proposal_hash = match proposal_hash_noun.in_space(space).as_atom() {
            Ok(_) => Self::proposal_hash_from_noun(proposal_hash_noun, space)?,
            Err(_) => {
                let digest = NounDigest::from_noun(&proposal_hash_noun, space).map_err(|_| {
                    BridgeError::Config("proposal-hash noun was not an atom".into())
                })?;
                Self::proposal_hash_from_limbs(digest)
            }
        };
        self.sign_hash(&proposal_hash).await
    }

    /// Sign a 32-byte hash directly with Ethereum secp256k1.
    /// This uses EIP-191 message prefix for Ethereum compatibility.
    pub async fn sign_hash(&self, hash: &[u8; 32]) -> Result<Signature, BridgeError> {
        tracing::trace!("signing Ethereum proposal hash");

        let signature = self
            .wallet
            .sign_message(hash)
            .await
            .map_err(|e| BridgeError::ContractInteraction(format!("Signing failed: {}", e)))?;

        tracing::trace!("generated Ethereum signature");
        Ok(signature)
    }

    /// Convert a cued noun representing a proposal hash into a 32-byte big-endian array
    fn proposal_hash_from_noun(
        hash_noun: Noun,
        space: &NounSpace,
    ) -> Result<[u8; 32], BridgeError> {
        let atom = hash_noun
            .in_space(space)
            .as_atom()
            .map_err(|_| BridgeError::Config("proposal-hash noun was not an atom".to_string()))?;
        let mut b = atom.to_be_bytes();
        if b.len() > 32 {
            b = b.split_off(b.len() - 32);
        } else if b.len() < 32 {
            let mut padded = vec![0u8; 32 - b.len()];
            padded.extend_from_slice(&b);
            b = padded;
        }
        let mut arr = [0u8; 32];
        arr.copy_from_slice(&b);
        Ok(arr)
    }

    fn proposal_hash_from_limbs(digest: NounDigest) -> [u8; 32] {
        // Concatenate five u64 limbs big-endian and take the last 32 bytes (drop top 8 bytes)
        let mut bytes = [0u8; 40];
        for (i, limb) in digest.0.iter().enumerate() {
            let be = limb.0.to_be_bytes();
            bytes[i * 8..(i + 1) * 8].copy_from_slice(&be);
        }
        let mut out = [0u8; 32];
        out.copy_from_slice(&bytes[8..]);
        out
    }

    /// Get the Ethereum address of this signer
    pub fn address(&self) -> Address {
        self.wallet.address()
    }
}

/// Bridge signer that only handles Ethereum signatures
/// Nockchain signatures are handled in Hoon
pub type BridgeSigner = EthereumSigner;

pub type BridgeNodeEthAddressMap = HashMap<u64, Address>;

/// Wire for signature results
#[allow(dead_code)]
pub enum SignatureWire {
    EthSignatureResult {
        proposal_hash: [u8; 32],
        signature: Vec<u8>,
    },
}

impl Wire for SignatureWire {
    const VERSION: u64 = 1;
    const SOURCE: &'static str = "signature";

    fn to_wire(&self) -> WireRepr {
        match self {
            SignatureWire::EthSignatureResult { .. } => {
                WireRepr::new(Self::SOURCE, Self::VERSION, vec!["poke".into()])
            }
        }
    }
}

/// Verify that a signature is from an authorized bridge node.
/// Returns the recovered address if valid, or None if invalid.
pub fn verify_bridge_signature(
    proposal_hash: &[u8; 32],
    signature: &[u8],
    valid_addresses: &HashSet<Address>,
) -> Option<Address> {
    use alloy::primitives::{Signature as AlloySignature, B256};

    if signature.len() != 65 {
        return None;
    }

    let mut r = [0u8; 32];
    let mut s = [0u8; 32];
    r.copy_from_slice(&signature[0..32]);
    s.copy_from_slice(&signature[32..64]);
    let v = signature[64];

    // v must be 27 or 28 for Ethereum signatures
    if v != 27 && v != 28 {
        return None;
    }

    let y_parity = v == 28;
    let sig = AlloySignature::new(
        alloy::primitives::U256::from_be_bytes(r),
        alloy::primitives::U256::from_be_bytes(s),
        y_parity,
    );

    // EIP-191 recovery matches the Solidity contract's _verifySignatures
    let hash = B256::from_slice(proposal_hash);
    let recovered = match sig.recover_address_from_msg(hash.as_slice()) {
        Ok(addr) => addr,
        Err(e) => {
            tracing::warn!(
                target: "bridge.propose",
                error=%e,
                sig_r=%hex::encode(r),
                sig_s=%hex::encode(s),
                sig_v=v,
                hash=%hex::encode(proposal_hash),
                "failed to recover address from signature"
            );
            return None;
        }
    };

    tracing::debug!(
        target: "bridge.propose",
        recovered=%recovered,
        valid_addresses=?valid_addresses.iter().map(|a| format!("{}", a)).collect::<Vec<_>>(),
        "checking if recovered address is in valid set"
    );

    if valid_addresses.contains(&recovered) {
        Some(recovered)
    } else {
        tracing::warn!(
            target: "bridge.propose",
            recovered=%recovered,
            valid_addresses=?valid_addresses.iter().map(|a| format!("{}", a)).collect::<Vec<_>>(),
            "recovered address not in valid set"
        );
        None
    }
}

/// Extract bridge node Ethereum addresses keyed by node id from node config.
pub fn extract_bridge_node_eth_addresses(node_config: &NodeConfig) -> BridgeNodeEthAddressMap {
    node_config
        .nodes
        .iter()
        .enumerate()
        .filter_map(|(idx, node)| {
            if node.eth_pubkey.0.len() == 20 {
                Some((idx as u64, Address::from_slice(&node.eth_pubkey.0)))
            } else {
                None
            }
        })
        .collect()
}

/// Extract valid bridge node Ethereum addresses from node config.
pub fn extract_valid_bridge_addresses(node_config: &NodeConfig) -> HashSet<Address> {
    tracing::info!(
        target: "bridge.propose",
        node_count = node_config.nodes.len(),
        "extracting valid bridge addresses from config"
    );
    node_config
        .nodes
        .iter()
        .filter_map(|node| {
            tracing::debug!(
                target: "bridge.propose",
                ip = %node.ip,
                eth_pubkey_len = node.eth_pubkey.0.len(),
                eth_pubkey_hex = %hex::encode(&node.eth_pubkey.0),
                "checking node eth_pubkey"
            );
            if node.eth_pubkey.0.len() == 20 {
                let addr = Address::from_slice(&node.eth_pubkey.0);
                tracing::info!(
                    target: "bridge.propose",
                    ip = %node.ip,
                    address = %addr,
                    "added valid bridge address"
                );
                Some(addr)
            } else {
                tracing::warn!(
                    "node eth_pubkey is not 20 bytes (got {}), skipping for signature verification",
                    node.eth_pubkey.0.len()
                );
                None
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use nockvm::mem::{NockStack, NOCK_STACK_SIZE_TINY};
    use nockvm::noun::NounAllocator;
    use nockvm::pma::{Pma, PmaCopy};
    use tempfile::TempDir;

    use super::*;

    #[tokio::test]
    async fn test_ethereum_signing() {
        // Use a test private key
        let private_key = "0x4c0883a69102937d6231471b5dbb6204fe5129617082792ae468d01a3f362318";
        let signer = EthereumSigner::new(private_key.to_string())
            .expect("Failed to create EthereumSigner with test private key");

        let proposal_hash = [1u8; 32];
        let mut slab = nockapp::noun::slab::NounSlab::<nockapp::noun::slab::NockJammer>::new();
        let noun = unsafe {
            let mut ia =
                nockvm::noun::IndirectAtom::new_raw_bytes(&mut slab, 32, proposal_hash.as_ptr());
            let space = slab.noun_space();
            ia.normalize_as_atom(&space)
        };
        let space = slab.noun_space();
        let signature = signer
            .sign_proposal(noun.as_noun(), &space)
            .await
            .expect("Failed to sign proposal in test");

        // Verify signature is valid
        assert_ne!(signature.r(), alloy::primitives::U256::ZERO);
        assert_ne!(signature.s(), alloy::primitives::U256::ZERO);
        let recovery_id = signature.as_bytes()[64];
        assert!(recovery_id == 27 || recovery_id == 28);
    }

    #[tokio::test]
    #[cfg_attr(miri, ignore = "PMA file-backed arena unsupported in Miri")]
    async fn test_ethereum_signing_accepts_pma_offset_noun() {
        let private_key = "0x4c0883a69102937d6231471b5dbb6204fe5129617082792ae468d01a3f362318";
        let signer = EthereumSigner::new(private_key.to_string())
            .expect("Failed to create EthereumSigner with test private key");

        let proposal_hash = [7u8; 32];
        let mut stack = NockStack::new(NOCK_STACK_SIZE_TINY, 0);
        let mut noun = unsafe {
            let mut ia =
                nockvm::noun::IndirectAtom::new_raw_bytes(&mut stack, 32, proposal_hash.as_ptr());
            let space = stack.noun_space();
            ia.normalize_as_atom(&space).as_noun()
        };

        let tempdir = TempDir::new().expect("create temp dir for PMA");
        let pma_path = tempdir.path().join("proposal-hash-test.pma");
        let mut pma = Pma::new(1 << 10, pma_path).expect("create PMA");
        unsafe {
            noun.copy_to_pma(&stack, &mut pma);
        }
        let pma_space = NounSpace::pma_only(&pma);
        assert!(matches!(
            noun.in_space(&pma_space).repr().location(),
            Some(loc) if loc.is_offset()
        ));

        let signature = signer
            .sign_proposal(noun, &pma_space)
            .await
            .expect("Failed to sign proposal from PMA noun");

        assert_ne!(signature.r(), alloy::primitives::U256::ZERO);
        assert_ne!(signature.s(), alloy::primitives::U256::ZERO);
        let recovery_id = signature.as_bytes()[64];
        assert!(recovery_id == 27 || recovery_id == 28);
    }

    // Nockchain signing tests are now in Hoon
}

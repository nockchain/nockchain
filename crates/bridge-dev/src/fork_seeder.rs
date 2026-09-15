use std::collections::HashSet;
use std::time::Duration;

use alloy::primitives::{keccak256, Address, Bytes, B256, U256};
use bridge::shared::types::WITHDRAWAL_POLICY_V1_BASE_UNITS_PER_NICK;
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::anvil::AnvilBackend;
use crate::base_backend::{BaseBackendError, TransactionReceiptFacts};
use crate::fork_preflight::{PristineDeploymentFacts, VerifiedPristineFork};

const RECEIPT_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ForkContractState {
    pub owner: String,
    pub nock_owner: String,
    pub bridge_nodes: [String; 5],
    pub threshold: u64,
    pub withdrawals_enabled: bool,
    pub message_inbox_nock: String,
    pub nock_inbox: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum OverrideKind {
    BridgeNode { index: usize },
    WithdrawalsEnabled,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OverrideRecord {
    pub kind: OverrideKind,
    pub before: String,
    pub after: String,
    pub receipt: TransactionReceiptFacts,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ForkSeedReport {
    pub before: ForkContractState,
    pub after: ForkContractState,
    pub owner_balance_before: U256,
    pub owner_balance_after: U256,
    pub overrides: Vec<OverrideRecord>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ForkBalanceSeedRequest {
    pub holder: Address,
    pub required_nicks: u64,
    pub headroom_nicks: u64,
    pub gas_accounts: Vec<Address>,
    pub gas_balance_wei: U256,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GasFundingRecord {
    pub address: Address,
    pub before_wei: U256,
    pub after_wei: U256,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MintRecord {
    pub holder: Address,
    pub target_nicks: u64,
    pub target_base_units: U256,
    pub holder_before_base_units: U256,
    pub holder_after_base_units: U256,
    pub total_supply_before_base_units: U256,
    pub total_supply_after_base_units: U256,
    pub minted_base_units: U256,
    pub receipt: Option<TransactionReceiptFacts>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ForkBalanceSeedReport {
    pub gas_funding: Vec<GasFundingRecord>,
    pub mint: MintRecord,
    pub nock_inbox_before: String,
    pub nock_inbox_after: String,
    pub bridge_state_after: ForkContractState,
}

pub struct ForkBalanceSeeder;

#[derive(Debug, Error)]
pub enum ForkBalanceSeedError {
    #[error("invalid fork balance seed request: {0}")]
    InvalidRequest(&'static str),
    #[error("holder balance {current} exceeds deterministic target {target}")]
    HolderAboveTarget { current: U256, target: U256 },
    #[error("fork balance seed failed during {stage} and rollback completed={reverted}: {reason}")]
    RolledBack {
        stage: &'static str,
        reason: String,
        reverted: bool,
    },
    #[error("fork balance seed validation failed: {0}")]
    Validation(&'static str),
    #[error("fork balance seed state read failed: {0}")]
    StateRead(String),
    #[error(transparent)]
    Backend(#[from] BaseBackendError),
}

pub struct ForkSeeder;

impl ForkSeeder {
    pub async fn seed(
        backend: &AnvilBackend,
        pristine: &VerifiedPristineFork,
        deterministic_signers: [Address; 5],
    ) -> Result<ForkSeedReport, ForkSeedError> {
        validate_signers(&deterministic_signers)?;
        let facts = pristine.facts();
        let proxy = parse_address("MessageInbox proxy", &facts.message_inbox_proxy.address)?;
        let nock = parse_address("Nock", &facts.nock.address)?;
        let owner = parse_address("pristine owner", &facts.message_inbox_owner)?;
        if owner == Address::ZERO {
            return Err(ForkSeedError::InvalidOwner);
        }

        let before = read_contract_state(backend, proxy, nock).await?;
        validate_pristine_state(&before, facts)?;
        let target_nodes = deterministic_signers.map(|address| format!("{address:#x}"));
        let signer_changes = before
            .bridge_nodes
            .iter()
            .zip(target_nodes.iter())
            .filter(|(before, after)| before != after)
            .count();
        let gate_change = !before.withdrawals_enabled;
        let owner_balance_before = backend.balance(owner).await?;
        if signer_changes == 0 && !gate_change {
            return Ok(ForkSeedReport {
                before: before.clone(),
                after: before,
                owner_balance_before,
                owner_balance_after: owner_balance_before,
                overrides: Vec::new(),
            });
        }

        let snapshot = backend.snapshot().await?;
        let apply_result = apply_overrides(
            backend, proxy, owner, &before, deterministic_signers, owner_balance_before,
        )
        .await;
        match apply_result {
            Ok((overrides, owner_balance_after)) => {
                let after = read_contract_state(backend, proxy, nock)
                    .await
                    .map_err(|error| ForkSeedError::Apply {
                        stage: "post-seed reread",
                        reason: error.to_string(),
                    });
                let after = match after {
                    Ok(after) => after,
                    Err(error) => {
                        let _ = backend.stop_impersonating(owner).await;
                        let reverted = backend.revert(&snapshot).await.unwrap_or(false);
                        return Err(ForkSeedError::RolledBack {
                            stage: "post-seed reread",
                            reason: error.to_string(),
                            reverted,
                        });
                    }
                };
                if let Err(error) = validate_seeded_state(&before, &after, &target_nodes) {
                    let _ = backend.stop_impersonating(owner).await;
                    let reverted = backend.revert(&snapshot).await.unwrap_or(false);
                    return Err(ForkSeedError::RolledBack {
                        stage: "post-seed validation",
                        reason: error.to_string(),
                        reverted,
                    });
                }
                Ok(ForkSeedReport {
                    before,
                    after,
                    owner_balance_before,
                    owner_balance_after,
                    overrides,
                })
            }
            Err(error) => {
                let _ = backend.stop_impersonating(owner).await;
                let reverted = backend.revert(&snapshot).await.unwrap_or(false);
                Err(ForkSeedError::RolledBack {
                    stage: error.stage(),
                    reason: error.to_string(),
                    reverted,
                })
            }
        }
    }
}

impl ForkBalanceSeeder {
    pub async fn seed(
        backend: &AnvilBackend,
        configured_state: &ForkContractState,
        request: ForkBalanceSeedRequest,
    ) -> Result<ForkBalanceSeedReport, ForkBalanceSeedError> {
        validate_balance_request(&request)?;
        let target_nicks = request
            .required_nicks
            .checked_add(request.headroom_nicks)
            .ok_or(ForkBalanceSeedError::InvalidRequest(
                "required plus headroom nicks overflow u64",
            ))?;
        let target_base_units = U256::from(target_nicks)
            .checked_mul(U256::from(WITHDRAWAL_POLICY_V1_BASE_UNITS_PER_NICK))
            .ok_or(ForkBalanceSeedError::InvalidRequest(
                "target base-unit amount overflows uint256",
            ))?;
        let nock =
            parse_address_for_balance("configured Nock", &configured_state.message_inbox_nock)?;
        let inbox =
            parse_address_for_balance("configured MessageInbox", &configured_state.nock_inbox)?;
        let owner = parse_address_for_balance("configured owner", &configured_state.owner)?;
        let holder_before = read_token_balance(backend, nock, request.holder).await?;
        if holder_before > target_base_units {
            return Err(ForkBalanceSeedError::HolderAboveTarget {
                current: holder_before,
                target: target_base_units,
            });
        }
        let total_supply_before = read_total_supply(backend, nock).await?;
        let nock_inbox_before = read_address(backend, nock, "inbox()", None)
            .await
            .map_err(|error| ForkBalanceSeedError::StateRead(error.to_string()))?;
        if nock_inbox_before != configured_state.nock_inbox {
            return Err(ForkBalanceSeedError::Validation(
                "Nock.inbox does not match configured pairing before seed",
            ));
        }

        let snapshot = backend.snapshot().await?;
        let result = apply_balance_seed(
            backend, configured_state, &request, nock, inbox, owner, target_nicks,
            target_base_units, holder_before, total_supply_before, nock_inbox_before,
        )
        .await;
        match result {
            Ok(report) => Ok(report),
            Err(error) => {
                let _ = backend.stop_impersonating(inbox).await;
                let reverted = backend.revert(&snapshot).await.unwrap_or(false);
                Err(ForkBalanceSeedError::RolledBack {
                    stage: error.stage,
                    reason: error.reason,
                    reverted,
                })
            }
        }
    }
}

struct BalanceApplyError {
    stage: &'static str,
    reason: String,
}

async fn apply_balance_seed(
    backend: &AnvilBackend,
    configured_state: &ForkContractState,
    request: &ForkBalanceSeedRequest,
    nock: Address,
    inbox: Address,
    owner: Address,
    target_nicks: u64,
    target_base_units: U256,
    holder_before: U256,
    total_supply_before: U256,
    nock_inbox_before: String,
) -> Result<ForkBalanceSeedReport, BalanceApplyError> {
    let mut gas_targets = Vec::new();
    let mut seen = HashSet::new();
    for address in [request.holder, owner, inbox]
        .into_iter()
        .chain(
            configured_state
                .bridge_nodes
                .iter()
                .filter_map(|node| node.parse().ok()),
        )
        .chain(request.gas_accounts.iter().copied())
    {
        if seen.insert(address) {
            gas_targets.push(address);
        }
    }
    let mut gas_funding = Vec::with_capacity(gas_targets.len());
    for address in gas_targets {
        let before = backend
            .balance(address)
            .await
            .map_err(|error| BalanceApplyError {
                stage: "gas balance read",
                reason: error.to_string(),
            })?;
        if before != request.gas_balance_wei {
            backend
                .set_balance(address, request.gas_balance_wei)
                .await
                .map_err(|error| BalanceApplyError {
                    stage: "gas balance funding",
                    reason: error.to_string(),
                })?;
        }
        let after = backend
            .balance(address)
            .await
            .map_err(|error| BalanceApplyError {
                stage: "gas balance verification",
                reason: error.to_string(),
            })?;
        if after != request.gas_balance_wei {
            return Err(BalanceApplyError {
                stage: "gas balance verification",
                reason: format!("expected {}, observed {after}", request.gas_balance_wei),
            });
        }
        gas_funding.push(GasFundingRecord {
            address,
            before_wei: before,
            after_wei: after,
        });
    }

    let minted_base_units = target_base_units - holder_before;
    let receipt = if minted_base_units == U256::ZERO {
        None
    } else {
        backend
            .impersonate(inbox)
            .await
            .map_err(|error| BalanceApplyError {
                stage: "MessageInbox impersonation",
                reason: error.to_string(),
            })?;
        let hash = backend
            .backend()
            .send_transaction(inbox, nock, encode_mint(request.holder, minted_base_units))
            .await
            .map_err(|error| BalanceApplyError {
                stage: "Nock mint transaction",
                reason: error.to_string(),
            })?;
        let receipt = backend
            .backend()
            .wait_for_receipt(hash, RECEIPT_TIMEOUT)
            .await
            .map_err(|error| BalanceApplyError {
                stage: "Nock mint receipt",
                reason: error.to_string(),
            })?;
        if !receipt.success {
            return Err(BalanceApplyError {
                stage: "Nock mint receipt",
                reason: format!("transaction {} reverted", receipt.transaction_hash),
            });
        }
        require_mint_event(&receipt, nock, request.holder, minted_base_units).map_err(|error| {
            BalanceApplyError {
                stage: "Nock mint event",
                reason: error,
            }
        })?;
        backend
            .stop_impersonating(inbox)
            .await
            .map_err(|error| BalanceApplyError {
                stage: "MessageInbox impersonation stop",
                reason: error.to_string(),
            })?;
        Some(receipt)
    };

    let holder_after = read_token_balance(backend, nock, request.holder)
        .await
        .map_err(|error| BalanceApplyError {
            stage: "holder balance reread",
            reason: error.to_string(),
        })?;
    let total_supply_after =
        read_total_supply(backend, nock)
            .await
            .map_err(|error| BalanceApplyError {
                stage: "total supply reread",
                reason: error.to_string(),
            })?;
    let nock_inbox_after = read_address(backend, nock, "inbox()", None)
        .await
        .map_err(|error| BalanceApplyError {
            stage: "Nock.inbox reread",
            reason: error.to_string(),
        })?;
    let expected_supply_after = total_supply_before
        .checked_add(minted_base_units)
        .ok_or_else(|| BalanceApplyError {
            stage: "mint arithmetic validation",
            reason: "total supply delta overflowed uint256".to_owned(),
        })?;
    if holder_after != target_base_units || total_supply_after != expected_supply_after {
        return Err(BalanceApplyError {
            stage: "mint arithmetic validation",
            reason: "holder balance or total supply delta did not match exact mint".to_owned(),
        });
    }
    if nock_inbox_after != nock_inbox_before {
        return Err(BalanceApplyError {
            stage: "Nock.inbox validation",
            reason: "Nock.inbox changed during fork balance seed".to_owned(),
        });
    }
    let proxy = parse_address("configured MessageInbox", &configured_state.nock_inbox).map_err(
        |error| BalanceApplyError {
            stage: "bridge state reread",
            reason: error.to_string(),
        },
    )?;
    let bridge_state_after = read_contract_state(backend, proxy, nock)
        .await
        .map_err(|error| BalanceApplyError {
            stage: "bridge state reread",
            reason: error.to_string(),
        })?;
    if &bridge_state_after != configured_state {
        return Err(BalanceApplyError {
            stage: "bridge state validation",
            reason: "gate, signers, owner, threshold, or pairing changed".to_owned(),
        });
    }

    Ok(ForkBalanceSeedReport {
        gas_funding,
        mint: MintRecord {
            holder: request.holder,
            target_nicks,
            target_base_units,
            holder_before_base_units: holder_before,
            holder_after_base_units: holder_after,
            total_supply_before_base_units: total_supply_before,
            total_supply_after_base_units: total_supply_after,
            minted_base_units,
            receipt,
        },
        nock_inbox_before,
        nock_inbox_after,
        bridge_state_after,
    })
}

fn validate_balance_request(request: &ForkBalanceSeedRequest) -> Result<(), ForkBalanceSeedError> {
    if request.holder == Address::ZERO {
        return Err(ForkBalanceSeedError::InvalidRequest(
            "holder must not be zero",
        ));
    }
    if request.required_nicks == 0 {
        return Err(ForkBalanceSeedError::InvalidRequest(
            "required nicks must be positive",
        ));
    }
    if request.gas_balance_wei == U256::ZERO {
        return Err(ForkBalanceSeedError::InvalidRequest(
            "gas balance must be positive",
        ));
    }
    if request.gas_accounts.contains(&Address::ZERO) {
        return Err(ForkBalanceSeedError::InvalidRequest(
            "gas account must not be zero",
        ));
    }
    Ok(())
}

pub(crate) async fn read_token_balance(
    backend: &AnvilBackend,
    nock: Address,
    holder: Address,
) -> Result<U256, ForkBalanceSeedError> {
    let mut data = selector("balanceOf(address)");
    data.extend_from_slice(&word_address(holder));
    let output = backend
        .backend()
        .call(nock, Bytes::from(data), "latest")
        .await?;
    decode_u256_word(output.as_ref()).ok_or(ForkBalanceSeedError::Validation(
        "Nock.balanceOf returned malformed data",
    ))
}

pub(crate) async fn read_total_supply(
    backend: &AnvilBackend,
    nock: Address,
) -> Result<U256, ForkBalanceSeedError> {
    let output = backend
        .backend()
        .call(nock, Bytes::from(selector("totalSupply()")), "latest")
        .await?;
    decode_u256_word(output.as_ref()).ok_or(ForkBalanceSeedError::Validation(
        "Nock.totalSupply returned malformed data",
    ))
}

fn encode_mint(holder: Address, amount: U256) -> Bytes {
    let mut data = selector("mint(address,uint256)");
    data.extend_from_slice(&word_address(holder));
    data.extend_from_slice(&amount.to_be_bytes::<32>());
    Bytes::from(data)
}

fn require_mint_event(
    receipt: &TransactionReceiptFacts,
    nock: Address,
    holder: Address,
    amount: U256,
) -> Result<(), String> {
    let transfer = keccak256("Transfer(address,address,uint256)".as_bytes());
    let matched = receipt.logs.iter().any(|log| {
        log.address == nock
            && log.topics.len() == 3
            && log.topics[0] == transfer
            && topic_address(log.topics[1]) == Some(Address::ZERO)
            && topic_address(log.topics[2]) == Some(holder)
            && decode_u256_word(log.data.as_ref()) == Some(amount)
    });
    if matched {
        Ok(())
    } else {
        Err(format!(
            "receipt {} did not contain the exact mint Transfer event",
            receipt.transaction_hash
        ))
    }
}

fn decode_u256_word(word: &[u8]) -> Option<U256> {
    (word.len() == 32).then(|| U256::from_be_slice(word))
}

fn parse_address_for_balance(
    field: &'static str,
    value: &str,
) -> Result<Address, ForkBalanceSeedError> {
    value
        .parse()
        .map_err(|_| ForkBalanceSeedError::InvalidRequest(field))
}

async fn apply_overrides(
    backend: &AnvilBackend,
    proxy: Address,
    owner: Address,
    before: &ForkContractState,
    deterministic_signers: [Address; 5],
    owner_balance_before: U256,
) -> Result<(Vec<OverrideRecord>, U256), ForkSeedError> {
    let minimum_balance = U256::from(1_000_000_000_000_000_000u64);
    if owner_balance_before < minimum_balance {
        backend.set_balance(owner, minimum_balance).await?;
    }
    backend.impersonate(owner).await?;
    let mut overrides = Vec::new();
    for (index, signer) in deterministic_signers.into_iter().enumerate() {
        let after = format!("{signer:#x}");
        if before.bridge_nodes[index] == after {
            continue;
        }
        let data = encode_update_bridge_node(index, signer);
        let hash = backend
            .backend()
            .send_transaction(owner, proxy, data)
            .await
            .map_err(|error| ForkSeedError::Apply {
                stage: "bridge signer transaction",
                reason: error.to_string(),
            })?;
        let receipt = backend
            .backend()
            .wait_for_receipt(hash, RECEIPT_TIMEOUT)
            .await
            .map_err(|error| ForkSeedError::Apply {
                stage: "bridge signer receipt",
                reason: error.to_string(),
            })?;
        require_success(&receipt, "bridge signer receipt")?;
        require_bridge_node_event(&receipt, proxy, index, &before.bridge_nodes[index], &after)?;
        overrides.push(OverrideRecord {
            kind: OverrideKind::BridgeNode { index },
            before: before.bridge_nodes[index].clone(),
            after,
            receipt,
        });
    }
    if !before.withdrawals_enabled {
        let hash = backend
            .backend()
            .send_transaction(owner, proxy, encode_set_withdrawals_enabled(true))
            .await
            .map_err(|error| ForkSeedError::Apply {
                stage: "withdrawal gate transaction",
                reason: error.to_string(),
            })?;
        let receipt = backend
            .backend()
            .wait_for_receipt(hash, RECEIPT_TIMEOUT)
            .await
            .map_err(|error| ForkSeedError::Apply {
                stage: "withdrawal gate receipt",
                reason: error.to_string(),
            })?;
        require_success(&receipt, "withdrawal gate receipt")?;
        require_gate_event(&receipt, proxy, true)?;
        overrides.push(OverrideRecord {
            kind: OverrideKind::WithdrawalsEnabled,
            before: "false".to_owned(),
            after: "true".to_owned(),
            receipt,
        });
    }
    backend.stop_impersonating(owner).await?;
    let owner_balance_after = backend.balance(owner).await?;
    Ok((overrides, owner_balance_after))
}

fn require_success(
    receipt: &TransactionReceiptFacts,
    stage: &'static str,
) -> Result<(), ForkSeedError> {
    if receipt.success {
        Ok(())
    } else {
        Err(ForkSeedError::Apply {
            stage,
            reason: format!("transaction {} reverted", receipt.transaction_hash),
        })
    }
}

fn require_bridge_node_event(
    receipt: &TransactionReceiptFacts,
    proxy: Address,
    index: usize,
    before: &str,
    after: &str,
) -> Result<(), ForkSeedError> {
    let topic = keccak256("BridgeNodeUpdated(uint256,address,address)".as_bytes());
    let before = parse_address("event previous bridge node", before)?;
    let after = parse_address("event new bridge node", after)?;
    let matched = receipt.logs.iter().any(|log| {
        log.address == proxy
            && log.topics.len() == 4
            && log.topics[0] == topic
            && topic_u64(log.topics[1]) == Some(index as u64)
            && topic_address(log.topics[2]) == Some(before)
            && topic_address(log.topics[3]) == Some(after)
    });
    if matched {
        Ok(())
    } else {
        Err(ForkSeedError::Apply {
            stage: "bridge signer event",
            reason: format!(
                "receipt {} did not contain the expected event",
                receipt.transaction_hash
            ),
        })
    }
}

fn require_gate_event(
    receipt: &TransactionReceiptFacts,
    proxy: Address,
    enabled: bool,
) -> Result<(), ForkSeedError> {
    let topic = keccak256("WithdrawalsToggled(bool)".as_bytes());
    let matched = receipt.logs.iter().any(|log| {
        log.address == proxy
            && log.topics.as_slice() == [topic]
            && decode_bool_word(log.data.as_ref()) == Some(enabled)
    });
    if matched {
        Ok(())
    } else {
        Err(ForkSeedError::Apply {
            stage: "withdrawal gate event",
            reason: format!(
                "receipt {} did not contain the expected event",
                receipt.transaction_hash
            ),
        })
    }
}

pub(crate) async fn read_contract_state(
    backend: &AnvilBackend,
    proxy: Address,
    nock: Address,
) -> Result<ForkContractState, ForkSeedError> {
    let owner = read_address(backend, proxy, "owner()", None).await?;
    let nock_owner = read_address(backend, nock, "owner()", None).await?;
    let mut nodes = Vec::with_capacity(5);
    for index in 0..5u64 {
        nodes.push(read_address(backend, proxy, "bridgeNodes(uint256)", Some(index)).await?);
    }
    let bridge_nodes = nodes.try_into().map_err(|_| {
        ForkSeedError::InvalidRpcState("MessageInbox did not return five bridge nodes")
    })?;
    let threshold = read_u64(backend, proxy, "THRESHOLD()").await?;
    let withdrawals_enabled = read_bool(backend, proxy, "withdrawalsEnabled()").await?;
    let message_inbox_nock = read_address(backend, proxy, "nock()", None).await?;
    let nock_inbox = read_address(backend, nock, "inbox()", None).await?;
    Ok(ForkContractState {
        owner,
        nock_owner,
        bridge_nodes,
        threshold,
        withdrawals_enabled,
        message_inbox_nock,
        nock_inbox,
    })
}

fn validate_pristine_state(
    state: &ForkContractState,
    facts: &PristineDeploymentFacts,
) -> Result<(), ForkSeedError> {
    let expected = ForkContractState {
        owner: facts.message_inbox_owner.clone(),
        nock_owner: facts.nock_owner.clone(),
        bridge_nodes: facts.bridge_nodes.clone(),
        threshold: facts.threshold,
        withdrawals_enabled: facts.withdrawals_enabled,
        message_inbox_nock: facts.reciprocal_pairing.message_inbox_nock.clone(),
        nock_inbox: facts.reciprocal_pairing.nock_inbox.clone(),
    };
    if state == &expected {
        Ok(())
    } else {
        Err(ForkSeedError::PristineStateMismatch {
            expected: Box::new(expected),
            observed: Box::new(state.clone()),
        })
    }
}

fn validate_seeded_state(
    before: &ForkContractState,
    after: &ForkContractState,
    target_nodes: &[String; 5],
) -> Result<(), ForkSeedError> {
    if after.owner != before.owner
        || after.nock_owner != before.nock_owner
        || after.threshold != before.threshold
        || after.message_inbox_nock != before.message_inbox_nock
        || after.nock_inbox != before.nock_inbox
    {
        return Err(ForkSeedError::InvalidSeededState(
            "owner, threshold, or contract pairing changed",
        ));
    }
    if &after.bridge_nodes != target_nodes {
        return Err(ForkSeedError::InvalidSeededState(
            "bridge signer reread did not match target",
        ));
    }
    if !after.withdrawals_enabled {
        return Err(ForkSeedError::InvalidSeededState(
            "withdrawal gate remained disabled",
        ));
    }
    Ok(())
}

fn validate_signers(signers: &[Address; 5]) -> Result<(), ForkSeedError> {
    let mut unique = HashSet::with_capacity(signers.len());
    for signer in signers {
        if *signer == Address::ZERO {
            return Err(ForkSeedError::InvalidSignerSet(
                "bridge signer must not be zero",
            ));
        }
        if !unique.insert(*signer) {
            return Err(ForkSeedError::InvalidSignerSet(
                "bridge signers must be unique",
            ));
        }
    }
    Ok(())
}

async fn read_address(
    backend: &AnvilBackend,
    contract: Address,
    signature: &str,
    argument: Option<u64>,
) -> Result<String, ForkSeedError> {
    let mut data = selector(signature);
    if let Some(argument) = argument {
        data.extend_from_slice(&word_u64(argument));
    }
    let output = backend
        .backend()
        .call(contract, Bytes::from(data), "latest")
        .await?;
    let address = decode_address_word(output.as_ref()).ok_or(ForkSeedError::InvalidRpcState(
        "contract address getter returned malformed data",
    ))?;
    Ok(format!("{address:#x}"))
}

async fn read_u64(
    backend: &AnvilBackend,
    contract: Address,
    signature: &str,
) -> Result<u64, ForkSeedError> {
    let output = backend
        .backend()
        .call(contract, Bytes::from(selector(signature)), "latest")
        .await?;
    decode_u64_word(output.as_ref()).ok_or(ForkSeedError::InvalidRpcState(
        "contract integer getter returned malformed data",
    ))
}

async fn read_bool(
    backend: &AnvilBackend,
    contract: Address,
    signature: &str,
) -> Result<bool, ForkSeedError> {
    let output = backend
        .backend()
        .call(contract, Bytes::from(selector(signature)), "latest")
        .await?;
    decode_bool_word(output.as_ref()).ok_or(ForkSeedError::InvalidRpcState(
        "contract boolean getter returned malformed data",
    ))
}

fn encode_update_bridge_node(index: usize, signer: Address) -> Bytes {
    let mut data = selector("updateBridgeNode(uint256,address)");
    data.extend_from_slice(&word_u64(index as u64));
    data.extend_from_slice(&word_address(signer));
    Bytes::from(data)
}

pub(crate) fn encode_set_withdrawals_enabled(enabled: bool) -> Bytes {
    let mut data = selector("setWithdrawalsEnabled(bool)");
    data.extend_from_slice(&word_u64(u64::from(enabled)));
    Bytes::from(data)
}

fn selector(signature: &str) -> Vec<u8> {
    keccak256(signature.as_bytes()).as_slice()[..4].to_vec()
}

fn word_u64(value: u64) -> [u8; 32] {
    let mut word = [0u8; 32];
    word[24..].copy_from_slice(&value.to_be_bytes());
    word
}

fn word_address(address: Address) -> [u8; 32] {
    let mut word = [0u8; 32];
    word[12..].copy_from_slice(address.as_slice());
    word
}

fn decode_address_word(word: &[u8]) -> Option<Address> {
    (word.len() == 32 && word[..12].iter().all(|byte| *byte == 0))
        .then(|| Address::from_slice(&word[12..]))
}

fn decode_u64_word(word: &[u8]) -> Option<u64> {
    if word.len() != 32 || word[..24].iter().any(|byte| *byte != 0) {
        return None;
    }
    let mut bytes = [0u8; 8];
    bytes.copy_from_slice(&word[24..]);
    Some(u64::from_be_bytes(bytes))
}

fn decode_bool_word(word: &[u8]) -> Option<bool> {
    match decode_u64_word(word)? {
        0 => Some(false),
        1 => Some(true),
        _ => None,
    }
}

fn topic_address(topic: B256) -> Option<Address> {
    decode_address_word(topic.as_slice())
}

fn topic_u64(topic: B256) -> Option<u64> {
    decode_u64_word(topic.as_slice())
}

fn parse_address(field: &'static str, value: &str) -> Result<Address, ForkSeedError> {
    value
        .parse()
        .map_err(|_| ForkSeedError::InvalidAddress { field })
}

#[derive(Debug, Error)]
pub enum ForkSeedError {
    #[error("invalid deterministic signer set: {0}")]
    InvalidSignerSet(&'static str),
    #[error("pristine fork owner must not be zero")]
    InvalidOwner,
    #[error("invalid {field} address")]
    InvalidAddress { field: &'static str },
    #[error("fork state no longer matches pristine preflight")]
    PristineStateMismatch {
        expected: Box<ForkContractState>,
        observed: Box<ForkContractState>,
    },
    #[error("fork RPC state is invalid: {0}")]
    InvalidRpcState(&'static str),
    #[error("seeded fork state is invalid: {0}")]
    InvalidSeededState(&'static str),
    #[error("fork seed failed during {stage}: {reason}")]
    Apply { stage: &'static str, reason: String },
    #[error("fork seed failed during {stage} and rollback completed={reverted}: {reason}")]
    RolledBack {
        stage: &'static str,
        reason: String,
        reverted: bool,
    },
    #[error(transparent)]
    Backend(#[from] BaseBackendError),
}

impl ForkSeedError {
    fn stage(&self) -> &'static str {
        match self {
            Self::Apply { stage, .. } | Self::RolledBack { stage, .. } => stage,
            Self::Backend(_) => "Base backend operation",
            Self::InvalidSignerSet(_) => "signer validation",
            Self::InvalidOwner => "owner validation",
            Self::InvalidAddress { .. } => "address validation",
            Self::PristineStateMismatch { .. } => "pristine state validation",
            Self::InvalidRpcState(_) => "RPC state validation",
            Self::InvalidSeededState(_) => "seeded state validation",
        }
    }
}

use alloy::primitives::{Address, B256, U256};
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::anvil::AnvilBackend;
use crate::base_backend::{BaseBackendError, SnapshotId};
use crate::fork_seeder::{
    read_contract_state, read_token_balance, read_total_supply, ForkBalanceSeedError,
    ForkContractState, ForkSeedError,
};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GasBalanceFacts {
    pub address: Address,
    pub balance_wei: U256,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ForkBaselineFacts {
    pub block_number: u64,
    pub block_hash: B256,
    pub holder: Address,
    pub holder_balance_base_units: U256,
    pub total_supply_base_units: U256,
    pub gas_balances: Vec<GasBalanceFacts>,
    pub bridge_state: ForkContractState,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MinedBlockFacts {
    pub blocks: u64,
    pub before_height: u64,
    pub before_hash: B256,
    pub after_height: u64,
    pub after_hash: B256,
}

pub struct ForkState {
    baseline_snapshot: SnapshotId,
    baseline: ForkBaselineFacts,
    nock: Address,
    proxy: Address,
    tracked_gas_accounts: Vec<Address>,
}

impl ForkState {
    pub async fn capture(
        backend: &AnvilBackend,
        configured_state: &ForkContractState,
        holder: Address,
        tracked_gas_accounts: Vec<Address>,
    ) -> Result<Self, ForkStateError> {
        if holder == Address::ZERO {
            return Err(ForkStateError::InvalidAddress("holder"));
        }
        if tracked_gas_accounts.contains(&Address::ZERO) {
            return Err(ForkStateError::InvalidAddress("tracked gas account"));
        }
        let nock = configured_state
            .message_inbox_nock
            .parse()
            .map_err(|_| ForkStateError::InvalidAddress("Nock"))?;
        let proxy = configured_state
            .nock_inbox
            .parse()
            .map_err(|_| ForkStateError::InvalidAddress("MessageInbox"))?;
        let baseline = read_baseline(
            backend, configured_state, holder, nock, proxy, &tracked_gas_accounts,
        )
        .await?;
        let baseline_snapshot = backend.snapshot().await?;
        Ok(Self {
            baseline_snapshot,
            baseline,
            nock,
            proxy,
            tracked_gas_accounts,
        })
    }

    pub fn baseline(&self) -> &ForkBaselineFacts {
        &self.baseline
    }

    pub async fn reset_to_baseline(
        &mut self,
        backend: &AnvilBackend,
    ) -> Result<ForkBaselineFacts, ForkStateError> {
        if !backend.revert(&self.baseline_snapshot).await? {
            return Err(ForkStateError::SnapshotUnavailable);
        }
        let observed = read_baseline(
            backend, &self.baseline.bridge_state, self.baseline.holder, self.nock, self.proxy,
            &self.tracked_gas_accounts,
        )
        .await?;
        if observed != self.baseline {
            return Err(ForkStateError::BaselineMismatch {
                expected: Box::new(self.baseline.clone()),
                observed: Box::new(observed),
            });
        }
        self.baseline_snapshot = backend.snapshot().await?;
        Ok(self.baseline.clone())
    }

    pub async fn mine_base_blocks(
        &self,
        backend: &AnvilBackend,
        blocks: u64,
    ) -> Result<MinedBlockFacts, ForkStateError> {
        if blocks == 0 {
            return Err(ForkStateError::InvalidBlockCount);
        }
        let before_height = backend.block_number().await?;
        let before_hash = backend.block_hash(before_height).await?;
        backend.mine(blocks).await?;
        let after_height = backend.block_number().await?;
        let expected_height = before_height
            .checked_add(blocks)
            .ok_or(ForkStateError::BlockHeightOverflow)?;
        if after_height != expected_height {
            return Err(ForkStateError::UnexpectedMinedHeight {
                expected: expected_height,
                observed: after_height,
            });
        }
        let after_hash = backend.block_hash(after_height).await?;
        Ok(MinedBlockFacts {
            blocks,
            before_height,
            before_hash,
            after_height,
            after_hash,
        })
    }
}

async fn read_baseline(
    backend: &AnvilBackend,
    configured_state: &ForkContractState,
    holder: Address,
    nock: Address,
    proxy: Address,
    tracked_gas_accounts: &[Address],
) -> Result<ForkBaselineFacts, ForkStateError> {
    let block_number = backend.block_number().await?;
    let block_hash = backend.block_hash(block_number).await?;
    let holder_balance_base_units = read_token_balance(backend, nock, holder).await?;
    let total_supply_base_units = read_total_supply(backend, nock).await?;
    let bridge_state = read_contract_state(backend, proxy, nock).await?;
    if &bridge_state != configured_state {
        return Err(ForkStateError::BridgeStateMismatch);
    }
    let mut gas_balances = Vec::with_capacity(tracked_gas_accounts.len());
    for address in tracked_gas_accounts {
        gas_balances.push(GasBalanceFacts {
            address: *address,
            balance_wei: backend.balance(*address).await?,
        });
    }
    Ok(ForkBaselineFacts {
        block_number,
        block_hash,
        holder,
        holder_balance_base_units,
        total_supply_base_units,
        gas_balances,
        bridge_state,
    })
}

#[derive(Debug, Error)]
pub enum ForkStateError {
    #[error("invalid {0} address")]
    InvalidAddress(&'static str),
    #[error("baseline snapshot is no longer available")]
    SnapshotUnavailable,
    #[error("fork reset did not reproduce baseline facts")]
    BaselineMismatch {
        expected: Box<ForkBaselineFacts>,
        observed: Box<ForkBaselineFacts>,
    },
    #[error("bridge state changed from configured seed state")]
    BridgeStateMismatch,
    #[error("mined block count must be positive")]
    InvalidBlockCount,
    #[error("block height overflow")]
    BlockHeightOverflow,
    #[error("mining returned height {observed}, expected {expected}")]
    UnexpectedMinedHeight { expected: u64, observed: u64 },
    #[error(transparent)]
    Backend(#[from] BaseBackendError),
    #[error(transparent)]
    BalanceSeed(#[from] ForkBalanceSeedError),
    #[error("failed to read bridge state: {0}")]
    BridgeState(String),
}

impl From<ForkSeedError> for ForkStateError {
    fn from(error: ForkSeedError) -> Self {
        Self::BridgeState(error.to_string())
    }
}

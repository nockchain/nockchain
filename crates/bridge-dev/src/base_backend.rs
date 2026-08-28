use std::str::FromStr;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use alloy::primitives::{Address, Bytes, B256, U256};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use thiserror::Error;
use tokio::time::{sleep, Instant};

use crate::nonproduction_guard::{GuardedBaseRpc, NonproductionGuardError, ReadOnlyRpc};

pub struct BaseBackend {
    guarded_rpc: GuardedBaseRpc,
    rpc: ReadOnlyRpc,
    nonce_epoch: AtomicU64,
}

impl BaseBackend {
    pub fn new(guarded_rpc: GuardedBaseRpc) -> Result<Self, BaseBackendError> {
        let rpc = guarded_rpc.endpoint().read_only_rpc()?;
        Ok(Self {
            guarded_rpc,
            rpc,
            nonce_epoch: AtomicU64::new(0),
        })
    }

    pub fn guarded_rpc(&self) -> &GuardedBaseRpc {
        &self.guarded_rpc
    }

    pub fn nonce_epoch(&self) -> u64 {
        self.nonce_epoch.load(Ordering::Acquire)
    }

    pub async fn accounts(&self) -> Result<Vec<Address>, BaseBackendError> {
        let accounts: Vec<String> = self.rpc.call("eth_accounts", json!([])).await?;
        accounts
            .iter()
            .map(|account| {
                Address::from_str(account).map_err(|_| BaseBackendError::InvalidRpcValue {
                    field: "account address",
                })
            })
            .collect()
    }

    pub async fn snapshot(&self) -> Result<SnapshotId, BaseBackendError> {
        let id: String = self.rpc.call("evm_snapshot", json!([])).await?;
        SnapshotId::parse(id)
    }

    pub async fn revert(&self, snapshot: &SnapshotId) -> Result<bool, BaseBackendError> {
        let reverted: bool = self
            .rpc
            .call("evm_revert", json!([snapshot.as_str()]))
            .await?;
        if reverted {
            self.nonce_epoch.fetch_add(1, Ordering::AcqRel);
        }
        Ok(reverted)
    }

    pub async fn mine(&self, blocks: u64) -> Result<(), BaseBackendError> {
        if blocks == 0 {
            return Err(BaseBackendError::InvalidArgument(
                "mine requires at least one block",
            ));
        }
        let _: Value = self
            .rpc
            .call("anvil_mine", json!([format!("0x{blocks:x}")]))
            .await?;
        Ok(())
    }

    pub async fn set_balance(
        &self,
        address: Address,
        balance: U256,
    ) -> Result<(), BaseBackendError> {
        let _: Value = self
            .rpc
            .call(
                "anvil_setBalance",
                json!([format!("{address:#x}"), format!("{balance:#x}")]),
            )
            .await?;
        Ok(())
    }

    pub async fn balance(&self, address: Address) -> Result<U256, BaseBackendError> {
        let value: String = self
            .rpc
            .call("eth_getBalance", json!([format!("{address:#x}"), "latest"]))
            .await?;
        U256::from_str(&value).map_err(|_| BaseBackendError::InvalidRpcValue {
            field: "account balance",
        })
    }

    pub async fn impersonate(&self, address: Address) -> Result<(), BaseBackendError> {
        let _: Value = self
            .rpc
            .call("anvil_impersonateAccount", json!([format!("{address:#x}")]))
            .await?;
        Ok(())
    }

    pub async fn stop_impersonating(&self, address: Address) -> Result<(), BaseBackendError> {
        let _: Value = self
            .rpc
            .call(
                "anvil_stopImpersonatingAccount",
                json!([format!("{address:#x}")]),
            )
            .await?;
        Ok(())
    }

    pub async fn send_transaction(
        &self,
        from: Address,
        to: Address,
        data: Bytes,
    ) -> Result<B256, BaseBackendError> {
        let hash: String = self
            .rpc
            .call(
                "eth_sendTransaction",
                json!([{
                    "from": format!("{from:#x}"),
                    "to": format!("{to:#x}"),
                    "data": format!("0x{}", hex::encode(data)),
                }]),
            )
            .await?;
        B256::from_str(&hash).map_err(|_| BaseBackendError::InvalidRpcValue {
            field: "transaction hash",
        })
    }

    pub async fn wait_for_receipt(
        &self,
        hash: B256,
        timeout: Duration,
    ) -> Result<TransactionReceiptFacts, BaseBackendError> {
        if timeout.is_zero() {
            return Err(BaseBackendError::InvalidArgument(
                "receipt timeout must be nonzero",
            ));
        }
        let deadline = Instant::now() + timeout;
        loop {
            let receipt: Option<RpcReceipt> = self
                .rpc
                .call("eth_getTransactionReceipt", json!([format!("{hash:#x}")]))
                .await?;
            if let Some(receipt) = receipt {
                return TransactionReceiptFacts::try_from(receipt);
            }
            if Instant::now() >= deadline {
                return Err(BaseBackendError::ReceiptTimeout { hash });
            }
            sleep(Duration::from_millis(25)).await;
        }
    }

    pub async fn call(
        &self,
        to: Address,
        data: Bytes,
        block: &str,
    ) -> Result<Bytes, BaseBackendError> {
        let output: String = self
            .rpc
            .call(
                "eth_call",
                json!([{
                    "to": format!("{to:#x}"),
                    "data": format!("0x{}", hex::encode(data)),
                }, block]),
            )
            .await?;
        decode_hex_bytes("eth_call output", &output)
    }
    pub async fn code(&self, address: Address, block: &str) -> Result<Bytes, BaseBackendError> {
        let output: String = self
            .rpc
            .call("eth_getCode", json!([format!("{address:#x}"), block]))
            .await?;
        decode_hex_bytes("contract code", &output)
    }

    pub async fn storage_at(
        &self,
        address: Address,
        slot: B256,
        block: &str,
    ) -> Result<B256, BaseBackendError> {
        let output: String = self
            .rpc
            .call(
                "eth_getStorageAt",
                json!([format!("{address:#x}"), format!("{slot:#x}"), block]),
            )
            .await?;
        B256::from_str(&output).map_err(|_| BaseBackendError::InvalidRpcValue {
            field: "storage word",
        })
    }

    pub async fn block_transactions(&self, number: u64) -> Result<Vec<B256>, BaseBackendError> {
        let block: RpcBlock = self
            .rpc
            .call(
                "eth_getBlockByNumber",
                json!([format!("0x{number:x}"), true]),
            )
            .await?;
        block
            .transactions
            .iter()
            .map(|transaction| {
                let hash = transaction
                    .get("hash")
                    .and_then(Value::as_str)
                    .or_else(|| transaction.as_str())
                    .ok_or(BaseBackendError::InvalidRpcValue {
                        field: "block transaction hash",
                    })?;
                B256::from_str(hash).map_err(|_| BaseBackendError::InvalidRpcValue {
                    field: "block transaction hash",
                })
            })
            .collect()
    }
    pub async fn block_number(&self) -> Result<u64, BaseBackendError> {
        let value: String = self.rpc.call("eth_blockNumber", json!([])).await?;
        decode_quantity("block number", &value)
    }

    pub async fn transaction_receipt(
        &self,
        hash: B256,
    ) -> Result<Option<TransactionReceiptFacts>, BaseBackendError> {
        let receipt: Option<RpcReceipt> = self
            .rpc
            .call("eth_getTransactionReceipt", json!([format!("{hash:#x}")]))
            .await?;
        receipt.map(TransactionReceiptFacts::try_from).transpose()
    }
    pub async fn block_hash(&self, number: u64) -> Result<B256, BaseBackendError> {
        let block: RpcBlock = self
            .rpc
            .call(
                "eth_getBlockByNumber",
                json!([format!("0x{number:x}"), false]),
            )
            .await?;
        let hash = block.hash.ok_or(BaseBackendError::InvalidRpcValue {
            field: "block hash",
        })?;
        B256::from_str(&hash).map_err(|_| BaseBackendError::InvalidRpcValue {
            field: "block hash",
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SnapshotId(String);

impl SnapshotId {
    fn parse(value: String) -> Result<Self, BaseBackendError> {
        let digits = value
            .strip_prefix("0x")
            .ok_or(BaseBackendError::InvalidRpcValue {
                field: "snapshot id",
            })?;
        if digits.is_empty() || !digits.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return Err(BaseBackendError::InvalidRpcValue {
                field: "snapshot id",
            });
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TransactionReceiptFacts {
    pub transaction_hash: B256,
    pub block_number: u64,
    pub success: bool,
    pub contract_address: Option<Address>,
    pub logs: Vec<TransactionLogFacts>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TransactionLogFacts {
    pub address: Address,
    pub topics: Vec<B256>,
    pub data: Bytes,
}

impl TryFrom<RpcReceipt> for TransactionReceiptFacts {
    type Error = BaseBackendError;

    fn try_from(receipt: RpcReceipt) -> Result<Self, Self::Error> {
        let transaction_hash = B256::from_str(&receipt.transaction_hash).map_err(|_| {
            BaseBackendError::InvalidRpcValue {
                field: "receipt transaction hash",
            }
        })?;
        let block_number = decode_quantity(
            "receipt block number",
            receipt
                .block_number
                .as_deref()
                .ok_or(BaseBackendError::InvalidRpcValue {
                    field: "receipt block number",
                })?,
        )?;
        let contract_address = receipt
            .contract_address
            .as_deref()
            .map(Address::from_str)
            .transpose()
            .map_err(|_| BaseBackendError::InvalidRpcValue {
                field: "receipt contract address",
            })?;
        let success = receipt.status.as_deref() == Some("0x1");
        let logs = receipt
            .logs
            .into_iter()
            .map(TransactionLogFacts::try_from)
            .collect::<Result<Vec<_>, _>>()?;
        Ok(Self {
            transaction_hash,
            block_number,
            success,
            contract_address,
            logs,
        })
    }
}

impl TryFrom<RpcLog> for TransactionLogFacts {
    type Error = BaseBackendError;

    fn try_from(log: RpcLog) -> Result<Self, Self::Error> {
        let address =
            Address::from_str(&log.address).map_err(|_| BaseBackendError::InvalidRpcValue {
                field: "receipt log address",
            })?;
        let topics = log
            .topics
            .iter()
            .map(|topic| {
                B256::from_str(topic).map_err(|_| BaseBackendError::InvalidRpcValue {
                    field: "receipt log topic",
                })
            })
            .collect::<Result<Vec<_>, _>>()?;
        let data = decode_hex_bytes("receipt log data", &log.data)?;
        Ok(Self {
            address,
            topics,
            data,
        })
    }
}

#[derive(Debug, Error)]
pub enum BaseBackendError {
    #[error(transparent)]
    Guard(#[from] NonproductionGuardError),
    #[error("invalid Base backend argument: {0}")]
    InvalidArgument(&'static str),
    #[error("Base RPC returned invalid {field}")]
    InvalidRpcValue { field: &'static str },
    #[error("timed out waiting for transaction receipt {hash:#x}")]
    ReceiptTimeout { hash: B256 },
}

fn decode_quantity(field: &'static str, value: &str) -> Result<u64, BaseBackendError> {
    let digits = value
        .strip_prefix("0x")
        .ok_or(BaseBackendError::InvalidRpcValue { field })?;
    u64::from_str_radix(digits, 16).map_err(|_| BaseBackendError::InvalidRpcValue { field })
}

fn decode_hex_bytes(field: &'static str, value: &str) -> Result<Bytes, BaseBackendError> {
    let digits = value
        .strip_prefix("0x")
        .ok_or(BaseBackendError::InvalidRpcValue { field })?;
    let bytes = hex::decode(digits).map_err(|_| BaseBackendError::InvalidRpcValue { field })?;
    Ok(Bytes::from(bytes))
}

#[derive(Deserialize)]
struct RpcBlock {
    hash: Option<String>,
    #[serde(default)]
    transactions: Vec<Value>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RpcReceipt {
    transaction_hash: String,
    block_number: Option<String>,
    contract_address: Option<String>,
    status: Option<String>,
    logs: Vec<RpcLog>,
}

#[derive(Deserialize)]
struct RpcLog {
    address: String,
    topics: Vec<String>,
    data: String,
}

use alloy::primitives::{keccak256, Address, Bytes, B256, U256};
use alloy::providers::Provider;
use alloy::rpc::types::eth::{BlockNumberOrTag, Filter};
use alloy::sol_types::SolEvent;
use async_trait::async_trait;

use crate::deposit::ports::BaseContractPort;
use crate::deposit::types::DepositSubmission;
use crate::shared::base::{BaseBridge, MessageInboxContract, NOCK_BASE_PER_NICK};
use crate::shared::errors::BridgeError;
use crate::shared::types::Tip5Hash;

/// Result of a successful deposit submission to Base.
#[derive(Clone, Debug)]
pub struct DepositSubmissionResult {
    /// Transaction hash on Base.
    pub tx_hash: String,
    /// Block number where the transaction was included.
    pub block_number: u64,
}

impl BaseBridge {
    /// Submit a deposit to Base, converting nicks to NOCK base units (ERC-20).
    /// 1 NOCK = 10^16 base units, 1 NOCK = 65,536 nicks, so 1 nick = NOCK_BASE_PER_NICK.
    /// Conversion: amount_base = nicks * NOCK_BASE_PER_NICK.
    pub async fn submit_deposit(
        &self,
        submission: DepositSubmission,
    ) -> Result<DepositSubmissionResult, BridgeError> {
        let recipient = Address::from(submission.recipient);
        let amount = U256::from(submission.amount) * U256::from(NOCK_BASE_PER_NICK);
        let block_height = U256::from(submission.block_height);

        tracing::info!(
            recipient = %recipient,
            amount = %amount,
            block_height = %block_height,
            "Submitting deposit to Base",
        );

        let eth_sigs = submission
            .signatures
            .eth_signatures
            .into_iter()
            .map(|sig| Bytes::from(sig.into_vec()))
            .collect::<Vec<Bytes>>();

        let inbox = MessageInboxContract::new(self.inbox_contract_address(), self.provider());
        let tx_id_sol = MessageInboxContract::Tip5Hash {
            limbs: submission.tx_id.to_array(),
        };
        let name_first_sol = MessageInboxContract::Tip5Hash {
            limbs: submission.name_first.to_array(),
        };
        let name_last_sol = MessageInboxContract::Tip5Hash {
            limbs: submission.name_last.to_array(),
        };
        let as_of_sol = MessageInboxContract::Tip5Hash {
            limbs: submission.as_of.to_array(),
        };

        let submit = inbox
            .submitDeposit(
                tx_id_sol,
                name_first_sol,
                name_last_sol,
                recipient,
                amount,
                block_height,
                as_of_sol,
                U256::from(submission.nonce),
                eth_sigs,
            )
            .from(self.default_signer_address());

        let pending_tx = submit
            .send()
            .await
            .map_err(|e| BridgeError::BaseBridgeSubmission(format!("Transaction failed: {e}")))?;

        let receipt = pending_tx
            .get_receipt()
            .await
            .map_err(|e| BridgeError::BaseBridgeSubmission(format!("Receipt failed: {e}")))?;

        let tx_hash = format!("{:?}", receipt.inner.transaction_hash);
        let block_number = receipt.inner.block_number.unwrap_or(0);
        let status_ok = receipt
            .inner
            .inner
            .receipt
            .as_receipt()
            .status
            .coerce_status();

        if !status_ok {
            return Err(BridgeError::BaseBridgeSubmission(format!(
                "Transaction reverted (status=0) tx_hash={tx_hash}"
            )));
        }

        tracing::info!(
            tx_hash = %tx_hash,
            block_number = %block_number,
            "Deposit submitted successfully!"
        );

        Ok(DepositSubmissionResult {
            tx_hash,
            block_number,
        })
    }

    /// Query the last deposit nonce from the MessageInbox contract.
    pub async fn get_last_deposit_nonce(&self) -> Result<u64, BridgeError> {
        let inbox = MessageInboxContract::new(self.inbox_contract_address(), self.provider());
        let nonce = inbox.lastDepositNonce().call().await.map_err(|e| {
            BridgeError::BaseBridgeQuery(format!("Failed to query lastDepositNonce: {e}"))
        })?;
        Ok(nonce.to::<u64>())
    }

    /// Query whether a nockchain txId has already been processed on-chain.
    pub async fn is_deposit_processed(&self, tx_id: &Tip5Hash) -> Result<bool, BridgeError> {
        use tiny_keccak::{Hasher, Keccak};

        let inbox = MessageInboxContract::new(self.inbox_contract_address(), self.provider());
        let mut hasher = Keccak::v256();
        hasher.update(&tx_id.to_be_limb_bytes());
        let mut out = [0u8; 32];
        hasher.finalize(&mut out);

        inbox
            .processedDeposits(B256::from_slice(&out))
            .call()
            .await
            .map_err(|e| {
                BridgeError::BaseBridgeQuery(format!("Failed to query processedDeposits: {e}"))
            })
    }

    /// Query the nonce for a processed deposit by tx_id.
    pub async fn get_deposit_processed_nonce_for_tx_id(
        &self,
        tx_id: &Tip5Hash,
        from_block: u64,
    ) -> Result<Option<u64>, BridgeError> {
        let provider = self.provider();
        let filter = Filter::new()
            .address(self.inbox_contract_address())
            .event_signature(MessageInboxContract::DepositProcessed::SIGNATURE_HASH)
            .topic1(keccak256(tx_id.to_be_limb_bytes()))
            .from_block(from_block)
            .to_block(BlockNumberOrTag::Latest);
        let logs = provider.get_logs(&filter).await.map_err(|e| {
            BridgeError::BaseBridgeQuery(format!("Failed to query DepositProcessed logs: {e}"))
        })?;

        let mut best: Option<(u64, u64, u64)> = None;
        for log in logs {
            let event = MessageInboxContract::DepositProcessed::decode_raw_log(
                log.topics().iter().cloned(),
                log.data().data.as_ref(),
            )
            .map_err(|e| {
                BridgeError::BaseBridgeQuery(format!("Failed to decode DepositProcessed log: {e}"))
            })?;
            let event_tx_id = Tip5Hash::from_limbs(&event.txIdFull.limbs);
            if &event_tx_id != tx_id {
                return Err(BridgeError::BaseBridgeQuery(
                    "DepositProcessed tx_id mismatch for tx_id filter".into(),
                ));
            }
            if event.nonce > U256::from(u64::MAX) {
                return Err(BridgeError::ValueConversion(
                    "DepositProcessed nonce exceeds u64 range".into(),
                ));
            }

            let block_number = log.block_number.unwrap_or(0);
            let log_index = log.log_index.unwrap_or(0);
            let nonce = event.nonce.to::<u64>();
            match best {
                Some((best_block, best_index, _))
                    if block_number < best_block
                        || (block_number == best_block && log_index <= best_index) => {}
                _ => best = Some((block_number, log_index, nonce)),
            }
        }

        Ok(best.map(|(_, _, nonce)| nonce))
    }
}

#[async_trait]
impl BaseContractPort for BaseBridge {
    async fn submit_deposit(
        &self,
        submission: DepositSubmission,
    ) -> Result<DepositSubmissionResult, BridgeError> {
        BaseBridge::submit_deposit(self, submission).await
    }

    async fn get_last_deposit_nonce(&self) -> Result<u64, BridgeError> {
        BaseBridge::get_last_deposit_nonce(self).await
    }

    async fn is_deposit_processed(&self, tx_id: &Tip5Hash) -> Result<bool, BridgeError> {
        BaseBridge::is_deposit_processed(self, tx_id).await
    }
}

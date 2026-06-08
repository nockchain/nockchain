use async_trait::async_trait;

use crate::deposit::base::DepositSubmissionResult;
use crate::deposit::types::DepositSubmission;
use crate::shared::errors::BridgeError;
use crate::shared::types::Tip5Hash;

#[async_trait]
pub trait BaseContractPort: Send + Sync {
    async fn submit_deposit(
        &self,
        submission: DepositSubmission,
    ) -> Result<DepositSubmissionResult, BridgeError>;

    async fn get_last_deposit_nonce(&self) -> Result<u64, BridgeError>;

    async fn is_deposit_processed(&self, tx_id: &Tip5Hash) -> Result<bool, BridgeError>;
}

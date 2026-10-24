use crate::shared::errors::BridgeError;
use crate::shared::proposer::{withdrawal_active_proposer, withdrawal_turn_proposer};
use crate::withdrawal::types::WithdrawalId;

pub fn scheduled_assembler_node_id(
    withdrawal_id: &WithdrawalId,
    epoch: u64,
    node_pkhs: &[nockchain_types::tx_engine::common::Hash],
) -> Result<u64, BridgeError> {
    scheduled_assembler_turn_node_id(withdrawal_id, epoch, 0, node_pkhs)
}

pub fn scheduled_assembler_turn_node_id(
    withdrawal_id: &WithdrawalId,
    epoch: u64,
    handoff_index: u64,
    node_pkhs: &[nockchain_types::tx_engine::common::Hash],
) -> Result<u64, BridgeError> {
    if node_pkhs.is_empty() {
        return Err(BridgeError::Runtime(
            "cannot choose withdrawal assembler from empty node set".into(),
        ));
    }
    let proposer = if handoff_index == 0 {
        withdrawal_active_proposer(withdrawal_id, epoch, node_pkhs)
    } else {
        withdrawal_turn_proposer(withdrawal_id, epoch, handoff_index, node_pkhs)
    };
    Ok(proposer as u64)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::withdrawal::types::WithdrawalId;

    fn sample_id() -> WithdrawalId {
        WithdrawalId {
            as_of: crate::shared::types::zero_tip5_hash(),
            base_event_id: crate::shared::types::BaseEventId(vec![1, 2, 3]),
        }
    }

    #[test]
    fn empty_node_set_is_invalid() {
        let err = scheduled_assembler_node_id(&sample_id(), 1, &[]).expect_err("empty node set");
        assert!(matches!(err, BridgeError::Runtime(_)));
    }
}

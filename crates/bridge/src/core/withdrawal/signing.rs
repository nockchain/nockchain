use crate::shared::errors::BridgeError;
use crate::withdrawal::types::WithdrawalId;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WithdrawalSigningSequencerDecision {
    Continue,
    SkipAlreadyAdvanced,
}

pub fn plan_signing_sequencer_status(
    id: &WithdrawalId,
    local_nonce: u64,
    sequencer_nonce: u64,
    found: bool,
    state: &str,
) -> Result<WithdrawalSigningSequencerDecision, BridgeError> {
    if sequencer_nonce != local_nonce {
        return Err(BridgeError::Runtime(format!(
            "withdrawal signing nonce mismatch for {:?}: local {}, sequencer {}",
            id, local_nonce, sequencer_nonce
        )));
    }

    if found && matches!(state, "authorized" | "mempool_accepted" | "confirmed") {
        return Ok(WithdrawalSigningSequencerDecision::SkipAlreadyAdvanced);
    }

    Ok(WithdrawalSigningSequencerDecision::Continue)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_id() -> WithdrawalId {
        WithdrawalId {
            as_of: crate::shared::types::zero_tip5_hash(),
            base_event_id: crate::shared::types::AtomBytes(vec![1, 2, 3]),
        }
    }

    #[test]
    fn mismatched_nonce_is_an_error() {
        let err = plan_signing_sequencer_status(&sample_id(), 1, 2, false, "")
            .expect_err("nonce mismatch should fail");
        assert!(matches!(err, BridgeError::Runtime(_)));
    }

    #[test]
    fn advanced_status_skips_signing() {
        assert_eq!(
            plan_signing_sequencer_status(&sample_id(), 1, 1, true, "mempool_accepted")
                .expect("planner"),
            WithdrawalSigningSequencerDecision::SkipAlreadyAdvanced
        );
    }
}

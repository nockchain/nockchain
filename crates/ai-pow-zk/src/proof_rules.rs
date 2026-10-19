//! Consensus-selected AI-PoW proof semantics. This value is never decoded from
//! a certificate. The node derives it from the candidate block's chain height.

pub const AI_POW_HARDENING_HEIGHT: u64 = 154_500;

/// Shared consensus-batch activation. Earlier blocks retain their original
/// admission rules and all proof versions retain their historical setup keys.
pub const AI_POW_CONSENSUS_BATCH_HEIGHT: Option<u64> = Some(170_000);

/// Full block admission policy, selected by the kernel rather than the proof.
/// Consensus batch changes native MoE admission, not the AIR or verifier setup keys.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VerificationRules {
    Legacy,
    Hardened,
    ConsensusBatch,
}

impl VerificationRules {
    pub const fn proof_rules(self) -> ProofRules {
        match self {
            Self::Legacy => ProofRules::Legacy,
            Self::Hardened | Self::ConsensusBatch => ProofRules::Hardened,
        }
    }

    pub const fn at_height(candidate_height: u64) -> Self {
        Self::at_height_with_consensus_batch_height(candidate_height, AI_POW_CONSENSUS_BATCH_HEIGHT)
    }

    /// The optional boundary is trusted chain configuration, never nonce data.
    pub const fn at_height_with_consensus_batch_height(
        candidate_height: u64,
        consensus_batch_height: Option<u64>,
    ) -> Self {
        if let Some(height) = consensus_batch_height {
            assert!(height > AI_POW_HARDENING_HEIGHT);
            if candidate_height >= height {
                return Self::ConsensusBatch;
            }
        }
        if candidate_height < AI_POW_HARDENING_HEIGHT {
            Self::Legacy
        } else {
            Self::Hardened
        }
    }
}

impl From<ProofRules> for VerificationRules {
    fn from(rules: ProofRules) -> Self {
        match rules {
            ProofRules::Legacy => Self::Legacy,
            ProofRules::Hardened => Self::Hardened,
        }
    }
}

#[derive(
    Clone, Copy, Debug, PartialEq, Eq, Hash, Ord, PartialOrd, serde::Serialize, serde::Deserialize,
)]
pub enum ProofRules {
    /// Historical programs, coinbase-only Pearl inclusion, and routing limits.
    Legacy,
    /// Activated proof rules and transaction-bearing Pearl inclusion limits.
    Hardened,
}

impl ProofRules {
    pub const fn at_height(candidate_height: u64) -> Self {
        if candidate_height < AI_POW_HARDENING_HEIGHT {
            Self::Legacy
        } else {
            Self::Hardened
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn consensus_batch_preserves_history_and_uses_candidate_height_on_reorg() {
        // Use the production selector in both directions across each activation.
        for (height, expected) in [
            (126_000, VerificationRules::Legacy),
            (147_500, VerificationRules::Legacy),
            (154_499, VerificationRules::Legacy),
            (154_500, VerificationRules::Hardened),
            (154_501, VerificationRules::Hardened),
            (169_999, VerificationRules::Hardened),
            (170_000, VerificationRules::ConsensusBatch),
            (170_001, VerificationRules::ConsensusBatch),
            (169_999, VerificationRules::Hardened),
            (154_499, VerificationRules::Legacy),
            (154_500, VerificationRules::Hardened),
            (u64::MAX, VerificationRules::ConsensusBatch),
        ] {
            assert_eq!(VerificationRules::at_height(height), expected);
        }
        assert_eq!(
            VerificationRules::ConsensusBatch.proof_rules(),
            ProofRules::Hardened
        );
    }

    #[test]
    #[should_panic]
    fn consensus_batch_cannot_replace_the_historical_activation() {
        VerificationRules::at_height_with_consensus_batch_height(154_500, Some(154_500));
    }

    #[test]
    fn cutover_uses_candidate_height_in_both_directions() {
        for (height, expected) in [
            (126_000, ProofRules::Legacy),
            (147_500, ProofRules::Legacy),
            (153_500, ProofRules::Legacy),
            (154_000, ProofRules::Legacy),
            (154_001, ProofRules::Legacy),
            (154_499, ProofRules::Legacy),
            (154_500, ProofRules::Hardened),
            (154_501, ProofRules::Hardened),
            (169_999, ProofRules::Hardened),
            (170_000, ProofRules::Hardened),
            (170_001, ProofRules::Hardened),
            (169_999, ProofRules::Hardened),
            (154_499, ProofRules::Legacy), // Reorg back across activation.
        ] {
            assert_eq!(ProofRules::at_height(height), expected);
        }
    }
}

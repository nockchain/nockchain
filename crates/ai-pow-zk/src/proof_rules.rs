//! Consensus-selected AI-PoW proof semantics. This value is never decoded from
//! a certificate. The node derives it from the candidate block's chain height.

pub const AI_POW_HARDENING_HEIGHT: u64 = 154_500;

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
    fn cutover_uses_candidate_height_in_both_directions() {
        for (height, expected) in [
            (153_500, ProofRules::Legacy), // Former activations remain historical.
            (154_000, ProofRules::Legacy),
            (154_001, ProofRules::Legacy),
            (154_499, ProofRules::Legacy),
            (154_500, ProofRules::Hardened),
            (154_501, ProofRules::Hardened),
            (154_499, ProofRules::Legacy), // Reorg back across activation.
        ] {
            assert_eq!(ProofRules::at_height(height), expected);
        }
    }
}

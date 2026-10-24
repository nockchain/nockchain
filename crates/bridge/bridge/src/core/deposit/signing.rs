#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SigningTipDecisionInput {
    pub tip_height: Option<u64>,
    pub nonce_epoch_start_height: u64,
    pub logged_epoch_ready: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SigningTipDecision {
    WaitForTip,
    WaitForEpochStart { tip_height: u64 },
    EpochReachedFirstTime { tip_height: u64 },
    Ready { tip_height: u64 },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SigningNonceBaseDecisionInput {
    pub last_chain_nonce: u64,
    pub nonce_epoch_base: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SigningNonceBaseDecision {
    StopNonceEpochMismatch,
    Continue,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SigningLogProgressDecisionInput {
    pub last_chain_nonce: u64,
    pub first_epoch_nonce: u64,
    pub log_len: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SigningLogProgressDecision {
    WaitForLogCatchup { spent_epoch_nonces: u64 },
    Continue { next_nonce: u64 },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SigningEpochBoundsDecisionInput {
    pub is_before_start_key: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SigningEpochBoundsDecision {
    StopRecordBeforeStart,
    Continue,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SigningCandidatePrecheckInput {
    pub is_confirmed: bool,
    pub has_my_signature: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SigningCandidatePrecheckDecision {
    SkipConfirmed,
    SkipAlreadySigned,
    CheckProcessedOnChain,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SigningProcessedDecisionInput {
    pub processed_on_chain: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SigningProcessedDecision {
    SkipProcessed,
    ContinueSign,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SigningTickPlanInput {
    pub tip_height: Option<u64>,
    pub nonce_epoch_start_height: u64,
    pub logged_epoch_ready: bool,
    pub last_chain_nonce: Option<u64>,
    pub nonce_epoch_base: u64,
    pub first_epoch_nonce: u64,
    pub log_len: Option<u64>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SigningTickPlanAction {
    WaitForTip,
    WaitForEpochStart {
        tip_height: u64,
    },
    NeedLastChainNonce {
        tip_height: u64,
        reached_epoch_start: bool,
    },
    StopNonceEpochMismatch {
        tip_height: u64,
        reached_epoch_start: bool,
        last_chain_nonce: u64,
        nonce_epoch_base: u64,
    },
    NeedLogLen {
        tip_height: u64,
        reached_epoch_start: bool,
        last_chain_nonce: u64,
    },
    WaitForLogCatchup {
        tip_height: u64,
        reached_epoch_start: bool,
        last_chain_nonce: u64,
        nonce_epoch_base: u64,
        log_len: u64,
        spent_epoch_nonces: u64,
    },
    Continue {
        tip_height: u64,
        reached_epoch_start: bool,
        last_chain_nonce: u64,
        next_nonce: u64,
    },
}

pub struct SigningPlanner;

impl SigningPlanner {
    pub fn plan_tick(input: SigningTickPlanInput) -> SigningTickPlanAction {
        let (tip_height, reached_epoch_start) = match Self::plan_tip(SigningTipDecisionInput {
            tip_height: input.tip_height,
            nonce_epoch_start_height: input.nonce_epoch_start_height,
            logged_epoch_ready: input.logged_epoch_ready,
        }) {
            SigningTipDecision::WaitForTip => return SigningTickPlanAction::WaitForTip,
            SigningTipDecision::WaitForEpochStart { tip_height } => {
                return SigningTickPlanAction::WaitForEpochStart { tip_height };
            }
            SigningTipDecision::EpochReachedFirstTime { tip_height } => (tip_height, true),
            SigningTipDecision::Ready { tip_height } => (tip_height, false),
        };

        let Some(last_chain_nonce) = input.last_chain_nonce else {
            return SigningTickPlanAction::NeedLastChainNonce {
                tip_height,
                reached_epoch_start,
            };
        };

        if matches!(
            Self::plan_nonce_base(SigningNonceBaseDecisionInput {
                last_chain_nonce,
                nonce_epoch_base: input.nonce_epoch_base,
            }),
            SigningNonceBaseDecision::StopNonceEpochMismatch
        ) {
            return SigningTickPlanAction::StopNonceEpochMismatch {
                tip_height,
                reached_epoch_start,
                last_chain_nonce,
                nonce_epoch_base: input.nonce_epoch_base,
            };
        }

        let Some(log_len) = input.log_len else {
            return SigningTickPlanAction::NeedLogLen {
                tip_height,
                reached_epoch_start,
                last_chain_nonce,
            };
        };

        match Self::plan_log_progress(SigningLogProgressDecisionInput {
            last_chain_nonce,
            first_epoch_nonce: input.first_epoch_nonce,
            log_len,
        }) {
            SigningLogProgressDecision::WaitForLogCatchup { spent_epoch_nonces } => {
                SigningTickPlanAction::WaitForLogCatchup {
                    tip_height,
                    reached_epoch_start,
                    last_chain_nonce,
                    nonce_epoch_base: input.nonce_epoch_base,
                    log_len,
                    spent_epoch_nonces,
                }
            }
            SigningLogProgressDecision::Continue { next_nonce } => {
                SigningTickPlanAction::Continue {
                    tip_height,
                    reached_epoch_start,
                    last_chain_nonce,
                    next_nonce,
                }
            }
        }
    }

    pub fn plan_tip(input: SigningTipDecisionInput) -> SigningTipDecision {
        let Some(tip_height) = input.tip_height else {
            return SigningTipDecision::WaitForTip;
        };

        if tip_height < input.nonce_epoch_start_height {
            return SigningTipDecision::WaitForEpochStart { tip_height };
        }

        if !input.logged_epoch_ready {
            return SigningTipDecision::EpochReachedFirstTime { tip_height };
        }

        SigningTipDecision::Ready { tip_height }
    }

    pub fn plan_nonce_base(input: SigningNonceBaseDecisionInput) -> SigningNonceBaseDecision {
        if input.last_chain_nonce < input.nonce_epoch_base {
            SigningNonceBaseDecision::StopNonceEpochMismatch
        } else {
            SigningNonceBaseDecision::Continue
        }
    }

    pub fn plan_log_progress(input: SigningLogProgressDecisionInput) -> SigningLogProgressDecision {
        let spent_epoch_nonces = if input.last_chain_nonce < input.first_epoch_nonce {
            0
        } else {
            input.last_chain_nonce - input.first_epoch_nonce + 1
        };

        if spent_epoch_nonces > input.log_len {
            SigningLogProgressDecision::WaitForLogCatchup { spent_epoch_nonces }
        } else {
            SigningLogProgressDecision::Continue {
                next_nonce: input.last_chain_nonce + 1,
            }
        }
    }

    pub fn plan_candidate_precheck(
        input: SigningCandidatePrecheckInput,
    ) -> SigningCandidatePrecheckDecision {
        if input.is_confirmed {
            SigningCandidatePrecheckDecision::SkipConfirmed
        } else if input.has_my_signature {
            SigningCandidatePrecheckDecision::SkipAlreadySigned
        } else {
            SigningCandidatePrecheckDecision::CheckProcessedOnChain
        }
    }

    pub fn plan_epoch_bounds(input: SigningEpochBoundsDecisionInput) -> SigningEpochBoundsDecision {
        if input.is_before_start_key {
            SigningEpochBoundsDecision::StopRecordBeforeStart
        } else {
            SigningEpochBoundsDecision::Continue
        }
    }

    pub fn plan_processed(input: SigningProcessedDecisionInput) -> SigningProcessedDecision {
        if !input.processed_on_chain {
            return SigningProcessedDecision::ContinueSign;
        }
        SigningProcessedDecision::SkipProcessed
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn waits_when_tip_unavailable() {
        let decision = SigningPlanner::plan_tip(SigningTipDecisionInput {
            tip_height: None,
            nonce_epoch_start_height: 100,
            logged_epoch_ready: false,
        });
        assert_eq!(decision, SigningTipDecision::WaitForTip);
    }

    #[test]
    fn first_epoch_ready_tick_is_distinct() {
        let decision = SigningPlanner::plan_tip(SigningTipDecisionInput {
            tip_height: Some(100),
            nonce_epoch_start_height: 100,
            logged_epoch_ready: false,
        });
        assert_eq!(
            decision,
            SigningTipDecision::EpochReachedFirstTime { tip_height: 100 }
        );
    }

    #[test]
    fn detects_nonce_epoch_mismatch() {
        let decision = SigningPlanner::plan_nonce_base(SigningNonceBaseDecisionInput {
            last_chain_nonce: 5,
            nonce_epoch_base: 6,
        });
        assert_eq!(decision, SigningNonceBaseDecision::StopNonceEpochMismatch);
    }

    #[test]
    fn waits_for_log_when_chain_prefix_exceeds_log() {
        let decision = SigningPlanner::plan_log_progress(SigningLogProgressDecisionInput {
            last_chain_nonce: 10,
            first_epoch_nonce: 1,
            log_len: 5,
        });
        assert_eq!(
            decision,
            SigningLogProgressDecision::WaitForLogCatchup {
                spent_epoch_nonces: 10
            }
        );
    }

    #[test]
    fn continues_with_next_nonce_when_log_caught_up() {
        let decision = SigningPlanner::plan_log_progress(SigningLogProgressDecisionInput {
            last_chain_nonce: 10,
            first_epoch_nonce: 1,
            log_len: 10,
        });
        assert_eq!(
            decision,
            SigningLogProgressDecision::Continue { next_nonce: 11 }
        );
    }

    #[test]
    fn candidate_precheck_short_circuits_confirmed_and_signed() {
        let confirmed = SigningPlanner::plan_candidate_precheck(SigningCandidatePrecheckInput {
            is_confirmed: true,
            has_my_signature: false,
        });
        assert_eq!(confirmed, SigningCandidatePrecheckDecision::SkipConfirmed);

        let signed = SigningPlanner::plan_candidate_precheck(SigningCandidatePrecheckInput {
            is_confirmed: false,
            has_my_signature: true,
        });
        assert_eq!(signed, SigningCandidatePrecheckDecision::SkipAlreadySigned);

        let check = SigningPlanner::plan_candidate_precheck(SigningCandidatePrecheckInput {
            is_confirmed: false,
            has_my_signature: false,
        });
        assert_eq!(
            check,
            SigningCandidatePrecheckDecision::CheckProcessedOnChain
        );
    }

    #[test]
    fn epoch_bounds_decision_detects_before_start_key() {
        let stop = SigningPlanner::plan_epoch_bounds(SigningEpochBoundsDecisionInput {
            is_before_start_key: true,
        });
        assert_eq!(stop, SigningEpochBoundsDecision::StopRecordBeforeStart);

        let cont = SigningPlanner::plan_epoch_bounds(SigningEpochBoundsDecisionInput {
            is_before_start_key: false,
        });
        assert_eq!(cont, SigningEpochBoundsDecision::Continue);
    }

    #[test]
    fn processed_decision_skips_any_processed_candidate() {
        let skip = SigningPlanner::plan_processed(SigningProcessedDecisionInput {
            processed_on_chain: true,
        });
        assert_eq!(skip, SigningProcessedDecision::SkipProcessed);

        let cont = SigningPlanner::plan_processed(SigningProcessedDecisionInput {
            processed_on_chain: false,
        });
        assert_eq!(cont, SigningProcessedDecision::ContinueSign);
    }

    #[test]
    fn tick_plan_needs_chain_nonce_after_tip_gate() {
        let action = SigningPlanner::plan_tick(SigningTickPlanInput {
            tip_height: Some(100),
            nonce_epoch_start_height: 100,
            logged_epoch_ready: false,
            last_chain_nonce: None,
            nonce_epoch_base: 0,
            first_epoch_nonce: 1,
            log_len: None,
        });

        assert_eq!(
            action,
            SigningTickPlanAction::NeedLastChainNonce {
                tip_height: 100,
                reached_epoch_start: true
            }
        );
    }

    #[test]
    fn tick_plan_requests_log_len_after_nonce_checks() {
        let action = SigningPlanner::plan_tick(SigningTickPlanInput {
            tip_height: Some(100),
            nonce_epoch_start_height: 100,
            logged_epoch_ready: true,
            last_chain_nonce: Some(5),
            nonce_epoch_base: 0,
            first_epoch_nonce: 1,
            log_len: None,
        });

        assert_eq!(
            action,
            SigningTickPlanAction::NeedLogLen {
                tip_height: 100,
                reached_epoch_start: false,
                last_chain_nonce: 5
            }
        );
    }

    #[test]
    fn tick_plan_continues_with_next_nonce_when_ready() {
        let action = SigningPlanner::plan_tick(SigningTickPlanInput {
            tip_height: Some(100),
            nonce_epoch_start_height: 100,
            logged_epoch_ready: true,
            last_chain_nonce: Some(5),
            nonce_epoch_base: 0,
            first_epoch_nonce: 1,
            log_len: Some(10),
        });

        assert_eq!(
            action,
            SigningTickPlanAction::Continue {
                tip_height: 100,
                reached_epoch_start: false,
                last_chain_nonce: 5,
                next_nonce: 6
            }
        );
    }
}

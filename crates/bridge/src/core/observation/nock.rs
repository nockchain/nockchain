use crate::core::ports::{KernelStatePort, NockSourcePort};
use crate::shared::errors::BridgeError;
use crate::shared::runtime::ChainEvent;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NockObserverCore {
    pub confirmation_depth: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NockPlanInput {
    pub tip_height: Option<u64>,
    pub next_needed_height: Option<u64>,
    pub confirmation_depth: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NockPlanAction {
    NoTipAvailable,
    NoPendingHeight {
        tip_height: u64,
        confirmed_target: u64,
    },
    BootstrapUnconfirmed {
        tip_height: u64,
        confirmation_depth: u64,
    },
    NotYetConfirmed {
        tip_height: u64,
        confirmed_target: u64,
        next_needed_height: u64,
    },
    FetchHeight {
        tip_height: u64,
        confirmed_target: u64,
        height: u64,
    },
}

pub fn plan_nock_tick(input: NockPlanInput) -> NockPlanAction {
    let Some(tip_height) = input.tip_height else {
        return NockPlanAction::NoTipAvailable;
    };

    let confirmed_target = tip_height.saturating_sub(input.confirmation_depth);
    if confirmed_target == 0 {
        return NockPlanAction::BootstrapUnconfirmed {
            tip_height,
            confirmation_depth: input.confirmation_depth,
        };
    }

    let Some(next_needed_height) = input.next_needed_height else {
        return NockPlanAction::NoPendingHeight {
            tip_height,
            confirmed_target,
        };
    };

    if confirmed_target < next_needed_height {
        return NockPlanAction::NotYetConfirmed {
            tip_height,
            confirmed_target,
            next_needed_height,
        };
    }

    NockPlanAction::FetchHeight {
        tip_height,
        confirmed_target,
        height: next_needed_height,
    }
}

impl NockObserverCore {
    pub fn plan_tick(
        &self,
        tip_height: Option<u64>,
        next_needed_height: Option<u64>,
    ) -> NockPlanAction {
        plan_nock_tick(NockPlanInput {
            tip_height,
            next_needed_height,
            confirmation_depth: self.confirmation_depth,
        })
    }
}

pub struct NockObserverRunner<S, K> {
    pub core: NockObserverCore,
    pub source: S,
    pub kernel: K,
}

impl<S, K> NockObserverRunner<S, K>
where
    S: NockSourcePort,
    K: KernelStatePort,
{
    pub async fn tick_once(&mut self) -> Result<NockPlanAction, BridgeError> {
        let tip_info = self.source.tip_info().await?;
        let tip_height = tip_info.as_ref().map(|tip| tip.height);
        let next_needed_height = self.kernel.peek_nock_next_height().await?;
        let action = self.core.plan_tick(tip_height, next_needed_height);

        if let NockPlanAction::FetchHeight { height, .. } = action {
            if let Some(block) = self.source.fetch_block_at_height(height).await? {
                let _ = self
                    .kernel
                    .emit_chain_event(ChainEvent::Nock(block))
                    .await?;
            }
        }

        Ok(action)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_tip_when_heaviest_missing() {
        let action = plan_nock_tick(NockPlanInput {
            tip_height: None,
            next_needed_height: Some(1),
            confirmation_depth: 10,
        });
        assert_eq!(action, NockPlanAction::NoTipAvailable);
    }

    #[test]
    fn no_pending_height_when_kernel_idle() {
        let action = plan_nock_tick(NockPlanInput {
            tip_height: Some(200),
            next_needed_height: None,
            confirmation_depth: 100,
        });
        assert_eq!(
            action,
            NockPlanAction::NoPendingHeight {
                tip_height: 200,
                confirmed_target: 100,
            }
        );
    }

    #[test]
    fn bootstrap_until_confirmed_target_nonzero() {
        let action = plan_nock_tick(NockPlanInput {
            tip_height: Some(100),
            next_needed_height: Some(1),
            confirmation_depth: 100,
        });
        assert_eq!(
            action,
            NockPlanAction::BootstrapUnconfirmed {
                tip_height: 100,
                confirmation_depth: 100,
            }
        );
    }

    #[test]
    fn zero_depth_uses_current_tip_as_confirmed_target() {
        let action = plan_nock_tick(NockPlanInput {
            tip_height: Some(250),
            next_needed_height: Some(250),
            confirmation_depth: 0,
        });
        assert_eq!(
            action,
            NockPlanAction::FetchHeight {
                tip_height: 250,
                confirmed_target: 250,
                height: 250,
            }
        );
    }

    #[test]
    fn zero_depth_still_bootstraps_at_tip_zero() {
        let action = plan_nock_tick(NockPlanInput {
            tip_height: Some(0),
            next_needed_height: Some(1),
            confirmation_depth: 0,
        });
        assert_eq!(
            action,
            NockPlanAction::BootstrapUnconfirmed {
                tip_height: 0,
                confirmation_depth: 0,
            }
        );
    }

    #[test]
    fn not_yet_confirmed_when_target_behind_next_needed() {
        let action = plan_nock_tick(NockPlanInput {
            tip_height: Some(250),
            next_needed_height: Some(175),
            confirmation_depth: 100,
        });
        assert_eq!(
            action,
            NockPlanAction::NotYetConfirmed {
                tip_height: 250,
                confirmed_target: 150,
                next_needed_height: 175,
            }
        );
    }

    #[test]
    fn fetches_when_target_confirmed() {
        let action = plan_nock_tick(NockPlanInput {
            tip_height: Some(250),
            next_needed_height: Some(150),
            confirmation_depth: 100,
        });
        assert_eq!(
            action,
            NockPlanAction::FetchHeight {
                tip_height: 250,
                confirmed_target: 150,
                height: 150,
            }
        );
    }
}

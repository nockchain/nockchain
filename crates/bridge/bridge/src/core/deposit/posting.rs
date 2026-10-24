use async_trait::async_trait;

use crate::shared::errors::BridgeError;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PostingCandidateDecisionInput {
    pub proposal_nonce: u64,
    pub next_nonce: u64,
    pub my_node_id: usize,
    pub current_proposer: usize,
    pub num_nodes: usize,
    pub ready_at: Option<u64>,
    pub now_secs: u64,
    pub failover_backoff_secs: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PostingTickPlanInput {
    pub next_nonce: u64,
    pub my_node_id: usize,
    pub current_proposer: usize,
    pub num_nodes: usize,
    pub now_secs: u64,
    pub failover_backoff_secs: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PostingReadyProposal {
    pub nonce: u64,
    pub ready_at: Option<u64>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PostingCandidateDecision {
    MarkConfirmedOnChain,
    WaitForEarlierNonce,
    NotMyTurn,
    Submit { is_proposer: bool },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PostingTickRunnerInput {
    pub my_node_id: usize,
    pub current_proposer: usize,
    pub num_nodes: usize,
    pub now_secs: u64,
    pub failover_backoff_secs: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PostingTickPlannedCandidate {
    pub proposal: PostingReadyProposal,
    pub decision: PostingCandidateDecision,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PostingTickRunnerAction {
    NoReadyProposals,
    Planned {
        next_nonce: u64,
        candidates: Vec<PostingTickPlannedCandidate>,
    },
}

#[async_trait]
pub trait PostingTickSource: Send + Sync {
    async fn ready_proposals(&self) -> Result<Vec<PostingReadyProposal>, BridgeError>;
    async fn next_nonce(&self) -> Result<u64, BridgeError>;
}

pub struct PostingTickRunner<S> {
    pub source: S,
}

impl<S> PostingTickRunner<S>
where
    S: PostingTickSource,
{
    pub async fn tick_once(
        &self,
        input: PostingTickRunnerInput,
    ) -> Result<PostingTickRunnerAction, BridgeError> {
        PostingPlanner::tick_once(&self.source, input).await
    }
}

pub struct PostingPlanner;

impl PostingPlanner {
    pub async fn tick_once<S>(
        source: &S,
        input: PostingTickRunnerInput,
    ) -> Result<PostingTickRunnerAction, BridgeError>
    where
        S: PostingTickSource,
    {
        let proposals = source.ready_proposals().await?;
        if proposals.is_empty() {
            return Ok(PostingTickRunnerAction::NoReadyProposals);
        }

        let next_nonce = source.next_nonce().await?;
        let decisions = Self::plan_tick(
            PostingTickPlanInput {
                next_nonce,
                my_node_id: input.my_node_id,
                current_proposer: input.current_proposer,
                num_nodes: input.num_nodes,
                now_secs: input.now_secs,
                failover_backoff_secs: input.failover_backoff_secs,
            },
            &proposals,
        );
        let candidates = proposals
            .into_iter()
            .zip(decisions)
            .map(|(proposal, decision)| PostingTickPlannedCandidate { proposal, decision })
            .collect();

        Ok(PostingTickRunnerAction::Planned {
            next_nonce,
            candidates,
        })
    }

    pub fn plan_tick(
        input: PostingTickPlanInput,
        proposals: &[PostingReadyProposal],
    ) -> Vec<PostingCandidateDecision> {
        proposals
            .iter()
            .map(|proposal| {
                Self::plan_candidate(PostingCandidateDecisionInput {
                    proposal_nonce: proposal.nonce,
                    next_nonce: input.next_nonce,
                    my_node_id: input.my_node_id,
                    current_proposer: input.current_proposer,
                    num_nodes: input.num_nodes,
                    ready_at: proposal.ready_at,
                    now_secs: input.now_secs,
                    failover_backoff_secs: input.failover_backoff_secs,
                })
            })
            .collect()
    }

    pub fn plan_candidate(input: PostingCandidateDecisionInput) -> PostingCandidateDecision {
        if input.proposal_nonce < input.next_nonce {
            return PostingCandidateDecision::MarkConfirmedOnChain;
        }

        if input.proposal_nonce > input.next_nonce {
            return PostingCandidateDecision::WaitForEarlierNonce;
        }

        if input.my_node_id == input.current_proposer {
            return PostingCandidateDecision::Submit { is_proposer: true };
        }

        let Some(ready_at) = input.ready_at else {
            return PostingCandidateDecision::NotMyTurn;
        };

        let failover_slot = if input.my_node_id > input.current_proposer {
            input.my_node_id - input.current_proposer
        } else {
            input.num_nodes - input.current_proposer + input.my_node_id
        };
        let required_wait = input.failover_backoff_secs * failover_slot as u64;
        let elapsed = input.now_secs.saturating_sub(ready_at);
        if elapsed >= required_wait {
            PostingCandidateDecision::Submit { is_proposer: false }
        } else {
            PostingCandidateDecision::NotMyTurn
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use tokio::sync::Mutex;

    use super::*;

    #[test]
    fn marks_confirmed_when_nonce_is_behind_chain() {
        let decision = PostingPlanner::plan_candidate(PostingCandidateDecisionInput {
            proposal_nonce: 4,
            next_nonce: 5,
            my_node_id: 0,
            current_proposer: 0,
            num_nodes: 5,
            ready_at: Some(100),
            now_secs: 200,
            failover_backoff_secs: 120,
        });
        assert_eq!(decision, PostingCandidateDecision::MarkConfirmedOnChain);
    }

    #[test]
    fn waits_when_nonce_is_ahead_of_chain() {
        let decision = PostingPlanner::plan_candidate(PostingCandidateDecisionInput {
            proposal_nonce: 6,
            next_nonce: 5,
            my_node_id: 0,
            current_proposer: 0,
            num_nodes: 5,
            ready_at: Some(100),
            now_secs: 200,
            failover_backoff_secs: 120,
        });
        assert_eq!(decision, PostingCandidateDecision::WaitForEarlierNonce);
    }

    #[test]
    fn proposer_submits_immediately() {
        let decision = PostingPlanner::plan_candidate(PostingCandidateDecisionInput {
            proposal_nonce: 5,
            next_nonce: 5,
            my_node_id: 2,
            current_proposer: 2,
            num_nodes: 5,
            ready_at: Some(100),
            now_secs: 101,
            failover_backoff_secs: 120,
        });
        assert_eq!(
            decision,
            PostingCandidateDecision::Submit { is_proposer: true }
        );
    }

    #[test]
    fn failover_node_waits_until_backoff_elapsed() {
        let input = PostingCandidateDecisionInput {
            proposal_nonce: 5,
            next_nonce: 5,
            my_node_id: 3,
            current_proposer: 2,
            num_nodes: 5,
            ready_at: Some(100),
            now_secs: 200,
            failover_backoff_secs: 120,
        };

        let decision_wait = PostingPlanner::plan_candidate(input);
        assert_eq!(decision_wait, PostingCandidateDecision::NotMyTurn);

        let decision_submit = PostingPlanner::plan_candidate(PostingCandidateDecisionInput {
            now_secs: 220,
            ..input
        });
        assert_eq!(
            decision_submit,
            PostingCandidateDecision::Submit { is_proposer: false }
        );
    }

    #[test]
    fn plan_tick_returns_one_decision_per_proposal_in_order() {
        let decisions = PostingPlanner::plan_tick(
            PostingTickPlanInput {
                next_nonce: 5,
                my_node_id: 1,
                current_proposer: 0,
                num_nodes: 3,
                now_secs: 200,
                failover_backoff_secs: 120,
            },
            &[
                PostingReadyProposal {
                    nonce: 4,
                    ready_at: Some(100),
                },
                PostingReadyProposal {
                    nonce: 6,
                    ready_at: Some(100),
                },
                PostingReadyProposal {
                    nonce: 5,
                    ready_at: Some(100),
                },
            ],
        );

        assert_eq!(
            decisions,
            vec![
                PostingCandidateDecision::MarkConfirmedOnChain,
                PostingCandidateDecision::WaitForEarlierNonce,
                PostingCandidateDecision::NotMyTurn,
            ]
        );
    }

    #[derive(Clone)]
    struct FakePostingTickSource {
        proposals: Arc<Mutex<Vec<PostingReadyProposal>>>,
        next_nonce: u64,
    }

    #[async_trait]
    impl PostingTickSource for FakePostingTickSource {
        async fn ready_proposals(&self) -> Result<Vec<PostingReadyProposal>, BridgeError> {
            Ok(self.proposals.lock().await.clone())
        }

        async fn next_nonce(&self) -> Result<u64, BridgeError> {
            Ok(self.next_nonce)
        }
    }

    #[tokio::test]
    async fn tick_runner_queries_source_and_returns_planned_actions() {
        let source = FakePostingTickSource {
            proposals: Arc::new(Mutex::new(vec![
                PostingReadyProposal {
                    nonce: 4,
                    ready_at: Some(100),
                },
                PostingReadyProposal {
                    nonce: 5,
                    ready_at: Some(100),
                },
            ])),
            next_nonce: 5,
        };
        let runner = PostingTickRunner { source };

        let action = runner
            .tick_once(PostingTickRunnerInput {
                my_node_id: 1,
                current_proposer: 0,
                num_nodes: 3,
                now_secs: 200,
                failover_backoff_secs: 120,
            })
            .await
            .expect("tick succeeds");

        assert_eq!(
            action,
            PostingTickRunnerAction::Planned {
                next_nonce: 5,
                candidates: vec![
                    PostingTickPlannedCandidate {
                        proposal: PostingReadyProposal {
                            nonce: 4,
                            ready_at: Some(100),
                        },
                        decision: PostingCandidateDecision::MarkConfirmedOnChain
                    },
                    PostingTickPlannedCandidate {
                        proposal: PostingReadyProposal {
                            nonce: 5,
                            ready_at: Some(100),
                        },
                        decision: PostingCandidateDecision::NotMyTurn
                    },
                ],
            }
        );
    }
}

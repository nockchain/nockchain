use std::collections::BTreeSet;

use bridge_dev::model::{
    ModelBurnState, ModelHoldKind, ModelNoteName, ModelProposalState, ModelPublicState,
    WithdrawalModelAction, WithdrawalModelError, WithdrawalModelEvent, WithdrawalModelState,
};

#[test]
fn happy_trace_reaches_terminal_with_all_safety_facts() {
    let mut state = WithdrawalModelState::default();
    for action in happy_actions() {
        state.apply(&action).expect("model action");
    }
    assert!(state.terminal);
    assert_eq!(state.public_state, ModelPublicState::Terminal);
    assert_eq!(state.payout_count, 1);
    assert_eq!(state.refund_count, 0);
    assert_eq!(state.kernel_settled_nodes, BTreeSet::from([0, 1, 2, 3, 4]));
    assert!(state.reservations.is_empty());
    assert_eq!(state.released_inputs, inputs());
    state.validate().expect("terminal invariants");
}

#[test]
fn every_major_transition_has_explicit_illegal_preconditions() {
    let mut empty = WithdrawalModelState::default();
    for action in [
        WithdrawalModelAction::Prepare,
        WithdrawalModelAction::Canonicalize,
        WithdrawalModelAction::Submit {
            transaction_id: "tx-1".to_owned(),
        },
        WithdrawalModelAction::Confirm {
            transaction_id: "tx-1".to_owned(),
            height: 10,
            block_id: "block-10".to_owned(),
        },
        WithdrawalModelAction::SettleKernel { node_id: 0 },
        WithdrawalModelAction::RecordPayout,
        WithdrawalModelAction::RecordRefund,
        WithdrawalModelAction::ReplayJournal { generation: 1 },
    ] {
        assert!(matches!(
            empty.apply(&action),
            Err(WithdrawalModelError::Precondition { .. })
        ));
    }

    empty
        .apply(&WithdrawalModelAction::ObserveBurn {
            withdrawal_id: "withdrawal-1".to_owned(),
            nonce: 1,
        })
        .expect("burn");
    assert!(empty
        .apply(&WithdrawalModelAction::Assemble {
            epoch: 1,
            handoff: 0,
            proposal_hash: "proposal-1".to_owned(),
            selected_inputs: BTreeSet::new(),
        })
        .is_err());
    assert!(empty
        .apply(&WithdrawalModelAction::Reserve {
            owner: "other-withdrawal".to_owned(),
            inputs: inputs(),
        })
        .is_err());
    assert!(empty
        .apply(&WithdrawalModelAction::Publish {
            state: ModelPublicState::Terminal,
        })
        .is_err());
}

#[test]
fn duplicate_observations_and_rpc_actions_are_idempotent() {
    let mut state = WithdrawalModelState::default();
    let burn = WithdrawalModelAction::ObserveBurn {
        withdrawal_id: "withdrawal-1".to_owned(),
        nonce: 1,
    };
    state.apply(&burn).expect("first burn");
    let duplicate = state.apply(&burn).expect("duplicate burn");
    assert!(!duplicate.changed);
    assert_eq!(
        duplicate.events,
        vec![WithdrawalModelEvent::DuplicateIgnored]
    );

    for action in happy_actions().into_iter().skip(1) {
        state.apply(&action).expect("model action");
        if matches!(
            action,
            WithdrawalModelAction::Prepare
                | WithdrawalModelAction::Canonicalize
                | WithdrawalModelAction::Submit { .. }
                | WithdrawalModelAction::Include { .. }
                | WithdrawalModelAction::Confirm { .. }
                | WithdrawalModelAction::SettleKernel { .. }
                | WithdrawalModelAction::RecordPayout
                | WithdrawalModelAction::ReleaseReservations { .. }
                | WithdrawalModelAction::Publish { .. }
        ) {
            let duplicate = state.apply(&action).expect("duplicate action");
            assert!(!duplicate.changed, "action={}", action.name());
        }
    }
}

#[test]
fn restart_requires_exact_generation_replay_and_reservation_restore() {
    let mut state = canonicalized_reserved_state();
    state
        .apply(&WithdrawalModelAction::Authorize {
            epoch: 1,
            transaction_id: "tx-1".to_owned(),
        })
        .expect("authorize");
    state
        .apply(&WithdrawalModelAction::Restart {
            component: "sequencer".to_owned(),
        })
        .expect("restart sequencer");
    assert!(state.replay_required);
    assert!(state.reservations.is_empty());
    assert!(state
        .apply(&WithdrawalModelAction::Submit {
            transaction_id: "tx-1".to_owned(),
        })
        .is_err());
    assert!(state
        .apply(&WithdrawalModelAction::ReplayJournal { generation: 0 })
        .is_err());
    state
        .apply(&WithdrawalModelAction::RestoreReservations)
        .expect("restore reservations");
    state
        .apply(&WithdrawalModelAction::ReplayJournal { generation: 1 })
        .expect("replay exact generation");
    state
        .apply(&WithdrawalModelAction::Submit {
            transaction_id: "tx-1".to_owned(),
        })
        .expect("submit after recovery");
}

#[test]
fn proposal_replacement_requires_new_epoch_and_one_authorized_tx_per_epoch() {
    let mut state = canonicalized_reserved_state();
    state
        .apply(&WithdrawalModelAction::Authorize {
            epoch: 1,
            transaction_id: "tx-1".to_owned(),
        })
        .expect("first authorization");
    assert!(state
        .apply(&WithdrawalModelAction::Authorize {
            epoch: 1,
            transaction_id: "tx-2".to_owned(),
        })
        .is_err());

    let mut replacement = WithdrawalModelState::default();
    prepare_burn(&mut replacement);
    replacement
        .apply(&WithdrawalModelAction::Assemble {
            epoch: 1,
            handoff: 0,
            proposal_hash: "proposal-1".to_owned(),
            selected_inputs: inputs(),
        })
        .expect("first proposal");
    assert!(replacement
        .apply(&WithdrawalModelAction::Assemble {
            epoch: 1,
            handoff: 1,
            proposal_hash: "proposal-stale".to_owned(),
            selected_inputs: inputs(),
        })
        .is_err());
    replacement
        .apply(&WithdrawalModelAction::Assemble {
            epoch: 2,
            handoff: 0,
            proposal_hash: "proposal-2".to_owned(),
            selected_inputs: inputs(),
        })
        .expect("replacement proposal");
    assert_eq!(replacement.proposal.as_ref().expect("proposal").epoch, 2);
}

#[test]
fn shallow_base_reorg_orphans_and_readmits_before_new_epoch() {
    let mut state = canonicalized_reserved_state();
    state
        .apply(&WithdrawalModelAction::BaseReorg { deep: false })
        .expect("shallow Base reorg");
    assert_eq!(
        state.burn.as_ref().expect("burn").state,
        ModelBurnState::Orphaned
    );
    assert!(state.proposal.is_none());
    state
        .apply(&WithdrawalModelAction::ReadmitBurn)
        .expect("readmit burn");
    assert_eq!(
        state.burn.as_ref().expect("burn").state,
        ModelBurnState::Canonical
    );
    state
        .apply(&WithdrawalModelAction::Assemble {
            epoch: 2,
            handoff: 0,
            proposal_hash: "proposal-2".to_owned(),
            selected_inputs: inputs(),
        })
        .expect("new proposal after readmit");
}

#[test]
fn same_transaction_reinclusion_clears_confirmation_until_new_block_confirms() {
    let mut state = confirmed_state();
    state
        .apply(&WithdrawalModelAction::NockReorg {
            deep: false,
            reinclusion_height: Some(11),
            reinclusion_block_id: Some("block-11".to_owned()),
        })
        .expect("same tx re-inclusion");
    assert!(!state.confirmed);
    assert!(state.kernel_settled_nodes.is_empty());
    assert_eq!(state.public_state, ModelPublicState::Submitted);
    assert_eq!(state.inclusion.as_ref().expect("inclusion").height, 11);
    state
        .apply(&WithdrawalModelAction::Confirm {
            transaction_id: "tx-1".to_owned(),
            height: 11,
            block_id: "block-11".to_owned(),
        })
        .expect("confirm re-inclusion");
}

#[test]
fn deep_reorg_enters_hold_before_mutating_terminal_facts_and_requires_recovery() {
    let mut state = terminal_state();
    state
        .apply(&WithdrawalModelAction::BaseReorg { deep: true })
        .expect("deep Base hold");
    assert_eq!(state.hold, Some(ModelHoldKind::DeepBaseReorg));
    assert_eq!(state.public_state, ModelPublicState::ReorgHold);
    assert!(!state.terminal);
    assert_eq!(state.payout_count, 1);
    assert!(state.apply(&WithdrawalModelAction::ReadmitBurn).is_err());
    state
        .apply(&WithdrawalModelAction::RecoverHold {
            hold: ModelHoldKind::DeepBaseReorg,
        })
        .expect("reviewed hold recovery");
    assert!(state.hold.is_none());
    state
        .apply(&WithdrawalModelAction::Publish {
            state: ModelPublicState::Terminal,
        })
        .expect("restore terminal proof");
}

#[test]
fn refund_and_payout_are_mutually_exclusive_and_never_repeat() {
    let mut refund = WithdrawalModelState::default();
    prepare_burn(&mut refund);
    refund
        .apply(&WithdrawalModelAction::InvalidateBurn)
        .expect("orphan burn");
    refund
        .apply(&WithdrawalModelAction::RecordRefund)
        .expect("record refund");
    let duplicate = refund
        .apply(&WithdrawalModelAction::RecordRefund)
        .expect("duplicate refund");
    assert!(!duplicate.changed);
    assert!(refund.apply(&WithdrawalModelAction::RecordPayout).is_err());

    let mut payout = terminal_state();
    let duplicate = payout
        .apply(&WithdrawalModelAction::RecordPayout)
        .expect("duplicate payout");
    assert!(!duplicate.changed);
    assert!(payout.apply(&WithdrawalModelAction::RecordRefund).is_err());
}

#[test]
fn public_state_regression_and_stale_generation_are_rejected() {
    let mut state = confirmed_state();
    state
        .apply(&WithdrawalModelAction::Publish {
            state: ModelPublicState::SequencerConfirmed,
        })
        .expect("publish confirmed");
    assert!(state
        .apply(&WithdrawalModelAction::Publish {
            state: ModelPublicState::Ready,
        })
        .is_err());
    state
        .apply(&WithdrawalModelAction::Restart {
            component: "sequencer".to_owned(),
        })
        .expect("restart");
    assert!(state
        .apply(&WithdrawalModelAction::ReplayJournal { generation: 2 })
        .is_err());
}

#[test]
fn invariant_mutations_are_distinct_from_action_precondition_failures() {
    let mut reservation_conflict = canonicalized_reserved_state();
    reservation_conflict
        .reservations
        .insert(note(99), "other-owner".to_owned());
    assert!(matches!(
        reservation_conflict.validate(),
        Err(WithdrawalModelError::Invariant(_))
    ));

    let terminal_without_chain = WithdrawalModelState {
        terminal: true,
        public_state: ModelPublicState::Terminal,
        ..WithdrawalModelState::default()
    };
    assert!(matches!(
        terminal_without_chain.validate(),
        Err(WithdrawalModelError::Invariant(_))
    ));

    let mut double_release = terminal_state();
    double_release.reservation_release_count = 2;
    assert!(matches!(
        double_release.validate(),
        Err(WithdrawalModelError::Invariant(_))
    ));
}

#[test]
fn state_actions_and_outcomes_serialize_deterministically_for_replay() {
    let actions = happy_actions();
    let actions_json = serde_json::to_string(&actions).expect("serialize actions");
    let decoded: Vec<WithdrawalModelAction> =
        serde_json::from_str(&actions_json).expect("decode actions");
    assert_eq!(decoded, actions);

    let mut first = WithdrawalModelState::default();
    let mut first_hashes = Vec::new();
    for action in &actions {
        first_hashes.push(first.apply(action).expect("model action").state_sha256);
    }
    let mut second = WithdrawalModelState::default();
    let mut second_hashes = Vec::new();
    for action in &decoded {
        second_hashes.push(second.apply(action).expect("model action").state_sha256);
    }
    assert_eq!(first_hashes, second_hashes);
    let state_json = serde_json::to_string(&first).expect("serialize state");
    assert_eq!(
        WithdrawalModelState::from_json(&state_json).expect("decode state"),
        first
    );
}

fn happy_actions() -> Vec<WithdrawalModelAction> {
    let mut actions = vec![
        WithdrawalModelAction::ObserveBurn {
            withdrawal_id: "withdrawal-1".to_owned(),
            nonce: 1,
        },
        WithdrawalModelAction::Assemble {
            epoch: 1,
            handoff: 0,
            proposal_hash: "proposal-1".to_owned(),
            selected_inputs: inputs(),
        },
        WithdrawalModelAction::Prepare,
        WithdrawalModelAction::Canonicalize,
        WithdrawalModelAction::Reserve {
            owner: "withdrawal-1".to_owned(),
            inputs: inputs(),
        },
        WithdrawalModelAction::Publish {
            state: ModelPublicState::Ready,
        },
        WithdrawalModelAction::Authorize {
            epoch: 1,
            transaction_id: "tx-1".to_owned(),
        },
        WithdrawalModelAction::Submit {
            transaction_id: "tx-1".to_owned(),
        },
        WithdrawalModelAction::Publish {
            state: ModelPublicState::Submitted,
        },
        WithdrawalModelAction::Include {
            transaction_id: "tx-1".to_owned(),
            height: 10,
            block_id: "block-10".to_owned(),
        },
        WithdrawalModelAction::Confirm {
            transaction_id: "tx-1".to_owned(),
            height: 10,
            block_id: "block-10".to_owned(),
        },
        WithdrawalModelAction::Publish {
            state: ModelPublicState::SequencerConfirmed,
        },
    ];
    for node_id in 0..5 {
        actions.push(WithdrawalModelAction::SettleKernel { node_id });
    }
    actions.extend([
        WithdrawalModelAction::RecordPayout,
        WithdrawalModelAction::ReleaseReservations {
            owner: "withdrawal-1".to_owned(),
            inputs: inputs(),
        },
        WithdrawalModelAction::Publish {
            state: ModelPublicState::Terminal,
        },
    ]);
    actions
}

fn prepare_burn(state: &mut WithdrawalModelState) {
    state
        .apply(&WithdrawalModelAction::ObserveBurn {
            withdrawal_id: "withdrawal-1".to_owned(),
            nonce: 1,
        })
        .expect("burn");
}

fn canonicalized_reserved_state() -> WithdrawalModelState {
    let mut state = WithdrawalModelState::default();
    for action in happy_actions().into_iter().take(6) {
        state.apply(&action).expect("model action");
    }
    assert_eq!(
        state.proposal.as_ref().expect("proposal").state,
        ModelProposalState::Canonicalized
    );
    state
}

fn confirmed_state() -> WithdrawalModelState {
    let mut state = WithdrawalModelState::default();
    for action in happy_actions().into_iter().take(12) {
        state.apply(&action).expect("model action");
    }
    assert!(state.confirmed);
    state
}

fn terminal_state() -> WithdrawalModelState {
    let mut state = WithdrawalModelState::default();
    for action in happy_actions() {
        state.apply(&action).expect("model action");
    }
    state
}

fn inputs() -> BTreeSet<ModelNoteName> {
    BTreeSet::from([note(1), note(2)])
}

fn note(seed: u64) -> ModelNoteName {
    ModelNoteName {
        first: format!("first-{seed}"),
        last: format!("last-{seed}"),
    }
}

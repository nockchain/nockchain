use std::collections::{BTreeMap, VecDeque};
use std::time::Duration;

use async_trait::async_trait;
use bridge_dev::nockchain_probe::{
    CanonicalRawTransactionFacts, NockchainInclusionFacts, NockchainInputSnapshotFacts,
    NockchainTransactionFacts, NoteNameFacts, SelectedInputNoteFacts,
};
use bridge_dev::settlement_oracle::{
    wait_for_terminal_withdrawal, ArithmeticEquationProof, BridgeKernelTerminalFacts, EquationTerm,
    ExactNicks, KernelFrontierFacts, PublicWithdrawalState, PublicWithdrawalTerminalFacts,
    ReservationTerminalFacts, SequencerTerminalFacts, SequencerTerminalState,
    SettlementConservationProof, SettlementOutputProof, TerminalChainSource, TerminalKernelSource,
    TerminalOracleError, TerminalOracleSources, TerminalPublicSource, TerminalReservationSource,
    TerminalSequencerSource, TerminalWithdrawalTarget, TimedTerminalFact,
};

#[tokio::test]
async fn independently_lagging_sources_converge_then_pass_two_stable_observations() {
    let target = target();
    let chain_good = timed(10, chain(100, "block-a", 105));
    let mut chain = ChainScript::new(
        vec![
            Ok(chain_good.clone()),
            Ok(timed(20, chain_good.facts.clone())),
            Ok(timed(30, chain_good.facts.clone())),
        ],
        timed(40, chain_good.facts.clone()),
    );
    let kernels_lag = timed(10, kernels(99, "block-old", true));
    let kernels_good = timed(20, kernels(100, "block-a", false));
    let mut kernels = KernelScript::new(
        vec![
            Ok(kernels_lag),
            Ok(kernels_good.clone()),
            Ok(timed(30, kernels_good.facts.clone())),
        ],
        timed(40, kernels_good.facts.clone()),
    );
    let sequencer_pending = timed(10, sequencer(SequencerTerminalState::Pending, None));
    let sequencer_good = timed(
        20,
        sequencer(SequencerTerminalState::Confirmed, Some((100, "block-a"))),
    );
    let mut sequencer = SequencerScript::new(
        vec![
            Ok(sequencer_pending),
            Ok(sequencer_good.clone()),
            Ok(timed(30, sequencer_good.facts.clone())),
        ],
        timed(40, sequencer_good.facts.clone()),
    );
    let reservations_lag = timed(10, reservations(0, vec![input_name()]));
    let reservations_good = timed(20, reservations(1, Vec::new()));
    let mut reservations = ReservationScript::new(
        vec![
            Ok(reservations_lag),
            Ok(reservations_good.clone()),
            Ok(timed(30, reservations_good.facts.clone())),
        ],
        timed(40, reservations_good.facts.clone()),
    );
    let public_pending = timed(10, public(PublicWithdrawalState::Pending, None));
    let public_good = timed(
        20,
        public(PublicWithdrawalState::Confirmed, Some((100, "block-a"))),
    );
    let mut public = PublicScript::new(
        vec![
            Ok(public_pending),
            Ok(public_good.clone()),
            Ok(timed(30, public_good.facts.clone())),
        ],
        timed(40, public_good.facts.clone()),
    );
    let mut sources = TerminalOracleSources {
        chain: &mut chain,
        kernels: &mut kernels,
        sequencer: &mut sequencer,
        reservations: &mut reservations,
        public: &mut public,
    };

    let proof = wait_for_terminal_withdrawal(
        &target,
        &passing_settlement(),
        &mut sources,
        Duration::from_secs(1),
        Duration::ZERO,
    )
    .await
    .expect("sources must converge");
    assert!(chain.calls() >= 3);
    assert_eq!(proof.chain.correlation_group, "scripted-terminal-fixture");
    assert_eq!(
        proof.sequencer.correlation_group,
        proof.public.correlation_group
    );
    assert_eq!(proof.stable_observations, 2);
    assert_eq!(proof.chain.facts.inclusion.block_id, "block-a");
    assert!(proof
        .kernels
        .facts
        .iter()
        .all(|kernel| !kernel.matching_unsettled_withdrawal));
    assert_eq!(proof.reservations.facts.release_count, 1);
    assert_eq!(proof.public.facts.state, PublicWithdrawalState::Confirmed);
    assert_eq!(
        serde_json::to_value(&proof).expect("terminal proof serializes")["schema_version"],
        2
    );
}

#[tokio::test]
async fn advancing_chain_and_kernel_heads_do_not_prevent_stability() {
    let target = target();
    let mut chain = ChainScript::new(
        vec![
            Ok(timed(10, chain(100, "block-a", 105))),
            Ok(timed(20, chain(100, "block-a", 106))),
        ],
        timed(30, chain(100, "block-a", 107)),
    );
    let mut kernels = KernelScript::new(
        vec![
            Ok(timed(10, kernels(100, "frontier-a", false))),
            Ok(timed(20, kernels(101, "frontier-b", false))),
        ],
        timed(30, kernels(102, "frontier-c", false)),
    );
    let confirmed = sequencer(SequencerTerminalState::Confirmed, Some((100, "block-a")));
    let mut sequencer = SequencerScript::new(
        vec![Ok(timed(10, confirmed.clone())), Ok(timed(20, confirmed.clone()))],
        timed(30, confirmed),
    );
    let released = reservations(1, Vec::new());
    let mut reservations = ReservationScript::new(
        vec![Ok(timed(10, released.clone())), Ok(timed(20, released.clone()))],
        timed(30, released),
    );
    let public_confirmed = public(PublicWithdrawalState::Confirmed, Some((100, "block-a")));
    let mut public = PublicScript::new(
        vec![Ok(timed(10, public_confirmed.clone())), Ok(timed(20, public_confirmed.clone()))],
        timed(30, public_confirmed),
    );
    let mut sources = TerminalOracleSources {
        chain: &mut chain,
        kernels: &mut kernels,
        sequencer: &mut sequencer,
        reservations: &mut reservations,
        public: &mut public,
    };

    let proof = wait_for_terminal_withdrawal(
        &target,
        &passing_settlement(),
        &mut sources,
        Duration::from_secs(1),
        Duration::ZERO,
    )
    .await
    .expect("monotonic head progress must preserve terminal stability");

    assert_eq!(proof.stable_observations, 2);
    assert_eq!(proof.chain.facts.tip_height, 106);
    assert!(proof
        .kernels
        .facts
        .iter()
        .all(|kernel| kernel.frontier.height == 101));
}

#[tokio::test]
async fn reinclusion_resets_stability_until_every_reference_converges_twice() {
    let target = target();
    let old_chain = timed(1, chain(100, "block-old", 103));
    let new_chain = timed(2, chain(101, "block-new", 104));
    let mut chain = ChainScript::new(
        vec![Ok(old_chain), Ok(new_chain.clone()), Ok(timed(3, new_chain.facts.clone()))],
        timed(4, new_chain.facts.clone()),
    );
    let old_kernels = timed(1, kernels(100, "block-old", false));
    let new_kernels = timed(2, kernels(101, "block-new", false));
    let mut kernels = KernelScript::new(
        vec![
            Ok(old_kernels),
            Ok(new_kernels.clone()),
            Ok(timed(3, new_kernels.facts.clone())),
        ],
        timed(4, new_kernels.facts.clone()),
    );
    let old_sequencer = timed(
        1,
        sequencer(SequencerTerminalState::Confirmed, Some((100, "block-old"))),
    );
    let new_sequencer = timed(
        2,
        sequencer(SequencerTerminalState::Confirmed, Some((101, "block-new"))),
    );
    let mut sequencer = SequencerScript::new(
        vec![
            Ok(old_sequencer),
            Ok(new_sequencer.clone()),
            Ok(timed(3, new_sequencer.facts.clone())),
        ],
        timed(4, new_sequencer.facts.clone()),
    );
    let good_reservations = timed(1, reservations(1, Vec::new()));
    let mut reservations = ReservationScript::new(
        vec![Ok(good_reservations.clone()), Ok(timed(2, good_reservations.facts.clone()))],
        timed(3, good_reservations.facts.clone()),
    );
    let old_public = timed(
        1,
        public(PublicWithdrawalState::Confirmed, Some((100, "block-old"))),
    );
    let new_public = timed(
        2,
        public(PublicWithdrawalState::Confirmed, Some((101, "block-new"))),
    );
    let mut public = PublicScript::new(
        vec![Ok(old_public), Ok(new_public.clone()), Ok(timed(3, new_public.facts.clone()))],
        timed(4, new_public.facts.clone()),
    );
    let mut sources = TerminalOracleSources {
        chain: &mut chain,
        kernels: &mut kernels,
        sequencer: &mut sequencer,
        reservations: &mut reservations,
        public: &mut public,
    };

    let proof = wait_for_terminal_withdrawal(
        &target,
        &passing_settlement(),
        &mut sources,
        Duration::from_secs(1),
        Duration::ZERO,
    )
    .await
    .expect("new inclusion must stabilize");
    assert_eq!(proof.chain.facts.inclusion.height, 101);
    assert_eq!(proof.chain.facts.inclusion.block_id, "block-new");
    assert!(chain.calls() >= 3);
}

#[tokio::test]
async fn stale_source_snapshots_never_complete_terminal_stability() -> Result<(), String> {
    let target = target();
    let mut chain = ChainScript::steady(timed(1, chain(100, "block-a", 105)));
    let mut kernels = KernelScript::steady(timed(1, kernels(100, "block-a", false)));
    let mut sequencer = SequencerScript::steady(timed(
        1,
        sequencer(SequencerTerminalState::Confirmed, Some((100, "block-a"))),
    ));
    let mut reservations = ReservationScript::steady(timed(1, reservations(1, Vec::new())));
    let mut public = PublicScript::steady(timed(
        1,
        public(PublicWithdrawalState::Confirmed, Some((100, "block-a"))),
    ));
    let mut sources = TerminalOracleSources {
        chain: &mut chain,
        kernels: &mut kernels,
        sequencer: &mut sequencer,
        reservations: &mut reservations,
        public: &mut public,
    };

    let error = wait_for_terminal_withdrawal(
        &target,
        &passing_settlement(),
        &mut sources,
        Duration::from_millis(2),
        Duration::ZERO,
    )
    .await
    .expect_err("stale source timestamps must not satisfy stability");
    match error {
        TerminalOracleError::Timeout { diagnostics, .. } => assert!(diagnostics
            .iter()
            .any(|line| line.contains("timestamp did not advance"))),
        other => return Err(format!("unexpected stale-source result: {other}")),
    }
    Ok(())
}

#[tokio::test]
async fn kernel_target_identity_mismatch_is_rejected_directly() {
    let target = target();
    let mut wrong_kernels = kernels(100, "block-a", false);
    wrong_kernels[3].target_withdrawal_id = "another-withdrawal".to_owned();
    let mut chain = ChainScript::steady(timed(1, chain(100, "block-a", 105)));
    let mut kernels = KernelScript::steady(timed(1, wrong_kernels));
    let mut sequencer = SequencerScript::steady(timed(
        1,
        sequencer(SequencerTerminalState::Confirmed, Some((100, "block-a"))),
    ));
    let mut reservations = ReservationScript::steady(timed(1, reservations(1, Vec::new())));
    let mut public = PublicScript::steady(timed(
        1,
        public(PublicWithdrawalState::Confirmed, Some((100, "block-a"))),
    ));
    let mut sources = TerminalOracleSources {
        chain: &mut chain,
        kernels: &mut kernels,
        sequencer: &mut sequencer,
        reservations: &mut reservations,
        public: &mut public,
    };

    assert!(matches!(
        wait_for_terminal_withdrawal(
            &target,
            &passing_settlement(),
            &mut sources,
            Duration::from_secs(1),
            Duration::ZERO,
        )
        .await,
        Err(TerminalOracleError::Mismatch {
            source_name: "kernels",
            ..
        })
    ));
}

#[tokio::test]
async fn indirect_confirmation_and_release_claims_are_rejected() {
    let mut indirect_sequencer =
        sequencer(SequencerTerminalState::Confirmed, Some((100, "block-a")));
    indirect_sequencer.confirmation_event_id = None;
    assert!(matches!(
        immediate_terminal_error(indirect_sequencer, reservations(1, Vec::new())).await,
        TerminalOracleError::Mismatch {
            source_name: "sequencer",
            ..
        }
    ));

    let mut indirect_release = reservations(1, Vec::new());
    indirect_release.release_event_ids.clear();
    assert!(matches!(
        immediate_terminal_error(
            sequencer(SequencerTerminalState::Confirmed, Some((100, "block-a"))),
            indirect_release,
        )
        .await,
        TerminalOracleError::Mismatch {
            source_name: "reservations",
            ..
        }
    ));
}

async fn immediate_terminal_error(
    sequencer_facts: SequencerTerminalFacts,
    reservation_facts: ReservationTerminalFacts,
) -> TerminalOracleError {
    let mut chain = ChainScript::steady(timed(1, chain(100, "block-a", 105)));
    let mut kernels = KernelScript::steady(timed(1, kernels(100, "block-a", false)));
    let mut sequencer = SequencerScript::steady(timed(1, sequencer_facts));
    let mut reservations = ReservationScript::steady(timed(1, reservation_facts));
    let mut public = PublicScript::steady(timed(
        1,
        public(PublicWithdrawalState::Confirmed, Some((100, "block-a"))),
    ));
    let mut sources = TerminalOracleSources {
        chain: &mut chain,
        kernels: &mut kernels,
        sequencer: &mut sequencer,
        reservations: &mut reservations,
        public: &mut public,
    };
    wait_for_terminal_withdrawal(
        &target(),
        &passing_settlement(),
        &mut sources,
        Duration::from_secs(1),
        Duration::ZERO,
    )
    .await
    .expect_err("indirect evidence must fail")
}

#[tokio::test]
async fn stale_kernel_reservation_leak_and_public_reference_mismatch_time_out() -> Result<(), String>
{
    for failure in [FailureMode::StaleKernel, FailureMode::ReservationLeak, FailureMode::PublicLag]
    {
        let error = run_failure(failure)
            .await
            .expect_err("permanent lag must not report terminal success");
        let rendered = error.to_string();
        match error {
            TerminalOracleError::Timeout { diagnostics, .. } => {
                let expected_source = match failure {
                    FailureMode::StaleKernel => "kernels",
                    FailureMode::ReservationLeak => "reservations",
                    FailureMode::PublicLag => "public",
                };
                assert!(diagnostics
                    .iter()
                    .any(|line| line.starts_with(expected_source)));
                assert!(
                    rendered.contains(expected_source),
                    "timeout display omitted {expected_source} diagnostics: {rendered}"
                );
            }
            other => return Err(format!("unexpected error: {other}")),
        }
    }
    Ok(())
}

#[tokio::test]
async fn reorg_hold_and_stopped_bridge_interrupt_wait() {
    let target = target();
    {
        let mut chain = ChainScript::steady(timed(1, chain(100, "block-a", 105)));
        let mut kernels = KernelScript::steady(timed(1, kernels(100, "block-a", false)));
        let mut sequencer = SequencerScript::steady(timed(
            1,
            sequencer(SequencerTerminalState::ReorgHold, Some((100, "block-a"))),
        ));
        let mut reservations = ReservationScript::steady(timed(1, reservations(1, Vec::new())));
        let mut public = PublicScript::steady(timed(
            1,
            public(PublicWithdrawalState::Confirmed, Some((100, "block-a"))),
        ));
        let mut sources = TerminalOracleSources {
            chain: &mut chain,
            kernels: &mut kernels,
            sequencer: &mut sequencer,
            reservations: &mut reservations,
            public: &mut public,
        };
        assert!(matches!(
            wait_for_terminal_withdrawal(
                &target,
                &passing_settlement(),
                &mut sources,
                Duration::from_secs(1),
                Duration::ZERO,
            )
            .await,
            Err(TerminalOracleError::ReorgHold {
                source_name: "sequencer",
                ..
            })
        ));
    }

    let mut stopped = kernels(100, "block-a", false);
    stopped[4].running = false;
    let mut chain = ChainScript::steady(timed(1, chain(100, "block-a", 105)));
    let mut kernels = KernelScript::steady(timed(1, stopped));
    let mut sequencer = SequencerScript::steady(timed(
        1,
        sequencer(SequencerTerminalState::Confirmed, Some((100, "block-a"))),
    ));
    let mut reservations = ReservationScript::steady(timed(1, reservations(1, Vec::new())));
    let mut public = PublicScript::steady(timed(
        1,
        public(PublicWithdrawalState::Confirmed, Some((100, "block-a"))),
    ));
    let mut sources = TerminalOracleSources {
        chain: &mut chain,
        kernels: &mut kernels,
        sequencer: &mut sequencer,
        reservations: &mut reservations,
        public: &mut public,
    };
    assert!(matches!(
        wait_for_terminal_withdrawal(
            &target,
            &passing_settlement(),
            &mut sources,
            Duration::from_secs(1),
            Duration::ZERO,
        )
        .await,
        Err(TerminalOracleError::KernelStoppedOrHeld { node_id: 4, .. })
    ));
}

#[tokio::test]
async fn transient_source_failure_and_unrelated_pending_withdrawals_do_not_block() {
    let target = target();
    let chain_good = timed(2, chain(100, "block-a", 105));
    let mut chain = ChainScript::new(
        vec![
            Err("temporary unavailable".to_owned()),
            Ok(chain_good.clone()),
            Ok(timed(3, chain_good.facts.clone())),
        ],
        timed(4, chain_good.facts.clone()),
    );
    let mut unrelated = kernels(100, "block-a", false);
    unrelated[2].other_unsettled_withdrawals = 7;
    let kernel_good = timed(2, unrelated);
    let mut kernels = KernelScript::new(
        vec![
            Err("bridge snapshot timeout".to_owned()),
            Ok(kernel_good.clone()),
            Ok(timed(3, kernel_good.facts.clone())),
        ],
        timed(4, kernel_good.facts.clone()),
    );
    let sequencer_good = timed(
        2,
        sequencer(SequencerTerminalState::Confirmed, Some((100, "block-a"))),
    );
    let mut sequencer = SequencerScript::new(
        vec![Ok(sequencer_good.clone()), Ok(timed(3, sequencer_good.facts.clone()))],
        timed(4, sequencer_good.facts.clone()),
    );
    let reservations_good = timed(2, reservations(1, Vec::new()));
    let mut reservations = ReservationScript::new(
        vec![Ok(reservations_good.clone()), Ok(timed(3, reservations_good.facts.clone()))],
        timed(4, reservations_good.facts.clone()),
    );
    let public_good = timed(
        2,
        public(PublicWithdrawalState::Confirmed, Some((100, "block-a"))),
    );
    let mut public = PublicScript::new(
        vec![Ok(public_good.clone()), Ok(timed(3, public_good.facts.clone()))],
        timed(4, public_good.facts.clone()),
    );
    let mut sources = TerminalOracleSources {
        chain: &mut chain,
        kernels: &mut kernels,
        sequencer: &mut sequencer,
        reservations: &mut reservations,
        public: &mut public,
    };
    let proof = wait_for_terminal_withdrawal(
        &target,
        &passing_settlement(),
        &mut sources,
        Duration::from_secs(1),
        Duration::ZERO,
    )
    .await
    .expect("transient failures must recover");
    assert!(proof
        .diagnostics
        .iter()
        .any(|line| line.contains("temporary unavailable")));
    assert_eq!(proof.kernels.facts[2].other_unsettled_withdrawals, 7);
}

#[derive(Clone, Copy)]
enum FailureMode {
    StaleKernel,
    ReservationLeak,
    PublicLag,
}

async fn run_failure(
    mode: FailureMode,
) -> Result<bridge_dev::settlement_oracle::TerminalWithdrawalProof, TerminalOracleError> {
    let target = target();
    let mut chain = ChainScript::steady(timed(1, chain(100, "block-a", 105)));
    let kernel_facts = match mode {
        FailureMode::StaleKernel => kernels(99, "block-old", true),
        _ => kernels(100, "block-a", false),
    };
    let mut kernels = KernelScript::steady(timed(1, kernel_facts));
    let mut sequencer = SequencerScript::steady(timed(
        1,
        sequencer(SequencerTerminalState::Confirmed, Some((100, "block-a"))),
    ));
    let reservation_facts = match mode {
        FailureMode::ReservationLeak => reservations(0, vec![input_name()]),
        _ => reservations(1, Vec::new()),
    };
    let mut reservations = ReservationScript::steady(timed(1, reservation_facts));
    let public_facts = match mode {
        FailureMode::PublicLag => public(PublicWithdrawalState::Confirmed, Some((99, "block-old"))),
        _ => public(PublicWithdrawalState::Confirmed, Some((100, "block-a"))),
    };
    let mut public = PublicScript::steady(timed(1, public_facts));
    let mut sources = TerminalOracleSources {
        chain: &mut chain,
        kernels: &mut kernels,
        sequencer: &mut sequencer,
        reservations: &mut reservations,
        public: &mut public,
    };
    wait_for_terminal_withdrawal(
        &target,
        &passing_settlement(),
        &mut sources,
        Duration::from_millis(2),
        Duration::ZERO,
    )
    .await
}

fn target() -> TerminalWithdrawalTarget {
    TerminalWithdrawalTarget {
        withdrawal_id: "withdrawal-1".to_owned(),
        withdrawal_nonce: 7,
        base_event_id: "base-event-1".to_owned(),
        transaction_id: "nock-tx-1".to_owned(),
        confirmation_depth: 3,
        reserved_inputs: vec![input_name()],
    }
}

fn chain(height: u64, block_id: &str, tip: u64) -> NockchainTransactionFacts {
    NockchainTransactionFacts {
        transaction_id: "nock-tx-1".to_owned(),
        inclusion: NockchainInclusionFacts {
            height,
            block_id: block_id.to_owned(),
        },
        tip_height: tip,
        confirmation_depth: tip - height,
        inclusion_history: vec![NockchainInclusionFacts {
            height,
            block_id: block_id.to_owned(),
        }],
        raw_transaction: CanonicalRawTransactionFacts {
            version: 1,
            embedded_transaction_id: "nock-tx-1".to_owned(),
            computed_transaction_id: "nock-tx-1".to_owned(),
            size_bytes: 1,
            spends: Vec::new(),
        },
        input_snapshot: NockchainInputSnapshotFacts {
            height: height - 1,
            block_id: "snapshot".to_owned(),
        },
        selected_inputs: vec![SelectedInputNoteFacts {
            name: input_name(),
            note_version: 1,
            assets_nicks: 2,
            origin_height: height - 1,
            origin_transaction_id: Some("origin-tx".to_owned()),
            origin_is_coinbase: Some(false),
        }],
        outputs: Vec::new(),
        transaction_fee_nicks: 1,
        total_input_nicks: 2,
        total_output_nicks: 1,
        unaccounted_nicks: 0,
        matching_recipient_output_indices: vec![0],
    }
}

fn kernels(
    height: u64,
    block_id: &str,
    matching_unsettled: bool,
) -> Vec<BridgeKernelTerminalFacts> {
    (0..5)
        .map(|node_id| BridgeKernelTerminalFacts {
            node_id,
            available: true,
            running: true,
            hold_reason: None,
            target_withdrawal_id: "withdrawal-1".to_owned(),
            target_base_event_id: "base-event-1".to_owned(),
            frontier: KernelFrontierFacts {
                height,
                block_id: block_id.to_owned(),
            },
            matching_unsettled_withdrawal: matching_unsettled,
            other_unsettled_withdrawals: 0,
        })
        .collect()
}

fn sequencer(
    state: SequencerTerminalState,
    reference: Option<(u64, &str)>,
) -> SequencerTerminalFacts {
    SequencerTerminalFacts {
        withdrawal_id: "withdrawal-1".to_owned(),
        withdrawal_nonce: 7,
        transaction_id: Some("nock-tx-1".to_owned()),
        confirmation_event_id: reference.map(|_| "confirmation-event-1".to_owned()),
        state,
        confirmed_height: reference.map(|(height, _)| height),
        confirmed_block_id: reference.map(|(_, block_id)| block_id.to_owned()),
    }
}

fn reservations(
    release_count: u64,
    currently_reserved_inputs: Vec<NoteNameFacts>,
) -> ReservationTerminalFacts {
    ReservationTerminalFacts {
        withdrawal_id: "withdrawal-1".to_owned(),
        tracked_inputs: vec![input_name()],
        release_event_ids: (0..release_count)
            .map(|index| format!("release-event-{index}"))
            .collect(),
        currently_reserved_inputs,
        release_count,
    }
}

fn public(
    state: PublicWithdrawalState,
    reference: Option<(u64, &str)>,
) -> PublicWithdrawalTerminalFacts {
    PublicWithdrawalTerminalFacts {
        withdrawal_id: "withdrawal-1".to_owned(),
        withdrawal_nonce: 7,
        state,
        base_event_id: "base-event-1".to_owned(),
        transaction_id: Some("nock-tx-1".to_owned()),
        confirmed_height: reference.map(|(height, _)| height),
        confirmed_block_id: reference.map(|(_, block_id)| block_id.to_owned()),
    }
}

fn input_name() -> NoteNameFacts {
    NoteNameFacts {
        first: "input-first".to_owned(),
        last: "input-last".to_owned(),
    }
}

fn passing_settlement() -> SettlementConservationProof {
    let recipient = SettlementOutputProof {
        index: 0,
        name: NoteNameFacts {
            first: "recipient-first".to_owned(),
            last: "recipient-last".to_owned(),
        },
        lock_root: "recipient-root".to_owned(),
        assets_nicks: ExactNicks("80".to_owned()),
        note_version: 1,
        origin_height: 100,
        origin_transaction_id: "nock-tx-1".to_owned(),
    };
    SettlementConservationProof {
        schema_version: 1,
        base_event_id: "base-event-1".to_owned(),
        nock_transaction_id: "nock-tx-1".to_owned(),
        gross_burn_nicks: ExactNicks("100".to_owned()),
        bridge_fee_nicks: ExactNicks("10".to_owned()),
        transaction_fee_nicks: ExactNicks("10".to_owned()),
        recipient_payout_nicks: ExactNicks("80".to_owned()),
        recipient_lock_root: "recipient-root".to_owned(),
        recipient_output: recipient,
        change_outputs: Vec::new(),
        change_total_nicks: ExactNicks("0".to_owned()),
        total_input_nicks: ExactNicks("90".to_owned()),
        total_output_nicks: ExactNicks("80".to_owned()),
        input_note_version_counts: BTreeMap::from([(1, 1)]),
        transaction_conservation: equation("transaction_conservation", "90", "80", "10"),
        burn_to_payout: ArithmeticEquationProof {
            name: "burn_to_payout".to_owned(),
            left: EquationTerm {
                name: "gross_burn_nicks".to_owned(),
                nicks: ExactNicks("100".to_owned()),
            },
            right_terms: vec![
                term("bridge_fee_nicks", "10"),
                term("transaction_fee_nicks", "10"),
                term("recipient_payout_nicks", "80"),
            ],
            right_total: ExactNicks("100".to_owned()),
            verdict: true,
        },
    }
}

fn equation(name: &str, left: &str, output: &str, fee: &str) -> ArithmeticEquationProof {
    ArithmeticEquationProof {
        name: name.to_owned(),
        left: EquationTerm {
            name: "total_input_nicks".to_owned(),
            nicks: ExactNicks(left.to_owned()),
        },
        right_terms: vec![term("total_output_nicks", output), term("transaction_fee_nicks", fee)],
        right_total: ExactNicks(left.to_owned()),
        verdict: true,
    }
}

fn term(name: &str, value: &str) -> EquationTerm {
    EquationTerm {
        name: name.to_owned(),
        nicks: ExactNicks(value.to_owned()),
    }
}

fn timed<T>(observed_unix_ms: u64, facts: T) -> TimedTerminalFact<T> {
    TimedTerminalFact {
        observed_unix_ms,
        source_name: "scripted-terminal-source".to_owned(),
        correlation_group: "scripted-terminal-fixture".to_owned(),
        facts,
    }
}

struct Script<T: Clone> {
    values: VecDeque<Result<T, String>>,
    repeat: T,
    calls: usize,
}

impl<T: Clone> Script<T> {
    fn new(values: Vec<Result<T, String>>, repeat: T) -> Self {
        Self {
            values: values.into(),
            repeat,
            calls: 0,
        }
    }

    fn steady(repeat: T) -> Self {
        Self::new(vec![Ok(repeat.clone())], repeat)
    }

    fn next(&mut self) -> Result<T, String> {
        self.calls += 1;
        self.values
            .pop_front()
            .unwrap_or_else(|| Ok(self.repeat.clone()))
    }
}

struct ChainScript(Script<TimedTerminalFact<NockchainTransactionFacts>>);
struct KernelScript(Script<TimedTerminalFact<Vec<BridgeKernelTerminalFacts>>>);
struct SequencerScript(Script<TimedTerminalFact<SequencerTerminalFacts>>);
struct ReservationScript(Script<TimedTerminalFact<ReservationTerminalFacts>>);
struct PublicScript(Script<TimedTerminalFact<PublicWithdrawalTerminalFacts>>);

macro_rules! script_constructors {
    ($type:ident, $facts:ty) => {
        impl $type {
            fn new(values: Vec<Result<$facts, String>>, repeat: $facts) -> Self {
                Self(Script::new(values, repeat))
            }

            fn steady(repeat: $facts) -> Self {
                Self(Script::steady(repeat))
            }
        }
    };
}

script_constructors!(ChainScript, TimedTerminalFact<NockchainTransactionFacts>);
script_constructors!(
    KernelScript,
    TimedTerminalFact<Vec<BridgeKernelTerminalFacts>>
);
script_constructors!(SequencerScript, TimedTerminalFact<SequencerTerminalFacts>);
script_constructors!(
    ReservationScript,
    TimedTerminalFact<ReservationTerminalFacts>
);
script_constructors!(
    PublicScript,
    TimedTerminalFact<PublicWithdrawalTerminalFacts>
);

impl ChainScript {
    fn calls(&self) -> usize {
        self.0.calls
    }
}

#[async_trait]
impl TerminalChainSource for ChainScript {
    async fn observe_chain(
        &mut self,
    ) -> Result<TimedTerminalFact<NockchainTransactionFacts>, String> {
        self.0.next()
    }
}

#[async_trait]
impl TerminalKernelSource for KernelScript {
    async fn observe_kernels(
        &mut self,
    ) -> Result<TimedTerminalFact<Vec<BridgeKernelTerminalFacts>>, String> {
        self.0.next()
    }
}

#[async_trait]
impl TerminalSequencerSource for SequencerScript {
    async fn observe_sequencer(
        &mut self,
    ) -> Result<TimedTerminalFact<SequencerTerminalFacts>, String> {
        self.0.next()
    }
}

#[async_trait]
impl TerminalReservationSource for ReservationScript {
    async fn observe_reservations(
        &mut self,
    ) -> Result<TimedTerminalFact<ReservationTerminalFacts>, String> {
        self.0.next()
    }
}

#[async_trait]
impl TerminalPublicSource for PublicScript {
    async fn observe_public(
        &mut self,
    ) -> Result<TimedTerminalFact<PublicWithdrawalTerminalFacts>, String> {
        self.0.next()
    }
}

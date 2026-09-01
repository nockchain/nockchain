use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use alloy::primitives::{Address, B256, U256};
use bridge::shared::base::{
    encode_withdrawal_burn_calldata, parse_withdrawal_burn_calldata, BurnForWithdrawalDecodeError,
};
use bridge::shared::types::{Tip5Hash, WithdrawalPolicy};
use bridge_dev::base_backend::TransactionReceiptFacts;
use bridge_dev::client_driver::{ClientEncodingProof, WithdrawalClientMode};
use bridge_dev::e2e::{
    normalize_core_withdrawal_evidence, ordinary_burn_calldata, CoreWithdrawalEvidence,
    CoreWithdrawalPhase, CoreWithdrawalPrerequisites, CoreWithdrawalProgress,
};
use bridge_dev::iris_artifact::IrisArtifactFacts;
use bridge_dev::iris_driver::{BurnEventProof, BurnSubmissionProof};
use bridge_dev::nockchain_probe::{
    CanonicalRawTransactionFacts, NockchainInclusionFacts, NockchainInputSnapshotFacts,
    NockchainTransactionFacts, NoteNameFacts, SelectedInputNoteFacts,
};
use bridge_dev::scenario::{
    core_withdrawal_amount_nicks, core_withdrawal_amount_nocks, LocalBaseEnvironment,
    ScenarioHarness,
};
use bridge_dev::settlement_oracle::{
    ArithmeticEquationProof, BridgeKernelTerminalFacts, EquationTerm, ExactNicks,
    KernelFrontierFacts, PublicWithdrawalState, PublicWithdrawalTerminalFacts,
    ReservationTerminalFacts, SequencerTerminalFacts, SequencerTerminalState,
    SettlementConservationProof, SettlementOutputProof, TerminalWithdrawalProof,
    TerminalWithdrawalTarget, TimedTerminalFact,
};

#[test]
fn local_cluster_environment_is_typed_loopback_and_write_once() {
    let run_dir = preserved_run_dir("local-env");
    fs::create_dir_all(&run_dir).expect("create preserved run dir");
    let harness = ScenarioHarness::for_e2e_run(
        "local-env",
        PathBuf::from("/workspace"),
        PathBuf::from("/workspace/bridge-dev"),
        &run_dir,
    )
    .expect("construct local scenario");
    let environment = LocalBaseEnvironment {
        http_url: "http://127.0.0.1:49111".to_owned(),
        ws_url: "ws://127.0.0.1:49111".to_owned(),
        chain_id: 31_338,
        start_height: 7,
        inbox_contract: "0x0000000000000000000000000000000000000001".to_owned(),
        nock_contract: "0x0000000000000000000000000000000000000002".to_owned(),
    };
    let path = harness
        .write_local_base_environment(&environment)
        .expect("write local environment");
    let contents = fs::read_to_string(path).expect("read local environment");
    assert!(contents.contains("BASE_CHAIN_ID=\"31338\""));
    assert!(contents.contains("BASE_RPC_URL=\"http://127.0.0.1:49111\""));
    assert!(harness.write_local_base_environment(&environment).is_err());

    let mut wrong_chain = environment;
    wrong_chain.chain_id = 84_532;
    assert!(harness.write_local_base_environment(&wrong_chain).is_err());
}

#[test]
fn active_policy_derives_admissible_amount_and_never_regresses_to_1001_nock() {
    let policy = WithdrawalPolicy::v1();
    let amount_nocks = core_withdrawal_amount_nocks(&policy, 1).expect("policy amount");
    let amount_nicks = core_withdrawal_amount_nicks(&policy, 1).expect("policy nicks");
    assert_eq!(amount_nocks, policy.minimum_gross_nocks + 1);
    assert_eq!(amount_nicks, amount_nocks * policy.nicks_per_nock);
    assert_ne!(amount_nocks, 1_001);

    let mut changed_policy = policy;
    changed_policy.minimum_gross_nocks = 123_456;
    assert_eq!(
        core_withdrawal_amount_nocks(&changed_policy, 2).expect("changed policy amount"),
        123_458
    );
}

#[test]
fn core_progress_requires_every_named_phase_official_iris_and_terminal_proof() {
    let evidence = evidence();
    let mut progress = CoreWithdrawalProgress::default();
    for phase in [
        CoreWithdrawalPhase::Pending,
        CoreWithdrawalPhase::Ready,
        CoreWithdrawalPhase::Submitted,
        CoreWithdrawalPhase::SequencerConfirmed,
        CoreWithdrawalPhase::Terminal,
    ] {
        progress.record(phase).expect("phase order");
    }
    let execution = progress
        .finish(evidence.clone())
        .expect("terminal execution");
    assert_eq!(execution.steps_executed, 5);
    assert!(execution.facts.get("terminal").is_some());

    let mut missing_phase = CoreWithdrawalProgress::default();
    missing_phase
        .record(CoreWithdrawalPhase::Pending)
        .expect("pending phase");
    assert!(missing_phase.finish(evidence.clone()).is_err());
    assert!(CoreWithdrawalProgress::default()
        .finish(evidence.clone())
        .is_err());

    let mut wrong_order = CoreWithdrawalProgress::default();
    assert!(wrong_order.record(CoreWithdrawalPhase::Ready).is_err());

    let mut rust_fallback = evidence;
    rust_fallback.burn.client.official_client = false;
    rust_fallback.burn.client.client_mode = WithdrawalClientMode::RustReference;
    rust_fallback.burn.client.artifact = None;
    let progress = complete_progress();
    assert!(progress.finish(rust_fallback).is_err());
}

#[test]
fn reset_runs_have_equivalent_normalized_terminal_facts() {
    let first = evidence();
    let mut second = first.clone();
    second.burn.transaction_hash = B256::repeat_byte(9);
    second.burn.block_number = 999;
    second.burn.receipt.transaction_hash = second.burn.transaction_hash;
    second.burn.receipt.block_number = 999;
    second.terminal.chain.observed_unix_ms = 9_999;
    second.terminal.public.observed_unix_ms = 10_000;

    assert_eq!(
        normalize_core_withdrawal_evidence(&first).expect("first normalized facts"),
        normalize_core_withdrawal_evidence(&second).expect("second normalized facts")
    );
}

#[test]
fn ordinary_68_byte_burn_is_an_isolated_negative_wire_case() {
    let contract_address = address(1);
    let burner = address(2);
    let root = Tip5Hash::from_limbs(&[1, 2, 3, 4, 5]);
    let full = encode_withdrawal_burn_calldata(contract_address, burner, U256::from(10_u64), &root);
    let ordinary = ordinary_burn_calldata(&full).expect("ordinary calldata");
    assert_eq!(ordinary.len(), 68);
    assert!(matches!(
        parse_withdrawal_burn_calldata(&ordinary),
        Err(BurnForWithdrawalDecodeError::MissingCalldataTrailer { actual_len: 68 })
    ));
    assert_eq!(full.len(), 116);
}

#[test]
fn missing_local_prerequisites_fail_instead_of_skipping() {
    assert!(CoreWithdrawalPrerequisites {
        anvil: false,
        bridge_artifacts: true,
        iris_artifact: true,
    }
    .require()
    .is_err());
    assert!(CoreWithdrawalPrerequisites {
        anvil: true,
        bridge_artifacts: false,
        iris_artifact: true,
    }
    .require()
    .is_err());
    assert!(CoreWithdrawalPrerequisites {
        anvil: true,
        bridge_artifacts: true,
        iris_artifact: false,
    }
    .require()
    .is_err());
    CoreWithdrawalPrerequisites {
        anvil: true,
        bridge_artifacts: true,
        iris_artifact: true,
    }
    .require()
    .expect("all prerequisites");
}

fn complete_progress() -> CoreWithdrawalProgress {
    let mut progress = CoreWithdrawalProgress::default();
    for phase in [
        CoreWithdrawalPhase::Pending,
        CoreWithdrawalPhase::Ready,
        CoreWithdrawalPhase::Submitted,
        CoreWithdrawalPhase::SequencerConfirmed,
        CoreWithdrawalPhase::Terminal,
    ] {
        progress.record(phase).expect("phase order");
    }
    progress
}

fn evidence() -> CoreWithdrawalEvidence {
    let base_event_id = "0xbase-event".to_owned();
    let nock_tx = "nock-tx-1".to_owned();
    let client = client_proof();
    let event = BurnEventProof {
        nock_token: format!("{:#x}", address(1)),
        burner: format!("{:#x}", address(2)),
        amount_base_units: "1000000000000000000000".to_owned(),
        amount_nicks: "6553600000".to_owned(),
        commitment: format!("{:#x}", B256::repeat_byte(3)),
        lock_root: "recipient-root".to_owned(),
        log_index: 0,
        base_event_id: base_event_id.clone(),
    };
    let receipt = TransactionReceiptFacts {
        transaction_hash: B256::repeat_byte(4),
        block_number: 10,
        success: true,
        contract_address: None,
        logs: Vec::new(),
    };
    let burn = BurnSubmissionProof {
        transaction_hash: receipt.transaction_hash,
        block_number: receipt.block_number,
        mined_from: address(2),
        mined_to: address(1),
        mined_input_hex: client.calldata_hex.clone(),
        client,
        event,
        receipt,
    };
    let settlement = settlement(&base_event_id, &nock_tx);
    let target = TerminalWithdrawalTarget {
        withdrawal_id: "withdrawal-1".to_owned(),
        withdrawal_nonce: 7,
        base_event_id: base_event_id.clone(),
        transaction_id: nock_tx.clone(),
        confirmation_depth: 3,
        reserved_inputs: vec![input_name()],
    };
    let chain = nockchain_transaction(&nock_tx);
    CoreWithdrawalEvidence {
        burn,
        terminal: TerminalWithdrawalProof {
            schema_version: 2,
            target,
            settlement,
            chain: timed(100, chain),
            kernels: timed(101, kernels(&base_event_id)),
            sequencer: timed(
                102,
                SequencerTerminalFacts {
                    withdrawal_id: "withdrawal-1".to_owned(),
                    withdrawal_nonce: 7,
                    transaction_id: Some(nock_tx.clone()),
                    state: SequencerTerminalState::Confirmed,
                    confirmation_event_id: Some("confirmation-event-1".to_owned()),
                    confirmed_height: Some(100),
                    confirmed_block_id: Some("nock-block-1".to_owned()),
                },
            ),
            reservations: timed(
                103,
                ReservationTerminalFacts {
                    withdrawal_id: "withdrawal-1".to_owned(),
                    tracked_inputs: vec![input_name()],
                    release_event_ids: vec!["release-event-1".to_owned()],
                    currently_reserved_inputs: Vec::new(),
                    release_count: 1,
                },
            ),
            public: timed(
                104,
                PublicWithdrawalTerminalFacts {
                    withdrawal_id: "withdrawal-1".to_owned(),
                    withdrawal_nonce: 7,
                    state: PublicWithdrawalState::Confirmed,
                    base_event_id,
                    transaction_id: Some(nock_tx),
                    confirmed_height: Some(100),
                    confirmed_block_id: Some("nock-block-1".to_owned()),
                },
            ),
            stable_observations: 2,
            diagnostics: Vec::new(),
        },
    }
}

fn client_proof() -> ClientEncodingProof {
    ClientEncodingProof {
        client_mode: WithdrawalClientMode::IrisSdk,
        official_client: true,
        wire_protocol: "WithdrawalWireV1".to_owned(),
        withdrawal_policy: "withdrawal-policy-v1".to_owned(),
        nock_token: format!("{:#x}", address(1)),
        burner: format!("{:#x}", address(2)),
        amount_base_units: "1000000000000000000000".to_owned(),
        amount_nicks: "6553600000".to_owned(),
        bridge_fee_nicks: "19500000".to_owned(),
        net_after_bridge_fee_nicks: "6534100000".to_owned(),
        destination_kind: "lock_root".to_owned(),
        destination_value: "recipient-root".to_owned(),
        lock_root: "recipient-root".to_owned(),
        lock_root_limbs: ["1", "2", "3", "4", "5"].map(str::to_owned),
        commitment: format!("{:#x}", B256::repeat_byte(3)),
        calldata_hex: format!("0x{}", "11".repeat(116)),
        calldata_byte_length: 116,
        artifact: Some(IrisArtifactFacts {
            package_name: "@nockbox/iris-sdk".to_owned(),
            package_version: "0.3.0".to_owned(),
            git_revision: "a".repeat(40),
            tarball_path: PathBuf::from("/volatile/iris.tgz"),
            tarball_sha256: "b".repeat(64),
            npm_integrity: "sha512-fixture".to_owned(),
            npm_shasum: "c".repeat(40),
            files: Vec::new(),
            packed_node_version: "v26.0.0".to_owned(),
            packed_npm_version: "11.0.0".to_owned(),
            runtime_node_version: "v26.0.0".to_owned(),
            driver_path: PathBuf::from("/volatile/driver.js"),
        }),
    }
}

fn settlement(base_event_id: &str, nock_tx: &str) -> SettlementConservationProof {
    SettlementConservationProof {
        schema_version: 1,
        base_event_id: base_event_id.to_owned(),
        nock_transaction_id: nock_tx.to_owned(),
        gross_burn_nicks: ExactNicks("100".to_owned()),
        bridge_fee_nicks: ExactNicks("10".to_owned()),
        transaction_fee_nicks: ExactNicks("10".to_owned()),
        recipient_payout_nicks: ExactNicks("80".to_owned()),
        recipient_lock_root: "recipient-root".to_owned(),
        recipient_output: SettlementOutputProof {
            index: 0,
            name: NoteNameFacts {
                first: "recipient-first".to_owned(),
                last: "recipient-last".to_owned(),
            },
            lock_root: "recipient-root".to_owned(),
            assets_nicks: ExactNicks("80".to_owned()),
            note_version: 1,
            origin_height: 100,
            origin_transaction_id: nock_tx.to_owned(),
        },
        change_outputs: Vec::new(),
        change_total_nicks: ExactNicks("0".to_owned()),
        total_input_nicks: ExactNicks("90".to_owned()),
        total_output_nicks: ExactNicks("80".to_owned()),
        input_note_version_counts: BTreeMap::from([(1, 1)]),
        transaction_conservation: equation(
            "transaction_conservation",
            "total_input_nicks",
            "90",
            [("total_output_nicks", "80"), ("transaction_fee_nicks", "10")],
        ),
        burn_to_payout: equation(
            "burn_to_payout",
            "gross_burn_nicks",
            "100",
            [
                ("bridge_fee_nicks", "10"),
                ("transaction_fee_nicks", "10"),
                ("recipient_payout_nicks", "80"),
            ],
        ),
    }
}

fn equation<const N: usize>(
    name: &str,
    left_name: &str,
    left: &str,
    right: [(&str, &str); N],
) -> ArithmeticEquationProof {
    ArithmeticEquationProof {
        name: name.to_owned(),
        left: EquationTerm {
            name: left_name.to_owned(),
            nicks: ExactNicks(left.to_owned()),
        },
        right_terms: right
            .into_iter()
            .map(|(name, value)| EquationTerm {
                name: name.to_owned(),
                nicks: ExactNicks(value.to_owned()),
            })
            .collect(),
        right_total: ExactNicks(left.to_owned()),
        verdict: true,
    }
}

fn nockchain_transaction(transaction_id: &str) -> NockchainTransactionFacts {
    NockchainTransactionFacts {
        transaction_id: transaction_id.to_owned(),
        inclusion: NockchainInclusionFacts {
            height: 100,
            block_id: "nock-block-1".to_owned(),
        },
        tip_height: 103,
        confirmation_depth: 3,
        inclusion_history: vec![NockchainInclusionFacts {
            height: 100,
            block_id: "nock-block-1".to_owned(),
        }],
        raw_transaction: CanonicalRawTransactionFacts {
            version: 1,
            embedded_transaction_id: transaction_id.to_owned(),
            computed_transaction_id: transaction_id.to_owned(),
            size_bytes: 1,
            spends: Vec::new(),
        },
        input_snapshot: NockchainInputSnapshotFacts {
            height: 99,
            block_id: "snapshot".to_owned(),
        },
        selected_inputs: vec![SelectedInputNoteFacts {
            name: input_name(),
            note_version: 1,
            assets_nicks: 90,
            origin_height: 90,
            origin_transaction_id: "origin".to_owned(),
            origin_is_coinbase: false,
        }],
        outputs: Vec::new(),
        transaction_fee_nicks: 10,
        total_input_nicks: 90,
        total_output_nicks: 80,
        unaccounted_nicks: 0,
        matching_recipient_output_indices: vec![0],
    }
}

fn kernels(base_event_id: &str) -> Vec<BridgeKernelTerminalFacts> {
    (0..5)
        .map(|node_id| BridgeKernelTerminalFacts {
            node_id,
            available: true,
            running: true,
            target_withdrawal_id: "withdrawal-1".to_owned(),
            target_base_event_id: base_event_id.to_owned(),
            hold_reason: None,
            frontier: KernelFrontierFacts {
                height: 100,
                block_id: "nock-block-1".to_owned(),
            },
            matching_unsettled_withdrawal: false,
            other_unsettled_withdrawals: 0,
        })
        .collect()
}

fn input_name() -> NoteNameFacts {
    NoteNameFacts {
        first: "input-first".to_owned(),
        last: "input-last".to_owned(),
    }
}

fn timed<T>(observed_unix_ms: u64, facts: T) -> TimedTerminalFact<T> {
    TimedTerminalFact {
        observed_unix_ms,
        source_name: "core-e2e-source".to_owned(),
        correlation_group: "core-e2e-fixture".to_owned(),
        facts,
    }
}

fn address(seed: u64) -> Address {
    let mut bytes = [0_u8; 20];
    bytes[12..].copy_from_slice(&seed.to_be_bytes());
    Address::from(bytes)
}

fn preserved_run_dir(label: &str) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock follows Unix epoch")
        .as_nanos();
    std::env::temp_dir().join(format!(
        "nockbridge-core-withdrawal-{label}-{}-{nanos}",
        std::process::id()
    ))
}

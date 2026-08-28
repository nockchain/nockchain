use std::collections::BTreeMap;
use std::path::PathBuf;

use alloy::primitives::{Address, B256};
use bridge_dev::artifacts::{ArtifactBuildMetadata, ArtifactFile, ArtifactRole, E2eArtifacts};
use bridge_dev::base_backend::TransactionReceiptFacts;
use bridge_dev::client_driver::{ClientEncodingProof, WithdrawalClientMode};
use bridge_dev::evidence::{
    EvidenceArtifacts, EvidenceAssertion, EvidenceDeploymentFacts, EvidenceEnvironmentFacts,
    EvidenceEnvironmentMode, EvidenceFrontier, EvidenceKernelFacts, EvidenceNockNote,
    EvidenceNockchainFacts, EvidenceOverrideFacts, EvidenceRunFacts, EvidenceRunStatus,
    EvidenceSequencerFacts, EvidenceStep, EvidenceTerminalFacts, ExternalArtifactReference,
    RedactionDeclaration, WithdrawalEvidenceCapsuleV1, WITHDRAWAL_EVIDENCE_SCHEMA_ID,
    WITHDRAWAL_EVIDENCE_SCHEMA_VERSION,
};
use bridge_dev::iris_artifact::IrisArtifactFacts;
use bridge_dev::iris_driver::{BurnEventProof, BurnSubmissionProof};
use bridge_dev::nockchain_probe::NoteNameFacts;
use bridge_dev::settlement_oracle::{
    ArithmeticEquationProof, BridgeKernelTerminalFacts, EquationTerm, ExactNicks,
    KernelFrontierFacts, PublicWithdrawalState, PublicWithdrawalTerminalFacts,
    ReservationTerminalFacts, SequencerTerminalFacts, SequencerTerminalState,
    SettlementConservationProof, SettlementOutputProof, TimedTerminalFact,
};
use serde_json::json;

#[test]
fn partial_failure_round_trips_and_unknown_schema_or_fields_fail_closed() {
    let mut capsule = partial_capsule();
    let hash = capsule
        .seal_normalized_hash()
        .expect("seal normalized hash");
    assert_eq!(hash.len(), 64);
    let json = serde_json::to_string_pretty(&capsule).expect("serialize capsule");
    let decoded = WithdrawalEvidenceCapsuleV1::from_json(&json).expect("round trip capsule");
    assert_eq!(decoded, capsule);

    let mut wrong_version: serde_json::Value = serde_json::from_str(&json).expect("json value");
    wrong_version["schema_version"] = json!(99);
    assert!(WithdrawalEvidenceCapsuleV1::from_json(&wrong_version.to_string()).is_err());

    let mut unknown_field: serde_json::Value = serde_json::from_str(&json).expect("json value");
    unknown_field["unexpected"] = json!(true);
    assert!(WithdrawalEvidenceCapsuleV1::from_json(&unknown_field.to_string()).is_err());
}

#[test]
fn full_capsule_has_distinct_sections_and_lossless_financial_strings() {
    let mut capsule = full_capsule();
    capsule.validate().expect("full capsule validates");
    capsule
        .seal_normalized_hash()
        .expect("seal normalized hash");
    let value = serde_json::to_value(&capsule).expect("serialize full capsule");

    assert_eq!(value["schema_id"], WITHDRAWAL_EVIDENCE_SCHEMA_ID);
    assert_eq!(value["schema_version"], WITHDRAWAL_EVIDENCE_SCHEMA_VERSION);
    for section in [
        "artifacts", "deployment", "base", "sequencer", "nockchain", "kernels", "public",
        "conservation", "terminal", "redaction",
    ] {
        assert!(!value[section].is_null(), "missing section {section}");
    }
    assert_eq!(
        value["nockchain"]["total_input_nicks"],
        "18446744073709551615"
    );
    assert!(value["nockchain"]["total_input_nicks"].is_string());
    assert!(value["base"]["event"]["amount_base_units"].is_string());
    assert!(value["conservation"]["gross_burn_nicks"].is_string());

    let round_trip = WithdrawalEvidenceCapsuleV1::from_json(&value.to_string())
        .expect("full capsule round trip");
    assert_eq!(round_trip, capsule);
}

#[test]
fn normalization_removes_only_declared_volatile_fields() {
    let first = full_capsule();
    let mut second = first.clone();
    second.run.run_id = "different-run".to_owned();
    second.run.started_at_unix_ms += 10_000;
    second.run.finished_at_unix_ms = Some(99_999);
    second.steps[0].started_at_unix_ms += 5_000;
    second.steps[0].finished_at_unix_ms += 5_000;
    second.steps[0].duration_ms = 999;
    second.steps[0].detail = Some(json!({
        "run_id": "volatile",
        "duration_ms": 999,
        "semantic": "kept",
    }));
    first_semantic_detail(&mut second);
    second
        .artifacts
        .as_mut()
        .expect("artifacts")
        .bridge_runtime
        .bridge
        .path = PathBuf::from("/another/run/bridge");
    second
        .artifacts
        .as_mut()
        .expect("artifacts")
        .iris
        .as_mut()
        .expect("iris")
        .driver_path = PathBuf::from("/another/run/driver");
    second.external_artifacts[0].path = "/another/run/trace.zip".to_owned();
    second
        .sequencer
        .as_mut()
        .expect("sequencer")
        .sequencer
        .observed_unix_ms += 5_000;

    assert_eq!(
        first.normalized_sha256().expect("first normalized hash"),
        second.normalized_sha256().expect("second normalized hash")
    );

    second
        .nockchain
        .as_mut()
        .expect("nockchain")
        .total_output_nicks = "18446744073709551613".to_owned();
    assert_ne!(
        first.normalized_sha256().expect("first normalized hash"),
        second
            .normalized_sha256()
            .expect("semantic normalized hash")
    );
}

#[test]
fn sealed_hash_detects_semantic_mutation_and_passed_capsule_cannot_be_partial() {
    let mut capsule = full_capsule();
    capsule.seal_normalized_hash().expect("seal capsule");
    capsule
        .conservation
        .as_mut()
        .expect("conservation")
        .recipient_payout_nicks = ExactNicks("79".to_owned());
    assert!(capsule.validate().is_err());

    let mut partial = partial_capsule();
    partial.run.status = EvidenceRunStatus::Passed;
    partial.run.error = None;
    assert!(partial.validate().is_err());
}

#[test]
fn normalized_artifact_identity_omits_paths_but_keeps_hash_and_size() {
    let capsule = full_capsule();
    let normalized = capsule.normalized_value().expect("normalized value");
    let text = normalized.to_string();
    assert!(!text.contains("/volatile/"));
    assert!(text.contains(&"11".repeat(32)));
    assert!(text.contains("bridge_binary"));
    assert!(text.contains("123"));
}

fn partial_capsule() -> WithdrawalEvidenceCapsuleV1 {
    WithdrawalEvidenceCapsuleV1::new(
        EvidenceRunFacts {
            run_id: "run-partial".to_owned(),
            scenario: "core-withdrawal".to_owned(),
            seed: 7,
            status: EvidenceRunStatus::Failed,
            error: Some("setup failed".to_owned()),
            started_at_unix_ms: 1_000,
            finished_at_unix_ms: Some(2_000),
        },
        environment(),
        redaction(),
    )
}

fn full_capsule() -> WithdrawalEvidenceCapsuleV1 {
    let mut capsule = WithdrawalEvidenceCapsuleV1::new(
        EvidenceRunFacts {
            run_id: "run-full".to_owned(),
            scenario: "core-withdrawal".to_owned(),
            seed: 9,
            status: EvidenceRunStatus::Passed,
            error: None,
            started_at_unix_ms: 1_000,
            finished_at_unix_ms: Some(2_000),
        },
        environment(),
        redaction(),
    );
    capsule.artifacts = Some(EvidenceArtifacts {
        bridge_runtime: artifacts(),
        iris: Some(iris_artifact()),
        nockswap_bundle: None,
    });
    capsule.deployment = Some(EvidenceDeploymentFacts {
        environment_id: "hermetic-current-artifacts".to_owned(),
        addresses: BTreeMap::from([
            ("message_inbox".to_owned(), format!("{:#x}", address(1))),
            ("nock".to_owned(), format!("{:#x}", address(2))),
        ]),
        runtime_code_hashes: BTreeMap::from([(
            "nock".to_owned(),
            format!("{:#x}", B256::repeat_byte(2)),
        )]),
        pristine_state: Some(BTreeMap::from([("threshold".to_owned(), json!(3))])),
        overrides: vec![EvidenceOverrideFacts {
            kind: "bridge_signers".to_owned(),
            before: "source".to_owned(),
            after: "deterministic".to_owned(),
            transaction_hash: Some(format!("{:#x}", B256::repeat_byte(3))),
        }],
    });
    capsule.steps.push(EvidenceStep {
        index: 0,
        action: "submit_burn".to_owned(),
        status: "passed".to_owned(),
        started_at_unix_ms: 1_100,
        finished_at_unix_ms: 1_200,
        duration_ms: 100,
        frontier_before: Some(frontier(9)),
        frontier_after: Some(frontier(10)),
        detail: Some(json!({"semantic": "kept"})),
    });
    capsule.assertions.push(EvidenceAssertion {
        assertion: "calldata_equal".to_owned(),
        status: "passed".to_owned(),
        detail: None,
    });
    capsule.base = Some(base_proof());
    capsule.sequencer = Some(EvidenceSequencerFacts {
        proposal_hash: Some("proposal-hash".to_owned()),
        journal_id: Some("journal-id".to_owned()),
        sequencer: timed(sequencer()),
        reservations: timed(reservations()),
    });
    capsule.nockchain = Some(nockchain());
    capsule.kernels = Some(EvidenceKernelFacts {
        observed_unix_ms: 1_500,
        nodes: kernels(),
    });
    capsule.public = Some(timed(public()));
    capsule.conservation = Some(conservation());
    capsule.terminal = Some(EvidenceTerminalFacts {
        withdrawal_id: "withdrawal-1".to_owned(),
        base_event_id: "base-event-1".to_owned(),
        transaction_id: "nock-tx-1".to_owned(),
        stable_observations: 2,
        chain_inclusion_height: 100,
        chain_inclusion_block_id: "nock-block-1".to_owned(),
        source_observed_unix_ms: BTreeMap::from([("chain".to_owned(), 1_500)]),
        source_names: BTreeMap::from([("chain".to_owned(), "nockchain-grpc".to_owned())]),
        source_correlation_groups: BTreeMap::from([(
            "chain".to_owned(),
            "nockchain-node-0".to_owned(),
        )]),
    });
    capsule.external_artifacts.push(ExternalArtifactReference {
        kind: "browser_trace".to_owned(),
        path: "/volatile/trace.zip".to_owned(),
        sha256: "d".repeat(64),
        size_bytes: "9876543210".to_owned(),
        media_type: "application/zip".to_owned(),
    });
    capsule
}

fn first_semantic_detail(capsule: &mut WithdrawalEvidenceCapsuleV1) {
    capsule.steps[0].detail = Some(json!({
        "run_id": "other-volatile",
        "duration_ms": 1,
        "semantic": "kept",
    }));
}

fn environment() -> EvidenceEnvironmentFacts {
    EvidenceEnvironmentFacts {
        mode: EvidenceEnvironmentMode::Hermetic,
        environment_id: "hermetic-current-artifacts".to_owned(),
        source_manifest_sha256: None,
        source_chain_id: None,
        source_block_number: None,
        source_block_hash: None,
        local_chain_id: 31_338,
        rpc_endpoint_class: "loopback_anvil".to_owned(),
    }
}

fn redaction() -> RedactionDeclaration {
    RedactionDeclaration {
        policy: "withdrawal-e2e-redaction-v1".to_owned(),
        removed_secret_classes: vec!["private_keys".to_owned(), "rpc_credentials".to_owned()],
        raw_logs_embedded: false,
        external_artifacts_only: true,
    }
}

fn artifacts() -> E2eArtifacts {
    E2eArtifacts {
        bridge: artifact(ArtifactRole::BridgeBinary, "/volatile/bridge", "11"),
        node: artifact(ArtifactRole::NodeBinary, "/volatile/node", "22"),
        sequencer_ctl: Some(artifact(
            ArtifactRole::SequencerCtlBinary,
            "/volatile/ctl",
            "33",
        )),
        bridge_jam: artifact(ArtifactRole::BridgeJam, "/volatile/bridge.jam", "44"),
        roswell_jam: artifact(ArtifactRole::RoswellJam, "/volatile/roswell.jam", "55"),
        fakenet_genesis_jam: artifact(
            ArtifactRole::FakenetGenesisJam,
            "/volatile/fakenet.jam",
            "66",
        ),
        build: ArtifactBuildMetadata {
            package_version: "0.1.15".to_owned(),
            git_revision: Some("a".repeat(40)),
            target_arch: "aarch64".to_owned(),
            target_os: "macos".to_owned(),
        },
    }
}

fn artifact(role: ArtifactRole, path: &str, hash_byte: &str) -> ArtifactFile {
    ArtifactFile {
        role,
        path: PathBuf::from(path),
        sha256: hash_byte.repeat(32),
        size_bytes: 123,
        modified_unix_seconds: Some(1_000),
        architecture: None,
    }
}

fn iris_artifact() -> IrisArtifactFacts {
    IrisArtifactFacts {
        package_name: "@nockbox/iris-sdk".to_owned(),
        package_version: "0.3.0".to_owned(),
        git_revision: "b".repeat(40),
        tarball_path: PathBuf::from("/volatile/iris.tgz"),
        tarball_sha256: "77".repeat(32),
        npm_integrity: "sha512-fixture".to_owned(),
        npm_shasum: "c".repeat(40),
        files: Vec::new(),
        packed_node_version: "v26.0.0".to_owned(),
        packed_npm_version: "11.0.0".to_owned(),
        runtime_node_version: "v26.0.0".to_owned(),
        driver_path: PathBuf::from("/volatile/driver.js"),
    }
}

fn base_proof() -> BurnSubmissionProof {
    BurnSubmissionProof {
        transaction_hash: B256::repeat_byte(8),
        block_number: 10,
        mined_from: address(3),
        mined_to: address(2),
        mined_input_hex: format!("0x{}", "99".repeat(116)),
        client: ClientEncodingProof {
            client_mode: WithdrawalClientMode::IrisSdk,
            official_client: true,
            wire_protocol: "WithdrawalWireV1".to_owned(),
            withdrawal_policy: "withdrawal-policy-v1".to_owned(),
            nock_token: format!("{:#x}", address(2)),
            burner: format!("{:#x}", address(3)),
            amount_base_units: "340282366920938463463374607431768211455".to_owned(),
            amount_nicks: "18446744073709551615".to_owned(),
            bridge_fee_nicks: "1".to_owned(),
            net_after_bridge_fee_nicks: "18446744073709551614".to_owned(),
            destination_kind: "lock_root".to_owned(),
            destination_value: "recipient-root".to_owned(),
            lock_root: "recipient-root".to_owned(),
            lock_root_limbs: ["1", "2", "3", "4", "5"].map(str::to_owned),
            commitment: format!("{:#x}", B256::repeat_byte(9)),
            calldata_hex: format!("0x{}", "99".repeat(116)),
            calldata_byte_length: 116,
            artifact: Some(iris_artifact()),
        },
        event: BurnEventProof {
            nock_token: format!("{:#x}", address(2)),
            burner: format!("{:#x}", address(3)),
            amount_base_units: "340282366920938463463374607431768211455".to_owned(),
            amount_nicks: "18446744073709551615".to_owned(),
            commitment: format!("{:#x}", B256::repeat_byte(9)),
            lock_root: "recipient-root".to_owned(),
            log_index: 0,
            base_event_id: "base-event-1".to_owned(),
        },
        receipt: TransactionReceiptFacts {
            transaction_hash: B256::repeat_byte(8),
            block_number: 10,
            success: true,
            contract_address: None,
            logs: Vec::new(),
        },
    }
}

fn nockchain() -> EvidenceNockchainFacts {
    EvidenceNockchainFacts {
        transaction_id: "nock-tx-1".to_owned(),
        inclusion_height: 100,
        inclusion_block_id: "nock-block-1".to_owned(),
        tip_height: 103,
        confirmation_depth: 3,
        selected_inputs: vec![EvidenceNockNote {
            index: 0,
            name: note_name("input"),
            note_version: 1,
            assets_nicks: "18446744073709551615".to_owned(),
            lock_root: "bridge-root".to_owned(),
            origin_height: 90,
            origin_transaction_id: Some("origin-tx".to_owned()),
            origin_is_coinbase: Some(false),
        }],
        outputs: vec![EvidenceNockNote {
            index: 0,
            name: note_name("output"),
            note_version: 1,
            assets_nicks: "18446744073709551614".to_owned(),
            lock_root: "recipient-root".to_owned(),
            origin_height: 100,
            origin_transaction_id: Some("nock-tx-1".to_owned()),
            origin_is_coinbase: Some(false),
        }],
        raw_spend_fees_nicks: vec!["1".to_owned()],
        transaction_fee_nicks: "1".to_owned(),
        total_input_nicks: "18446744073709551615".to_owned(),
        total_output_nicks: "18446744073709551614".to_owned(),
        unaccounted_nicks: "0".to_owned(),
        matching_recipient_output_indices: vec![0],
    }
}

fn kernels() -> Vec<BridgeKernelTerminalFacts> {
    (0..5)
        .map(|node_id| BridgeKernelTerminalFacts {
            node_id,
            available: true,
            running: true,
            target_withdrawal_id: "withdrawal-1".to_owned(),
            target_base_event_id: "base-event-1".to_owned(),
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

fn sequencer() -> SequencerTerminalFacts {
    SequencerTerminalFacts {
        withdrawal_id: "withdrawal-1".to_owned(),
        withdrawal_nonce: 1,
        transaction_id: Some("nock-tx-1".to_owned()),
        confirmation_event_id: Some("confirmation-event-1".to_owned()),
        state: SequencerTerminalState::Confirmed,
        confirmed_height: Some(100),
        confirmed_block_id: Some("nock-block-1".to_owned()),
    }
}

fn reservations() -> ReservationTerminalFacts {
    ReservationTerminalFacts {
        withdrawal_id: "withdrawal-1".to_owned(),
        tracked_inputs: vec![note_name("input")],
        release_event_ids: vec!["release-event-1".to_owned()],
        currently_reserved_inputs: Vec::new(),
        release_count: 1,
    }
}

fn public() -> PublicWithdrawalTerminalFacts {
    PublicWithdrawalTerminalFacts {
        withdrawal_id: "withdrawal-1".to_owned(),
        withdrawal_nonce: 1,
        state: PublicWithdrawalState::Confirmed,
        base_event_id: "base-event-1".to_owned(),
        transaction_id: Some("nock-tx-1".to_owned()),
        confirmed_height: Some(100),
        confirmed_block_id: Some("nock-block-1".to_owned()),
    }
}

fn conservation() -> SettlementConservationProof {
    SettlementConservationProof {
        schema_version: 1,
        base_event_id: "base-event-1".to_owned(),
        nock_transaction_id: "nock-tx-1".to_owned(),
        gross_burn_nicks: ExactNicks("18446744073709551615".to_owned()),
        bridge_fee_nicks: ExactNicks("0".to_owned()),
        transaction_fee_nicks: ExactNicks("1".to_owned()),
        recipient_payout_nicks: ExactNicks("18446744073709551614".to_owned()),
        recipient_lock_root: "recipient-root".to_owned(),
        recipient_output: SettlementOutputProof {
            index: 0,
            name: note_name("output"),
            lock_root: "recipient-root".to_owned(),
            assets_nicks: ExactNicks("18446744073709551614".to_owned()),
            note_version: 1,
            origin_height: 100,
            origin_transaction_id: "nock-tx-1".to_owned(),
        },
        change_outputs: Vec::new(),
        change_total_nicks: ExactNicks("0".to_owned()),
        total_input_nicks: ExactNicks("18446744073709551615".to_owned()),
        total_output_nicks: ExactNicks("18446744073709551614".to_owned()),
        input_note_version_counts: BTreeMap::from([(1, 1)]),
        transaction_conservation: equation(
            "transaction_conservation",
            "18446744073709551615",
            [("total_output_nicks", "18446744073709551614"), ("transaction_fee_nicks", "1")],
        ),
        burn_to_payout: equation(
            "burn_to_payout",
            "18446744073709551615",
            [
                ("bridge_fee_nicks", "0"),
                ("transaction_fee_nicks", "1"),
                ("recipient_payout_nicks", "18446744073709551614"),
            ],
        ),
    }
}

fn equation<const N: usize>(
    name: &str,
    total: &str,
    terms: [(&str, &str); N],
) -> ArithmeticEquationProof {
    ArithmeticEquationProof {
        name: name.to_owned(),
        left: EquationTerm {
            name: "left".to_owned(),
            nicks: ExactNicks(total.to_owned()),
        },
        right_terms: terms
            .into_iter()
            .map(|(name, value)| EquationTerm {
                name: name.to_owned(),
                nicks: ExactNicks(value.to_owned()),
            })
            .collect(),
        right_total: ExactNicks(total.to_owned()),
        verdict: true,
    }
}

fn frontier(height: u64) -> EvidenceFrontier {
    EvidenceFrontier {
        base_height: Some(height),
        base_block_hash: Some(format!("base-{height}")),
        nock_height: Some(height),
        nock_block_id: Some(format!("nock-{height}")),
    }
}

fn note_name(prefix: &str) -> NoteNameFacts {
    NoteNameFacts {
        first: format!("{prefix}-first"),
        last: format!("{prefix}-last"),
    }
}

fn timed<T>(facts: T) -> TimedTerminalFact<T> {
    TimedTerminalFact {
        observed_unix_ms: 1_500,
        source_name: "evidence-schema-source".to_owned(),
        correlation_group: "evidence-schema-fixture".to_owned(),
        facts,
    }
}

fn address(seed: u64) -> Address {
    let mut bytes = [0_u8; 20];
    bytes[12..].copy_from_slice(&seed.to_be_bytes());
    Address::from(bytes)
}

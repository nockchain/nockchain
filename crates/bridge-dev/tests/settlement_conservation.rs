use alloy::primitives::U256;
use bridge::shared::types::{
    WITHDRAWAL_POLICY_V1_BASE_UNITS_PER_NICK,
    WITHDRAWAL_POLICY_V1_BRIDGE_FEE_NICKS_PER_STARTED_NOCK,
    WITHDRAWAL_POLICY_V1_MINIMUM_GROSS_NOCKS, WITHDRAWAL_POLICY_V1_NICKS_PER_NOCK,
};
use bridge_dev::iris_driver::BurnEventProof;
use bridge_dev::nockchain_probe::{
    CanonicalRawTransactionFacts, NockchainInclusionFacts, NockchainInputSnapshotFacts,
    NockchainTransactionFacts, NoteNameFacts, RawSpendFacts, SelectedInputNoteFacts,
    TransactionOutputFacts,
};
use bridge_dev::settlement_oracle::{SettlementOracle, SettlementOracleError};

const RECIPIENT_ROOT: &str = "recipient-root";

#[test]
fn proves_exact_minimum_boundary_multi_input_change_and_mixed_eras() {
    let gross = minimum_nicks();
    let (burn, transaction) = valid_fixture(gross);
    let proof = SettlementOracle::prove(&burn, &transaction).expect("valid settlement proof");

    assert_eq!(proof.gross_burn_nicks.0, gross.to_string());
    assert_eq!(
        proof.bridge_fee_nicks.0,
        (WITHDRAWAL_POLICY_V1_MINIMUM_GROSS_NOCKS
            * WITHDRAWAL_POLICY_V1_BRIDGE_FEE_NICKS_PER_STARTED_NOCK)
            .to_string()
    );
    assert_eq!(proof.transaction_fee_nicks.0, "1000");
    assert_eq!(proof.recipient_lock_root, RECIPIENT_ROOT);
    assert_eq!(proof.recipient_output.index, 0);
    assert_eq!(proof.change_outputs.len(), 2);
    assert_eq!(proof.change_total_nicks.0, "150000");
    assert_eq!(proof.input_note_version_counts.get(&0), Some(&1));
    assert_eq!(proof.input_note_version_counts.get(&1), Some(&1));
    assert!(proof.transaction_conservation.verdict);
    assert!(proof.burn_to_payout.verdict);
    let serialized = serde_json::to_value(&proof).expect("proof serializes");
    assert!(serialized["gross_burn_nicks"].is_string());
    assert_eq!(serialized["burn_to_payout"]["verdict"], true);
}

#[test]
fn started_nock_fee_rounds_up_for_fractional_amounts() {
    for extra_nicks in [1_u64, WITHDRAWAL_POLICY_V1_NICKS_PER_NOCK / 2] {
        let gross = minimum_nicks() + extra_nicks;
        let (burn, transaction) = valid_fixture(gross);
        let proof = SettlementOracle::prove(&burn, &transaction).expect("rounded settlement");
        let expected_started_nocks = WITHDRAWAL_POLICY_V1_MINIMUM_GROSS_NOCKS + 1;
        assert_eq!(
            proof.bridge_fee_nicks.0,
            (expected_started_nocks * WITHDRAWAL_POLICY_V1_BRIDGE_FEE_NICKS_PER_STARTED_NOCK)
                .to_string()
        );
    }
}

#[test]
fn rejects_one_nick_burn_equation_and_full_conservation_mismatches() {
    let gross = minimum_nicks();
    let (burn, mut payout_mismatch) = valid_fixture(gross);
    payout_mismatch.outputs[0].assets_nicks -= 1;
    reconcile_conservation(&mut payout_mismatch).expect("fixture conservation");
    assert!(matches!(
        SettlementOracle::prove(&burn, &payout_mismatch),
        Err(SettlementOracleError::EquationFailed {
            equation: "burn_to_payout",
            ..
        })
    ));

    let (_, mut transaction_mismatch) = valid_fixture(gross);
    transaction_mismatch.selected_inputs[0].assets_nicks += 1;
    transaction_mismatch.total_input_nicks += 1;
    assert!(matches!(
        SettlementOracle::prove(&burn, &transaction_mismatch),
        Err(SettlementOracleError::EquationFailed {
            equation: "transaction_conservation",
            ..
        })
    ));
}

#[test]
fn rejects_duplicate_missing_wrong_and_zero_recipient_outputs() {
    let gross = minimum_nicks();
    let (burn, mut duplicate) = valid_fixture(gross);
    duplicate.outputs.push(output(3, RECIPIENT_ROOT, 1, 1));
    duplicate.matching_recipient_output_indices = vec![0, 3];
    reconcile_conservation(&mut duplicate).expect("duplicate fixture conservation");
    assert!(matches!(
        SettlementOracle::prove(&burn, &duplicate),
        Err(SettlementOracleError::DuplicateRecipient { count: 2, .. })
    ));

    let (mut wrong_root, mut transaction) = valid_fixture(gross);
    wrong_root.lock_root = "wrong-root".to_owned();
    transaction.matching_recipient_output_indices.clear();
    assert!(matches!(
        SettlementOracle::prove(&wrong_root, &transaction),
        Err(SettlementOracleError::MissingRecipient(_))
    ));

    let (_, mut zero) = valid_fixture(gross);
    zero.outputs[0].assets_nicks = 0;
    reconcile_conservation(&mut zero).expect("zero fixture conservation");
    assert!(matches!(
        SettlementOracle::prove(&burn, &zero),
        Err(SettlementOracleError::ZeroRecipientPayout)
    ));

    let (_, mut stale_candidates) = valid_fixture(gross);
    stale_candidates.matching_recipient_output_indices.clear();
    assert!(matches!(
        SettlementOracle::prove(&burn, &stale_candidates),
        Err(SettlementOracleError::EvidenceMismatch(_))
    ));
}

#[test]
fn rejects_fee_underflow_overflow_and_burn_event_mismatch() {
    let gross = minimum_nicks();
    let (burn, mut excessive_fee) = valid_fixture(gross);
    excessive_fee.raw_transaction.spends[0].fee_nicks = gross;
    excessive_fee.raw_transaction.spends[1].fee_nicks = 0;
    excessive_fee.transaction_fee_nicks = gross;
    excessive_fee.outputs[0].assets_nicks = 1;
    reconcile_conservation(&mut excessive_fee).expect("fee fixture conservation");
    assert!(matches!(
        SettlementOracle::prove(&burn, &excessive_fee),
        Err(SettlementOracleError::FeeExceedsBurn { .. })
    ));

    let (_, mut overflow) = valid_fixture(gross);
    overflow.selected_inputs[0].assets_nicks = u64::MAX;
    overflow.selected_inputs[1].assets_nicks = 1;
    assert!(matches!(
        SettlementOracle::prove(&burn, &overflow),
        Err(SettlementOracleError::ArithmeticOverflow)
    ));

    let (mut mismatched_burn, transaction) = valid_fixture(gross);
    mismatched_burn.amount_base_units =
        (U256::from(gross + 1) * U256::from(WITHDRAWAL_POLICY_V1_BASE_UNITS_PER_NICK)).to_string();
    assert!(matches!(
        SettlementOracle::prove(&mismatched_burn, &transaction),
        Err(SettlementOracleError::BurnAmountMismatch)
    ));
}

#[test]
fn rejects_duplicate_output_identity_and_stale_total_facts() {
    let gross = minimum_nicks();
    let (burn, mut duplicate_name) = valid_fixture(gross);
    duplicate_name.outputs[2].name = duplicate_name.outputs[1].name.clone();
    assert!(matches!(
        SettlementOracle::prove(&burn, &duplicate_name),
        Err(SettlementOracleError::EvidenceMismatch(
            "duplicate output note name"
        ))
    ));

    let (_, mut stale_total) = valid_fixture(gross);
    stale_total.total_output_nicks += 1;
    assert!(matches!(
        SettlementOracle::prove(&burn, &stale_total),
        Err(SettlementOracleError::EvidenceMismatch(_))
    ));
}

fn valid_fixture(gross_burn_nicks: u64) -> (BurnEventProof, NockchainTransactionFacts) {
    let bridge_fee = bridge_fee(gross_burn_nicks);
    let transaction_fee = 1_000;
    let recipient_payout = gross_burn_nicks - bridge_fee - transaction_fee;
    let outputs = vec![
        output(0, RECIPIENT_ROOT, recipient_payout, 1),
        output(1, "change-root-a", 100_000, 0),
        output(2, "change-root-b", 50_000, 1),
    ];
    let mut transaction = NockchainTransactionFacts {
        transaction_id: "nock-transaction".to_owned(),
        inclusion: NockchainInclusionFacts {
            height: 100,
            block_id: "block-id".to_owned(),
        },
        tip_height: 103,
        confirmation_depth: 3,
        inclusion_history: vec![NockchainInclusionFacts {
            height: 100,
            block_id: "block-id".to_owned(),
        }],
        raw_transaction: CanonicalRawTransactionFacts {
            version: 1,
            embedded_transaction_id: "nock-transaction".to_owned(),
            computed_transaction_id: "nock-transaction".to_owned(),
            size_bytes: 512,
            spends: vec![raw_spend(400, "legacy"), raw_spend(600, "witness")],
        },
        input_snapshot: NockchainInputSnapshotFacts {
            height: 99,
            block_id: "snapshot-block".to_owned(),
        },
        selected_inputs: vec![selected_input(0, 1), selected_input(1, 0)],
        outputs,
        transaction_fee_nicks: transaction_fee,
        total_input_nicks: 0,
        total_output_nicks: 0,
        unaccounted_nicks: 0,
        matching_recipient_output_indices: vec![0],
    };
    reconcile_conservation(&mut transaction).expect("valid fixture arithmetic");
    let burn = BurnEventProof {
        nock_token: "0x0000000000000000000000000000000000000001".to_owned(),
        burner: "0x0000000000000000000000000000000000000002".to_owned(),
        amount_base_units: (U256::from(gross_burn_nicks)
            * U256::from(WITHDRAWAL_POLICY_V1_BASE_UNITS_PER_NICK))
        .to_string(),
        amount_nicks: gross_burn_nicks.to_string(),
        commitment: format!("0x{}", "11".repeat(32)),
        lock_root: RECIPIENT_ROOT.to_owned(),
        log_index: 0,
        base_event_id: format!("0x{}", "22".repeat(32)),
    };
    (burn, transaction)
}

fn reconcile_conservation(transaction: &mut NockchainTransactionFacts) -> Result<(), &'static str> {
    let total_output = transaction
        .outputs
        .iter()
        .try_fold(0_u64, |total, output| {
            total
                .checked_add(output.assets_nicks)
                .ok_or("output overflow")
        })?;
    let total_input = total_output
        .checked_add(transaction.transaction_fee_nicks)
        .ok_or("input overflow")?;
    transaction.selected_inputs[1].assets_nicks = 500;
    transaction.selected_inputs[0].assets_nicks = total_input
        .checked_sub(500)
        .ok_or("input split underflow")?;
    transaction.total_input_nicks = total_input;
    transaction.total_output_nicks = total_output;
    transaction.unaccounted_nicks = 0;
    Ok(())
}

fn output(
    index: usize,
    lock_root: &str,
    assets_nicks: u64,
    note_version: u64,
) -> TransactionOutputFacts {
    TransactionOutputFacts {
        index,
        name: NoteNameFacts {
            first: format!("name-first-{index}"),
            last: format!("name-last-{index}"),
        },
        note_version,
        assets_nicks,
        lock_root: lock_root.to_owned(),
        origin_height: 100,
        origin_transaction_id: "nock-transaction".to_owned(),
        origin_is_coinbase: false,
        note_data_keys: Vec::new(),
    }
}

fn selected_input(index: usize, note_version: u64) -> SelectedInputNoteFacts {
    SelectedInputNoteFacts {
        name: NoteNameFacts {
            first: format!("input-first-{index}"),
            last: format!("input-last-{index}"),
        },
        note_version,
        assets_nicks: 1,
        origin_height: 90 + index as u64,
        origin_transaction_id: format!("origin-{index}"),
        origin_is_coinbase: false,
    }
}

fn raw_spend(fee_nicks: u64, kind: &str) -> RawSpendFacts {
    RawSpendFacts {
        input_name: NoteNameFacts {
            first: format!("spend-{kind}-first"),
            last: format!("spend-{kind}-last"),
        },
        spend_kind: kind.to_owned(),
        fee_nicks,
        seeds: Vec::new(),
    }
}

fn minimum_nicks() -> u64 {
    WITHDRAWAL_POLICY_V1_MINIMUM_GROSS_NOCKS * WITHDRAWAL_POLICY_V1_NICKS_PER_NOCK
}

fn bridge_fee(gross_burn_nicks: u64) -> u64 {
    gross_burn_nicks.div_ceil(WITHDRAWAL_POLICY_V1_NICKS_PER_NOCK)
        * WITHDRAWAL_POLICY_V1_BRIDGE_FEE_NICKS_PER_STARTED_NOCK
}

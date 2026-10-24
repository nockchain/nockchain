diesel::table! {
    sequencer_journal_cursor (journal_id) {
        journal_id -> Text,
        last_sequence -> BigInt,
        last_event_id -> Text,
        updated_at -> BigInt,
    }
}

diesel::table! {
    withdrawal_submission_events (event_id) {
        event_id -> BigInt,
        created_at -> BigInt,
        withdrawal_id_as_of -> Binary,
        withdrawal_id_base_event_id -> Binary,
        epoch -> BigInt,
        proposal_hash -> Text,
        transaction_name -> Text,
        event_type -> Text,
        signer_node_id -> Nullable<BigInt>,
        commit_certificate -> Nullable<Binary>,
        transaction_jam -> Nullable<Binary>,
        snapshot_height -> Nullable<BigInt>,
        snapshot_block_id -> Nullable<Binary>,
        confirmed_height -> Nullable<BigInt>,
        confirmed_block_id -> Nullable<Binary>,
    }
}

diesel::table! {
    sequencer_withdrawals (withdrawal_id_base_event_id) {
        withdrawal_id_as_of -> Binary,
        withdrawal_id_base_event_id -> Binary,
        withdrawal_nonce -> BigInt,
        current_epoch -> BigInt,
        proposal_hash -> Nullable<Text>,
        request_recipient -> Nullable<Binary>,
        request_burned_amount -> Nullable<BigInt>,
        request_base_batch_end -> Nullable<BigInt>,
        canonical_amount -> Nullable<BigInt>,
        canonical_base_batch_end -> Nullable<BigInt>,
        canonical_transaction_jam -> Nullable<Binary>,
        canonical_selected_inputs_jam -> Nullable<Binary>,
        canonical_snapshot_height -> Nullable<BigInt>,
        canonical_snapshot_block_id -> Nullable<Binary>,
        peer_commit_certificate -> Nullable<Binary>,
        authorized_transaction_name -> Nullable<Text>,
        authorized_transaction_jam -> Nullable<Binary>,
        authorized_raw_tx -> Nullable<Binary>,
        handoff_index -> BigInt,
        turn_started_base_height -> Nullable<BigInt>,
        submit_attempt_count -> BigInt,
        last_submit_attempt_base_height -> Nullable<BigInt>,
        last_submit_error -> Nullable<Text>,
        state -> Text,
        created_at -> BigInt,
        updated_at -> BigInt,
    }
}

diesel::table! {
    withdrawal_reserved_inputs (
        withdrawal_id_as_of,
        withdrawal_id_base_event_id,
        input_first,
        input_last
    ) {
        withdrawal_id_as_of -> Binary,
        withdrawal_id_base_event_id -> Binary,
        epoch -> BigInt,
        input_first -> Binary,
        input_last -> Binary,
        created_at -> BigInt,
        updated_at -> BigInt,
    }
}

diesel::allow_tables_to_appear_in_same_query!(
    sequencer_journal_cursor, sequencer_withdrawals, withdrawal_reserved_inputs,
    withdrawal_submission_events,
);

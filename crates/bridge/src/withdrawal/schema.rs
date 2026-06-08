// Operator-side withdrawal request/state projection.
//
// Proposal bodies are intentionally not stored in this SQLite schema. Prepared
// proposals and signature contributions live in memory, while canonical
// proposal/transaction artifacts are owned by the sequencer journal.

diesel::table! {
    withdrawals (id) {
        id -> BigInt,
        base_as_of -> Binary,
        base_event_id -> Binary,
        recipient -> Binary,
        gross_burned_amount -> BigInt,
        base_batch_end -> BigInt,
        withdrawal_nonce -> BigInt,
        current_epoch -> BigInt,
        proposal_hash -> Nullable<Text>,
        peer_commit_certificate -> Nullable<Binary>,
        state -> Text,
        turn_started_base_height -> Nullable<BigInt>,
        submitted_tx_name -> Nullable<Text>,
        submitted_tx_hash -> Nullable<Text>,
        submitted_at -> Nullable<BigInt>,
        confirmed_height -> Nullable<BigInt>,
        confirmed_block_id -> Nullable<Binary>,
        created_at -> BigInt,
        updated_at -> BigInt,
    }
}

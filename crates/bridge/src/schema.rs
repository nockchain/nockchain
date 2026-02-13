diesel::table! {
    deposit_log (tx_id) {
        tx_id -> Binary,
        block_height -> BigInt,
        as_of -> Binary,
        name_first -> Binary,
        name_last -> Binary,
        recipient -> Binary,
        amount_to_mint -> BigInt,
    }
}

diesel::allow_tables_to_appear_in_same_query!(deposit_log,);

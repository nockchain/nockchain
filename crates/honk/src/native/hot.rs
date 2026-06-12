use std::sync::OnceLock;

use nockvm::jets::hot::{HotEntry, URBIT_HOT_STATE};

pub fn native_hot_state() -> &'static [HotEntry] {
    static HOT_STATE: OnceLock<Vec<HotEntry>> = OnceLock::new();
    HOT_STATE
        .get_or_init(|| {
            let mut hot_state = Vec::new();
            hot_state.extend_from_slice(URBIT_HOT_STATE);
            hot_state.extend(zkvm_jetpack::hot::produce_prover_hot_state());
            hot_state
        })
        .as_slice()
}

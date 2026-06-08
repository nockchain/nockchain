pub mod service;
pub mod state;

pub use service::{BridgeStatusState, LastSubmittedDeposit, StatusService};
pub use state::{
    run_hourly_rotation, BridgeStatus, ALERT_HISTORY_CAPACITY, PROPOSAL_HISTORY_CAPACITY,
    TX_CAPACITY,
};

pub use crate::observability::status_proto::proto;

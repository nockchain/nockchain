//! TUI panel components.
//!
//! Each panel is a self-contained widget that renders a specific
//! aspect of the bridge state.

pub mod alerts;
pub mod deposit_log;
pub mod health;
pub mod network_state;
pub mod proposals;
pub mod transactions;

pub use alerts::AlertPanel;
pub use deposit_log::DepositLogPanel;
pub use health::HealthPanel;
pub use network_state::NetworkStatePanel;
pub use proposals::ProposalPanel;
pub use transactions::TransactionPanel;

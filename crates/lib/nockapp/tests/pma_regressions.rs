use std::error::Error;
use std::sync::Mutex;

mod pma_regressions {
    pub(crate) mod boot_active_resize;
    pub(crate) mod checkpoint_bootstrap_size;
    pub(crate) mod event_preflight_growth;
    pub(crate) mod event_resize_failure_boundary;
    pub(crate) mod failed_preserve_recovery;
    pub(crate) mod fresh_replay_boundary;
    pub(crate) mod pma_meta;
    pub(crate) mod snapshot_restore_expand;
    pub(crate) mod sqlite_boundary_recovery;
    pub(crate) mod stale_checkpoint_refusal;
    pub(crate) mod upgrade_from_65a;
}

type TestResult = Result<(), Box<dyn Error>>;

static PMA_REGRESSION_LOCK: Mutex<()> = Mutex::new(());

fn run_serialized(test: fn() -> TestResult) -> TestResult {
    let _guard = PMA_REGRESSION_LOCK
        .lock()
        .expect("PMA regression test lock poisoned");
    test()
}

#[test]
fn pma_boot_active_resize_regression() -> TestResult {
    run_serialized(pma_regressions::boot_active_resize::run_regression)
}

#[test]
fn pma_checkpoint_bootstrap_size_regression() -> TestResult {
    run_serialized(pma_regressions::checkpoint_bootstrap_size::run_regression)
}

#[test]
fn pma_event_preflight_growth_regression() -> TestResult {
    run_serialized(pma_regressions::event_preflight_growth::run_regression)
}

#[test]
fn pma_event_resize_failure_boundary_regression() -> TestResult {
    run_serialized(pma_regressions::event_resize_failure_boundary::run_regression)
}

#[test]
fn pma_failed_preserve_recovery_regression() -> TestResult {
    run_serialized(pma_regressions::failed_preserve_recovery::run_regression)
}

#[test]
fn pma_fresh_replay_boundary_regression() -> TestResult {
    run_serialized(pma_regressions::fresh_replay_boundary::run_regression)
}

#[test]
fn pma_snapshot_restore_expand_regression() -> TestResult {
    run_serialized(pma_regressions::snapshot_restore_expand::run_regression)
}

#[test]
fn pma_sqlite_boundary_recovery_regression() -> TestResult {
    run_serialized(pma_regressions::sqlite_boundary_recovery::run_regression)
}

#[test]
fn pma_stale_checkpoint_refusal_regression() -> TestResult {
    run_serialized(pma_regressions::stale_checkpoint_refusal::run_regression)
}

#[test]
fn pma_upgrade_from_65a_regression() -> TestResult {
    run_serialized(pma_regressions::upgrade_from_65a::run_regression)
}

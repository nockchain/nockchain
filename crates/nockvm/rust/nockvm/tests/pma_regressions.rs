use std::error::Error;
use std::sync::Mutex;

mod pma_regressions {
    pub(crate) mod multi_resize_bootstrap;
    pub(crate) mod resize_exhaustion;
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
fn pma_multi_resize_bootstrap_regression() -> TestResult {
    run_serialized(pma_regressions::multi_resize_bootstrap::run_regression)
}

#[test]
fn pma_resize_exhaustion_regression() -> TestResult {
    run_serialized(pma_regressions::resize_exhaustion::run_regression)
}

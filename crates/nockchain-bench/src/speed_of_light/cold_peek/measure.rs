use std::mem::MaybeUninit;
use std::time::{Duration, Instant};

use nockapp::nockapp::NockApp;

use crate::speed_of_light::peek_bench::{peek_height_result, PeekResultSample};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct FaultCounters {
    minflt: u64,
    majflt: u64,
}

#[derive(Debug, Clone, Copy)]
pub struct StepMeasurement {
    pub duration: Duration,
    pub minflt_delta: u64,
    pub majflt_delta: u64,
}

#[derive(Debug, Clone, Copy)]
pub struct PeekMeasurement {
    pub(crate) sample: PeekResultSample,
    pub measurement: StepMeasurement,
}

impl PeekMeasurement {
    pub fn duration(&self) -> Duration {
        self.measurement.duration
    }

    pub fn minflt_delta(&self) -> u64 {
        self.measurement.minflt_delta
    }

    pub fn majflt_delta(&self) -> u64 {
        self.measurement.majflt_delta
    }
}

pub fn measure_sync<T, E>(
    operation: impl FnOnce() -> Result<T, E>,
) -> (Result<T, E>, StepMeasurement) {
    let before = getrusage_self();
    let started_at = Instant::now();
    let result = operation();
    let measurement = finish_measurement(before, started_at);
    (result, measurement)
}

pub async fn measure_peek(
    nockapp: &mut NockApp,
    height: u64,
) -> Result<PeekMeasurement, nockapp::nockapp::NockAppError> {
    let before = getrusage_self();
    let started_at = Instant::now();
    let sample = peek_height_result(nockapp, height).await?;
    let measurement = finish_measurement(before, started_at);

    Ok(PeekMeasurement {
        sample,
        measurement,
    })
}

fn finish_measurement(before: Option<FaultCounters>, started_at: Instant) -> StepMeasurement {
    let duration = started_at.elapsed();
    let after = getrusage_self();
    let (minflt_delta, majflt_delta) = match (before, after) {
        (Some(before), Some(after)) => (
            after.minflt.saturating_sub(before.minflt),
            after.majflt.saturating_sub(before.majflt),
        ),
        _ => (0, 0),
    };

    StepMeasurement {
        duration,
        minflt_delta,
        majflt_delta,
    }
}

fn getrusage_self() -> Option<FaultCounters> {
    let mut usage = MaybeUninit::<libc::rusage>::uninit();
    let ret = unsafe { libc::getrusage(libc::RUSAGE_SELF, usage.as_mut_ptr()) };
    if ret != 0 {
        return None;
    }

    let usage = unsafe { usage.assume_init() };
    Some(FaultCounters {
        minflt: usage.ru_minflt as u64,
        majflt: usage.ru_majflt as u64,
    })
}

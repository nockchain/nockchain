use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use super::{
    is_release_build, DEFAULT_THROUGHPUT_CV_THRESHOLD, SUMMARY_SCHEMA_VERSION,
    VERDICT_SCHEMA_VERSION,
};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ValueStats {
    pub median: f64,
    pub min: f64,
    pub max: f64,
    pub mad: f64,
    pub stddev: f64,
    pub cv: f64,
    pub values: Vec<f64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RunMetrics {
    pub steps_per_second: Option<f64>,
    pub block_pokes_per_second: Option<f64>,
    pub pokes_per_second: Option<f64>,
    pub raw_tx_pokes_per_second: Option<f64>,
    pub peeks_per_second: Option<f64>,
    pub cold_peeks_per_second: Option<f64>,
    pub init_time_secs: f64,
    pub total_step_time_secs: f64,
    pub average_block_time_ms: f64,
    pub peak_process_rss_bytes: Option<f64>,
    pub minor_faults_total: Option<f64>,
    pub major_faults_total: Option<f64>,
}

#[derive(Debug, Clone, PartialEq)]
struct RunMetricStats {
    steps_per_second: Option<ValueStats>,
    block_pokes_per_second: Option<ValueStats>,
    pokes_per_second: Option<ValueStats>,
    raw_tx_pokes_per_second: Option<ValueStats>,
    peeks_per_second: Option<ValueStats>,
    cold_peeks_per_second: Option<ValueStats>,
    init_time_secs: Option<ValueStats>,
    total_step_time_secs: Option<ValueStats>,
    average_block_time_ms: Option<ValueStats>,
    peak_process_rss_bytes: Option<ValueStats>,
    minor_faults_total: Option<ValueStats>,
    major_faults_total: Option<ValueStats>,
}

impl RunMetricStats {
    fn from_metrics(metrics: &[RunMetrics]) -> Self {
        Self {
            steps_per_second: stats_option(metrics.iter().map(|run| run.steps_per_second)),
            block_pokes_per_second: stats_option(
                metrics.iter().map(|run| run.block_pokes_per_second),
            ),
            pokes_per_second: stats_option(metrics.iter().map(|run| run.pokes_per_second)),
            raw_tx_pokes_per_second: stats_option(
                metrics.iter().map(|run| run.raw_tx_pokes_per_second),
            ),
            peeks_per_second: stats_option(metrics.iter().map(|run| run.peeks_per_second)),
            cold_peeks_per_second: stats_option(
                metrics.iter().map(|run| run.cold_peeks_per_second),
            ),
            init_time_secs: stats(metrics.iter().map(|run| run.init_time_secs)),
            total_step_time_secs: stats(metrics.iter().map(|run| run.total_step_time_secs)),
            average_block_time_ms: stats(metrics.iter().map(|run| run.average_block_time_ms)),
            peak_process_rss_bytes: stats_option(
                metrics.iter().map(|run| run.peak_process_rss_bytes),
            ),
            minor_faults_total: stats_option(metrics.iter().map(|run| run.minor_faults_total)),
            major_faults_total: stats_option(metrics.iter().map(|run| run.major_faults_total)),
        }
    }

    fn aggregate_metrics(&self) -> BTreeMap<String, ValueStats> {
        let mut aggregate = BTreeMap::new();
        for (key, value) in [
            ("steps_per_second", &self.steps_per_second),
            ("block_pokes_per_second", &self.block_pokes_per_second),
            ("pokes_per_second", &self.pokes_per_second),
            ("raw_tx_pokes_per_second", &self.raw_tx_pokes_per_second),
            ("peeks_per_second", &self.peeks_per_second),
            ("cold_peeks_per_second", &self.cold_peeks_per_second),
            ("init_time_secs", &self.init_time_secs),
            ("total_step_time_secs", &self.total_step_time_secs),
            ("peak_process_rss_bytes", &self.peak_process_rss_bytes),
            ("minor_faults_total", &self.minor_faults_total),
            ("major_faults_total", &self.major_faults_total),
        ] {
            if let Some(value) = value {
                aggregate.insert(key.to_string(), value.clone());
            }
        }
        aggregate
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunFailure {
    pub run_id: String,
    pub reason: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RunSummary {
    #[serde(default = "summary_schema_version")]
    pub schema_version: String,
    #[serde(default = "default_benchmark")]
    pub benchmark: String,
    pub measured_runs_requested: u32,
    pub measured_runs_succeeded: usize,
    pub failed_runs: Vec<RunFailure>,
    #[serde(default)]
    pub aggregate: BTreeMap<String, ValueStats>,
    #[serde(default)]
    pub by_step_type: BTreeMap<String, StepTypeSummary>,
    #[serde(default)]
    pub steps: Vec<StepSummary>,
    #[serde(default)]
    pub steps_per_second: Option<ValueStats>,
    #[serde(default)]
    pub block_pokes_per_second: Option<ValueStats>,
    #[serde(default)]
    pub pokes_per_second: Option<ValueStats>,
    #[serde(default)]
    pub raw_tx_pokes_per_second: Option<ValueStats>,
    #[serde(default)]
    pub peeks_per_second: Option<ValueStats>,
    #[serde(default)]
    pub cold_peeks_per_second: Option<ValueStats>,
    pub init_time_secs: Option<ValueStats>,
    pub total_step_time_secs: Option<ValueStats>,
    pub average_block_time_ms: Option<ValueStats>,
    pub peak_process_rss_bytes: Option<ValueStats>,
    pub minor_faults_total: Option<ValueStats>,
    pub major_faults_total: Option<ValueStats>,
}

#[derive(Debug, Clone)]
pub struct RunSummaryInput {
    pub measured_run_count: u32,
    pub run_failures: Vec<RunFailure>,
    pub throughput_cv: Option<f64>,
    pub cv_threshold: f64,
    pub release_build: bool,
    pub allow_debug_benchmark: bool,
    pub allow_version_skew: bool,
    pub allow_degraded_cold: bool,
    pub invalid_reasons: Vec<String>,
    pub partial_reasons: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StepTypeSummary {
    pub count_per_run: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<ValueStats>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub throughput_per_second: Option<ValueStats>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error_count: Option<ValueStats>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub success_count: Option<ValueStats>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub missing_count: Option<ValueStats>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cold_verified_count: Option<ValueStats>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cold_unverified_count: Option<ValueStats>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub minflt_delta: Option<ValueStats>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub majflt_delta: Option<ValueStats>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StepSummary {
    pub step_index: usize,
    pub step_id: String,
    #[serde(rename = "type")]
    pub step_type: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub height: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<ValueStats>,
    pub outcomes: BTreeMap<String, u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Validity {
    Valid,
    Partial { reasons: Vec<String> },
    Invalid { reasons: Vec<String> },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Verdict {
    #[serde(default = "verdict_schema_version")]
    pub schema_version: String,
    pub allow_debug_benchmark: bool,
    pub allow_version_skew: bool,
    pub allow_degraded_cold: bool,
    pub cv_threshold: f64,
    pub validity: Validity,
}

fn summary_schema_version() -> String {
    SUMMARY_SCHEMA_VERSION.to_string()
}

fn default_benchmark() -> String {
    "sol-orchestrate".to_string()
}

fn verdict_schema_version() -> String {
    VERDICT_SCHEMA_VERSION.to_string()
}

pub fn summarize_runs(
    metrics: &[RunMetrics],
    failed_runs: &[RunFailure],
    measured_runs_requested: u32,
) -> RunSummary {
    let stats = RunMetricStats::from_metrics(metrics);
    RunSummary {
        schema_version: SUMMARY_SCHEMA_VERSION.to_string(),
        benchmark: "sol-orchestrate".to_string(),
        measured_runs_requested,
        measured_runs_succeeded: metrics.len(),
        failed_runs: failed_runs.to_vec(),
        aggregate: stats.aggregate_metrics(),
        by_step_type: BTreeMap::new(),
        steps: Vec::new(),
        steps_per_second: stats.steps_per_second,
        block_pokes_per_second: stats.block_pokes_per_second,
        pokes_per_second: stats.pokes_per_second,
        raw_tx_pokes_per_second: stats.raw_tx_pokes_per_second,
        peeks_per_second: stats.peeks_per_second,
        cold_peeks_per_second: stats.cold_peeks_per_second,
        init_time_secs: stats.init_time_secs,
        total_step_time_secs: stats.total_step_time_secs,
        average_block_time_ms: stats.average_block_time_ms,
        peak_process_rss_bytes: stats.peak_process_rss_bytes,
        minor_faults_total: stats.minor_faults_total,
        major_faults_total: stats.major_faults_total,
    }
}

pub fn evaluate_verdict(input: &RunSummaryInput) -> Verdict {
    let mut invalid_reasons = input.invalid_reasons.clone();
    if input.measured_run_count > 0 && input.run_failures.len() == input.measured_run_count as usize
    {
        invalid_reasons.push("all measured runs failed".to_string());
    }
    if !input.release_build && !input.allow_debug_benchmark {
        invalid_reasons.push(
            "trusted runs require a release build unless --allow-debug-benchmark is set"
                .to_string(),
        );
    }

    if !invalid_reasons.is_empty() {
        return Verdict {
            schema_version: VERDICT_SCHEMA_VERSION.to_string(),
            allow_debug_benchmark: input.allow_debug_benchmark,
            allow_version_skew: input.allow_version_skew,
            allow_degraded_cold: input.allow_degraded_cold,
            cv_threshold: input.cv_threshold,
            validity: Validity::Invalid {
                reasons: invalid_reasons,
            },
        };
    }

    let mut partial_reasons = input.partial_reasons.clone();
    if !input.release_build && input.allow_debug_benchmark {
        partial_reasons.push("debug build used under --allow-debug-benchmark override".to_string());
    }

    for failure in &input.run_failures {
        partial_reasons.push(format!(
            "measured run {} failed: {}",
            failure.run_id, failure.reason
        ));
    }

    if let Some(cv) = input.throughput_cv {
        if cv > input.cv_threshold {
            partial_reasons.push(format!(
                "throughput CV {:.3} exceeded threshold {:.2}",
                cv, input.cv_threshold
            ));
        }
    }

    if partial_reasons.is_empty() {
        Verdict {
            schema_version: VERDICT_SCHEMA_VERSION.to_string(),
            allow_debug_benchmark: input.allow_debug_benchmark,
            allow_version_skew: input.allow_version_skew,
            allow_degraded_cold: input.allow_degraded_cold,
            cv_threshold: input.cv_threshold,
            validity: Validity::Valid,
        }
    } else {
        Verdict {
            schema_version: VERDICT_SCHEMA_VERSION.to_string(),
            allow_debug_benchmark: input.allow_debug_benchmark,
            allow_version_skew: input.allow_version_skew,
            allow_degraded_cold: input.allow_degraded_cold,
            cv_threshold: input.cv_threshold,
            validity: Validity::Partial {
                reasons: partial_reasons,
            },
        }
    }
}

pub fn current_release_build_verdict(
    measured_run_count: u32,
    run_failures: Vec<RunFailure>,
    throughput_cv: Option<f64>,
    allow_debug_benchmark: bool,
) -> Verdict {
    evaluate_verdict(&RunSummaryInput {
        measured_run_count,
        run_failures,
        throughput_cv,
        cv_threshold: DEFAULT_THROUGHPUT_CV_THRESHOLD,
        release_build: is_release_build(),
        allow_debug_benchmark,
        allow_version_skew: false,
        allow_degraded_cold: false,
        invalid_reasons: Vec::new(),
        partial_reasons: Vec::new(),
    })
}

pub(crate) fn stats(values: impl Iterator<Item = f64>) -> Option<ValueStats> {
    let values: Vec<f64> = values.collect();
    if values.is_empty() {
        return None;
    }
    Some(compute_stats(values))
}

pub(crate) fn stats_option(values: impl Iterator<Item = Option<f64>>) -> Option<ValueStats> {
    let values: Vec<f64> = values.flatten().collect();
    if values.is_empty() {
        return None;
    }
    Some(compute_stats(values))
}

fn compute_stats(mut values: Vec<f64>) -> ValueStats {
    values.sort_by(|left, right| left.total_cmp(right));
    let mean = values.iter().copied().sum::<f64>() / values.len() as f64;
    let median_value = median(&values);
    let deviations: Vec<f64> = values.iter().map(|value| (value - mean).powi(2)).collect();
    let stddev = (deviations.iter().sum::<f64>() / values.len() as f64).sqrt();
    let mut mad_values: Vec<f64> = values
        .iter()
        .map(|value| (value - median_value).abs())
        .collect();
    mad_values.sort_by(|left, right| left.total_cmp(right));
    let mad = median(&mad_values);

    ValueStats {
        median: median_value,
        min: *values.first().unwrap_or(&0.0),
        max: *values.last().unwrap_or(&0.0),
        mad,
        stddev,
        cv: if mean.abs() > f64::EPSILON {
            stddev / mean.abs()
        } else {
            0.0
        },
        values,
    }
}

fn median(values: &[f64]) -> f64 {
    if values.is_empty() {
        return 0.0;
    }
    let middle = values.len() / 2;
    if values.len() % 2 == 0 {
        (values[middle - 1] + values[middle]) / 2.0
    } else {
        values[middle]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn harness_summary_computes_spread_metrics() {
        let summary = summarize_runs(
            &[
                RunMetrics {
                    steps_per_second: None,
                    block_pokes_per_second: None,
                    pokes_per_second: Some(10.0),
                    raw_tx_pokes_per_second: None,
                    peeks_per_second: None,
                    cold_peeks_per_second: None,
                    init_time_secs: 1.0,
                    total_step_time_secs: 2.0,
                    average_block_time_ms: 100.0,
                    peak_process_rss_bytes: Some(100.0),
                    minor_faults_total: Some(10.0),
                    major_faults_total: Some(1.0),
                },
                RunMetrics {
                    steps_per_second: None,
                    block_pokes_per_second: None,
                    pokes_per_second: Some(14.0),
                    raw_tx_pokes_per_second: None,
                    peeks_per_second: None,
                    cold_peeks_per_second: None,
                    init_time_secs: 3.0,
                    total_step_time_secs: 4.0,
                    average_block_time_ms: 140.0,
                    peak_process_rss_bytes: Some(200.0),
                    minor_faults_total: Some(30.0),
                    major_faults_total: Some(2.0),
                },
                RunMetrics {
                    steps_per_second: None,
                    block_pokes_per_second: None,
                    pokes_per_second: Some(18.0),
                    raw_tx_pokes_per_second: None,
                    peeks_per_second: None,
                    cold_peeks_per_second: None,
                    init_time_secs: 5.0,
                    total_step_time_secs: 6.0,
                    average_block_time_ms: 180.0,
                    peak_process_rss_bytes: Some(300.0),
                    minor_faults_total: Some(50.0),
                    major_faults_total: Some(3.0),
                },
            ],
            &[],
            3,
        );

        let throughput = summary.pokes_per_second.expect("poke throughput stats");
        assert_eq!(throughput.median, 14.0);
        assert_eq!(throughput.min, 10.0);
        assert_eq!(throughput.max, 18.0);
        assert!((throughput.mad - 4.0).abs() < 1e-9);
        assert!(throughput.stddev > 0.0);
        assert!(throughput.cv > 0.0);
        assert_eq!(
            summary
                .aggregate
                .get("peak_process_rss_bytes")
                .expect("RSS aggregate")
                .median,
            200.0
        );
        assert_eq!(
            summary
                .aggregate
                .get("minor_faults_total")
                .expect("minor faults aggregate")
                .median,
            30.0
        );
    }

    #[test]
    fn trusted_summary_omits_replay_only_fields_when_absent() {
        let summary = summarize_runs(
            &[RunMetrics {
                steps_per_second: Some(10.0),
                block_pokes_per_second: None,
                pokes_per_second: Some(5.0),
                raw_tx_pokes_per_second: None,
                peeks_per_second: None,
                cold_peeks_per_second: None,
                init_time_secs: 0.0,
                total_step_time_secs: 1.0,
                average_block_time_ms: 0.0,
                peak_process_rss_bytes: None,
                minor_faults_total: None,
                major_faults_total: None,
            }],
            &[],
            1,
        );
        let value = serde_json::to_value(summary).expect("summary json");

        assert!(value.get("throughput_blocks_per_second").is_none());
        assert!(value.get("failed_pokes").is_none());
    }
}

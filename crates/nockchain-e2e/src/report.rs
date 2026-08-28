use std::path::Path;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

pub const REPORT_SCHEMA_VERSION: u64 = 1;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Report {
    pub schema_version: u64,
    pub scenario: String,
    pub seed: u64,
    pub run_id: String,
    pub status: String,
    pub error: Option<String>,
    pub started_at_epoch_secs: u64,
    pub finished_at_epoch_secs: Option<u64>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub steps: Vec<StepRecord>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub asserts: Vec<AssertOutcome>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub nodes: Vec<NodeSummary>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StepRecord {
    pub index: usize,
    pub action: String,
    pub duration_ms: u64,
    pub status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AssertOutcome {
    pub assert_type: String,
    pub status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NodeSummary {
    pub id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub final_height: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub final_block_id: Option<String>,
}

pub struct StepTimer {
    start: Instant,
}

impl StepTimer {
    pub fn start() -> Self {
        Self {
            start: Instant::now(),
        }
    }

    pub fn elapsed_ms(&self) -> u64 {
        self.start.elapsed().as_millis() as u64
    }
}

impl Report {
    pub fn started(scenario: &str, seed: u64, run_id: &str) -> Self {
        Self {
            schema_version: REPORT_SCHEMA_VERSION,
            scenario: scenario.to_string(),
            seed,
            run_id: run_id.to_string(),
            status: "running".to_string(),
            error: None,
            started_at_epoch_secs: now_epoch_secs(),
            finished_at_epoch_secs: None,
            steps: Vec::new(),
            asserts: Vec::new(),
            nodes: Vec::new(),
        }
    }

    pub fn from_json(input: &str) -> anyhow::Result<Self> {
        let report: Self = serde_json::from_str(input)?;
        if report.schema_version != REPORT_SCHEMA_VERSION {
            anyhow::bail!(
                "unsupported nockchain-e2e report schema version {}", report.schema_version
            );
        }
        Ok(report)
    }

    pub fn record_step(&mut self, index: usize, action: &str, duration_ms: u64, ok: bool) {
        self.steps.push(StepRecord {
            index,
            action: action.to_string(),
            duration_ms,
            status: if ok { "passed" } else { "failed" }.to_string(),
            detail: None,
        });
    }

    pub fn record_step_with_detail(
        &mut self,
        index: usize,
        action: &str,
        duration_ms: u64,
        ok: bool,
        detail: String,
    ) {
        self.steps.push(StepRecord {
            index,
            action: action.to_string(),
            duration_ms,
            status: if ok { "passed" } else { "failed" }.to_string(),
            detail: Some(detail),
        });
    }

    pub fn record_assert(&mut self, assert_type: &str, ok: bool, detail: Option<String>) {
        self.asserts.push(AssertOutcome {
            assert_type: assert_type.to_string(),
            status: if ok { "passed" } else { "failed" }.to_string(),
            detail,
        });
    }

    pub fn record_node(&mut self, id: &str, height: Option<u64>, block_id: Option<String>) {
        self.nodes.push(NodeSummary {
            id: id.to_string(),
            final_height: height,
            final_block_id: block_id,
        });
    }

    pub fn finish_ok(&mut self) {
        self.status = "passed".to_string();
        self.finished_at_epoch_secs = Some(now_epoch_secs());
    }

    pub fn finish_err(&mut self, err: &str) {
        self.status = "failed".to_string();
        self.error = Some(err.to_string());
        self.finished_at_epoch_secs = Some(now_epoch_secs());
    }

    pub fn write_json(&self, path: impl AsRef<Path>) -> anyhow::Result<()> {
        let data = serde_json::to_vec_pretty(self)?;
        if let Some(parent) = path.as_ref().parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(path, data)?;
        Ok(())
    }
}

fn now_epoch_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

#[cfg(test)]
mod tests {
    use super::{Report, REPORT_SCHEMA_VERSION};

    #[test]
    fn report_v1_round_trips_with_legacy_fields_intact() {
        let mut report = Report::started("scenario", 7, "run-1");
        report.record_step(0, "start", 12, true);
        report.record_assert("height", true, Some("matched".to_owned()));
        report.record_node("node-0", Some(10), Some("block-10".to_owned()));
        report.finish_ok();
        let json = serde_json::to_string(&report).expect("serialize report");
        let decoded = Report::from_json(&json).expect("decode report");
        assert_eq!(decoded, report);
        assert_eq!(decoded.schema_version, REPORT_SCHEMA_VERSION);
        assert_eq!(decoded.steps[0].action, "start");
        assert_eq!(decoded.asserts[0].assert_type, "height");
        assert_eq!(decoded.nodes[0].id, "node-0");
    }

    #[test]
    fn report_rejects_unknown_schema_version() {
        let report = Report::started("scenario", 7, "run-1");
        let mut value = serde_json::to_value(report).expect("serialize report");
        value["schema_version"] = serde_json::json!(99);
        assert!(Report::from_json(&value.to_string()).is_err());
    }
}

use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::error::TestkitError;

#[derive(Debug, Clone, Deserialize)]
pub struct Scenario {
    pub name: String,
    #[serde(default = "default_seed")]
    pub seed: u64,
    #[serde(default)]
    pub protocol_version: Option<String>,
    #[serde(default)]
    pub binaries: BTreeMap<String, PathBuf>,
    #[serde(default)]
    pub nodes: Vec<NodeSpec>,
    #[serde(default)]
    pub steps: Vec<Action>,
    #[serde(default)]
    pub asserts: Vec<Assert>,
}

impl Scenario {
    pub fn load_from_path(path: impl AsRef<Path>) -> Result<Self, TestkitError> {
        let contents = std::fs::read_to_string(path)?;
        let scenario: Scenario = serde_yaml::from_str(&contents)?;
        scenario.validate()?;
        Ok(scenario)
    }

    fn validate(&self) -> Result<(), TestkitError> {
        if self.name.trim().is_empty() {
            return Err(TestkitError::Invalid(
                "scenario name is required".to_string(),
            ));
        }
        let mut seen = HashSet::new();
        for node in &self.nodes {
            if node.id.trim().is_empty() {
                return Err(TestkitError::Invalid("node id is required".to_string()));
            }
            if !seen.insert(node.id.clone()) {
                return Err(TestkitError::Invalid(format!(
                    "duplicate node id '{}'",
                    node.id
                )));
            }
        }
        Ok(())
    }
}

fn default_seed() -> u64 {
    1
}

fn default_true() -> bool {
    true
}

#[derive(Debug, Clone, Deserialize)]
pub struct NodeSpec {
    pub id: String,
    #[serde(default)]
    pub grpc_public_addr: Option<String>,
    #[serde(default)]
    pub grpc_private_port: Option<u16>,
    #[serde(default = "default_true")]
    pub grpc_enabled: bool,
    #[serde(default)]
    pub data_dir: Option<PathBuf>,
    #[serde(default)]
    pub fakenet: bool,
    #[serde(default)]
    pub mine: bool,
    #[serde(default)]
    pub mining_pkh: Option<String>,
    #[serde(default)]
    pub peers: Vec<String>,
    #[serde(default)]
    pub peer_from: Vec<PeerFrom>,
    #[serde(default)]
    pub restart_peer_from: Vec<PeerFrom>,
    #[serde(default)]
    pub force_peers: Vec<String>,
    #[serde(default)]
    pub bind: Vec<String>,
    #[serde(default)]
    pub new_state: bool,
    #[serde(default)]
    pub no_default_peers: bool,
    #[serde(default)]
    pub allowed_peers_path: Option<PathBuf>,
    #[serde(default)]
    pub fakenet_pow_len: Option<u64>,
    #[serde(default)]
    pub fakenet_log_difficulty: Option<u64>,
    #[serde(default)]
    pub fakenet_v1_phase: Option<u64>,
    #[serde(default)]
    pub fakenet_bythos_phase: Option<u64>,
    #[serde(default)]
    pub fakenet_update_candidate_interval_secs: Option<u64>,
    #[serde(default)]
    pub fakenet_genesis_jam_path: Option<PathBuf>,
    #[serde(default)]
    pub extra_args: Vec<String>,
    #[serde(default)]
    pub env: BTreeMap<String, String>,
    #[serde(default)]
    pub binary: Option<PathBuf>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct PeerFrom {
    pub node: String,
    pub listen: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case")]
pub enum Action {
    StartNodes {
        ids: Vec<String>,
    },
    StopNodes {
        ids: Vec<String>,
    },
    WaitForGrpc {
        node: String,
        #[serde(default)]
        timeout_ms: Option<u64>,
    },
    WaitForHeight {
        node: String,
        height: u64,
        #[serde(default)]
        timeout_ms: Option<u64>,
    },
    WaitForHeadsEqual {
        nodes: Vec<String>,
        #[serde(default)]
        timeout_ms: Option<u64>,
    },
    WaitForTxAccepted {
        node: String,
        tx: String,
        #[serde(default)]
        timeout_ms: Option<u64>,
    },
    WaitForTxInBlock {
        node: String,
        tx: String,
        #[serde(default)]
        timeout_ms: Option<u64>,
    },
    PeekConstants {
        node: String,
    },
    Sleep {
        millis: u64,
    },
    SubmitTx {
        node: String,
        fixture: String,
        #[serde(default)]
        wallet: Option<String>,
        #[serde(default)]
        expect: Option<SubmitTxExpect>,
        #[serde(default)]
        tx_id_override: Option<String>,
        #[serde(default)]
        store_as: Option<String>,
    },
    InjectBlock {
        node: String,
        fixture: String,
    },
    SetMiningPkh {
        node: String,
        value: String,
    },
    DisableMining {
        node: String,
    },
    SetMiningEnabled {
        node: String,
        enabled: bool,
    },
    SetNodeEnv {
        node: String,
        key: String,
        value: String,
    },
    Partition {
        groups: Vec<Vec<String>>,
    },
    Upgrade {
        node: String,
        version: String,
    },
    Tick {
        millis: u64,
    },
    Wallet {
        wallet: String,
        node: String,
        command: String,
        #[serde(default)]
        args: Vec<String>,
        #[serde(default)]
        expect: Option<String>,
        #[serde(default)]
        expect_exit_code: Option<i32>,
        #[serde(default)]
        capture: Option<WalletCapture>,
    },
    CloneWallet {
        from: String,
        to: String,
    },
    Command {
        command: String,
        #[serde(default)]
        args: Vec<String>,
        #[serde(default)]
        env: BTreeMap<String, String>,
        #[serde(default)]
        cwd: Option<PathBuf>,
        #[serde(default)]
        expect: Option<String>,
        #[serde(default)]
        expect_exit_code: Option<i32>,
    },
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SubmitTxExpect {
    Ack,
    Error,
}

#[derive(Debug, Clone, Deserialize)]
pub struct WalletCapture {
    pub regex: String,
    pub store_as: String,
    #[serde(default)]
    pub source: WalletCaptureSource,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WalletCaptureSource {
    #[default]
    Stdout,
    Stderr,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "assert", rename_all = "snake_case")]
pub enum Assert {
    GrpcReady {
        node: String,
    },
    HeadsEqual {
        nodes: Vec<String>,
        #[serde(default)]
        timeout_ms: Option<u64>,
    },
    HeadsNotEqual {
        nodes: Vec<String>,
    },
    HeightAtLeast {
        node: String,
        height: u64,
    },
    TxAccepted {
        node: String,
        tx: String,
    },
    TxInBlock {
        node: String,
        tx: String,
    },
    TxNotAccepted {
        node: String,
        tx: String,
    },
    ReqResGeneration {
        node: String,
        peer: String,
        generation: ReqResGenerationExpectation,
        #[serde(default)]
        timeout_ms: Option<u64>,
    },
}

#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ReqResGenerationExpectation {
    Gen1,
    Gen2,
}

#[cfg(test)]
mod tests {
    #[test]
    fn heads_equal_timeout_deserializes() {
        let scenario: crate::scenario::Scenario = serde_yaml::from_str(
            r#"
name: "wait_for_sync"
asserts:
  - assert: heads_equal
    nodes: ["node-a", "node-b"]
    timeout_ms: 120000
"#,
        )
        .expect("scenario should deserialize");

        match &scenario.asserts[0] {
            crate::scenario::Assert::HeadsEqual { nodes, timeout_ms } => {
                assert_eq!(nodes, &vec!["node-a".to_string(), "node-b".to_string()]);
                assert_eq!(*timeout_ms, Some(120000));
            }
            other => panic!("unexpected assert variant: {other:?}"),
        }
    }

    #[test]
    fn wait_for_heads_equal_action_deserializes() {
        let scenario: crate::scenario::Scenario = serde_yaml::from_str(
            r#"
name: "wait_for_heads_equal_step"
steps:
  - action: wait_for_heads_equal
    nodes: ["node-a", "node-b"]
    timeout_ms: 180000
"#,
        )
        .expect("scenario should deserialize");

        match &scenario.steps[0] {
            crate::scenario::Action::WaitForHeadsEqual { nodes, timeout_ms } => {
                assert_eq!(nodes, &vec!["node-a".to_string(), "node-b".to_string()]);
                assert_eq!(*timeout_ms, Some(180000));
            }
            other => panic!("unexpected action variant: {other:?}"),
        }
    }

    #[test]
    fn set_node_env_action_deserializes() {
        let scenario: crate::scenario::Scenario = serde_yaml::from_str(
            r#"
name: "set_node_env"
steps:
  - action: set_node_env
    node: "node-a"
    key: "NOCKCHAIN_LIBP2P_REQ_RES_GEN2_SEND_ENABLED"
    value: "true"
"#,
        )
        .expect("scenario should deserialize");

        match &scenario.steps[0] {
            crate::scenario::Action::SetNodeEnv { node, key, value } => {
                assert_eq!(node, "node-a");
                assert_eq!(key, "NOCKCHAIN_LIBP2P_REQ_RES_GEN2_SEND_ENABLED");
                assert_eq!(value, "true");
            }
            other => panic!("unexpected action variant: {other:?}"),
        }
    }

    #[test]
    fn restart_peer_from_deserializes() {
        let scenario: crate::scenario::Scenario = serde_yaml::from_str(
            r#"
name: "restart_peer_from"
nodes:
  - id: "node-a"
    restart_peer_from:
      - node: "node-b"
        listen: "/ip4/127.0.0.1/udp/4101/quic-v1"
"#,
        )
        .expect("scenario should deserialize");

        let node = &scenario.nodes[0];
        assert_eq!(node.restart_peer_from.len(), 1);
        assert_eq!(node.restart_peer_from[0].node, "node-b");
        assert_eq!(
            node.restart_peer_from[0].listen,
            "/ip4/127.0.0.1/udp/4101/quic-v1"
        );
    }

    #[test]
    fn wait_for_tx_in_block_and_assert_deserialize() {
        let scenario: crate::scenario::Scenario = serde_yaml::from_str(
            r#"
name: "tx_in_block"
steps:
  - action: wait_for_tx_in_block
    node: "node-a"
    tx: "last"
    timeout_ms: 45000
asserts:
  - assert: tx_in_block
    node: "node-b"
    tx: "last"
"#,
        )
        .expect("scenario should deserialize");

        match &scenario.steps[0] {
            crate::scenario::Action::WaitForTxInBlock {
                node,
                tx,
                timeout_ms,
            } => {
                assert_eq!(node, "node-a");
                assert_eq!(tx, "last");
                assert_eq!(*timeout_ms, Some(45000));
            }
            other => panic!("unexpected action variant: {other:?}"),
        }

        match &scenario.asserts[0] {
            crate::scenario::Assert::TxInBlock { node, tx } => {
                assert_eq!(node, "node-b");
                assert_eq!(tx, "last");
            }
            other => panic!("unexpected assert variant: {other:?}"),
        }
    }

    #[test]
    fn command_action_deserializes() {
        let scenario: crate::scenario::Scenario = serde_yaml::from_str(
            r#"
name: "run_command"
steps:
  - action: command
    command: "bash"
    args: ["./scripts/testnet/block_stuffer.sh"]
    env:
      MAX_ITERATIONS: "4"
    cwd: "target/testnet-stuffer"
    expect: "completed successfully"
    expect_exit_code: 0
"#,
        )
        .expect("scenario should deserialize");

        match &scenario.steps[0] {
            crate::scenario::Action::Command {
                command,
                args,
                env,
                cwd,
                expect,
                expect_exit_code,
            } => {
                assert_eq!(command, "bash");
                assert_eq!(
                    args,
                    &vec!["./scripts/testnet/block_stuffer.sh".to_string()]
                );
                assert_eq!(env.get("MAX_ITERATIONS").map(String::as_str), Some("4"));
                assert_eq!(
                    cwd.as_ref().map(|path| path.to_string_lossy().to_string()),
                    Some("target/testnet-stuffer".to_string())
                );
                assert_eq!(expect.as_deref(), Some("completed successfully"));
                assert_eq!(*expect_exit_code, Some(0));
            }
            other => panic!("unexpected action variant: {other:?}"),
        }
    }

    #[test]
    fn req_res_generation_assert_deserializes() {
        let scenario: crate::scenario::Scenario = serde_yaml::from_str(
            r#"
name: "req_res_generation"
asserts:
  - assert: req_res_generation
    node: "node-b"
    peer: "node-a"
    generation: gen1
    timeout_ms: 30000
"#,
        )
        .expect("scenario should deserialize");

        match &scenario.asserts[0] {
            crate::scenario::Assert::ReqResGeneration {
                node,
                peer,
                generation,
                timeout_ms,
            } => {
                assert_eq!(node, "node-b");
                assert_eq!(peer, "node-a");
                assert_eq!(
                    *generation,
                    crate::scenario::ReqResGenerationExpectation::Gen1
                );
                assert_eq!(*timeout_ms, Some(30000));
            }
            other => panic!("unexpected assert variant: {other:?}"),
        }
    }

    #[test]
    fn nous_testnet_gen2_send_scenario_deserializes() {
        let scenario: crate::scenario::Scenario = serde_yaml::from_str(include_str!(
            "../../../tests/e2e/scenarios/nous_testnet_gen2_send.yaml"
        ))
        .expect("scenario should deserialize");

        assert_eq!(scenario.name, "nous_testnet_gen2_send");
        assert_eq!(scenario.seed, 26);
        assert_eq!(scenario.nodes.len(), 4);
        assert!(scenario
            .steps
            .iter()
            .any(|step| matches!(step, crate::scenario::Action::Command { .. })));
        assert!(scenario
            .steps
            .iter()
            .any(|step| matches!(step, crate::scenario::Action::SetNodeEnv { .. })));

        let stuffer_clone_count = scenario
            .steps
            .iter()
            .filter(|step| {
                matches!(
                    step,
                    crate::scenario::Action::CloneWallet { from, to }
                    if from == "miner-a" && to == "stuffer"
                )
            })
            .count();
        assert_eq!(
            stuffer_clone_count, 2,
            "expected staged-send rehearsal to refresh the stuffer wallet before each load leg"
        );
    }

    #[test]
    fn nous_gen2_multi_sender_scenario_deserializes() {
        let scenario: crate::scenario::Scenario = serde_yaml::from_str(include_str!(
            "../../../tests/e2e/scenarios/nous_gen2_multi_sender.yaml"
        ))
        .expect("scenario should deserialize");

        assert_eq!(scenario.name, "nous_gen2_multi_sender");
        assert_eq!(scenario.seed, 31);
        assert_eq!(scenario.nodes.len(), 4);
        assert!(scenario
            .steps
            .iter()
            .any(|step| matches!(step, crate::scenario::Action::Command { .. })));
        assert!(scenario
            .asserts
            .iter()
            .any(|assertion| matches!(assertion, crate::scenario::Assert::HeadsEqual { .. })));
    }

    #[test]
    fn rollout_matrix_scenarios_include_req_res_generation_asserts() {
        for (path, contents) in [
            (
                "../../../tests/e2e/scenarios/nous_shipped_default.yaml",
                include_str!("../../../tests/e2e/scenarios/nous_shipped_default.yaml"),
            ),
            (
                "../../../tests/e2e/scenarios/nous_gen2_enabled.yaml",
                include_str!("../../../tests/e2e/scenarios/nous_gen2_enabled.yaml"),
            ),
            (
                "../../../tests/e2e/scenarios/nous_mixed_generation.yaml",
                include_str!("../../../tests/e2e/scenarios/nous_mixed_generation.yaml"),
            ),
            (
                "../../../tests/e2e/scenarios/nous_rollback.yaml",
                include_str!("../../../tests/e2e/scenarios/nous_rollback.yaml"),
            ),
            (
                "../../../tests/e2e/scenarios/nous_old_new_fallback.yaml",
                include_str!("../../../tests/e2e/scenarios/nous_old_new_fallback.yaml"),
            ),
            (
                "../../../tests/e2e/scenarios/nous_testnet_gen2_send.yaml",
                include_str!("../../../tests/e2e/scenarios/nous_testnet_gen2_send.yaml"),
            ),
        ] {
            let scenario: crate::scenario::Scenario =
                serde_yaml::from_str(contents).expect("scenario should deserialize");

            assert!(
                scenario.asserts.iter().any(|assertion| matches!(
                    assertion,
                    crate::scenario::Assert::ReqResGeneration { .. }
                )),
                "expected req_res_generation assert in {path}"
            );
        }
    }

    #[test]
    fn nous_gen2_partition_reorg_scenario_deserializes() {
        let scenario: crate::scenario::Scenario = serde_yaml::from_str(include_str!(
            "../../../tests/e2e/scenarios/nous_gen2_partition_reorg.yaml"
        ))
        .expect("scenario should deserialize");

        assert_eq!(scenario.name, "nous_gen2_partition_reorg");
        assert_eq!(scenario.nodes.len(), 3);
        assert!(scenario
            .steps
            .iter()
            .any(|step| matches!(step, crate::scenario::Action::Partition { .. })));
        assert!(scenario
            .steps
            .iter()
            .any(|step| matches!(step, crate::scenario::Action::WaitForTxInBlock { .. })));
        assert!(scenario
            .asserts
            .iter()
            .any(|assert| matches!(assert, crate::scenario::Assert::HeadsEqual { .. })));
    }

    #[test]
    fn nous_gen2_soak_scenario_deserializes() {
        let scenario: crate::scenario::Scenario = serde_yaml::from_str(include_str!(
            "../../../tests/e2e/scenarios/nous_gen2_soak.yaml"
        ))
        .expect("scenario should deserialize");

        assert_eq!(scenario.name, "nous_gen2_soak");
        assert_eq!(scenario.seed, 31);
        assert_eq!(scenario.nodes.len(), 4);
        assert!(scenario
            .steps
            .iter()
            .any(|step| matches!(step, crate::scenario::Action::CloneWallet { .. })));
        assert!(
            scenario
                .steps
                .iter()
                .filter(|step| matches!(step, crate::scenario::Action::Command { .. }))
                .count()
                >= 2
        );
        assert!(scenario
            .steps
            .iter()
            .any(|step| matches!(step, crate::scenario::Action::Sleep { .. })));
    }

    #[test]
    fn nous_gen2_long_haul_scenario_deserializes() {
        let scenario: crate::scenario::Scenario = serde_yaml::from_str(include_str!(
            "../../../tests/e2e/scenarios/nous_gen2_long_haul.yaml"
        ))
        .expect("scenario should deserialize");

        assert_eq!(scenario.name, "nous_gen2_long_haul");
        assert_eq!(scenario.seed, 41);
        assert_eq!(scenario.nodes.len(), 7);
        assert!(
            scenario
                .steps
                .iter()
                .filter(|step| matches!(step, crate::scenario::Action::WaitForHeight { .. }))
                .count()
                >= 3
        );
        assert!(
            scenario
                .steps
                .iter()
                .filter(|step| matches!(step, crate::scenario::Action::Command { .. }))
                .count()
                >= 2
        );
        assert!(scenario
            .steps
            .iter()
            .any(|step| matches!(step, crate::scenario::Action::Sleep { .. })));
        assert!(scenario
            .asserts
            .iter()
            .any(|assertion| matches!(assertion, crate::scenario::Assert::HeadsEqual { .. })));
    }

    #[test]
    fn nous_gen2_double_spend_scenario_deserializes() {
        let scenario: crate::scenario::Scenario = serde_yaml::from_str(include_str!(
            "../../../tests/e2e/scenarios/nous_gen2_double_spend.yaml"
        ))
        .expect("scenario should deserialize");

        assert_eq!(scenario.name, "nous_gen2_double_spend");
        assert_eq!(scenario.nodes.len(), 2);
        assert!(scenario.steps.iter().any(|step| matches!(
            step,
            crate::scenario::Action::Wallet { command, .. } if command == "send-tx"
        )));
        assert!(scenario
            .asserts
            .iter()
            .any(|assert| matches!(assert, crate::scenario::Assert::TxNotAccepted { .. })));
    }

    #[test]
    fn nous_gen2_invalid_tx_scenario_deserializes() {
        let scenario: crate::scenario::Scenario = serde_yaml::from_str(include_str!(
            "../../../tests/e2e/scenarios/nous_gen2_invalid_tx.yaml"
        ))
        .expect("scenario should deserialize");

        assert_eq!(scenario.name, "nous_gen2_invalid_tx");
        assert_eq!(scenario.nodes.len(), 2);
        assert!(scenario
            .steps
            .iter()
            .any(|step| matches!(step, crate::scenario::Action::SubmitTx { .. })));
        assert!(scenario.steps.iter().any(|step| matches!(
            step,
            crate::scenario::Action::Wallet { command, .. } if command == "send-tx"
        )));
    }
}

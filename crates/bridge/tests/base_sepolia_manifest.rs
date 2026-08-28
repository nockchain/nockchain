use std::collections::HashSet;

use bridge::shared::e2e_environment::{
    BaseSepoliaE2eManifest, BASE_SEPOLIA_BRIDGE_THRESHOLD, BASE_SEPOLIA_E2E_CHAIN_ID,
    BASE_SEPOLIA_E2E_ENVIRONMENT_ID, BASE_SEPOLIA_E2E_SCHEMA_ID, BASE_SEPOLIA_E2E_SCHEMA_VERSION,
    BASE_SEPOLIA_SOURCE_CHAIN_ID,
};
use serde_json::{json, Value};

const MANIFEST_JSON: &str = include_str!("../e2e/environments/base-sepolia.json");
const SCRIPT_ENV_EXAMPLE: &str = include_str!("../scripts/environments/base-sepolia.env.example");
const TENDERLY_CONTRACTS: &str = include_str!("../contracts/tenderly.yaml");
const DEPLOYMENT_REFERENCE: &str =
    include_str!("../contracts/environments/base-sepolia-testnet-accounts.md");

fn manifest_value() -> Value {
    serde_json::from_str(MANIFEST_JSON).expect("checked-in manifest must be JSON")
}

fn parse_value(value: &Value) -> Result<BaseSepoliaE2eManifest, String> {
    BaseSepoliaE2eManifest::from_json(&value.to_string()).map_err(|error| error.to_string())
}

fn collect_urls<'a>(value: &'a Value, urls: &mut Vec<&'a str>) {
    match value {
        Value::Array(values) => {
            for value in values {
                collect_urls(value, urls);
            }
        }
        Value::Object(values) => {
            for value in values.values() {
                collect_urls(value, urls);
            }
        }
        Value::String(value) if value.starts_with("http://") || value.starts_with("https://") => {
            urls.push(value);
        }
        _ => {}
    }
}

fn collect_keys<'a>(value: &'a Value, keys: &mut Vec<&'a str>) {
    match value {
        Value::Array(values) => {
            for value in values {
                collect_keys(value, keys);
            }
        }
        Value::Object(values) => {
            for (key, value) in values {
                keys.push(key);
                collect_keys(value, keys);
            }
        }
        _ => {}
    }
}

#[test]
fn checked_in_manifest_parses_and_round_trips() {
    let manifest = BaseSepoliaE2eManifest::from_json(MANIFEST_JSON)
        .expect("checked-in Base Sepolia manifest must validate");

    assert_eq!(manifest.schema_id, BASE_SEPOLIA_E2E_SCHEMA_ID);
    assert_eq!(manifest.schema_version, BASE_SEPOLIA_E2E_SCHEMA_VERSION);
    assert_eq!(manifest.environment_id, BASE_SEPOLIA_E2E_ENVIRONMENT_ID);
    assert_eq!(manifest.source_chain.chain_id, BASE_SEPOLIA_SOURCE_CHAIN_ID);
    assert_eq!(manifest.local_fork.chain_id, BASE_SEPOLIA_E2E_CHAIN_ID);
    assert_eq!(
        manifest.pristine_state.threshold,
        BASE_SEPOLIA_BRIDGE_THRESHOLD
    );

    let rendered = manifest
        .to_pretty_json()
        .expect("validated manifest must serialize");
    let reparsed =
        BaseSepoliaE2eManifest::from_json(&rendered).expect("serialized manifest must validate");
    assert_eq!(reparsed, manifest);
}

#[test]
fn manifest_rejects_missing_fields_and_unknown_shape() {
    let mut missing = manifest_value();
    missing
        .as_object_mut()
        .expect("manifest must be an object")
        .remove("source_chain");
    assert!(parse_value(&missing).is_err());

    let mut unknown = manifest_value();
    unknown["contracts"]["nock"]["rpc_url"] = json!("https://example.invalid");
    assert!(parse_value(&unknown).is_err());
}

#[test]
fn manifest_rejects_duplicate_bridge_nodes() {
    let mut value = manifest_value();
    let first = value["pristine_state"]["bridge_nodes"][0].clone();
    value["pristine_state"]["bridge_nodes"][1] = first;

    let error = parse_value(&value).expect_err("duplicate bridge nodes must be rejected");
    assert!(
        error.contains("duplicate address"),
        "unexpected error: {error}"
    );
}

#[test]
fn manifest_rejects_threshold_drift() {
    let mut value = manifest_value();
    value["pristine_state"]["threshold"] = json!(BASE_SEPOLIA_BRIDGE_THRESHOLD + 1);

    let error = parse_value(&value).expect_err("threshold drift must be rejected");
    assert!(
        error.contains("contract-observed value"),
        "unexpected error: {error}"
    );
}

#[test]
fn manifest_rejects_wrong_hash_lengths_and_noncanonical_addresses() {
    let mut short_hash = manifest_value();
    short_hash["contracts"]["message_inbox"]["proxy"]["runtime_code_keccak256"] = json!("0x1234");
    let error = parse_value(&short_hash).expect_err("short code hash must be rejected");
    assert!(error.contains("32-byte"), "unexpected error: {error}");

    let mut mixed_case = manifest_value();
    mixed_case["contracts"]["nock"]["address"] =
        json!("0xA9cd4087D9B050D8B35727AAf810296CA957c7B3");
    let error = parse_value(&mixed_case).expect_err("mixed-case address must be rejected");
    assert!(
        error.contains("canonical lowercase hex"),
        "unexpected error: {error}"
    );
}

#[test]
fn manifest_rejects_pinned_block_number_and_hash_mismatch() {
    let manifest = BaseSepoliaE2eManifest::from_json(MANIFEST_JSON)
        .expect("checked-in Base Sepolia manifest must validate");
    let block = &manifest.source_chain.fork_block;

    manifest
        .validate_pinned_block(block.number, &block.hash)
        .expect("matching pinned block must validate");
    let number_error = manifest
        .validate_pinned_block(block.number + 1, &block.hash)
        .expect_err("wrong block number must fail");
    assert!(number_error.to_string().contains("number mismatch"));

    let wrong_hash = "0x1111111111111111111111111111111111111111111111111111111111111111";
    let hash_error = manifest
        .validate_pinned_block(block.number, wrong_hash)
        .expect_err("wrong block hash must fail");
    assert!(hash_error.to_string().contains("hash mismatch"));
}

#[test]
fn manifest_is_the_script_environment_address_source() {
    let manifest = BaseSepoliaE2eManifest::from_json(MANIFEST_JSON)
        .expect("checked-in Base Sepolia manifest must validate");
    let env_lower = SCRIPT_ENV_EXAMPLE.to_ascii_lowercase();

    assert!(SCRIPT_ENV_EXAMPLE.contains(".contracts.message_inbox.proxy.address"));
    assert!(SCRIPT_ENV_EXAMPLE.contains(".contracts.nock.address"));
    assert!(SCRIPT_ENV_EXAMPLE.contains(".pristine_state.bridge_nodes"));
    assert!(!env_lower.contains(&manifest.contracts.message_inbox.proxy.address));
    assert!(!env_lower.contains(&manifest.contracts.nock.address));

    let tenderly_lower = TENDERLY_CONTRACTS.to_ascii_lowercase();
    for address in [
        &manifest.contracts.message_inbox.proxy.address,
        &manifest.contracts.message_inbox.implementation.address, &manifest.contracts.nock.address,
    ] {
        assert!(
            tenderly_lower.contains(address),
            "Tenderly reference does not match manifest address {address}"
        );
    }
    assert!(DEPLOYMENT_REFERENCE.contains("e2e/environments/base-sepolia.json"));
}

#[test]
fn manifest_contains_only_public_explorer_urls_and_no_secret_fields() {
    let value = manifest_value();
    let mut urls = Vec::new();
    collect_urls(&value, &mut urls);
    assert!(!urls.is_empty());
    assert!(urls
        .iter()
        .all(|url| url.starts_with("https://base-sepolia.blockscout.com/")));

    let mut keys = Vec::new();
    collect_keys(&value, &mut keys);
    let forbidden: HashSet<&str> = [
        "rpc_url", "ws_url", "private_key", "secret", "credential", "access_key", "token",
    ]
    .into_iter()
    .collect();
    assert!(keys.iter().all(|key| !forbidden.contains(*key)));
}

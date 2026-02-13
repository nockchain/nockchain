use std::fs;
use std::str::FromStr;

use alloy::primitives::Address;
use bridge::config::BridgeConfigToml;
use ibig::UBig;
use nockchain_math::belt::{Belt, PRIME};
use nockchain_types::tx_engine::common::Hash as NockPkh;
use tempfile::TempDir;

// FIXME: Bad third-party inputs shouldn't be able to induce panics, this should be Result<>
fn base58_belts<const N: usize>(value: &str) -> [Belt; N] {
    let bytes = bs58::decode(value)
        .into_vec()
        .expect("Failed to decode base58 string");
    assert!(!bytes.is_empty());
    let mut big = UBig::from_be_bytes(&bytes);
    let prime = UBig::from(PRIME);
    let mut belts = [Belt(0); N];
    for belt in belts.iter_mut() {
        let rem = (&big % &prime)
            .try_into()
            .expect("Failed to convert remainder to u64");
        *belt = Belt(rem);
        big /= &prime;
    }
    assert!(big == UBig::from(0u8));
    belts
}

/// Fake test PKHs (valid base58 format, deterministic for tests)
/// These are NOT real operator PKHs - just valid format placeholders
fn sample_pkhs_b58() -> Vec<&'static str> {
    vec![
        "2222222222222222222222222222222222222222222222222222", // test node 0
        "3333333333333333333333333333333333333333333333333333", // test node 1
        "4444444444444444444444444444444444444444444444444444", // test node 2
        "5555555555555555555555555555555555555555555555555555", // test node 3
        "6666666666666666666666666666666666666666666666666666", // test node 4
    ]
}

fn sample_pkh_b58() -> &'static str {
    sample_pkhs_b58()[0]
}

#[test]
fn test_missing_confirmation_depths_fails_parse() {
    let temp_dir = TempDir::new().expect("Failed to create temporary directory");
    let config_path = temp_dir.path().join("bridge-conf.toml");
    let pkhs = sample_pkhs_b58();

    let config_content = format!(
        r#"
node_id = 0
base_ws_url = "wss://mainnet.base.org"
inbox_contract_address = "0x1234567890123456789012345678901234567890"
nock_contract_address = "0x0000000000000000000000000000000000000001"
my_eth_key = "0x4c0883a69102937d6231471b5dbb6204fe5129617082792ae468d01a3f362318"
my_nock_key = "3yZe7d"
grpc_address = "http://localhost:5555"
[[nodes]]
ip = "localhost:8001"
eth_pubkey = "0x2c7536E3605D9C16a7a3D7b1898e529396a65c23"
nock_pkh = "{pkh0}"

[[nodes]]
ip = "localhost:8002"
eth_pubkey = "0x0EE156f080d9cB3BaA3C0DB53D07f13D69CEf4C9"
nock_pkh = "{pkh1}"

[[nodes]]
ip = "localhost:8003"
eth_pubkey = "0x274BD645de480C325D618c60c661F11275eB77F1"
nock_pkh = "{pkh2}"

[[nodes]]
ip = "localhost:8004"
eth_pubkey = "0x6dc59eb20f7928935c47A391e35545a2CEC51013"
nock_pkh = "{pkh3}"

[[nodes]]
ip = "localhost:8005"
eth_pubkey = "0xcaB10dA05fC0aDBb7e91Eadc30f224bcDF601375"
nock_pkh = "{pkh4}"
"#,
        pkh0 = pkhs[0],
        pkh1 = pkhs[1],
        pkh2 = pkhs[2],
        pkh3 = pkhs[3],
        pkh4 = pkhs[4]
    );

    fs::write(&config_path, config_content).expect("Failed to write test config file");

    let result = BridgeConfigToml::from_file(&config_path);
    assert!(result.is_err());
}

#[test]
fn test_parse_confirmation_depths_present() {
    let temp_dir = TempDir::new().expect("Failed to create temporary directory");
    let config_path = temp_dir.path().join("bridge-conf.toml");
    let pkhs = sample_pkhs_b58();

    let config_content = format!(
        r#"
node_id = 0
base_ws_url = "wss://mainnet.base.org"
inbox_contract_address = "0x1234567890123456789012345678901234567890"
nock_contract_address = "0x0000000000000000000000000000000000000001"
my_eth_key = "0x4c0883a69102937d6231471b5dbb6204fe5129617082792ae468d01a3f362318"
my_nock_key = "3yZe7d"
grpc_address = "http://localhost:5555"
base_confirmation_depth = 301
nockchain_confirmation_depth = 123

[[nodes]]
ip = "localhost:8001"
eth_pubkey = "0x2c7536E3605D9C16a7a3D7b1898e529396a65c23"
nock_pkh = "{pkh0}"

[[nodes]]
ip = "localhost:8002"
eth_pubkey = "0x0EE156f080d9cB3BaA3C0DB53D07f13D69CEf4C9"
nock_pkh = "{pkh1}"

[[nodes]]
ip = "localhost:8003"
eth_pubkey = "0x274BD645de480C325D618c60c661F11275eB77F1"
nock_pkh = "{pkh2}"

[[nodes]]
ip = "localhost:8004"
eth_pubkey = "0x6dc59eb20f7928935c47A391e35545a2CEC51013"
nock_pkh = "{pkh3}"

[[nodes]]
ip = "localhost:8005"
eth_pubkey = "0xcaB10dA05fC0aDBb7e91Eadc30f224bcDF601375"
nock_pkh = "{pkh4}"
"#,
        pkh0 = pkhs[0],
        pkh1 = pkhs[1],
        pkh2 = pkhs[2],
        pkh3 = pkhs[3],
        pkh4 = pkhs[4]
    );

    fs::write(&config_path, config_content).expect("Failed to write test config file");

    let config =
        BridgeConfigToml::from_file(&config_path).expect("Failed to parse valid TOML config");
    assert_eq!(config.base_confirmation_depth, 301);
    assert_eq!(config.nockchain_confirmation_depth, 123);
}

#[test]
fn test_parse_valid_toml_config() {
    let temp_dir = TempDir::new().expect("Failed to create temporary directory");
    let config_path = temp_dir.path().join("bridge-conf.toml");
    let pkhs = sample_pkhs_b58();

    let config_content = format!(
        r#"
node_id = 0
base_ws_url = "wss://mainnet.base.org"
inbox_contract_address = "0x1234567890123456789012345678901234567890"
nock_contract_address = "0x0000000000000000000000000000000000000001"
my_eth_key = "0x4c0883a69102937d6231471b5dbb6204fe5129617082792ae468d01a3f362318"
my_nock_key = "3yZe7d"
grpc_address = "http://localhost:5555"
base_confirmation_depth = 300
nockchain_confirmation_depth = 100

[[nodes]]
ip = "localhost:8001"
eth_pubkey = "0x2c7536E3605D9C16a7a3D7b1898e529396a65c23"
nock_pkh = "{pkh0}"

[[nodes]]
ip = "localhost:8002"
eth_pubkey = "0x0EE156f080d9cB3BaA3C0DB53D07f13D69CEf4C9"
nock_pkh = "{pkh1}"

[[nodes]]
ip = "localhost:8003"
eth_pubkey = "0x274BD645de480C325D618c60c661F11275eB77F1"
nock_pkh = "{pkh2}"

[[nodes]]
ip = "localhost:8004"
eth_pubkey = "0x6dc59eb20f7928935c47A391e35545a2CEC51013"
nock_pkh = "{pkh3}"

[[nodes]]
ip = "localhost:8005"
eth_pubkey = "0xcaB10dA05fC0aDBb7e91Eadc30f224bcDF601375"
nock_pkh = "{pkh4}"
"#,
        pkh0 = pkhs[0],
        pkh1 = pkhs[1],
        pkh2 = pkhs[2],
        pkh3 = pkhs[3],
        pkh4 = pkhs[4]
    );

    fs::write(&config_path, config_content).expect("Failed to write test config file");

    let config =
        BridgeConfigToml::from_file(&config_path).expect("Failed to parse valid TOML config");

    assert_eq!(config.node_id, 0);
    assert_eq!(config.base_ws_url, "wss://mainnet.base.org");
    assert_eq!(
        config.inbox_contract_address.as_deref(),
        Some("0x1234567890123456789012345678901234567890")
    );
    assert_eq!(
        config.nock_contract_address.as_deref(),
        Some("0x0000000000000000000000000000000000000001")
    );
    assert_eq!(config.nodes.len(), 5);
    assert_eq!(config.nodes[0].ip, "localhost:8001");

    let node_config = config
        .to_node_config()
        .expect("Failed to convert config to node config");
    let expected_pkh =
        NockPkh::from_base58(pkhs[0]).expect("Failed to parse expected pkh from base58");
    assert_eq!(node_config.nodes[0].nock_pkh, expected_pkh);
}

#[test]
fn test_hex_key_parsing() {
    let temp_dir = TempDir::new().expect("Failed to create temporary directory");
    let config_path = temp_dir.path().join("bridge-conf.toml");
    let pkhs = sample_pkhs_b58();

    let config_content = format!(
        r#"
node_id = 0
base_ws_url = "wss://mainnet.base.org"
inbox_contract_address = "0x1234567890123456789012345678901234567890"
nock_contract_address = "0x0000000000000000000000000000000000000001"
my_eth_key = "0xDEADBEEF"
my_nock_key = "3yZe7d"
grpc_address = "http://localhost:5555"
base_confirmation_depth = 300
nockchain_confirmation_depth = 100

[[nodes]]
ip = "localhost:8001"
eth_pubkey = "0x2c7536E3605D9C16a7a3D7b1898e529396a65c23"
nock_pkh = "{pkh0}"

[[nodes]]
ip = "localhost:8002"
eth_pubkey = "0x0EE156f080d9cB3BaA3C0DB53D07f13D69CEf4C9"
nock_pkh = "{pkh1}"

[[nodes]]
ip = "localhost:8003"
eth_pubkey = "0x274BD645de480C325D618c60c661F11275eB77F1"
nock_pkh = "{pkh2}"

[[nodes]]
ip = "localhost:8004"
eth_pubkey = "0x6dc59eb20f7928935c47A391e35545a2CEC51013"
nock_pkh = "{pkh3}"

[[nodes]]
ip = "localhost:8005"
eth_pubkey = "0xcaB10dA05fC0aDBb7e91Eadc30f224bcDF601375"
nock_pkh = "{pkh4}"
"#,
        pkh0 = pkhs[0],
        pkh1 = pkhs[1],
        pkh2 = pkhs[2],
        pkh3 = pkhs[3],
        pkh4 = pkhs[4]
    );

    fs::write(&config_path, config_content).expect("Failed to write test config file");

    let config_toml =
        BridgeConfigToml::from_file(&config_path).expect("Failed to parse TOML config");
    let node_config = config_toml
        .to_node_config()
        .expect("Failed to convert config to node config");

    assert_eq!(node_config.my_eth_key.as_slice(), &[0xDE, 0xAD, 0xBE, 0xEF]);
    assert_eq!(
        node_config.my_nock_key.limbs(),
        &base58_belts::<8>("3yZe7d")
    );
}

#[test]
fn test_base58_key_parsing() {
    let temp_dir = TempDir::new().expect("Failed to create temporary directory");
    let config_path = temp_dir.path().join("bridge-conf.toml");
    let pkhs = sample_pkhs_b58();

    let config_content = format!(
        r#"
node_id = 1
base_ws_url = "wss://mainnet.base.org"
inbox_contract_address = "0x1234567890123456789012345678901234567890"
nock_contract_address = "0x0000000000000000000000000000000000000001"
my_eth_key = "0x1234"
my_nock_key = "3yZe7d"
grpc_address = "http://localhost:5555"
base_confirmation_depth = 300
nockchain_confirmation_depth = 100

[[nodes]]
ip = "localhost:8001"
eth_pubkey = "0x2c7536E3605D9C16a7a3D7b1898e529396a65c23"
nock_pkh = "{pkh0}"

[[nodes]]
ip = "localhost:8002"
eth_pubkey = "0x0EE156f080d9cB3BaA3C0DB53D07f13D69CEf4C9"
nock_pkh = "{pkh1}"

[[nodes]]
ip = "localhost:8003"
eth_pubkey = "0x274BD645de480C325D618c60c661F11275eB77F1"
nock_pkh = "{pkh2}"

[[nodes]]
ip = "localhost:8004"
eth_pubkey = "0x6dc59eb20f7928935c47A391e35545a2CEC51013"
nock_pkh = "{pkh3}"

[[nodes]]
ip = "localhost:8005"
eth_pubkey = "0xcaB10dA05fC0aDBb7e91Eadc30f224bcDF601375"
nock_pkh = "{pkh4}"
"#,
        pkh0 = pkhs[0],
        pkh1 = pkhs[1],
        pkh2 = pkhs[2],
        pkh3 = pkhs[3],
        pkh4 = pkhs[4]
    );

    fs::write(&config_path, config_content).expect("Failed to write test config file");

    let config_toml =
        BridgeConfigToml::from_file(&config_path).expect("Failed to parse TOML config");
    let node_config = config_toml
        .to_node_config()
        .expect("Failed to convert config to node config");

    assert_eq!(
        node_config.my_nock_key.limbs(),
        &base58_belts::<8>("3yZe7d")
    );
}

#[test]
fn test_conversion_to_node_config() {
    let temp_dir = TempDir::new().expect("Failed to create temporary directory");
    let config_path = temp_dir.path().join("bridge-conf.toml");
    let pkhs = sample_pkhs_b58();

    let config_content = format!(
        r#"
node_id = 2
base_ws_url = "wss://mainnet.base.org"
inbox_contract_address = "0x1234567890123456789012345678901234567890"
nock_contract_address = "0x0000000000000000000000000000000000000001"
my_eth_key = "0xABCD"
my_nock_key = "3yZe7d"
deposit_address = "5HueCGU8rMjxEXxiPuD5BDku4MkFqeZyd4dZ1jvhTVqvbTLvyTJ"
grpc_address = "http://localhost:5555"
base_confirmation_depth = 300
nockchain_confirmation_depth = 100

[[nodes]]
ip = "node1.example.com"
eth_pubkey = "0x2c7536E3605D9C16a7a3D7b1898e529396a65c23"
nock_pkh = "{pkh0}"

[[nodes]]
ip = "node2.example.com"
eth_pubkey = "0x0EE156f080d9cB3BaA3C0DB53D07f13D69CEf4C9"
nock_pkh = "{pkh1}"

[[nodes]]
ip = "node3.example.com"
eth_pubkey = "0x274BD645de480C325D618c60c661F11275eB77F1"
nock_pkh = "{pkh2}"

[[nodes]]
ip = "node4.example.com"
eth_pubkey = "0x6dc59eb20f7928935c47A391e35545a2CEC51013"
nock_pkh = "{pkh3}"

[[nodes]]
ip = "node5.example.com"
eth_pubkey = "0xcaB10dA05fC0aDBb7e91Eadc30f224bcDF601375"
nock_pkh = "{pkh4}"
"#,
        pkh0 = pkhs[0],
        pkh1 = pkhs[1],
        pkh2 = pkhs[2],
        pkh3 = pkhs[3],
        pkh4 = pkhs[4]
    );

    fs::write(&config_path, config_content).expect("Failed to write test config file");

    let config_toml =
        BridgeConfigToml::from_file(&config_path).expect("Failed to parse TOML config");
    let node_config = config_toml
        .to_node_config()
        .expect("Failed to convert config to node config");

    assert_eq!(node_config.node_id, 2);
    assert_eq!(node_config.nodes.len(), 5);
    assert_eq!(node_config.nodes[0].ip, "node1.example.com");
    assert_eq!(node_config.nodes[4].ip, "node5.example.com");
}

#[test]
fn test_malformed_toml() {
    let temp_dir = TempDir::new().expect("Failed to create temporary directory");
    let config_path = temp_dir.path().join("bridge-conf.toml");

    let config_content = r#"
node_id = "not a number"
base_ws_url = "wss://mainnet.base.org"
"#;

    fs::write(&config_path, config_content).expect("Failed to write test config file");

    let result = BridgeConfigToml::from_file(&config_path);
    assert!(result.is_err());
}

#[test]
fn test_missing_config_file() {
    let temp_dir = TempDir::new().expect("Failed to create temporary directory");
    let config_path = temp_dir.path().join("nonexistent.toml");

    let result = BridgeConfigToml::from_file(&config_path);
    assert!(result.is_err());
}

#[test]
fn test_invalid_hex_key() {
    let temp_dir = TempDir::new().expect("Failed to create temporary directory");
    let config_path = temp_dir.path().join("bridge-conf.toml");
    let pkh = sample_pkh_b58();

    let config_content = format!(
        r#"
node_id = 0
base_ws_url = "wss://mainnet.base.org"
inbox_contract_address = "0x1234567890123456789012345678901234567890"
nock_contract_address = "0x0000000000000000000000000000000000000001"
my_eth_key = "0xZZZZ"
my_nock_key = "3yZe7d"
deposit_address = "5HueCGU8rMjxEXxiPuD5BDku4MkFqeZyd4dZ1jvhTVqvbTLvyTJ"
grpc_address = "http://localhost:5555"
base_confirmation_depth = 300
nockchain_confirmation_depth = 100

[[nodes]]
ip = "localhost:8001"
eth_pubkey = "0x2c7536E3605D9C16a7a3D7b1898e529396a65c23"
nock_pkh = "{pkh}"

[[nodes]]
ip = "localhost:8002"
eth_pubkey = "0x0EE156f080d9cB3BaA3C0DB53D07f13D69CEf4C9"
nock_pkh = "{pkh}"

[[nodes]]
ip = "localhost:8003"
eth_pubkey = "0x274BD645de480C325D618c60c661F11275eB77F1"
nock_pkh = "{pkh}"

[[nodes]]
ip = "localhost:8004"
eth_pubkey = "0x6dc59eb20f7928935c47A391e35545a2CEC51013"
nock_pkh = "{pkh}"

[[nodes]]
ip = "localhost:8005"
eth_pubkey = "0xcaB10dA05fC0aDBb7e91Eadc30f224bcDF601375"
nock_pkh = "{pkh}"
"#,
        pkh = pkh
    );

    fs::write(&config_path, config_content).expect("Failed to write test config file");

    let config_toml =
        BridgeConfigToml::from_file(&config_path).expect("Failed to parse TOML config");
    let result = config_toml.to_node_config();
    assert!(result.is_err());
}

#[test]
fn test_inbox_contract_address_parsing() {
    let temp_dir = TempDir::new().expect("Failed to create temporary directory");
    let config_path = temp_dir.path().join("bridge-conf.toml");
    let pkh = sample_pkh_b58();

    let config_content = format!(
        r#"
node_id = 0
base_ws_url = "wss://mainnet.base.org"
inbox_contract_address = "0x1234567890123456789012345678901234567890"
nock_contract_address = "0x0000000000000000000000000000000000000001"
my_eth_key = "0x1234"
my_nock_key = "3yZe7d"
deposit_address = "5HueCGU8rMjxEXxiPuD5BDku4MkFqeZyd4dZ1jvhTVqvbTLvyTJ"
grpc_address = "http://localhost:5555"
base_confirmation_depth = 300
nockchain_confirmation_depth = 100

[[nodes]]
ip = "localhost:8001"
eth_pubkey = "0x2c7536E3605D9C16a7a3D7b1898e529396a65c23"
nock_pkh = "{pkh}"

[[nodes]]
ip = "localhost:8002"
eth_pubkey = "0x0EE156f080d9cB3BaA3C0DB53D07f13D69CEf4C9"
nock_pkh = "{pkh}"

[[nodes]]
ip = "localhost:8003"
eth_pubkey = "0x274BD645de480C325D618c60c661F11275eB77F1"
nock_pkh = "{pkh}"

[[nodes]]
ip = "localhost:8004"
eth_pubkey = "0x6dc59eb20f7928935c47A391e35545a2CEC51013"
nock_pkh = "{pkh}"

[[nodes]]
ip = "localhost:8005"
eth_pubkey = "0xcaB10dA05fC0aDBb7e91Eadc30f224bcDF601375"
nock_pkh = "{pkh}"
"#,
        pkh = pkh
    );

    fs::write(&config_path, config_content).expect("Failed to write test config file");

    let config_toml =
        BridgeConfigToml::from_file(&config_path).expect("Failed to parse TOML config");
    let address = config_toml
        .inbox_contract_address()
        .expect("Failed to get inbox contract address");
    let expected_inbox = Address::from_str("0x1234567890123456789012345678901234567890")
        .expect("Failed to parse expected inbox address");
    assert_eq!(address, expected_inbox);

    let nock_address = config_toml
        .nock_contract_address()
        .expect("Failed to get nock contract address");
    let expected_nock = Address::from_str("0x0000000000000000000000000000000000000001")
        .expect("Failed to parse expected nock address");
    assert_eq!(nock_address, expected_nock);
}

#[test]
fn test_invalid_base58_pkh_length() {
    let temp_dir = TempDir::new().expect("Failed to create temporary directory");
    let config_path = temp_dir.path().join("bridge-conf.toml");
    let pkh = sample_pkh_b58();

    let config_content = format!(
        r#"
node_id = 0
base_ws_url = "wss://mainnet.base.org"
inbox_contract_address = "0x1234567890123456789012345678901234567890"
nock_contract_address = "0x0000000000000000000000000000000000000001"
my_eth_key = "0x1234"
my_nock_key = "3yZe7d"
deposit_address = "5HueCGU8rMjxEXxiPuD5BDku4MkFqeZyd4dZ1jvhTVqvbTLvyTJ"
grpc_address = "http://localhost:5555"
base_confirmation_depth = 300
nockchain_confirmation_depth = 100

[[nodes]]
ip = "localhost:8001"
eth_pubkey = "0x2c7536E3605D9C16a7a3D7b1898e529396a65c23"
nock_pkh = "short"

[[nodes]]
ip = "localhost:8002"
eth_pubkey = "0x0EE156f080d9cB3BaA3C0DB53D07f13D69CEf4C9"
nock_pkh = "{pkh}"

[[nodes]]
ip = "localhost:8003"
eth_pubkey = "0x274BD645de480C325D618c60c661F11275eB77F1"
nock_pkh = "{pkh}"

[[nodes]]
ip = "localhost:8004"
eth_pubkey = "0x6dc59eb20f7928935c47A391e35545a2CEC51013"
nock_pkh = "{pkh}"

[[nodes]]
ip = "localhost:8005"
eth_pubkey = "0xcaB10dA05fC0aDBb7e91Eadc30f224bcDF601375"
nock_pkh = "{pkh}"
"#,
        pkh = pkh
    );

    fs::write(&config_path, config_content).expect("Failed to write test config file");

    let config_toml =
        BridgeConfigToml::from_file(&config_path).expect("Failed to parse TOML config");
    let result = config_toml.to_node_config();
    assert!(result.is_err());
}

#[test]
fn test_invalid_eth_key() {
    let temp_dir = TempDir::new().expect("Failed to create temporary directory");
    let config_path = temp_dir.path().join("bridge-conf.toml");
    let pkh = sample_pkh_b58();

    let config_content = format!(
        r#"
node_id = 0
base_ws_url = "wss://mainnet.base.org"
inbox_contract_address = "0x1234567890123456789012345678901234567890"
nock_contract_address = "0x0000000000000000000000000000000000000001"
my_eth_key = "not a key"
my_nock_key = "3yZe7d"
deposit_address = "5HueCGU8rMjxEXxiPuD5BDku4MkFqeZyd4dZ1jvhTVqvbTLvyTJ"
grpc_address = "http://localhost:5555"
base_confirmation_depth = 300
nockchain_confirmation_depth = 100

[[nodes]]
ip = "localhost:8001"
eth_pubkey = "0x2c7536E3605D9C16a7a3D7b1898e529396a65c23"
nock_pkh = "{pkh}"

[[nodes]]
ip = "localhost:8002"
eth_pubkey = "0x0EE156f080d9cB3BaA3C0DB53D07f13D69CEf4C9"
nock_pkh = "{pkh}"

[[nodes]]
ip = "localhost:8003"
eth_pubkey = "0x274BD645de480C325D618c60c661F11275eB77F1"
nock_pkh = "{pkh}"

[[nodes]]
ip = "localhost:8004"
eth_pubkey = "0x6dc59eb20f7928935c47A391e35545a2CEC51013"
nock_pkh = "{pkh}"

[[nodes]]
ip = "localhost:8005"
eth_pubkey = "0xcaB10dA05fC0aDBb7e91Eadc30f224bcDF601375"
nock_pkh = "{pkh}"
"#,
        pkh = pkh
    );

    fs::write(&config_path, config_content).expect("Failed to write test config file");

    let config_toml =
        BridgeConfigToml::from_file(&config_path).expect("Failed to parse TOML config");
    let result = config_toml.to_node_config();
    assert!(result.is_err());
}

#[test]
fn test_invalid_node_count() {
    let temp_dir = TempDir::new().expect("Failed to create temporary directory");
    let config_path = temp_dir.path().join("bridge-conf.toml");
    let pkh = sample_pkh_b58();

    let config_content = format!(
        r#"
node_id = 0
base_ws_url = "wss://mainnet.base.org"
inbox_contract_address = "0x1234567890123456789012345678901234567890"
nock_contract_address = "0x0000000000000000000000000000000000000001"
my_eth_key = "0x1234"
my_nock_key = "3yZe7d"
deposit_address = "5HueCGU8rMjxEXxiPuD5BDku4MkFqeZyd4dZ1jvhTVqvbTLvyTJ"
grpc_address = "http://localhost:5555"
base_confirmation_depth = 300
nockchain_confirmation_depth = 100

[[nodes]]
ip = "localhost:8001"
eth_pubkey = "0x2c7536E3605D9C16a7a3D7b1898e529396a65c23"
nock_pkh = "{pkh}"

[[nodes]]
ip = "localhost:8002"
eth_pubkey = "0x0EE156f080d9cB3BaA3C0DB53D07f13D69CEf4C9"
nock_pkh = "{pkh}"

[[nodes]]
ip = "localhost:8003"
eth_pubkey = "0x274BD645de480C325D618c60c661F11275eB77F1"
nock_pkh = "{pkh}"
"#,
        pkh = pkh
    );

    fs::write(&config_path, config_content).expect("Failed to write test config file");

    let config_toml =
        BridgeConfigToml::from_file(&config_path).expect("Failed to parse TOML config");
    let result = config_toml.to_node_config();
    assert!(result.is_err());
}

#[test]
fn test_duplicate_node_ips() {
    let temp_dir = TempDir::new().expect("Failed to create temporary directory");
    let config_path = temp_dir.path().join("bridge-conf.toml");
    let pkh = sample_pkh_b58();

    let config_content = format!(
        r#"
node_id = 0
base_ws_url = "wss://mainnet.base.org"
inbox_contract_address = "0x1234567890123456789012345678901234567890"
nock_contract_address = "0x0000000000000000000000000000000000000001"
my_eth_key = "0x1234"
my_nock_key = "3yZe7d"
deposit_address = "5HueCGU8rMjxEXxiPuD5BDku4MkFqeZyd4dZ1jvhTVqvbTLvyTJ"
grpc_address = "http://localhost:5555"
base_confirmation_depth = 300
nockchain_confirmation_depth = 100

[[nodes]]
ip = "localhost:8001"
eth_pubkey = "0x2c7536E3605D9C16a7a3D7b1898e529396a65c23"
nock_pkh = "{pkh}"

[[nodes]]
ip = "localhost:8001"
eth_pubkey = "0x0EE156f080d9cB3BaA3C0DB53D07f13D69CEf4C9"
nock_pkh = "{pkh}"

[[nodes]]
ip = "localhost:8003"
eth_pubkey = "0x274BD645de480C325D618c60c661F11275eB77F1"
nock_pkh = "{pkh}"

[[nodes]]
ip = "localhost:8004"
eth_pubkey = "0x6dc59eb20f7928935c47A391e35545a2CEC51013"
nock_pkh = "{pkh}"

[[nodes]]
ip = "localhost:8005"
eth_pubkey = "0xcaB10dA05fC0aDBb7e91Eadc30f224bcDF601375"
nock_pkh = "{pkh}"
"#,
        pkh = pkh
    );

    fs::write(&config_path, config_content).expect("Failed to write test config file");

    let config_toml =
        BridgeConfigToml::from_file(&config_path).expect("Failed to parse TOML config");
    let result = config_toml.to_node_config();
    assert!(result.is_err());
}

#[test]
fn test_duplicate_eth_pubkeys() {
    let temp_dir = TempDir::new().expect("Failed to create temporary directory");
    let config_path = temp_dir.path().join("bridge-conf.toml");
    let pkh = sample_pkh_b58();

    let config_content = format!(
        r#"
node_id = 0
base_ws_url = "wss://mainnet.base.org"
inbox_contract_address = "0x1234567890123456789012345678901234567890"
nock_contract_address = "0x0000000000000000000000000000000000000001"
my_eth_key = "0x1234"
my_nock_key = "3yZe7d"
deposit_address = "5HueCGU8rMjxEXxiPuD5BDku4MkFqeZyd4dZ1jvhTVqvbTLvyTJ"
grpc_address = "http://localhost:5555"
base_confirmation_depth = 300
nockchain_confirmation_depth = 100

[[nodes]]
ip = "localhost:8001"
eth_pubkey = "0x2c7536E3605D9C16a7a3D7b1898e529396a65c23"
nock_pkh = "{pkh}"

[[nodes]]
ip = "localhost:8002"
eth_pubkey = "0x2c7536E3605D9C16a7a3D7b1898e529396a65c23"
nock_pkh = "{pkh}"

[[nodes]]
ip = "localhost:8003"
eth_pubkey = "0x274BD645de480C325D618c60c661F11275eB77F1"
nock_pkh = "{pkh}"

[[nodes]]
ip = "localhost:8004"
eth_pubkey = "0x6dc59eb20f7928935c47A391e35545a2CEC51013"
nock_pkh = "{pkh}"

[[nodes]]
ip = "localhost:8005"
eth_pubkey = "0xcaB10dA05fC0aDBb7e91Eadc30f224bcDF601375"
nock_pkh = "{pkh}"
"#,
        pkh = pkh
    );

    fs::write(&config_path, config_content).expect("Failed to write test config file");

    let config_toml =
        BridgeConfigToml::from_file(&config_path).expect("Failed to parse TOML config");
    let result = config_toml.to_node_config();
    assert!(result.is_err());
}

/// Validates that the production operator addresses are correctly formatted.
/// This test uses the REAL operator addresses from bridge-conf.sepolia.toml
/// to catch any typos or format errors before they cause runtime failures.
#[test]
fn test_production_operator_addresses() {
    use std::collections::HashSet;

    // Real production operator data - must match bridge-conf.sepolia.toml and bridge-conf.mainnet.toml
    let operators: [(&str, &str, &str); 5] = [
        (
            "Zorp #1", "0x091f5A663Dba081547D60bf0aa50D0a56F1e0964",
            "AD6Mw1QUnPUrnVpyj2gW2jT6Jd6WsuZQmPn79XpZoFEocuvV12iDkvh",
        ),
        (
            "Zorp #2", "0xcadEEee25c168b8Cf054f290f97dbF568a853F64",
            "6KrZT5hHLY1fva9AUDeGtZu5Jznm4RDLYfjcGjuU49nWoNym5ZeX5X5",
        ),
        (
            "Pero", "0x79A2740620d989D51e08f0de494F520e95E5b9cb",
            "CDLzgKWAKFXYABkuQaMwbttDSTDMh3Wy2Eoq2XiArsyxn7vScNHupBb",
        ),
        (
            "Nockbox", "0xDDDE747A20b17ea13F4c4EEB77615b1F18ca0b3B",
            "7E47xYNVEyt7jGmLsiChUHnyw88AfBvzJfXfEQkPmMo2ZWsdcPudwmV",
        ),
        (
            "SWPS", "0xf0441e68E994c20C998e54A23f0d40b76185Be76",
            "3xSyK6RQUaYzE8YDUamkpKRHALxaYo8E7eppawwE4sP35c3PASc6koq",
        ),
    ];

    let mut eth_addrs: HashSet<&str> = HashSet::new();
    let mut pkhs: HashSet<&str> = HashSet::new();

    for (name, eth_addr, pkh) in &operators {
        // Validate ETH address parses correctly
        Address::from_str(eth_addr)
            .unwrap_or_else(|e| panic!("{} ETH address '{}' is invalid: {}", name, eth_addr, e));

        // Validate PKH parses correctly (base58 -> 5 Belt limbs)
        NockPkh::from_base58(pkh)
            .unwrap_or_else(|e| panic!("{} PKH '{}' is invalid: {}", name, pkh, e));

        // Check uniqueness
        assert!(
            eth_addrs.insert(eth_addr),
            "{} has duplicate ETH address: {}",
            name,
            eth_addr
        );
        assert!(pkhs.insert(pkh), "{} has duplicate PKH: {}", name, pkh);
    }

    // Verify we have exactly 5 unique operators
    assert_eq!(eth_addrs.len(), 5, "Expected 5 unique ETH addresses");
    assert_eq!(pkhs.len(), 5, "Expected 5 unique PKHs");
}

/// Validates the Sepolia contract addresses are correctly formatted.
#[test]
fn test_sepolia_contract_addresses() {
    // Sepolia contract addresses from bridge-conf.sepolia.toml
    let inbox = "0x9b1becA13c39b9Be10dB616F1bE10C3CeF9Dfb36";
    let nock = "0xA9cd4087D9B050D8B35727AAf810296CA957c7B3";

    Address::from_str(inbox).expect("Sepolia inbox contract address is invalid");
    Address::from_str(nock).expect("Sepolia nock contract address is invalid");
}

#[test]
fn test_duplicate_nock_pkhs() {
    let temp_dir = TempDir::new().expect("Failed to create temporary directory");
    let config_path = temp_dir.path().join("bridge-conf.toml");
    let pkh = sample_pkh_b58();

    // Using the same PKH for all nodes should fail duplicate detection
    let config_content = format!(
        r#"
node_id = 0
base_ws_url = "wss://mainnet.base.org"
inbox_contract_address = "0x1234567890123456789012345678901234567890"
nock_contract_address = "0x0000000000000000000000000000000000000001"
my_eth_key = "0x1234"
my_nock_key = "3yZe7d"
deposit_address = "5HueCGU8rMjxEXxiPuD5BDku4MkFqeZyd4dZ1jvhTVqvbTLvyTJ"
grpc_address = "http://localhost:5555"
base_confirmation_depth = 300
nockchain_confirmation_depth = 100

[[nodes]]
ip = "localhost:8001"
eth_pubkey = "0x2c7536E3605D9C16a7a3D7b1898e529396a65c23"
nock_pkh = "{pkh}"

[[nodes]]
ip = "localhost:8002"
eth_pubkey = "0x0EE156f080d9cB3BaA3C0DB53D07f13D69CEf4C9"
nock_pkh = "{pkh}"

[[nodes]]
ip = "localhost:8003"
eth_pubkey = "0x274BD645de480C325D618c60c661F11275eB77F1"
nock_pkh = "{pkh}"

[[nodes]]
ip = "localhost:8004"
eth_pubkey = "0x6dc59eb20f7928935c47A391e35545a2CEC51013"
nock_pkh = "{pkh}"

[[nodes]]
ip = "localhost:8005"
eth_pubkey = "0xcaB10dA05fC0aDBb7e91Eadc30f224bcDF601375"
nock_pkh = "{pkh}"
"#,
        pkh = pkh
    );

    fs::write(&config_path, config_content).expect("Failed to write test config file");

    let config_toml =
        BridgeConfigToml::from_file(&config_path).expect("Failed to parse TOML config");
    let result = config_toml.to_node_config();
    assert!(result.is_err());
}

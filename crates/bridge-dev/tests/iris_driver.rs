use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use alloy::primitives::{Address, Bytes, B256, U256};
use anyhow::{anyhow, Context, Result};
use bridge::shared::base::{
    burn_for_withdrawal_signature_hash, encode_withdrawal_burn_calldata,
    parse_withdrawal_burn_calldata,
};
use bridge::shared::e2e_environment::BASE_SEPOLIA_E2E_CHAIN_ID;
use bridge::shared::types::{
    Tip5Hash, WITHDRAWAL_POLICY_V1_BASE_UNITS_PER_NICK,
    WITHDRAWAL_POLICY_V1_BRIDGE_FEE_NICKS_PER_STARTED_NOCK, WITHDRAWAL_POLICY_V1_ID,
    WITHDRAWAL_POLICY_V1_MINIMUM_GROSS_NOCKS, WITHDRAWAL_POLICY_V1_NICKS_PER_NOCK,
    WITHDRAWAL_WIRE_V1_ID,
};
use bridge_dev::anvil::{AnvilBackend, AnvilConfig};
use bridge_dev::base_backend::{TransactionLogFacts, TransactionReceiptFacts};
use bridge_dev::client_driver::{
    RustReferenceDriver, SelectedWithdrawalClient, WithdrawalClientDriver, WithdrawalClientMode,
    WithdrawalClientRequest,
};
use bridge_dev::cluster_config::deterministic_cluster_nodes;
use bridge_dev::environment::BaseE2eEnvironment;
use bridge_dev::fork_seeder::{ForkBalanceSeedRequest, ForkBalanceSeeder};
use bridge_dev::hermetic_deploy::{HermeticDeployConfig, HermeticDeployment};
use bridge_dev::iris_artifact::{IrisArtifact, IrisArtifactFacts, IrisDriverCommand};
use bridge_dev::iris_driver::{
    submit_withdrawal_burn, verify_burn_event, BurnSubmissionError, IrisSdkDriver,
};
use serde_json::{json, Value};

const MANIFEST_JSON: &str = include_str!("../../bridge/e2e/environments/base-sepolia.json");
static SEQUENCE: AtomicU64 = AtomicU64::new(0);

#[tokio::test]
async fn packed_iris_bytes_are_mined_unchanged_and_match_contract_and_rust_decoder() -> Result<()> {
    let backend = AnvilBackend::start(AnvilConfig::empty(), &checked_environment()).await?;
    assert_eq!(
        backend.backend().chain_id().await?,
        BASE_SEPOLIA_E2E_CHAIN_ID
    );
    let nodes = deterministic_cluster_nodes();
    let signers = nodes
        .iter()
        .map(|node| node.eth_address.parse::<Address>())
        .collect::<Result<Vec<_>, _>>()?
        .try_into()
        .map_err(|_| anyhow!("expected five deterministic signers"))?;
    let deployment = HermeticDeployment::deploy(
        &backend,
        HermeticDeployConfig::discover(&workspace_root()?, signers),
    )
    .await?;
    let holder = backend.backend().accounts().await?[0];
    let amount_nicks = WITHDRAWAL_POLICY_V1_MINIMUM_GROSS_NOCKS
        .checked_mul(WITHDRAWAL_POLICY_V1_NICKS_PER_NOCK)
        .context("minimum amount overflow")?;
    ForkBalanceSeeder::seed(
        &backend,
        &deployment.facts().bridge_state,
        ForkBalanceSeedRequest {
            holder,
            required_nicks: amount_nicks,
            headroom_nicks: 0,
            gas_accounts: vec![holder],
            gas_balance_wei: U256::from(10_u64).pow(U256::from(20_u64)),
        },
    )
    .await?;
    let request = client_request(
        deployment.facts().addresses.nock,
        holder,
        amount_nicks,
        Tip5Hash::from_limbs(&[101, 102, 103, 104, 105]),
    );
    let artifact = artifact_for_response(valid_iris_response(&request, &artifact_facts())?)?;
    let client = SelectedWithdrawalClient::select(WithdrawalClientMode::IrisSdk, Some(artifact))?;
    let output = client.encode(&request).await?;
    assert!(output.proof.official_client);
    assert_eq!(output.proof.client_mode, WithdrawalClientMode::IrisSdk);
    assert!(output.proof.artifact.is_some());
    assert_eq!(output.calldata.len(), 116);

    let proof = submit_withdrawal_burn(
        backend.backend(),
        &request,
        output.clone(),
        true,
        Duration::from_secs(5),
    )
    .await?;
    assert_eq!(proof.mined_input_hex, output.proof.calldata_hex);
    assert_eq!(
        proof.event.amount_base_units,
        request.amount_base_units.to_string()
    );
    assert_eq!(proof.event.amount_nicks, amount_nicks.to_string());
    assert_eq!(
        proof.event.lock_root,
        request.expected_lock_root.to_base58()
    );
    assert_eq!(proof.event.commitment, output.proof.commitment);
    assert_eq!(proof.mined_from, holder);
    assert_eq!(proof.mined_to, request.nock_token);
    assert!(proof.receipt.success);

    let block_before_mutations = backend.block_number().await?;
    for mutation in [Mutation::Burner, Mutation::Contract, Mutation::Amount, Mutation::Destination]
    {
        let mut changed = request.clone();
        match mutation {
            Mutation::Burner => changed.burner = address(0x777),
            Mutation::Contract => {
                changed = client_request(
                    address(0x778),
                    request.burner,
                    amount_nicks,
                    request.expected_lock_root.clone(),
                )
            }
            Mutation::Amount => {
                changed.amount_base_units += U256::from(WITHDRAWAL_POLICY_V1_BASE_UNITS_PER_NICK)
            }
            Mutation::Destination => {
                changed.expected_lock_root = Tip5Hash::from_limbs(&[201, 202, 203, 204, 205])
            }
        }
        assert!(matches!(
            submit_withdrawal_burn(
                backend.backend(),
                &changed,
                output.clone(),
                true,
                Duration::from_secs(1),
            )
            .await,
            Err(BurnSubmissionError::ClientBinding(_))
        ));
    }
    assert_eq!(backend.block_number().await?, block_before_mutations);

    let rust_output = RustReferenceDriver.encode(&request).await?;
    assert!(matches!(
        submit_withdrawal_burn(
            backend.backend(),
            &request,
            rust_output,
            true,
            Duration::from_secs(1),
        )
        .await,
        Err(BurnSubmissionError::OfficialIrisRequired)
    ));
    assert!(SelectedWithdrawalClient::select(WithdrawalClientMode::IrisSdk, None).is_err());
    backend.shutdown().await?;
    Ok(())
}

#[tokio::test]
async fn iris_response_schema_binding_and_process_failures_are_deterministic() -> Result<()> {
    let request = client_request(
        address(0x123),
        address(0x456),
        WITHDRAWAL_POLICY_V1_MINIMUM_GROSS_NOCKS * WITHDRAWAL_POLICY_V1_NICKS_PER_NOCK,
        Tip5Hash::from_limbs(&[1, 2, 3, 4, 5]),
    );
    let facts = artifact_facts();

    let malformed = IrisSdkDriver::new(artifact_for_script(
        "process.stdin.resume(); process.stdin.on('end', () => process.stdout.write('{}\\n'));",
        facts.clone(),
    )?);
    assert!(malformed.encode(&request).await.is_err());

    let crash = IrisSdkDriver::new(artifact_for_script("process.exit(7);", facts.clone())?);
    assert!(crash.encode(&request).await.is_err());

    let timeout_driver = IrisSdkDriver::with_timeout(
        artifact_for_script(
            "process.stdin.resume(); setTimeout(() => {}, 60000);",
            facts.clone(),
        )?,
        Duration::from_millis(10),
    )?;
    assert!(timeout_driver.encode(&request).await.is_err());

    let failure = json!({
        "protocol": "iris-withdrawal-e2e-result-v1",
        "ok": false,
        "error": {"code": "invalid_lock_root", "message": "fixture"}
    });
    let failure_driver = IrisSdkDriver::new(artifact_for_response(failure)?);
    assert!(failure_driver.encode(&request).await.is_err());

    for field in ["amount", "destination", "commitment", "calldata", "selector"] {
        let mut response = valid_iris_response(&request, &facts)?;
        match field {
            "amount" => response["amount"]["nicks"] = json!("1"),
            "destination" => {
                response["destination"]["lock_root"] =
                    json!(Tip5Hash::from_limbs(&[999, 1_000, 1_001, 1_002, 1_003]).to_base58())
            }
            "commitment" => response["commitment"] = json!(format!("{:#x}", B256::repeat_byte(7))),
            "calldata" => response["calldata"] = json!("0x00"),
            "selector" => response["selector"] = json!("0x00000000"),
            _ => return Err(anyhow!("unknown mutation field {field}")),
        }
        let driver = IrisSdkDriver::new(artifact_for_response(response)?);
        assert!(driver.encode(&request).await.is_err(), "field={field}");
    }
    Ok(())
}

#[tokio::test]
async fn event_verifier_requires_one_exact_log_and_accepts_log_index_zero() -> Result<()> {
    let request = client_request(
        address(0x321),
        address(0x654),
        WITHDRAWAL_POLICY_V1_MINIMUM_GROSS_NOCKS * WITHDRAWAL_POLICY_V1_NICKS_PER_NOCK,
        Tip5Hash::from_limbs(&[11, 12, 13, 14, 15]),
    );
    let output = RustReferenceDriver.encode(&request).await?;
    let (_, commitment, _) = parse_withdrawal_burn_calldata(&output.calldata)?;
    let log = burn_log(&request, commitment, 0);
    let receipt = TransactionReceiptFacts {
        transaction_hash: hash(500),
        block_number: 9,
        success: true,
        contract_address: None,
        logs: vec![log.clone()],
    };
    let event = verify_burn_event(&receipt, &request, &output.calldata, &output.proof)?;
    assert_eq!(event.log_index, 0);
    assert_eq!(event.commitment, output.proof.commitment);

    let mut none = receipt.clone();
    none.logs.clear();
    assert!(matches!(
        verify_burn_event(&none, &request, &output.calldata, &output.proof),
        Err(BurnSubmissionError::EventCount(0))
    ));
    let mut multiple = receipt.clone();
    multiple.logs.push(log);
    assert!(matches!(
        verify_burn_event(&multiple, &request, &output.calldata, &output.proof),
        Err(BurnSubmissionError::EventCount(2))
    ));
    let mut wrong_amount = receipt.clone();
    wrong_amount.logs[0].data = Bytes::from(U256::from(1_u64).to_be_bytes::<32>().to_vec());
    assert!(matches!(
        verify_burn_event(&wrong_amount, &request, &output.calldata, &output.proof),
        Err(BurnSubmissionError::EventDecode(_))
    ));
    let mut wrong_commitment = receipt;
    wrong_commitment.logs[0].topics[2] = B256::repeat_byte(9);
    assert!(matches!(
        verify_burn_event(&wrong_commitment, &request, &output.calldata, &output.proof,),
        Err(BurnSubmissionError::EventDecode(_))
    ));
    Ok(())
}

#[derive(Clone, Copy)]
enum Mutation {
    Burner,
    Contract,
    Amount,
    Destination,
}

fn client_request(
    token: Address,
    burner: Address,
    amount_nicks: u64,
    lock_root: Tip5Hash,
) -> WithdrawalClientRequest {
    WithdrawalClientRequest {
        nock_token: token,
        burner,
        amount_base_units: U256::from(amount_nicks)
            * U256::from(WITHDRAWAL_POLICY_V1_BASE_UNITS_PER_NICK),
        destination_kind: "lock_root".to_owned(),
        destination_value: lock_root.to_base58(),
        expected_lock_root: lock_root,
    }
}

fn valid_iris_response(
    request: &WithdrawalClientRequest,
    artifact: &IrisArtifactFacts,
) -> Result<Value> {
    let calldata = encode_withdrawal_burn_calldata(
        request.nock_token, request.burner, request.amount_base_units, &request.expected_lock_root,
    );
    let (amount, commitment, lock_root) = parse_withdrawal_burn_calldata(&calldata)?;
    let amount_nicks = (amount / U256::from(WITHDRAWAL_POLICY_V1_BASE_UNITS_PER_NICK)).to::<u64>();
    let started_nocks = amount_nicks.div_ceil(WITHDRAWAL_POLICY_V1_NICKS_PER_NOCK);
    let fee = started_nocks * WITHDRAWAL_POLICY_V1_BRIDGE_FEE_NICKS_PER_STARTED_NOCK;
    let limbs = lock_root.to_array().map(|limb| limb.to_string());
    Ok(json!({
        "protocol": "iris-withdrawal-e2e-result-v1",
        "ok": true,
        "sdk_metadata": {
            "package_name": artifact.package_name,
            "package_version": artifact.package_version,
            "revision": artifact.git_revision,
        },
        "wire_protocol": WITHDRAWAL_WIRE_V1_ID,
        "withdrawal_policy": WITHDRAWAL_POLICY_V1_ID,
        "selector": format!("0x{}", hex::encode(&calldata[..4])),
        "destination": {
            "kind": request.destination_kind,
            "normalized": request.destination_value,
            "lock_root": lock_root.to_base58(),
            "lock_root_limbs": limbs,
        },
        "amount": {
            "base_units": amount.to_string(),
            "nicks": amount_nicks.to_string(),
            "bridge_fee_nicks": fee.to_string(),
            "net_after_bridge_fee_nicks": (amount_nicks - fee).to_string(),
        },
        "commitment": format!("{commitment:#x}"),
        "calldata": format!("0x{}", hex::encode(&calldata)),
        "calldata_byte_length": calldata.len(),
        "self_validation": {
            "valid": true,
            "decoded_wire_protocol": WITHDRAWAL_WIRE_V1_ID,
            "decoded_amount_base_units": amount.to_string(),
            "decoded_commitment": format!("{commitment:#x}"),
            "decoded_lock_root_limbs": limbs,
        },
    }))
}

fn artifact_for_response(response: Value) -> Result<IrisArtifact> {
    artifact_for_script(
        &format!(
            "process.stdin.resume(); process.stdin.on('end', () => process.stdout.write(JSON.stringify({response}) + '\\n'));"
        ),
        artifact_facts(),
    )
}

fn artifact_for_script(source: &str, facts: IrisArtifactFacts) -> Result<IrisArtifact> {
    let root = preserved_root("driver");
    fs::create_dir_all(&root)?;
    let path = root.join("driver with spaces.mjs");
    fs::write(&path, source)?;
    Ok(IrisArtifact {
        facts,
        driver: IrisDriverCommand {
            program: PathBuf::from("node"),
            args: vec![path.into_os_string()],
        },
    })
}

fn artifact_facts() -> IrisArtifactFacts {
    IrisArtifactFacts {
        package_name: "@nockbox/iris-sdk".to_owned(),
        package_version: "0.3.0".to_owned(),
        git_revision: "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".to_owned(),
        tarball_path: PathBuf::from("/run/iris-sdk.tgz"),
        tarball_sha256: "b".repeat(64),
        npm_integrity: "sha512-Zml4dHVyZQ==".to_owned(),
        npm_shasum: "c".repeat(40),
        files: Vec::new(),
        packed_node_version: "v26.0.0".to_owned(),
        packed_npm_version: "11.0.0".to_owned(),
        runtime_node_version: "v26.0.0".to_owned(),
        driver_path: PathBuf::from("/run/driver.js"),
    }
}

fn burn_log(
    request: &WithdrawalClientRequest,
    commitment: B256,
    log_index: u64,
) -> TransactionLogFacts {
    let mut burner_topic = [0_u8; 32];
    burner_topic[12..].copy_from_slice(request.burner.as_slice());
    TransactionLogFacts {
        address: request.nock_token,
        topics: vec![burn_for_withdrawal_signature_hash(), B256::from(burner_topic), commitment],
        data: Bytes::from(request.amount_base_units.to_be_bytes::<32>().to_vec()),
        log_index,
    }
}

fn checked_environment() -> BaseE2eEnvironment {
    BaseE2eEnvironment::from_json(MANIFEST_JSON)
        .expect("checked-in Base Sepolia environment must validate")
}

fn workspace_root() -> Result<PathBuf> {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .map(Path::to_path_buf)
        .context("bridge-dev is not under workspace crates directory")
}

fn preserved_root(label: &str) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock follows Unix epoch")
        .as_nanos();
    let sequence = SEQUENCE.fetch_add(1, Ordering::Relaxed);
    std::env::temp_dir().join(format!(
        "nockbridge-iris-driver-{label}-{}-{nanos}-{sequence}",
        std::process::id()
    ))
}

fn address(seed: u64) -> Address {
    let mut bytes = [0_u8; 20];
    bytes[12..].copy_from_slice(&seed.to_be_bytes());
    Address::from(bytes)
}

fn hash(seed: u64) -> B256 {
    let mut bytes = [0_u8; 32];
    bytes[24..].copy_from_slice(&seed.to_be_bytes());
    B256::from(bytes)
}

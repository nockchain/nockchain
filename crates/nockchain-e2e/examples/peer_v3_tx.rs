//! Inspect a trusted wallet transaction and read the loopback kernel's mempool.

use std::num::NonZeroU16;
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{bail, ensure, Context, Result};
use bytes::Bytes;
use clap::{Parser, Subcommand};
use nockapp::noun::slab::NounSlab;
use nockapp::utils::make_tas;
use nockapp_grpc_proto::pb::private::v1::nock_app_service_client::NockAppServiceClient;
use nockapp_grpc_proto::pb::private::v1::{peek_response, PeekRequest, PeekResponse};
use nockchain_math::zoon::zset::ZSet;
use nockchain_types::tx_engine::common::{Hash, Version};
use nockchain_types::tx_engine::v1::{RawTx, Spend, Transaction, WitnessData};
use nockvm::noun::{NounAllocator, D, SIG, T};
use noun_serde::{NounDecode, NounEncode};
use serde_json::{json, Value};

#[derive(Parser)]
#[command(about = "Inspect a wallet v1 transaction or check its loopback mempool membership")]
struct Args {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Reconstruct the signed raw transaction from a trusted local wallet .tx file.
    Inspect { transaction_file: PathBuf },
    /// Check the kernel's excluded-txs set through its loopback private API.
    Pending {
        /// Private gRPC port on 127.0.0.1.
        port: NonZeroU16,
        #[arg(value_parser = parse_hash)]
        tx_id: Hash,
    },
}

fn parse_hash(value: &str) -> std::result::Result<Hash, String> {
    Hash::from_base58(value).map_err(|error| error.to_string())
}

fn inspect(path: &Path) -> Result<Value> {
    let data = std::fs::read(path)
        .with_context(|| format!("read trusted wallet transaction {}", path.display()))?;
    inspect_jam(data)
}

fn inspect_jam(data: Vec<u8>) -> Result<Value> {
    let mut slab: NounSlab = NounSlab::new();
    let noun = slab
        .cue_into(Bytes::from(data))
        .context("cue wallet transaction")?;
    let transaction = Transaction::from_noun(&noun, &slab.noun_space())
        .context("decode saved wallet v1 transaction")?;
    let raw_tx = signed_raw_tx(transaction)?;

    // Both WalletSendTransaction and peer gossip deliver this exact cause.
    // The event log hashes JAM(cause), excluding the wire and event header.
    // The wallet's debug raw JAM omits these applied witnesses and has a different
    // ID. Hash the signed raw transaction reconstructed from the saved .tx file.
    let mut cause: NounSlab = NounSlab::new();
    let fact = make_tas(&mut cause, "fact").as_noun();
    let heard_tx = make_tas(&mut cause, "heard-tx").as_noun();
    let raw_noun = raw_tx.to_noun(&mut cause);
    let root = T(&mut cause, &[fact, D(0), heard_tx, raw_noun]);
    cause.set_root(root);
    Ok(json!({
        "version": 1,
        "tx_id": raw_tx.id.to_base58(),
        "cause_hash": blake3::hash(&cause.jam()).to_hex().to_string(),
    }))
}

fn signed_raw_tx(transaction: Transaction) -> Result<RawTx> {
    let Transaction::V1(transaction) = transaction;
    let mut raw_tx = RawTx {
        version: Version::V1,
        id: Hash::from_limbs(&[0; 5]),
        spends: transaction.spends,
    };
    // The wallet names saved transactions using the unsigned spends. The name
    // remains stable when signatures are added; it is not the submitted tx ID.
    let named_id = Hash::from_base58(&transaction.name).context("decode transaction name")?;
    ensure!(
        raw_tx.compute_id()? == named_id,
        "transaction name does not match unsigned spends"
    );
    ensure!(
        !raw_tx.spends.0.is_empty(),
        "saved transaction has no spends"
    );
    for (name, spend) in &mut raw_tx.spends.0 {
        match (&transaction.witness_data, spend) {
            (WitnessData::Signatures(signatures), Spend::Legacy(spend)) => {
                spend.signature = signatures
                    .0
                    .iter()
                    .find(|(input, _)| input == name)
                    .context("saved transaction is missing an input signature")?
                    .1
                    .clone();
            }
            (WitnessData::Witnesses(witnesses), Spend::Witness(spend)) => {
                spend.witness = witnesses
                    .0
                    .iter()
                    .find(|(input, _)| input == name)
                    .context("saved transaction is missing an input witness")?
                    .1
                    .clone();
            }
            _ => bail!("saved transaction witness data does not match the input spend type"),
        }
    }
    raw_tx.id = raw_tx
        .compute_id()
        .context("compute signed transaction ID")?;
    Ok(raw_tx)
}

fn pending_json(response: PeekResponse, tx_id: &Hash) -> Result<Value> {
    let data = match response.result {
        Some(peek_response::Result::Data(data)) => data,
        Some(peek_response::Result::Error(error)) => {
            bail!("excluded-txs peek error: code={} message={}", error.code, error.message)
        }
        None => bail!("excluded-txs peek returned no result"),
    };
    let mut slab: NounSlab = NounSlab::new();
    let noun = slab
        .cue_into(Bytes::from(data))
        .context("cue excluded-txs")?;
    let set = Option::<Option<ZSet<Hash>>>::from_noun(&noun, &slab.noun_space())
        .context("decode excluded-txs")?
        .flatten()
        .context("excluded-txs peek returned no set")?;
    Ok(json!({"tx_id": tx_id.to_base58(), "pending": set.contains(tx_id)}))
}

async fn pending(port: NonZeroU16, tx_id: &Hash) -> Result<Value> {
    tokio::time::timeout(Duration::from_secs(10), async {
        let mut client = NockAppServiceClient::connect(format!("http://127.0.0.1:{port}"))
            .await
            .context("connect to loopback private API")?;
        let mut path: NounSlab = NounSlab::new();
        let tag = make_tas(&mut path, "excluded-txs").as_noun();
        let root = T(&mut path, &[tag, SIG]);
        path.set_root(root);
        let response = client
            .peek(PeekRequest {
                pid: 0,
                path: path.jam().to_vec(),
            })
            .await
            .context("query excluded-txs")?
            .into_inner();
        pending_json(response, tx_id)
    })
    .await
    .context("pending query exceeded ten seconds")?
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<()> {
    let result = match Args::parse().command {
        Command::Inspect { transaction_file } => inspect(&transaction_file)?,
        Command::Pending { port, tx_id } => pending(port, &tx_id).await?,
    };
    println!("{result}");
    Ok(())
}

#[cfg(test)]
mod tests {
    use nockapp_grpc_proto::pb::common::v1::ErrorStatus;
    use nockchain_types::tx_engine::common::Signature;
    use nockchain_types::tx_engine::v1::{
        InputMetadata, OutputLockMap, SignatureMap, Spend0, SpendConditionMap, TransactionMetadata,
        TransactionV1, WitnessMap,
    };

    use super::*;

    const RAW_TX: &[u8] = include_bytes!("../../nockchain-types/jams/v1/raw-tx.jam");
    const LEGACY_TX: &[u8] = include_bytes!("../../nockchain-types/jams/v0/raw-tx.jam");
    const WALLET_TX: &[u8] = include_bytes!(
        "../../bridge/test-fixtures/transactions/9MpGym52AumtwyBxYPyVsWHvcamUYwZkc1Nq7w3cFGF28u8ceVDwt3e.tx"
    );

    fn jam<T: NounEncode>(value: T) -> Vec<u8> {
        let mut slab: NounSlab = NounSlab::new();
        let noun = value.to_noun(&mut slab);
        slab.set_root(noun);
        slab.jam().to_vec()
    }

    fn peek_data<T: NounEncode>(value: T) -> PeekResponse {
        PeekResponse {
            result: Some(peek_response::Result::Data(jam(value))),
        }
    }

    fn raw_fixture() -> RawTx {
        let mut slab: NounSlab = NounSlab::new();
        let noun = slab.cue_into(Bytes::from_static(RAW_TX)).unwrap();
        RawTx::from_noun(&noun, &slab.noun_space()).unwrap()
    }

    fn wallet_artifact(signed: &RawTx) -> Transaction {
        let mut unsigned = signed.clone();
        let witness_data = match &signed.spends.0.first().unwrap().1 {
            Spend::Witness(_) => WitnessData::Witnesses(WitnessMap(
                unsigned
                    .spends
                    .0
                    .iter_mut()
                    .map(|(name, spend)| {
                        let Spend::Witness(spend) = spend else {
                            panic!("mixed fixture spends")
                        };
                        let witness = spend.witness.clone();
                        spend.witness.pkh_signature.0.clear();
                        (name.clone(), witness)
                    })
                    .collect(),
            )),
            Spend::Legacy(_) => WitnessData::Signatures(SignatureMap(
                unsigned
                    .spends
                    .0
                    .iter_mut()
                    .map(|(name, spend)| {
                        let Spend::Legacy(spend) = spend else {
                            panic!("mixed fixture spends")
                        };
                        let signature = spend.signature.clone();
                        spend.signature.0.clear();
                        (name.clone(), signature)
                    })
                    .collect(),
            )),
        };
        Transaction::V1(TransactionV1 {
            name: unsigned.compute_id_base58().unwrap(),
            spends: unsigned.spends,
            metadata: TransactionMetadata {
                inputs: InputMetadata::SpendConditions(SpendConditionMap(Vec::new())),
                outputs: OutputLockMap(Vec::new()),
            },
            witness_data,
        })
    }

    #[test]
    fn applied_witnesses_match_public_signed_raw_fixture_and_gossip_cause() {
        let signed = raw_fixture();
        let transaction = wallet_artifact(&signed);
        let Transaction::V1(saved) = &transaction;
        assert_ne!(saved.name, signed.id.to_base58());
        assert!(signed.spends.0.iter().any(|(_, spend)| {
            matches!(spend, Spend::Witness(spend) if !spend.witness.pkh_signature.0.is_empty())
        }));
        assert_eq!(signed_raw_tx(transaction.clone()).unwrap(), signed);
        let inspected = inspect_jam(jam(transaction)).unwrap();
        let mut original: NounSlab = NounSlab::new();
        let raw = original.cue_into(Bytes::from_static(RAW_TX)).unwrap();
        let space = original.noun_space();
        let id = Hash::from_noun(&raw.in_space(&space).slot(6).unwrap().noun(), &space).unwrap();
        // Wrap the original fixture noun, independently of RawTx's re-encoding.
        let fact = make_tas(&mut original, "fact").as_noun();
        let heard_tx = make_tas(&mut original, "heard-tx").as_noun();
        let message = T(&mut original, &[heard_tx, raw]);
        let cause = T(&mut original, &[fact, D(0), message]);
        original.set_root(cause);
        assert_eq!(inspected["version"], 1);
        assert_eq!(inspected["tx_id"], id.to_base58());
        assert_eq!(
            inspected["cause_hash"],
            blake3::hash(&original.jam()).to_hex().to_string()
        );
        assert_ne!(
            inspected["cause_hash"],
            blake3::hash(RAW_TX).to_hex().to_string()
        );

        let changed_cause = T(&mut original, &[fact, D(1), message]);
        original.set_root(changed_cause);
        assert_ne!(
            inspected["cause_hash"],
            blake3::hash(&original.jam()).to_hex().to_string()
        );
    }

    #[test]
    fn saved_public_wallet_fixture_applies_all_signatures() {
        let mut slab: NounSlab = NounSlab::new();
        let noun = slab.cue_into(Bytes::from_static(WALLET_TX)).unwrap();
        let transaction = Transaction::from_noun(&noun, &slab.noun_space()).unwrap();
        let Transaction::V1(saved) = &transaction;
        let signed = signed_raw_tx(transaction.clone()).unwrap();
        let WitnessData::Witnesses(witnesses) = &saved.witness_data else {
            panic!("public wallet fixture should use witnesses")
        };
        assert!(!witnesses.0.is_empty());
        for (name, witness) in &witnesses.0 {
            let (_, Spend::Witness(spend)) = signed
                .spends
                .0
                .iter()
                .find(|(input, _)| input == name)
                .unwrap()
            else {
                panic!("public wallet fixture should use witness spends")
            };
            assert_eq!(&spend.witness, witness);
        }
        assert_ne!(signed.id.to_base58(), saved.name);
        let inspected = inspect_jam(WALLET_TX.to_vec()).unwrap();
        assert_eq!(inspected["tx_id"], signed.id.to_base58());
    }

    #[test]
    fn legacy_input_signatures_are_replaced_in_v1_transactions() {
        let mut signed = raw_fixture();
        for (_, spend) in &mut signed.spends.0 {
            let Spend::Witness(witness_spend) = spend else {
                panic!("witness fixture expected")
            };
            let signature = Signature(
                witness_spend
                    .witness
                    .pkh_signature
                    .0
                    .iter()
                    .map(|entry| (entry.pubkey.clone(), entry.signature.clone()))
                    .collect(),
            );
            assert!(!signature.0.is_empty());
            *spend = Spend::Legacy(Spend0 {
                signature,
                seeds: witness_spend.seeds.clone(),
                fee: witness_spend.fee.clone(),
            });
        }
        signed.id = signed.compute_id().unwrap();
        assert_eq!(signed_raw_tx(wallet_artifact(&signed)).unwrap(), signed);
    }

    #[test]
    fn inspect_rejects_missing_malformed_raw_and_legacy_artifacts() {
        assert!(inspect(Path::new("")).is_err());
        for data in [Vec::new(), vec![0], vec![2], RAW_TX.to_vec(), LEGACY_TX.to_vec()] {
            assert!(inspect_jam(data).is_err());
        }
        let mut slab: NounSlab = NounSlab::new();
        let transaction = slab.cue_into(Bytes::from_static(WALLET_TX)).unwrap();
        let tail = transaction
            .in_space(&slab.noun_space())
            .as_cell()
            .unwrap()
            .tail()
            .noun();
        let wrong_version = T(&mut slab, &[D(2), tail]);
        slab.set_root(wrong_version);
        assert!(inspect_jam(slab.jam().to_vec()).is_err());
    }

    #[test]
    fn missing_witness_wrong_type_and_inconsistent_name_are_errors() {
        let Transaction::V1(original) = wallet_artifact(&raw_fixture());
        let mut missing = original.clone();
        let WitnessData::Witnesses(witnesses) = &mut missing.witness_data else {
            unreachable!()
        };
        witnesses.0.remove(0);
        assert!(signed_raw_tx(Transaction::V1(missing)).is_err());

        let mut wrong_type = original.clone();
        wrong_type.witness_data = WitnessData::Signatures(SignatureMap(Vec::new()));
        assert!(signed_raw_tx(Transaction::V1(wrong_type)).is_err());

        let mut wrong_name = original;
        wrong_name.name = Hash::from_limbs(&[0; 5]).to_base58();
        assert!(signed_raw_tx(Transaction::V1(wrong_name)).is_err());
    }

    #[test]
    fn pending_checks_exact_transaction_in_a_present_set() {
        let id = Hash::from_limbs(&[1, 2, 3, 4, 5]);
        let other = Hash::from_limbs(&[5, 4, 3, 2, 1]);
        let set = ZSet::try_from_items([id.clone()]).unwrap();
        assert_eq!(
            pending_json(peek_data(Some(Some(set.clone()))), &id).unwrap(),
            json!({"tx_id": id.to_base58(), "pending": true})
        );
        assert_eq!(
            pending_json(peek_data(Some(Some(set))), &other).unwrap()["pending"],
            false
        );
        assert_eq!(
            pending_json(peek_data(Some(Some(ZSet::<Hash>::new()))), &id).unwrap()["pending"],
            false
        );
    }

    #[test]
    fn pending_rejects_missing_and_malformed_peek_results() {
        let id = Hash::from_limbs(&[1, 2, 3, 4, 5]);
        for response in [
            PeekResponse { result: None },
            PeekResponse {
                result: Some(peek_response::Result::Error(ErrorStatus::default())),
            },
            PeekResponse {
                result: Some(peek_response::Result::Data(Vec::new())),
            },
            peek_data(None::<Option<ZSet<Hash>>>),
            peek_data(Some(None::<ZSet<Hash>>)),
            peek_data(7_u64),
            peek_data(Some(Some(7_u64))),
        ] {
            assert!(pending_json(response, &id).is_err());
        }
    }

    #[test]
    fn cli_accepts_only_the_expected_local_arguments() {
        let id = Hash::from_limbs(&[1, 2, 3, 4, 5]).to_base58();
        assert!(Args::try_parse_from(["tx", "inspect", "txs/example.tx"]).is_ok());
        assert!(Args::try_parse_from(["tx", "pending", "65535", &id]).is_ok());
        for arguments in [
            vec!["tx"],
            vec!["tx", "inspect"],
            vec!["tx", "inspect", "a.tx", "b.tx"],
            vec!["tx", "pending", "0", &id],
            vec!["tx", "pending", "65536", &id],
            vec!["tx", "pending", "127.0.0.1:8080", &id],
            vec!["tx", "pending", "example.com:8080", &id],
            vec!["tx", "pending", "8080"],
            vec!["tx", "pending", "8080", "invalid!"],
            vec!["tx", "pending", "8080", &id, "another-target"],
        ] {
            assert!(Args::try_parse_from(arguments).is_err());
        }
        assert!(Args::try_parse_from(["tx", "pending", "8080", &format!("1{id}")]).is_err());
    }
}

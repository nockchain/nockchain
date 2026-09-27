//! Read the current consensus head through the node's loopback private API.

use std::num::NonZeroU16;
use std::time::Duration;

use anyhow::{Context, Result};
use clap::Parser;
use nockchain_e2e::grpc::{fetch_heaviest_private, PrivateHeadInfo};
use serde_json::{json, Map, Value};

#[derive(Parser)]
#[command(about = "Read the current accepted heaviest head from a loopback node")]
struct Args {
    /// Private gRPC port on 127.0.0.1. Only a numeric, nonzero port is accepted.
    port: NonZeroU16,
}

fn head_json(head: PrivateHeadInfo) -> Result<Value> {
    let hash = head
        .block_id
        .context("node has no accepted heaviest head")?;
    let block_id: Map<String, Value> = hash
        .0
        .iter()
        .enumerate()
        .map(|(index, belt)| {
            // Match grpcurl's default ProtoJSON for common.v1.Hash: message
            // fields remain present, while zero-valued uint64 fields are omitted.
            let value = if belt.0 == 0 {
                json!({})
            } else {
                json!({"value": belt.0.to_string()})
            };
            (format!("belt{}", index + 1), value)
        })
        .collect();
    Ok(json!({"height": head.height, "block_id": block_id}))
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<()> {
    let args = Args::parse();
    let address = format!("127.0.0.1:{}", args.port);
    let head = tokio::time::timeout(Duration::from_secs(10), fetch_heaviest_private(&address))
        .await
        .context("head query exceeded ten seconds")??;
    println!("{}", head_json(head)?);
    Ok(())
}

#[cfg(test)]
mod tests {
    use nockchain_math::belt::{Belt, PRIME};
    use nockchain_types::tx_engine::common::Hash;

    use super::*;

    #[test]
    fn hash_json_matches_protojson_zero_omission_and_u64_strings() {
        let head = PrivateHeadInfo {
            height: 17,
            block_id: Some(Hash([Belt(0), Belt(1), Belt(PRIME - 1), Belt(3), Belt(0)])),
        };
        assert_eq!(
            head_json(head).unwrap(),
            json!({
                "height": 17,
                "block_id": {
                    "belt1": {},
                    "belt2": {"value": "1"},
                    "belt3": {"value": "18446744069414584320"},
                    "belt4": {"value": "3"},
                    "belt5": {}
                }
            })
        );
    }

    #[test]
    fn missing_head_is_an_error() {
        assert!(head_json(PrivateHeadInfo {
            height: 0,
            block_id: None,
        })
        .is_err());
    }

    #[test]
    fn cli_accepts_only_a_single_nonzero_port() {
        assert_eq!(
            Args::try_parse_from(["head", "65535"]).unwrap().port.get(),
            65535
        );
        for arguments in [
            vec!["head"],
            vec!["head", "0"],
            vec!["head", "65536"],
            vec!["head", "host:8080"],
            vec!["head", "127.0.0.1:8080"],
            vec!["head", "8080", "another-target"],
        ] {
            assert!(Args::try_parse_from(arguments).is_err());
        }
    }
}

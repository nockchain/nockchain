use anyhow::{Context, Result};
use prost_reflect::{Cardinality, DescriptorPool, FieldDescriptor, Kind, MessageDescriptor};
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq, clap::ValueEnum)]
#[serde(rename_all = "snake_case")]
pub enum ApiMode {
    Public,
    Private,
}

impl ApiMode {
    pub const fn default_backend(self) -> &'static str {
        match self {
            Self::Public => "http://23.252.122.18:5556",
            Self::Private => "http://127.0.0.1:5555",
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum OutputFormat {
    #[default]
    Json,
    Native,
}

pub const HOON_UNIX_EPOCH_SECONDS: &str = "9223372091860848000";

#[derive(Clone, Copy, Debug)]
pub struct Operation {
    pub name: &'static str,
    pub summary: &'static str,
    pub service: &'static str,
    pub method: &'static str,
    pub request_type: &'static str,
    pub response_type: &'static str,
    pub proto_file: &'static str,
    pub example: &'static str,
}

impl Operation {
    pub fn full_method(self) -> String {
        format!("/{}/{}", self.service, self.method)
    }
}

const PUBLIC_OPERATIONS: &[Operation] = &[
    Operation {
        name: "wallet_get_balance",
        summary: "Read a paginated wallet balance by base58 pubkey or first-name hash.",
        service: "nockchain.public.v2.NockchainService",
        method: "WalletGetBalance",
        request_type: "nockchain.public.v2.WalletGetBalanceRequest",
        response_type: "nockchain.public.v2.WalletGetBalanceResponse",
        proto_file: "nockchain/public/v2/nockchain.proto",
        example: r#"{"address":{"key":"<base58-pubkey>"},"page":{"clientPageItemsLimit":100,"pageToken":"","maxBytes":"0"}}"#,
    },
    Operation {
        name: "transaction_accepted",
        summary: "Check whether a transaction is currently accepted in the node's raw-tx set.",
        service: "nockchain.public.v2.NockchainService",
        method: "TransactionAccepted",
        request_type: "nockchain.public.v2.TransactionAcceptedRequest",
        response_type: "nockchain.public.v2.TransactionAcceptedResponse",
        proto_file: "nockchain/public/v2/nockchain.proto",
        example: r#"{"txId":{"hash":"<base58-transaction-id>"}}"#,
    },
    Operation {
        name: "get_blocks",
        summary: "List cached heaviest-chain blocks with stable pagination.",
        service: "nockchain.public.v2.NockchainBlockService",
        method: "GetBlocks",
        request_type: "nockchain.public.v2.GetBlocksRequest",
        response_type: "nockchain.public.v2.GetBlocksResponse",
        proto_file: "nockchain/public/v2/nockchain.proto",
        example: r#"{"page":{"clientPageItemsLimit":20,"pageToken":"","maxBytes":"0"}}"#,
    },
    Operation {
        name: "get_block_details",
        summary: "Read full cached block details by height or base58 block ID.",
        service: "nockchain.public.v2.NockchainBlockService",
        method: "GetBlockDetails",
        request_type: "nockchain.public.v2.GetBlockDetailsRequest",
        response_type: "nockchain.public.v2.GetBlockDetailsResponse",
        proto_file: "nockchain/public/v2/nockchain.proto",
        example: r#"{"height":"12345"}"#,
    },
    Operation {
        name: "get_transaction_block",
        summary: "Locate the block containing a transaction, or report that it is pending.",
        service: "nockchain.public.v2.NockchainBlockService",
        method: "GetTransactionBlock",
        request_type: "nockchain.public.v2.GetTransactionBlockRequest",
        response_type: "nockchain.public.v2.GetTransactionBlockResponse",
        proto_file: "nockchain/public/v2/nockchain.proto",
        example: r#"{"txId":{"hash":"<base58-transaction-id>"}}"#,
    },
    Operation {
        name: "get_transaction_details",
        summary: "Read decoded transaction inputs, outputs, fees, size, and containing block.",
        service: "nockchain.public.v2.NockchainBlockService",
        method: "GetTransactionDetails",
        request_type: "nockchain.public.v2.GetTransactionDetailsRequest",
        response_type: "nockchain.public.v2.GetTransactionDetailsResponse",
        proto_file: "nockchain/public/v2/nockchain.proto",
        example: r#"{"txId":{"hash":"<base58-transaction-id>"}}"#,
    },
    Operation {
        name: "get_explorer_metrics",
        summary: "Read explorer-cache coverage, freshness, seed state, and RPC latency metrics.",
        service: "nockchain.public.v2.NockchainMetricsService",
        method: "GetExplorerMetrics",
        request_type: "nockchain.public.v2.GetExplorerMetricsRequest",
        response_type: "nockchain.public.v2.GetExplorerMetricsResponse",
        proto_file: "nockchain/public/v2/nockchain.proto",
        example: "{}",
    },
    Operation {
        name: "get_peer_stats",
        summary: "Read per-peer request, byte, latency, failure, and propagation statistics.",
        service: "nockchain.public.v2.NockchainMetricsService",
        method: "GetPeerStats",
        request_type: "nockchain.public.v2.GetPeerStatsRequest",
        response_type: "nockchain.public.v2.GetPeerStatsResponse",
        proto_file: "nockchain/public/v2/nockchain.proto",
        example: "{}",
    },
    Operation {
        name: "get_req_res_metrics",
        summary: "Read generation-specific request/response timeout, batch, and fallback counters.",
        service: "nockchain.public.v2.NockchainMetricsService",
        method: "GetReqResMetrics",
        request_type: "nockchain.public.v2.GetReqResMetricsRequest",
        response_type: "nockchain.public.v2.GetReqResMetricsResponse",
        proto_file: "nockchain/public/v2/nockchain.proto",
        example: "{}",
    },
];

const PRIVATE_OPERATIONS: &[Operation] = &[Operation {
    name: "peek",
    summary: "Read kernel state through a JAM-encoded Nock peek path. Poke and effect watching are intentionally unavailable.",
    service: "nockchain.private.v1.NockAppService",
    method: "Peek",
    request_type: "nockchain.private.v1.PeekRequest",
    response_type: "nockchain.private.v1.PeekResponse",
    proto_file: "nockchain/private/v1/nockapp.proto",
    example: r#"{"pid":0,"path":{"preset":"heaviest-chain"}}"#,
}];

pub const fn operations(mode: ApiMode) -> &'static [Operation] {
    match mode {
        ApiMode::Public => PUBLIC_OPERATIONS,
        ApiMode::Private => PRIVATE_OPERATIONS,
    }
}

pub fn operation(mode: ApiMode, name: &str) -> Result<Operation> {
    operations(mode)
        .iter()
        .copied()
        .find(|operation| operation.name == name)
        .with_context(|| format!("unknown or unavailable {mode:?} operation {name:?}"))
}

pub fn descriptor_pool() -> Result<DescriptorPool> {
    DescriptorPool::decode(nockapp_grpc_proto::pb::FILE_DESCRIPTOR_SET)
        .context("decode the embedded Nockchain protobuf descriptor set")
}

pub fn catalog(mode: ApiMode) -> Result<Value> {
    let pool = descriptor_pool()?;
    let operations = operations(mode)
        .iter()
        .map(|operation| operation_json(&pool, *operation))
        .collect::<Result<Vec<_>>>()?;

    Ok(json!({
        "name": "nockchain",
        "mode": mode,
        "readOnly": true,
        "timeEncoding": {
            "blockTimestamp": {
                "name": "Hoon epoch absolute seconds",
                "protobufType": "uint64 encoded as a decimal string in JSON",
                "unixOffsetSeconds": HOON_UNIX_EPOCH_SECONDS,
                "toUnixSeconds": "BigInt(timestamp) - 9223372091860848000n",
                "warning": "This is not a Unix timestamp and exceeds JavaScript Number.MAX_SAFE_INTEGER. Parse it with BigInt and subtract the offset before converting to Number or Date."
            }
        },
        "operations": operations,
        "executeApi": {
            "request": "await codemode.request({ operation, input, format?: 'json' | 'native' })",
            "explain": "codemode.explain({ operation, input })",
            "notes": [
                "JSON is de-referenced for agents: scalar wrapper messages are unwrapped and hashes include base58.",
                "Native public results contain base64 protobuf response bytes; native private peek results contain base64 JAM bytes.",
                "Only operations in this catalog can be called. Mutation RPCs are absent and rejected.",
                "Block timestamps use Hoon epoch absolute seconds, not Unix time. In JavaScript use BigInt(timestamp) - 9223372091860848000n before converting to Number or Date."
            ]
        }
    }))
}

fn operation_json(pool: &DescriptorPool, operation: Operation) -> Result<Value> {
    let request = pool
        .get_message_by_name(operation.request_type)
        .with_context(|| format!("missing request descriptor {}", operation.request_type))?;
    let response = pool
        .get_message_by_name(operation.response_type)
        .with_context(|| format!("missing response descriptor {}", operation.response_type))?;
    let example: Value = serde_json::from_str(operation.example)
        .with_context(|| format!("invalid catalog example for {}", operation.name))?;
    let request_schema = if operation.name == "peek" {
        private_peek_schema()
    } else {
        message_schema(request, &mut Vec::new())
    };

    Ok(json!({
        "name": operation.name,
        "summary": operation.summary,
        "readOnly": true,
        "grpc": {
            "service": operation.service,
            "method": operation.method,
            "fullMethod": operation.full_method(),
            "requestType": operation.request_type,
            "responseType": operation.response_type,
            "protoFile": operation.proto_file,
        },
        "inputExample": example,
        "requestSchema": request_schema,
        "responseSchema": message_schema(response, &mut Vec::new()),
    }))
}

fn private_peek_schema() -> Value {
    json!({
        "type": "object",
        "description": "Convenience input compiled to the protobuf PeekRequest. Use jamBase64 or noun for arbitrary paths.",
        "properties": {
            "pid": {"type": "integer", "default": 0},
            "path": {
                "oneOf": [
                    {
                        "type": "string",
                        "enum": ["heaviest-chain", "heaviest-block", "constants", "blockchain-constants", "raw-transactions"]
                    },
                    {
                        "type": "object",
                        "properties": {
                            "preset": {
                                "type": "string",
                                "enum": ["heaviest-chain", "heaviest-block", "constants", "blockchain-constants", "raw-transactions", "heavy-n", "block-transactions", "raw-transaction"]
                            },
                            "argument": {"description": "Unsigned height for heavy-n; base58 ID for block-transactions/raw-transaction."},
                            "jamBase64": {"type": "string", "contentEncoding": "base64"},
                            "noun": {"description": "Recursive {atom:{unsigned|text|base64}} or {cell:[head,tail]} noun JSON."}
                        }
                    }
                ]
            }
        },
        "required": ["path"]
    })
}

fn message_schema(message: MessageDescriptor, parents: &mut Vec<String>) -> Value {
    if parents.iter().any(|parent| parent == message.full_name()) {
        return json!({"type": "object", "protobufType": message.full_name(), "recursive": true});
    }
    parents.push(message.full_name().to_string());
    let mut properties = Map::new();
    for field in message.fields() {
        properties.insert(field.json_name().to_string(), field_schema(&field, parents));
    }
    let oneofs = message
        .oneofs()
        .map(|oneof| {
            json!({
                "name": oneof.name(),
                "exactlyOneOf": oneof.fields().map(|field| field.json_name().to_string()).collect::<Vec<_>>()
            })
        })
        .collect::<Vec<_>>();
    parents.pop();
    json!({
        "type": "object",
        "protobufType": message.full_name(),
        "properties": properties,
        "oneofs": oneofs,
    })
}

fn field_schema(field: &FieldDescriptor, parents: &mut Vec<String>) -> Value {
    let mut schema = match field.kind() {
        Kind::Message(message) => message_schema(message, parents),
        Kind::Enum(enumeration) => json!({
            "type": "string",
            "enum": enumeration.values().map(|value| value.name().to_string()).collect::<Vec<_>>()
        }),
        Kind::Bool => json!({"type": "boolean"}),
        Kind::String => json!({"type": "string"}),
        Kind::Bytes => json!({"type": "string", "contentEncoding": "base64"}),
        Kind::Double | Kind::Float => json!({"type": "number"}),
        Kind::Int32 | Kind::Sint32 | Kind::Sfixed32 | Kind::Uint32 | Kind::Fixed32 => {
            json!({"type": "integer"})
        }
        Kind::Int64 | Kind::Sint64 | Kind::Sfixed64 | Kind::Uint64 | Kind::Fixed64 => json!({
            "type": ["string", "integer"],
            "description": "Protobuf JSON uses a decimal string for 64-bit integers."
        }),
    };
    let object = schema
        .as_object_mut()
        .expect("field schemas are always JSON objects");
    object.insert("fieldNumber".to_string(), json!(field.number()));
    if field.cardinality() == Cardinality::Repeated && !field.is_map() {
        schema = json!({"type": "array", "items": schema});
    }
    schema
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catalogs_only_read_operations() {
        let public = catalog(ApiMode::Public).expect("public catalog");
        let text = public.to_string();
        assert!(!text.contains("WalletSendTransaction"));
        assert!(!text.contains("Poke"));
        assert_eq!(public["operations"].as_array().map(Vec::len), Some(9));
        assert_eq!(
            public["timeEncoding"]["blockTimestamp"]["unixOffsetSeconds"],
            HOON_UNIX_EPOCH_SECONDS
        );
        assert_eq!(
            public["timeEncoding"]["blockTimestamp"]["toUnixSeconds"],
            "BigInt(timestamp) - 9223372091860848000n"
        );
        assert!(public["timeEncoding"]["blockTimestamp"]["warning"]
            .as_str()
            .is_some_and(|warning| warning.contains("Number.MAX_SAFE_INTEGER")));

        let private = catalog(ApiMode::Private).expect("private catalog");
        assert_eq!(private["operations"].as_array().map(Vec::len), Some(1));
        assert_eq!(private["operations"][0]["name"], "peek");
    }
}

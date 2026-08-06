use std::time::Duration;

use anyhow::{anyhow, bail, Context, Result};
use base64::engine::general_purpose::STANDARD as BASE64;
use base64::Engine;
use bytes::Bytes;
use nockapp::noun::slab::NounSlab;
use nockapp::noun::AtomExt;
use nockapp::utils::make_tas;
use nockapp_grpc_proto::pb::private::v1::nock_app_service_client::NockAppServiceClient;
use nockapp_grpc_proto::pb::private::v1::{peek_response, PeekRequest};
use nockapp_grpc_proto::pb::public::v2::nockchain_block_service_client::NockchainBlockServiceClient;
use nockapp_grpc_proto::pb::public::v2::nockchain_metrics_service_client::NockchainMetricsServiceClient;
use nockapp_grpc_proto::pb::public::v2::nockchain_service_client::NockchainServiceClient;
use nockapp_grpc_proto::pb::public::v2::*;
use nockchain_math::belt::Belt;
use nockchain_types::tx_engine::common::Hash;
use nockvm::noun::{Atom, Noun, NounAllocator, NounHandle, SIG, T};
use prost::Message;
use prost_reflect::DynamicMessage;
use serde_json::{json, Map, Value};
use tonic::transport::{Channel, ClientTlsConfig, Endpoint};

use crate::catalog::{descriptor_pool, operation, ApiMode, Operation, OutputFormat};

#[derive(Clone, Debug)]
pub struct Backend {
    pub mode: ApiMode,
    pub endpoint: String,
    pub timeout: Duration,
}

impl Backend {
    pub async fn request(
        &self,
        operation_name: &str,
        input: Value,
        format: OutputFormat,
    ) -> Result<Value> {
        let operation = operation(self.mode, operation_name)?;
        match self.mode {
            ApiMode::Public => self.request_public(operation, input, format).await,
            ApiMode::Private => self.request_private(operation, input, format).await,
        }
    }

    pub fn explain(&self, operation_name: &str, input: Value) -> Result<Value> {
        let operation = operation(self.mode, operation_name)?;
        let request = if self.mode == ApiMode::Private {
            let (pid, path) = private_peek_input(&input)?;
            json!({"pid": pid, "path": BASE64.encode(path)})
        } else {
            serde_json::to_value(dynamic_request(operation, input)?)?
        };
        let (grpcurl_target, plaintext) = grpcurl_target(&self.endpoint);
        let plaintext_flag = if plaintext { "-plaintext " } else { "" };
        let grpcurl = format!(
            "grpcurl {plaintext_flag}-d '{}' '{}' {}",
            shell_single_quote(&request.to_string()),
            shell_single_quote(&grpcurl_target),
            operation.full_method().trim_start_matches('/'),
        );
        Ok(json!({
            "operation": operation.name,
            "readOnly": true,
            "backend": self.endpoint,
            "grpc": {
                "service": operation.service,
                "method": operation.method,
                "fullMethod": operation.full_method(),
                "requestType": operation.request_type,
                "responseType": operation.response_type,
                "protoFile": operation.proto_file,
                "requestJson": request,
                "target": grpcurl_target,
                "plaintext": plaintext,
                "grpcurl": grpcurl,
            }
        }))
    }

    async fn channel(&self) -> Result<Channel> {
        let endpoint = normalize_endpoint(&self.endpoint);
        let mut builder = Endpoint::from_shared(endpoint.clone())
            .with_context(|| format!("invalid gRPC backend URI {endpoint:?}"))?;
        if endpoint.starts_with("https://") {
            builder = builder
                .tls_config(ClientTlsConfig::new().with_webpki_roots())
                .with_context(|| format!("configure TLS for gRPC backend {endpoint}"))?;
        }
        let channel = builder
            .connect_timeout(self.timeout)
            .timeout(self.timeout)
            .connect()
            .await
            .with_context(|| format!("connect to Nockchain gRPC backend {endpoint}"))?;
        Ok(channel)
    }

    async fn request_public(
        &self,
        operation: Operation,
        input: Value,
        format: OutputFormat,
    ) -> Result<Value> {
        let channel = self.channel().await?;
        match operation.name {
            "wallet_get_balance" => {
                let request = decode_request::<WalletGetBalanceRequest>(operation, input)?;
                let mut client = NockchainServiceClient::new(channel);
                let response = client.wallet_get_balance(request).await?.into_inner();
                public_result(operation, response, format)
            }
            "transaction_accepted" => {
                let request = decode_request::<TransactionAcceptedRequest>(operation, input)?;
                let mut client = NockchainServiceClient::new(channel);
                let response = client.transaction_accepted(request).await?.into_inner();
                public_result(operation, response, format)
            }
            "get_blocks" => {
                let request = decode_request::<GetBlocksRequest>(operation, input)?;
                let mut client = NockchainBlockServiceClient::new(channel);
                let response = client.get_blocks(request).await?.into_inner();
                public_result(operation, response, format)
            }
            "get_block_details" => {
                let request = decode_request::<GetBlockDetailsRequest>(operation, input)?;
                let mut client = NockchainBlockServiceClient::new(channel);
                let response = client.get_block_details(request).await?.into_inner();
                public_result(operation, response, format)
            }
            "get_transaction_block" => {
                let request = decode_request::<GetTransactionBlockRequest>(operation, input)?;
                let mut client = NockchainBlockServiceClient::new(channel);
                let response = client.get_transaction_block(request).await?.into_inner();
                public_result(operation, response, format)
            }
            "get_transaction_details" => {
                let request = decode_request::<GetTransactionDetailsRequest>(operation, input)?;
                let mut client = NockchainBlockServiceClient::new(channel);
                let response = client.get_transaction_details(request).await?.into_inner();
                public_result(operation, response, format)
            }
            "get_explorer_metrics" => {
                let request = decode_request::<GetExplorerMetricsRequest>(operation, input)?;
                let mut client = NockchainMetricsServiceClient::new(channel);
                let response = client.get_explorer_metrics(request).await?.into_inner();
                public_result(operation, response, format)
            }
            "get_peer_stats" => {
                let request = decode_request::<GetPeerStatsRequest>(operation, input)?;
                let mut client = NockchainMetricsServiceClient::new(channel);
                let response = client.get_peer_stats(request).await?.into_inner();
                public_result(operation, response, format)
            }
            "get_req_res_metrics" => {
                let request = decode_request::<GetReqResMetricsRequest>(operation, input)?;
                let mut client = NockchainMetricsServiceClient::new(channel);
                let response = client.get_req_res_metrics(request).await?.into_inner();
                public_result(operation, response, format)
            }
            _ => bail!("operation {} is not a public read operation", operation.name),
        }
    }

    async fn request_private(
        &self,
        operation: Operation,
        input: Value,
        format: OutputFormat,
    ) -> Result<Value> {
        if operation.name != "peek" {
            bail!("operation {} is not a private read operation", operation.name);
        }
        let (pid, path) = private_peek_input(&input)?;
        let request = PeekRequest { pid, path };
        let mut client = NockAppServiceClient::new(self.channel().await?);
        let response = client.peek(request).await?.into_inner();
        let data = match response.result {
            Some(peek_response::Result::Data(data)) => data,
            Some(peek_response::Result::Error(error)) => {
                bail!("private Peek failed: {:?}: {}", error.code(), error.message)
            }
            None => bail!("private Peek returned an empty response"),
        };
        match format {
            OutputFormat::Native => Ok(json!({
                "operation": operation.name,
                "format": "native",
                "encoding": "jam",
                "mediaType": "application/vnd.urbit.jam",
                "dataBase64": BASE64.encode(data),
            })),
            OutputFormat::Json => Ok(json!({
                "operation": operation.name,
                "format": "json",
                "encoding": "noun-json",
                "data": jam_to_json(&data)?,
            })),
        }
    }
}

fn normalize_endpoint(endpoint: &str) -> String {
    if endpoint.contains("://") {
        endpoint.to_string()
    } else if endpoint
        .rsplit_once(':')
        .is_some_and(|(_, port)| port == "443")
    {
        format!("https://{endpoint}")
    } else {
        format!("http://{endpoint}")
    }
}

fn decode_request<M>(operation: Operation, input: Value) -> Result<M>
where
    M: Message + Default,
{
    let dynamic = dynamic_request(operation, input)?;
    M::decode(dynamic.encode_to_vec().as_slice())
        .with_context(|| format!("decode typed {} request", operation.request_type))
}

fn dynamic_request(operation: Operation, input: Value) -> Result<DynamicMessage> {
    let pool = descriptor_pool()?;
    let descriptor = pool
        .get_message_by_name(operation.request_type)
        .with_context(|| format!("missing descriptor for {}", operation.request_type))?;
    let text = serde_json::to_string(&input)?;
    let mut deserializer = serde_json::Deserializer::from_str(&text);
    DynamicMessage::deserialize(descriptor, &mut deserializer)
        .with_context(|| format!("invalid {} request JSON", operation.name))
}

fn public_result<M>(operation: Operation, response: M, format: OutputFormat) -> Result<Value>
where
    M: Message,
{
    let bytes = response.encode_to_vec();
    match format {
        OutputFormat::Native => Ok(json!({
            "operation": operation.name,
            "format": "native",
            "encoding": "protobuf",
            "mediaType": "application/protobuf",
            "messageType": operation.response_type,
            "dataBase64": BASE64.encode(bytes),
        })),
        OutputFormat::Json => {
            let pool = descriptor_pool()?;
            let descriptor = pool
                .get_message_by_name(operation.response_type)
                .with_context(|| format!("missing descriptor for {}", operation.response_type))?;
            let dynamic = DynamicMessage::decode(descriptor, bytes.as_slice())?;
            let mut value = serde_json::to_value(dynamic)?;
            dereference_proto_json(&mut value);
            Ok(json!({
                "operation": operation.name,
                "format": "json",
                "encoding": "de-referenced-protobuf-json",
                "data": value,
            }))
        }
    }
}

fn dereference_proto_json(value: &mut Value) {
    match value {
        Value::Array(values) => {
            for value in values {
                dereference_proto_json(value);
            }
        }
        Value::Object(object) => {
            for value in object.values_mut() {
                dereference_proto_json(value);
            }

            if let Some(hash) = hash_json(object) {
                *value = hash;
                return;
            }
            if let Some(array) = belt_array_json(object) {
                *value = array;
                return;
            }
            if object.len() == 1 {
                if let Some(inner) = object.remove("value") {
                    *value = inner;
                } else if let Some(inner) = object.remove("hash") {
                    *value = inner;
                } else if let Some(inner) = object.remove("key") {
                    *value = inner;
                }
            }
        }
        _ => {}
    }
}

fn belt_array_json(object: &Map<String, Value>) -> Option<Value> {
    let count = if object.len() == 6 {
        6
    } else if object.len() == 8 {
        8
    } else {
        return None;
    };
    let values = (1..=count)
        .map(|index| object.get(&format!("belt{index}")))
        .collect::<Option<Vec<_>>>()?;
    Some(Value::Array(values.into_iter().cloned().collect()))
}

fn hash_json(object: &Map<String, Value>) -> Option<Value> {
    if object.len() != 5 {
        return None;
    }
    let mut belts = [Belt(0); 5];
    for (index, belt) in belts.iter_mut().enumerate() {
        let value = object.get(&format!("belt{}", index + 1))?;
        let number = match value {
            Value::String(value) => value.parse().ok()?,
            Value::Number(value) => value.as_u64()?,
            _ => return None,
        };
        *belt = Belt(number);
    }
    let hash = Hash(belts);
    Some(json!({
        "base58": hash.to_base58(),
        "belts": belts.map(|belt| belt.0.to_string()),
    }))
}

fn private_peek_input(input: &Value) -> Result<(i32, Vec<u8>)> {
    let object = input
        .as_object()
        .context("peek input must be a JSON object")?;
    let pid = match object.get("pid") {
        Some(Value::Number(pid)) => pid
            .as_i64()
            .and_then(|pid| i32::try_from(pid).ok())
            .context("peek pid must fit in a signed 32-bit integer")?,
        None => 0,
        _ => bail!("peek pid must be an integer"),
    };
    let path = object
        .get("path")
        .context("peek input requires path: {preset}, {jamBase64}, or {noun}")?;
    Ok((pid, encode_private_path(path)?))
}

fn encode_private_path(path: &Value) -> Result<Vec<u8>> {
    if let Some(text) = path.as_str() {
        return encode_preset(text, None);
    }
    let object = path
        .as_object()
        .context("peek path must be a string or object")?;
    if let Some(encoded) = object.get("jamBase64").and_then(Value::as_str) {
        return BASE64.decode(encoded).context("decode path.jamBase64");
    }
    if let Some(preset) = object.get("preset").and_then(Value::as_str) {
        return encode_preset(preset, object.get("argument"));
    }
    if let Some(noun) = object.get("noun") {
        let mut slab: NounSlab = NounSlab::new();
        let root = json_to_noun(&mut slab, noun, 0)?;
        slab.set_root(root);
        return Ok(slab.jam().to_vec());
    }
    bail!("peek path requires one of preset, jamBase64, or noun")
}

fn encode_preset(preset: &str, argument: Option<&Value>) -> Result<Vec<u8>> {
    let mut slab: NounSlab = NounSlab::new();
    let tag = make_tas(&mut slab, preset).as_noun();
    let root = match preset {
        "heaviest-chain"
        | "heaviest-block"
        | "constants"
        | "blockchain-constants"
        | "raw-transactions" => {
            if argument.is_some() {
                bail!("{preset} preset does not accept an argument")
            }
            T(&mut slab, &[tag, SIG])
        }
        "heavy-n" => {
            let height = argument
                .and_then(Value::as_u64)
                .context("heavy-n preset requires an unsigned integer argument")?;
            let height = Atom::new(&mut slab, height).as_noun();
            T(&mut slab, &[tag, height, SIG])
        }
        "block-transactions" | "raw-transaction" => {
            let text = argument
                .and_then(Value::as_str)
                .context("block-transactions/raw-transaction preset requires a string argument")?;
            let argument = Atom::from_value(&mut slab, text.as_bytes())
                .map_err(|error| anyhow!("encode preset argument as an atom: {error}"))?
                .as_noun();
            T(&mut slab, &[tag, argument, SIG])
        }
        _ => bail!("unknown private peek preset {preset:?}"),
    };
    slab.set_root(root);
    Ok(slab.jam().to_vec())
}

fn json_to_noun(slab: &mut NounSlab, value: &Value, depth: usize) -> Result<Noun> {
    if depth > 256 {
        bail!("noun JSON exceeds the maximum depth of 256")
    }
    let object = value
        .as_object()
        .context("noun must be {atom:{...}} or {cell:[head,tail]}")?;
    if let Some(atom) = object.get("atom") {
        let atom = atom.as_object().context("noun.atom must be an object")?;
        if let Some(unsigned) = atom.get("unsigned") {
            let number = match unsigned {
                Value::String(number) => number.parse::<u64>()?,
                Value::Number(number) => number.as_u64().context("atom unsigned is negative")?,
                _ => bail!("atom.unsigned must be a u64 number or decimal string"),
            };
            return Ok(Atom::new(slab, number).as_noun());
        }
        if let Some(text) = atom.get("text").and_then(Value::as_str) {
            return Ok(Atom::from_value(slab, text.as_bytes())
                .map_err(|error| anyhow!("encode noun text atom: {error}"))?
                .as_noun());
        }
        if let Some(encoded) = atom.get("base64").and_then(Value::as_str) {
            let bytes = BASE64.decode(encoded).context("decode atom.base64")?;
            return Ok(Atom::from_value(slab, bytes.as_slice())
                .map_err(|error| anyhow!("encode noun byte atom: {error}"))?
                .as_noun());
        }
        bail!("noun.atom requires unsigned, text, or base64")
    }
    if let Some(cell) = object.get("cell") {
        let pair = cell
            .as_array()
            .filter(|pair| pair.len() == 2)
            .context("noun.cell must be a two-element array [head, tail]")?;
        let head = json_to_noun(slab, &pair[0], depth + 1)?;
        let tail = json_to_noun(slab, &pair[1], depth + 1)?;
        return Ok(T(slab, &[head, tail]));
    }
    bail!("noun requires atom or cell")
}

fn jam_to_json(bytes: &[u8]) -> Result<Value> {
    let mut slab: NounSlab = NounSlab::new();
    let root = slab
        .cue_into(Bytes::copy_from_slice(bytes))
        .context("cue JAM response")?;
    let space = slab.noun_space();
    let mut nodes = 0usize;
    noun_to_json(NounHandle::new(root, &space), 0, &mut nodes)
}

fn noun_to_json(noun: NounHandle<'_>, depth: usize, nodes: &mut usize) -> Result<Value> {
    *nodes += 1;
    if *nodes > 100_000 {
        bail!("decoded noun exceeds the 100,000-node JSON limit")
    }
    if depth > 256 {
        bail!("decoded noun exceeds the maximum JSON depth of 256")
    }
    if let Some(atom) = noun.atom() {
        let little_endian = atom.to_le_bytes();
        let trimmed = little_endian
            .iter()
            .rposition(|byte| *byte != 0)
            .map_or(&[][..], |last| &little_endian[..=last]);
        let mut details = Map::new();
        details.insert(
            "littleEndianBase64".to_string(),
            json!(BASE64.encode(trimmed)),
        );
        details.insert("littleEndianHex".to_string(), json!(hex_string(trimmed)));
        if let Ok(number) = atom.as_u64() {
            details.insert("unsigned".to_string(), json!(number.to_string()));
        }
        if let Ok(text) = std::str::from_utf8(trimmed) {
            if !text.is_empty() && text.chars().all(|character| !character.is_control()) {
                details.insert("text".to_string(), json!(text));
            }
        }
        return Ok(json!({"atom": details}));
    }
    let cell = noun.as_cell().context("noun is neither atom nor cell")?;
    Ok(json!({
        "cell": [
            noun_to_json(cell.head(), depth + 1, nodes)?,
            noun_to_json(cell.tail(), depth + 1, nodes)?,
        ]
    }))
}

fn hex_string(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        output.push(char::from(HEX[(byte >> 4) as usize]));
        output.push(char::from(HEX[(byte & 0x0f) as usize]));
    }
    output
}

fn shell_single_quote(value: &str) -> String {
    value.replace('\'', "'\\''")
}

fn grpcurl_target(endpoint: &str) -> (String, bool) {
    let endpoint = normalize_endpoint(endpoint);
    if let Some(target) = endpoint.strip_prefix("http://") {
        (target.trim_end_matches('/').to_string(), true)
    } else if let Some(target) = endpoint.strip_prefix("https://") {
        (target.trim_end_matches('/').to_string(), false)
    } else {
        (endpoint.trim_end_matches('/').to_string(), true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn private_presets_are_jam_and_explain_as_grpc() {
        let backend = Backend {
            mode: ApiMode::Private,
            endpoint: "http://127.0.0.1:5555".to_string(),
            timeout: Duration::from_secs(1),
        };
        let explanation = backend
            .explain("peek", json!({"path": {"preset": "heaviest-chain"}}))
            .expect("explain private peek");
        assert_eq!(
            explanation["grpc"]["fullMethod"],
            "/nockchain.private.v1.NockAppService/Peek"
        );
        assert!(explanation["grpc"]["requestJson"]["path"]
            .as_str()
            .is_some_and(|path| !path.is_empty()));
        assert_eq!(explanation["grpc"]["target"], "127.0.0.1:5555");
        assert_eq!(explanation["grpc"]["plaintext"], true);
        assert!(explanation["grpc"]["grpcurl"]
            .as_str()
            .is_some_and(|command| command.contains("NockAppService/Peek")));
    }

    #[test]
    fn heavy_n_preset_matches_the_direct_kernel_path_shape() {
        let actual = encode_preset("heavy-n", Some(&json!(42))).expect("encode preset");

        let mut expected: NounSlab = NounSlab::new();
        let tag = make_tas(&mut expected, "heavy-n").as_noun();
        let height = Atom::new(&mut expected, 42).as_noun();
        let root = T(&mut expected, &[tag, height, SIG]);
        expected.set_root(root);

        assert_eq!(actual, expected.jam());
    }

    #[test]
    fn noun_json_round_trips_through_jam() {
        let input = json!({
            "noun": {
                "cell": [
                    {"atom": {"text": "hello"}},
                    {"atom": {"unsigned": "0"}}
                ]
            }
        });
        let jam = encode_private_path(&input).expect("encode noun");
        let output = jam_to_json(&jam).expect("decode noun");
        assert_eq!(output["cell"][0]["atom"]["text"], "hello");
        assert_eq!(output["cell"][1]["atom"]["unsigned"], "0");
    }

    #[test]
    fn mutations_are_rejected_by_catalog_lookup() {
        let backend = Backend {
            mode: ApiMode::Public,
            endpoint: "http://127.0.0.1:5555".to_string(),
            timeout: Duration::from_secs(1),
        };
        assert!(backend
            .explain("wallet_send_transaction", Value::Null)
            .is_err());
    }

    #[test]
    fn explain_validates_public_protobuf_json() {
        let backend = Backend {
            mode: ApiMode::Public,
            endpoint: "http://127.0.0.1:5556".to_string(),
            timeout: Duration::from_secs(1),
        };
        let error = backend
            .explain("get_blocks", json!({"notAField": true}))
            .expect_err("unknown protobuf field must fail");
        assert!(error
            .to_string()
            .contains("invalid get_blocks request JSON"));
    }

    #[test]
    fn bare_standard_tls_port_selects_https() {
        assert_eq!(
            normalize_endpoint("api.nockscan.net:443"),
            "https://api.nockscan.net:443"
        );
        assert_eq!(
            grpcurl_target("api.nockscan.net:443"),
            ("api.nockscan.net:443".to_string(), false)
        );
        assert_eq!(
            normalize_endpoint("127.0.0.1:5556"),
            "http://127.0.0.1:5556"
        );
    }
}

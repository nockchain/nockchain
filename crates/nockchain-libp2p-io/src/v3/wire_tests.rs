//! Fixed protobuf wire vectors with an independently authored semantic oracle.
//!
//! The Google reference runner consumes the same JSON expectations. No vector
//! input or expected noun is produced by the Prost encoder under test.

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

use base64::engine::general_purpose::STANDARD;
use base64::Engine as _;
use futures::io::Cursor;
use libp2p::request_response::Codec as _;
use libp2p::StreamProtocol;
use nockapp::noun::slab::NounSlab;
use nockapp::utils::make_tas;
use nockvm::noun::{Atom, Noun, D, T};
use prost::Message;
use serde::{Deserialize, Serialize};
use serde_bytes::ByteBuf;
use serde_json::{json, Map, Value};

use super::codec::ProtobufCodec;
use super::pb;
use crate::config::LibP2PConfig;
use crate::messages::{
    BatchErrorClass, BatchRequestItem, BatchResultStatus, NockchainRequest, NockchainResponse,
};

#[derive(Deserialize)]
struct Manifest {
    version: u32,
    vectors: Vec<Vector>,
}

#[derive(Deserialize)]
struct Vector {
    id: String,
    category: String,
    message_type: String,
    hex: String,
    protobuf_verdict: String,
    expected_semantics: Value,
    domain: Domain,
}

#[derive(Deserialize)]
struct Domain {
    verdict: String,
    canonical_nouns: Vec<NounSpec>,
}

/// Readable Hoon-shape oracle, independent of v3's DTO-to-noun conversion.
#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
enum NounSpec {
    Atom(u64),
    Tas(String),
    Tuple(Vec<Self>),
    List(Vec<Self>),
}

impl NounSpec {
    fn to_noun(&self, slab: &mut NounSlab) -> Noun {
        match self {
            Self::Atom(value) => Atom::new(slab, *value).as_noun(),
            Self::Tas(value) => make_tas(slab, value).as_noun(),
            Self::Tuple(fields) => {
                assert!(fields.len() >= 2, "oracle tuple needs at least two fields");
                let fields: Vec<_> = fields.iter().map(|field| field.to_noun(slab)).collect();
                T(slab, &fields)
            }
            Self::List(items) => {
                let mut tail = D(0);
                for item in items.iter().rev() {
                    let head = item.to_noun(slab);
                    tail = T(slab, &[head, tail]);
                }
                tail
            }
        }
    }

    fn jam(&self) -> Vec<u8> {
        let mut slab: NounSlab = NounSlab::new();
        let root = self.to_noun(&mut slab);
        slab.set_root(root);
        slab.jam().as_ref().to_vec()
    }
}

fn manifest_path() -> PathBuf {
    let relative = Path::new("crates/nockchain-libp2p-io/tests/fixtures/peer_v3_wire/vectors.json");
    let mut candidates = Vec::new();
    for variable in ["TEST_SRCDIR", "RUNFILES_DIR"] {
        if let Some(root) = std::env::var_os(variable) {
            let root = PathBuf::from(root);
            if let Some(workspace) = std::env::var_os("TEST_WORKSPACE") {
                candidates.push(root.join(workspace).join(relative));
            }
            candidates.push(root.join("_main").join(relative));
        }
    }
    candidates.push(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/peer_v3_wire/vectors.json"),
    );
    candidates.push(relative.to_path_buf());
    candidates.push(Path::new("open").join(relative));
    candidates
        .into_iter()
        .find(|path| path.is_file())
        .expect("wire vector manifest is available")
}

fn manifest() -> Manifest {
    let manifest: Manifest = serde_json::from_slice(&fs::read(manifest_path()).unwrap()).unwrap();
    assert_eq!(manifest.version, 1);
    assert!(!manifest.vectors.is_empty());
    let mut ids = BTreeSet::new();
    let mut categories = BTreeSet::new();
    for vector in &manifest.vectors {
        assert!(ids.insert(&vector.id), "duplicate wire vector id");
        assert!(!vector.category.is_empty());
        categories.insert(vector.category.as_str());
        assert_eq!(vector.protobuf_verdict, "accept");
        assert!(matches!(
            vector.domain.verdict.as_str(),
            "accept" | "reject"
        ));
        assert!(
            hex::decode(&vector.hex).unwrap().len() <= 256,
            "fixed vector bound: {}",
            vector.id
        );
    }
    for required in [
        "duplicate_scalar", "presence", "singular_message_merge", "oneof_same_message",
        "oneof_different_member", "oneof_switchback", "oneof_scalar", "repeated_numeric",
        "unknown_fields", "enum",
    ] {
        assert!(
            categories.contains(required),
            "missing wire requirement: {required}"
        );
    }
    manifest
}

fn field<T: Serialize>(map: &mut Map<String, Value>, key: &str, value: Option<T>) {
    if let Some(value) = value {
        map.insert(key.to_owned(), serde_json::to_value(value).unwrap());
    }
}

fn repeated<T: Serialize>(map: &mut Map<String, Value>, key: &str, values: Vec<T>) {
    if !values.is_empty() {
        field(map, key, Some(values));
    }
}

// This intentionally small ProtoJSON projection handles the exercised DTO
// shapes. Exhaustive struct destructuring catches added fields at compile time;
// unexercised message variants fail explicitly instead of silently disappearing.
fn hash_json(value: pb::Hash) -> Value {
    let pb::Hash {
        limb0,
        limb1,
        limb2,
        limb3,
        limb4,
    } = value;
    let mut map = Map::new();
    for (name, limb) in [
        ("limb0", limb0),
        ("limb1", limb1),
        ("limb2", limb2),
        ("limb3", limb3),
        ("limb4", limb4),
    ] {
        field(&mut map, name, limb.map(|value| value.to_string()));
    }
    Value::Object(map)
}

fn atom_json(value: pb::Atom) -> Value {
    let pb::Atom { little_endian } = value;
    let mut map = Map::new();
    field(
        &mut map,
        "little_endian",
        little_endian.map(|bytes| STANDARD.encode(bytes)),
    );
    Value::Object(map)
}

fn bignum_json(value: pb::PageBigNum) -> Value {
    let pb::PageBigNum { limbs } = value;
    let mut map = Map::new();
    repeated(&mut map, "limbs", limbs);
    Value::Object(map)
}

fn coinbase_json(value: pb::PageCoinbase) -> Value {
    let pb::PageCoinbase { kind } = value;
    match kind {
        None => json!({}),
        Some(pb::page_coinbase::Kind::Legacy(value)) => {
            let pb::PageLegacyCoinbase { entries } = value;
            assert!(
                entries.is_empty(),
                "wire oracle only uses empty coinbase maps"
            );
            json!({"legacy": {}})
        }
        Some(pb::page_coinbase::Kind::V1(value)) => {
            let pb::PageV1Coinbase { entries } = value;
            assert!(
                entries.is_empty(),
                "wire oracle only uses empty coinbase maps"
            );
            json!({"v1": {}})
        }
    }
}

fn page_json(value: pb::Page) -> Value {
    let pb::Page {
        version,
        digest,
        pow,
        parent,
        tx_ids,
        coinbase,
        timestamp,
        epoch_counter,
        target,
        accumulated_work,
        height,
        message,
    } = value;
    assert!(pow.is_none(), "wire oracle page has no proof artifact");
    let mut map = Map::new();
    field(&mut map, "version", version);
    field(&mut map, "digest", digest.map(hash_json));
    field(&mut map, "parent", parent.map(hash_json));
    repeated(
        &mut map,
        "tx_ids",
        tx_ids.into_iter().map(hash_json).collect(),
    );
    field(&mut map, "coinbase", coinbase.map(coinbase_json));
    field(&mut map, "timestamp", timestamp.map(atom_json));
    field(&mut map, "epoch_counter", epoch_counter.map(atom_json));
    field(&mut map, "target", target.map(bignum_json));
    field(
        &mut map,
        "accumulated_work",
        accumulated_work.map(bignum_json),
    );
    field(&mut map, "height", height.map(|value| value.to_string()));
    repeated(
        &mut map,
        "message",
        message.into_iter().map(|value| value.to_string()).collect(),
    );
    Value::Object(map)
}

fn data_request_json(value: pb::DataRequest) -> Value {
    use pb::data_request::Kind;
    let pb::DataRequest { kind } = value;
    match kind {
        None => json!({}),
        Some(Kind::BlockByHeight(height)) => json!({"block_by_height": height.to_string()}),
        Some(Kind::BlockRange(range)) => {
            let pb::BlockRangeRequest {
                start_height,
                length,
            } = range;
            let mut map = Map::new();
            field(
                &mut map,
                "start_height",
                start_height.map(|value| value.to_string()),
            );
            field(&mut map, "length", length);
            json!({"block_range": map})
        }
        Some(other) => panic!("unexercised data request in wire oracle: {other:?}"),
    }
}

fn request_json(value: pb::PeerRequest) -> Value {
    let pb::PeerRequest { kind } = value;
    match kind {
        None => json!({}),
        Some(pb::peer_request::Kind::Batch(batch)) => {
            let pb::BatchRequest {
                proof,
                nonce,
                items,
            } = batch;
            let mut map = Map::new();
            field(&mut map, "proof", proof.map(|bytes| STANDARD.encode(bytes)));
            field(&mut map, "nonce", nonce.map(|value| value.to_string()));
            repeated(
                &mut map,
                "items",
                items
                    .into_iter()
                    .map(|item| {
                        let pb::RequestItem { item_id, request } = item;
                        let mut map = Map::new();
                        field(&mut map, "item_id", item_id);
                        field(&mut map, "request", request.map(data_request_json));
                        Value::Object(map)
                    })
                    .collect(),
            );
            json!({"batch": map})
        }
        Some(other) => panic!("unexercised peer request in wire oracle: {other:?}"),
    }
}

fn response_json(value: pb::PeerResponse) -> Value {
    let pb::PeerResponse { kind } = value;
    match kind {
        None => json!({}),
        Some(pb::peer_response::Kind::Acknowledged(value)) => json!({"acknowledged": value}),
        Some(pb::peer_response::Kind::Batch(batch)) => {
            let pb::BatchResult { items } = batch;
            let mut map = Map::new();
            repeated(
                &mut map,
                "items",
                items
                    .into_iter()
                    .map(|item| {
                        let pb::ResultItem { item_id, outcome } = item;
                        let mut map = Map::new();
                        field(&mut map, "item_id", item_id);
                        match outcome {
                            None => (),
                            Some(pb::result_item::Outcome::Error(error)) => {
                                let pb::ErrorResult { classification } = error;
                                let mut error = Map::new();
                                field(
                                    &mut error,
                                    "classification",
                                    classification.map(|value| {
                                        match pb::ErrorClass::try_from(value) {
                                            Ok(value) => json!(value.as_str_name()),
                                            Err(_) => json!(value),
                                        }
                                    }),
                                );
                                field(&mut map, "error", Some(error));
                            }
                            Some(pb::result_item::Outcome::Result(envelope)) => {
                                let pb::ResponseEnvelope { kind } = envelope;
                                let result = match kind {
                                    None => json!({}),
                                    Some(pb::response_envelope::Kind::Block(page)) => {
                                        json!({"block": page_json(page)})
                                    }
                                    Some(other) => panic!(
                                        "unexercised response envelope in wire oracle: {other:?}"
                                    ),
                                };
                                field(&mut map, "result", Some(result));
                            }
                            Some(other) => {
                                panic!("unexercised result outcome in wire oracle: {other:?}")
                            }
                        }
                        Value::Object(map)
                    })
                    .collect(),
            );
            json!({"batch": map})
        }
    }
}

fn frame(bytes: &[u8]) -> Cursor<Vec<u8>> {
    let mut frame = u32::try_from(bytes.len()).unwrap().to_be_bytes().to_vec();
    frame.extend_from_slice(bytes);
    Cursor::new(frame)
}

fn assert_nouns(vector: &Vector, messages: Vec<&ByteBuf>) {
    assert_eq!(
        messages.len(),
        vector.domain.canonical_nouns.len(),
        "{}: noun count",
        vector.id
    );
    for (actual, expected) in messages.into_iter().zip(&vector.domain.canonical_nouns) {
        assert_eq!(
            actual.as_ref(),
            expected.jam(),
            "{}: independent canonical noun",
            vector.id
        );
    }
}

fn assert_request_metadata(vector: &Vector, checked: &NockchainRequest) {
    let expected = &vector.expected_semantics["batch"];
    let NockchainRequest::BatchRequest { pow, nonce, items } = checked else {
        panic!("{}: expected checked batch request", vector.id)
    };
    assert_eq!(
        pow.as_slice(),
        STANDARD
            .decode(expected["proof"].as_str().unwrap())
            .unwrap(),
        "{}: checked proof bytes",
        vector.id,
    );
    assert_eq!(
        *nonce,
        expected["nonce"].as_str().unwrap().parse::<u64>().unwrap(),
        "{}: checked nonce",
        vector.id,
    );
    let expected_items = expected["items"].as_array().unwrap();
    assert_eq!(
        items.len(),
        expected_items.len(),
        "{}: checked item count",
        vector.id
    );
    for (item, expected) in items.iter().zip(expected_items) {
        assert_eq!(
            u64::from(item.item_id),
            expected["item_id"].as_u64().unwrap(),
            "{}: checked item ID",
            vector.id
        );
    }
}

fn assert_response_metadata(vector: &Vector, checked: &NockchainResponse) {
    if let Some(expected) = vector.expected_semantics.get("acknowledged") {
        let NockchainResponse::Ack { acked } = checked else {
            panic!("{}: expected checked acknowledgement", vector.id)
        };
        assert_eq!(
            *acked,
            expected.as_bool().unwrap(),
            "{}: checked acknowledgement",
            vector.id
        );
        return;
    }
    let expected_items = vector.expected_semantics["batch"]["items"]
        .as_array()
        .unwrap();
    let NockchainResponse::BatchResult { results } = checked else {
        panic!("{}: expected checked batch response", vector.id)
    };
    assert_eq!(
        results.len(),
        expected_items.len(),
        "{}: checked result count",
        vector.id
    );
    for (item, expected) in results.iter().zip(expected_items) {
        assert_eq!(
            u64::from(item.item_id),
            expected["item_id"].as_u64().unwrap(),
            "{}: checked result ID",
            vector.id
        );
        if let Some(error) = expected.get("error") {
            let expected_error = match error["classification"].as_str().unwrap() {
                "ERROR_CLASS_DECODE" => BatchErrorClass::Decode,
                "ERROR_CLASS_BACKPRESSURE" => BatchErrorClass::Backpressure,
                "ERROR_CLASS_TOO_LARGE" => BatchErrorClass::TooLarge,
                "ERROR_CLASS_INVALID_POW" => BatchErrorClass::InvalidPow,
                "ERROR_CLASS_INTERNAL" => BatchErrorClass::Internal,
                other => panic!("unexpected accepted oracle classification {other}"),
            };
            assert_eq!(
                item.status,
                BatchResultStatus::Error,
                "{}: checked status",
                vector.id
            );
            assert_eq!(
                item.error,
                Some(expected_error),
                "{}: checked error class",
                vector.id
            );
            assert!(
                item.envelope.is_none(),
                "{}: error has no result envelope",
                vector.id
            );
        } else {
            assert!(expected.get("result").is_some(), "expected result oracle");
            assert_eq!(
                item.status,
                BatchResultStatus::Result,
                "{}: checked status",
                vector.id
            );
            assert!(
                item.error.is_none(),
                "{}: result has no error class",
                vector.id
            );
            assert!(
                item.envelope.is_some(),
                "{}: result envelope exists",
                vector.id
            );
        }
    }
}

#[tokio::test]
async fn fixed_wire_vectors_match_reference_semantics_and_checked_domain() {
    let manifest = manifest();
    let protocol = StreamProtocol::new(LibP2PConfig::req_res_protocol_version());
    let mut codec = ProtobufCodec::new(4096, 4096);
    for vector in &manifest.vectors {
        let bytes = hex::decode(&vector.hex).unwrap();
        let mut stream = frame(&bytes);
        let mut normalized = Cursor::new(Vec::new());
        match vector.message_type.as_str() {
            "nockchain.peer.v3.PeerRequest" => {
                let dto = pb::PeerRequest::decode(bytes.as_slice()).unwrap();
                assert_eq!(
                    request_json(dto),
                    vector.expected_semantics,
                    "{}: protobuf semantics",
                    vector.id
                );
                let result = codec.read_request(&protocol, &mut stream).await;
                if vector.domain.verdict == "reject" {
                    assert_eq!(
                        result.unwrap_err().kind(),
                        std::io::ErrorKind::InvalidData,
                        "{}",
                        vector.id
                    );
                    assert!(vector.domain.canonical_nouns.is_empty());
                    continue;
                }
                let checked = result.unwrap_or_else(|error| panic!("{}: {error}", vector.id));
                assert_request_metadata(vector, &checked);
                let messages = match &checked {
                    NockchainRequest::BatchRequest { items, .. } => {
                        items.iter().map(|item| &item.message).collect()
                    }
                    NockchainRequest::AuthenticatedGossip { message, .. } => vec![message],
                };
                assert_nouns(vector, messages);
                codec
                    .write_request(&protocol, &mut normalized, checked)
                    .await
                    .unwrap();
                let normalized = pb::PeerRequest::decode(&normalized.get_ref()[4..]).unwrap();
                assert_eq!(
                    request_json(normalized),
                    vector.expected_semantics,
                    "{}: accepted meaning after normalization",
                    vector.id
                );
            }
            "nockchain.peer.v3.PeerResponse" => {
                let dto = pb::PeerResponse::decode(bytes.as_slice()).unwrap();
                assert_eq!(
                    response_json(dto),
                    vector.expected_semantics,
                    "{}: protobuf semantics",
                    vector.id
                );
                let result = codec.read_response(&protocol, &mut stream).await;
                if vector.domain.verdict == "reject" {
                    assert_eq!(
                        result.unwrap_err().kind(),
                        std::io::ErrorKind::InvalidData,
                        "{}",
                        vector.id
                    );
                    assert!(vector.domain.canonical_nouns.is_empty());
                    continue;
                }
                let checked = result.unwrap_or_else(|error| panic!("{}: {error}", vector.id));
                assert_response_metadata(vector, &checked);
                let messages = match &checked {
                    NockchainResponse::Ack { .. } => Vec::new(),
                    NockchainResponse::BatchResult { results } => results
                        .iter()
                        .filter_map(|item| item.envelope.as_ref().map(|envelope| &envelope.message))
                        .collect(),
                    other => panic!("unexercised checked response: {other:?}"),
                };
                assert_nouns(vector, messages);
                codec
                    .write_response(&protocol, &mut normalized, checked)
                    .await
                    .unwrap();
                let normalized = pb::PeerResponse::decode(&normalized.get_ref()[4..]).unwrap();
                assert_eq!(
                    response_json(normalized),
                    vector.expected_semantics,
                    "{}: accepted meaning after normalization",
                    vector.id
                );
            }
            other => panic!("unsupported vector root {other}"),
        }
    }
}

fn varint(mut value: u64) -> Vec<u8> {
    let mut bytes = Vec::new();
    while value >= 128 {
        bytes.push((value as u8 & 127) | 128);
        value >>= 7;
    }
    bytes.push(value as u8);
    bytes
}

fn message_field(number: u64, bytes: &[u8]) -> Vec<u8> {
    let mut field = varint((number << 3) | 2);
    field.extend(varint(bytes.len() as u64));
    field.extend_from_slice(bytes);
    field
}

#[tokio::test]
async fn harmless_unknown_fields_and_field_order_preserve_authenticated_meaning() {
    let manifest = manifest();
    let source = manifest
        .vectors
        .iter()
        .find(|vector| vector.id == "optional_scalar_last_explicit_zero")
        .unwrap();
    let sender = libp2p::identity::Keypair::ed25519_from_bytes([17; 32])
        .unwrap()
        .public()
        .to_peer_id();
    let receiver = libp2p::identity::Keypair::ed25519_from_bytes([19; 32])
        .unwrap()
        .public()
        .to_peer_id();
    let mut builder = equix::EquiXBuilder::new();
    let original = NockchainRequest::new_batch_request(
        &mut builder,
        &sender,
        &receiver,
        vec![BatchRequestItem {
            item_id: 0,
            message: ByteBuf::from(source.domain.canonical_nouns[0].jam()),
        }],
    )
    .unwrap();
    let NockchainRequest::BatchRequest { pow, nonce, .. } = &original else {
        unreachable!()
    };
    // Build a second wire spelling directly: items, nonce, proof; unknown
    // fields occur both inside DataRequest and before the outer request. These
    // are harmless standards-defined variations of the same checked request.
    let item = hex::decode("080012050800f80301").unwrap();
    let mut batch = message_field(3, &item);
    batch.push(0x10);
    batch.extend(varint(*nonce));
    batch.extend(message_field(1, pow));
    let mut wire = hex::decode("f80301").unwrap();
    wire.extend(message_field(1, &batch));
    let protocol = StreamProtocol::new(LibP2PConfig::req_res_protocol_version());
    let mut codec = ProtobufCodec::new(4096, 4096);
    let decoded = codec
        .read_request(&protocol, &mut frame(&wire))
        .await
        .unwrap();
    assert_eq!(decoded, original);
    assert_eq!(
        decoded.replay_key().unwrap(),
        original.replay_key().unwrap()
    );
    decoded
        .verify_pow(&mut builder, &receiver, &sender)
        .unwrap();
}

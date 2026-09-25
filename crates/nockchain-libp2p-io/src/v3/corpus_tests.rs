//! Compatibility checks against captured history, never generated v3 output.
//!
//! The manifest pins independently extracted metadata. The raw captured JAM
//! itself is the byte oracle; even a mutually consistent encoder/decoder pair
//! must reproduce it exactly. Test transport envelopes are built around those
//! payloads here and are not represented as captured network requests.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Component, Path, PathBuf};

use bytes::Bytes;
use futures::io::Cursor;
use libp2p::request_response::Codec as _;
use libp2p::StreamProtocol;
use nockapp::noun::slab::NounSlab;
use nockapp::utils::make_tas;
use nockchain_types::common::Hash;
use nockchain_types::{v0, v1};
use nockvm::noun::{Noun, NounAllocator, NounHandle, T};
use noun_serde::NounDecode;
use prost::Message;
use serde::Deserialize;
use serde_bytes::ByteBuf;
use sha2::{Digest, Sha256};

use super::codec::ProtobufCodec;
use super::page::PeerPage;
use super::transaction::Transaction;
use super::{decode_request, encode_request, encode_response, pb};
use crate::config::LibP2PConfig;
use crate::messages::{
    BatchResultItem, BatchResultStatus, BundledTxEnvelope, NockchainFact, NockchainRequest,
    NockchainResponse, ResponseEnvelope,
};

#[derive(Debug, Deserialize)]
struct Manifest {
    version: u32,
    #[serde(default)]
    required: RequiredCoverage,
    public_checkpoints: Vec<PageReference>,
    captures: Vec<Capture>,
}

#[derive(Debug, Deserialize)]
struct PageReference {
    height: u64,
    id_base58: String,
}

#[derive(Debug, Deserialize)]
#[serde(default)]
struct RequiredCoverage {
    page_versions: Vec<u32>,
    transaction_versions: Vec<u32>,
    pow_versions: Vec<u64>,
    spend_versions: Vec<u32>,
    lock_merkle_proof_kinds: Vec<String>,
    nonempty_bundle_page_versions: Vec<u32>,
}

impl Default for RequiredCoverage {
    fn default() -> Self {
        Self {
            page_versions: vec![0, 1],
            transaction_versions: vec![0, 1],
            pow_versions: Vec::new(),
            spend_versions: Vec::new(),
            lock_merkle_proof_kinds: Vec::new(),
            nonempty_bundle_page_versions: Vec::new(),
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
enum Kind {
    Page,
    Transaction,
}

#[derive(Debug, Deserialize)]
struct Capture {
    file: String,
    kind: Kind,
    consensus_version: u32,
    id_base58: String,
    source_label: String,
    event_num: Option<u64>,
    validation: String,
    height: Option<u64>,
    pow_version: Option<u64>,
    transaction_count: Option<usize>,
    tx_ids_base58: Option<Vec<String>>,
    spend_versions: Option<Vec<u32>>,
    inputs_or_spends_count: Option<usize>,
    sha256: String,
    size_bytes: usize,
    #[serde(default)]
    included_in_pages: Vec<PageReference>,
}

#[derive(Debug, PartialEq, Eq)]
struct PageFields {
    version: u32,
    id: String,
    parent: String,
    height: u64,
    pow_version: Option<u64>,
    transaction_ids: BTreeSet<String>,
    coinbase_count: usize,
}

#[derive(Debug, PartialEq, Eq)]
struct TransactionFields {
    version: u32,
    id: String,
    entry_count: usize,
    spend_versions: BTreeSet<u32>,
    lock_merkle_proof_kinds: BTreeSet<String>,
}

fn corpus_dir() -> PathBuf {
    let relative = Path::new("crates/nockchain-libp2p-io/tests/fixtures/peer_v3");
    let mut candidates = Vec::new();
    for env in ["TEST_SRCDIR", "RUNFILES_DIR"] {
        if let Some(runfiles) = std::env::var_os(env) {
            let runfiles = PathBuf::from(runfiles);
            if let Some(workspace) = std::env::var_os("TEST_WORKSPACE") {
                candidates.push(runfiles.join(workspace).join(relative));
            }
            candidates.push(runfiles.join("_main").join(relative));
        }
    }
    candidates.push(Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/peer_v3"));
    candidates.push(relative.to_path_buf());
    candidates.push(Path::new("open").join(relative));
    candidates
        .into_iter()
        .find(|path| path.join("manifest.json").is_file())
        .expect("checked-in peer_v3 captured corpus manifest must be available to the test")
}

fn corpus() -> (PathBuf, Manifest) {
    let dir = corpus_dir();
    let manifest: Manifest = serde_json::from_slice(
        &fs::read(dir.join("manifest.json")).expect("captured corpus manifest is readable"),
    )
    .expect("captured corpus manifest is valid JSON");
    assert_eq!(manifest.version, 1, "unsupported corpus manifest format");
    assert!(
        !manifest.captures.is_empty(),
        "historical corpus must not be empty"
    );
    let mut files = BTreeSet::new();
    let mut ids = BTreeSet::new();
    for capture in &manifest.captures {
        let mut components = Path::new(&capture.file).components();
        assert!(
            matches!(components.next(), Some(Component::Normal(_)))
                && components.next().is_none()
                && capture.file.ends_with(".jam"),
            "capture must name one flat JAM file: {}",
            capture.file,
        );
        assert!(
            files.insert(&capture.file),
            "duplicate capture file: {}",
            capture.file
        );
        assert!(
            ids.insert((capture.kind, &capture.id_base58)),
            "duplicate captured identity: {}",
            capture.id_base58,
        );
        assert!(
            !capture.source_label.is_empty(),
            "capture must have provenance"
        );
        match capture.validation.as_str() {
            "captured-peer-fact" => assert!(
                capture.event_num.is_some(),
                "capture must identify its source event"
            ),
            "accepted-chain-page-peek" => assert_eq!(capture.kind, Kind::Page),
            "raw-transaction-peek" => assert_eq!(capture.kind, Kind::Transaction),
            _ => panic!("unknown corpus provenance class: {}", capture.validation),
        }
        assert!(capture.consensus_version <= 1, "{}", capture.file);
    }
    (dir, manifest)
}

fn load(dir: &Path, capture: &Capture) -> (Vec<u8>, NounSlab, Noun) {
    let bytes = fs::read(dir.join(&capture.file)).expect("captured JAM fixture is readable");
    assert!(!bytes.is_empty(), "empty capture: {}", capture.file);
    assert_eq!(
        bytes.len(),
        capture.size_bytes,
        "{}: captured size",
        capture.file
    );
    assert_eq!(
        hex::encode(Sha256::digest(&bytes)),
        capture.sha256,
        "{}: captured checksum",
        capture.file,
    );
    let mut slab: NounSlab = NounSlab::new();
    let root = slab
        .cue_into(Bytes::copy_from_slice(&bytes))
        .unwrap_or_else(|error| {
            panic!("{}: original capture cannot decode: {error}", capture.file)
        });
    slab.set_root(root);
    assert_eq!(
        slab.jam().as_ref(),
        bytes.as_slice(),
        "{}: capture itself must contain a complete canonical JAM noun",
        capture.file,
    );
    (bytes, slab, root)
}

// Independent tuple/tree readers: do not use the v3 conversion helpers when
// establishing the source metadata or the expected consensus representation.
fn tuple(mut noun: NounHandle<'_>, width: usize) -> Vec<NounHandle<'_>> {
    let mut fields = Vec::with_capacity(width);
    for _ in 1..width {
        let cell = noun.as_cell().expect("captured consensus tuple cell");
        fields.push(cell.head());
        noun = cell.tail();
    }
    fields.push(noun);
    fields
}

fn scalar(noun: NounHandle<'_>) -> u64 {
    noun.as_atom()
        .expect("captured scalar atom")
        .as_u64()
        .expect("captured scalar fits u64")
}

fn entries(noun: NounHandle<'_>) -> Vec<NounHandle<'_>> {
    let mut pending = vec![noun];
    let mut entries = Vec::new();
    while let Some(node) = pending.pop() {
        if node.is_atom() {
            assert_eq!(scalar(node), 0, "captured treap terminator");
            continue;
        }
        let fields = tuple(node, 3);
        entries.push(fields[0]);
        pending.push(fields[2]);
        pending.push(fields[1]);
    }
    entries
}

fn versioned_body(noun: NounHandle<'_>) -> (u32, NounHandle<'_>) {
    let first = noun.as_cell().expect("captured versioned noun");
    if first.head().is_atom() {
        assert_eq!(scalar(first.head()), 1, "captured tagged consensus version");
        (1, first.tail())
    } else {
        (0, noun)
    }
}

fn hash(noun: NounHandle<'_>) -> String {
    Hash::from_noun_handle(&noun)
        .expect("independent consensus hash decode")
        .to_base58()
}

fn page_fields(noun: NounHandle<'_>) -> PageFields {
    let (version, body) = versioned_body(noun);
    let fields = tuple(body, 11);
    let pow_version = if fields[1].is_atom() {
        assert_eq!(scalar(fields[1]), 0, "absent captured proof");
        None
    } else {
        let unit = tuple(fields[1], 2);
        assert_eq!(scalar(unit[0]), 0, "captured proof unit tag");
        let tag = unit[1]
            .as_cell()
            .expect("captured proof/artifact tuple")
            .head();
        Some(if tag.eq_bytes(b"ai-pow") {
            4
        } else {
            let version = scalar(tag);
            assert!(
                matches!(version, 0 | 1 | 2 | 3 | 5),
                "captured proof version"
            );
            version
        })
    };
    let raw_tx_ids = entries(fields[3]);
    let transaction_ids = raw_tx_ids
        .iter()
        .copied()
        .map(hash)
        .collect::<BTreeSet<_>>();
    assert_eq!(
        raw_tx_ids.len(),
        transaction_ids.len(),
        "captured page set is unique"
    );
    PageFields {
        version,
        id: hash(fields[0]),
        parent: hash(fields[2]),
        height: scalar(fields[9]),
        pow_version,
        transaction_ids,
        coinbase_count: entries(fields[4]).len(),
    }
}

fn transaction_fields(noun: NounHandle<'_>) -> TransactionFields {
    let (version, body) = versioned_body(noun);
    let fields = tuple(body, if version == 0 { 4 } else { 2 });
    let raw_entries = entries(fields[1]);
    let mut spend_versions = BTreeSet::new();
    let mut lock_merkle_proof_kinds = BTreeSet::new();
    if version == 1 {
        for entry in &raw_entries {
            let pair = tuple(*entry, 2);
            let spend = tuple(pair[1], 2);
            let tag = u32::try_from(scalar(spend[0])).expect("spend version fits u32");
            assert!(tag <= 1, "captured spend version");
            spend_versions.insert(tag);
            if tag == 1 {
                let spend_body = tuple(spend[1], 3);
                let witness = tuple(spend_body[0], 4);
                let head = witness[0]
                    .as_cell()
                    .expect("captured lock-merkle-proof")
                    .head();
                lock_merkle_proof_kinds.insert(
                    if head.eq_bytes(b"full") {
                        "full"
                    } else {
                        "stub"
                    }
                    .to_owned(),
                );
            }
        }
    }
    let id = hash(fields[0]);
    // Existing consensus types provide a second reader independent of the v3
    // schema, including the legacy input/note shape and v1 spend variants.
    if version == 0 {
        let raw = v0::RawTx::from_noun_handle(&noun).expect("independent legacy raw-tx decode");
        assert_eq!(raw.id.to_base58(), id);
        assert_eq!(raw.inputs.0.len(), raw_entries.len());
    } else {
        let raw = v1::RawTx::from_noun_handle(&noun).expect("independent v1 raw-tx decode");
        assert_eq!(raw.id.to_base58(), id);
        assert_eq!(
            raw.compute_id_base58()
                .expect("independent v1 transaction hash"),
            id,
            "captured transaction content commits to its declared identity"
        );
        assert_eq!(raw.spends.0.len(), raw_entries.len());
        let typed_versions = raw
            .spends
            .0
            .iter()
            .map(|(_, spend)| match spend {
                v1::Spend::Legacy(_) => 0,
                v1::Spend::Witness(_) => 1,
            })
            .collect::<BTreeSet<_>>();
        assert_eq!(typed_versions, spend_versions);
    }
    TransactionFields {
        version,
        id,
        entry_count: raw_entries.len(),
        spend_versions,
        lock_merkle_proof_kinds,
    }
}

fn check_page_manifest(capture: &Capture, fields: &PageFields) {
    assert_eq!(
        fields.version, capture.consensus_version,
        "{}",
        capture.file
    );
    assert_eq!(fields.id, capture.id_base58, "{}", capture.file);
    assert_eq!(Some(fields.height), capture.height, "{}", capture.file);
    assert_eq!(fields.pow_version, capture.pow_version, "{}", capture.file);
    assert_eq!(
        Some(fields.transaction_ids.len()),
        capture.transaction_count,
        "{}",
        capture.file
    );
    let expected_ids = capture
        .tx_ids_base58
        .as_ref()
        .expect("captured page transaction identities")
        .iter()
        .cloned()
        .collect::<BTreeSet<_>>();
    assert_eq!(fields.transaction_ids, expected_ids, "{}", capture.file);
}

fn check_transaction_manifest(capture: &Capture, fields: &TransactionFields) {
    assert_eq!(
        fields.version, capture.consensus_version,
        "{}",
        capture.file
    );
    assert_eq!(fields.id, capture.id_base58, "{}", capture.file);
    assert_eq!(
        Some(fields.entry_count),
        capture.inputs_or_spends_count,
        "{}",
        capture.file,
    );
    let expected = capture
        .spend_versions
        .as_ref()
        .expect("captured transaction spend versions")
        .iter()
        .copied()
        .collect::<BTreeSet<_>>();
    assert_eq!(fields.spend_versions, expected, "{}", capture.file);
}

fn original_fact(bytes: &[u8], kind: Kind) -> ByteBuf {
    let mut slab: NounSlab = NounSlab::new();
    let original = slab
        .cue_into(Bytes::copy_from_slice(bytes))
        .expect("captured payload");
    let name = match kind {
        Kind::Page => "heard-block",
        Kind::Transaction => "heard-tx",
    };
    let tag = make_tas(&mut slab, name).as_noun();
    let fact = T(&mut slab, &[tag, original]);
    slab.set_root(fact);
    ByteBuf::from(slab.jam().as_ref())
}

fn assert_fact_identity(capture: &Capture, message: &ByteBuf) {
    let identified =
        NockchainFact::from_message_bytes(message).expect("existing driver fact decoder");
    match (capture.kind, identified) {
        (Kind::Page, NockchainFact::HeardBlock(id, _))
        | (Kind::Transaction, NockchainFact::HeardTx(id, _)) => {
            assert_eq!(id, capture.id_base58, "{}", capture.file)
        }
        _ => panic!("captured fact changed kind"),
    }
}

fn response_with(envelope: ResponseEnvelope) -> NockchainResponse {
    NockchainResponse::BatchResult {
        results: vec![BatchResultItem {
            item_id: 23,
            status: BatchResultStatus::Result,
            error: None,
            envelope: Some(envelope),
        }],
    }
}

async fn framed_response_roundtrip(response: &NockchainResponse) -> NockchainResponse {
    let encoded = encode_response(response).expect("encode reconstructed response");
    let maximum = u64::try_from(encoded.len()).unwrap() + 4;
    let mut codec = ProtobufCodec::new(maximum, maximum);
    let protocol = StreamProtocol::new(LibP2PConfig::req_res_protocol_version());
    let mut stream = Cursor::new(Vec::new());
    codec
        .write_response(&protocol, &mut stream, response.clone())
        .await
        .expect("write reconstructed response frame");
    let frame = stream.get_ref();
    assert_eq!(frame.len(), encoded.len() + 4);
    assert_eq!(
        u32::from_be_bytes(frame[..4].try_into().unwrap()) as usize,
        encoded.len()
    );
    assert_eq!(&frame[4..], encoded);
    stream.set_position(0);
    codec
        .read_response(&protocol, &mut stream)
        .await
        .expect("read reconstructed response frame")
}

async fn assert_response_roundtrip(capture: &Capture, original: &[u8]) {
    let message = original_fact(original, capture.kind);
    let envelope = match capture.kind {
        Kind::Page => ResponseEnvelope::heard_block(capture.id_base58.clone(), &message),
        Kind::Transaction => ResponseEnvelope::heard_tx(capture.id_base58.clone(), &message),
    };
    let response = response_with(envelope);
    let decoded = framed_response_roundtrip(&response).await;
    assert_eq!(
        decoded, response,
        "{}: exact original response message",
        capture.file
    );
    let NockchainResponse::BatchResult { results } = decoded else {
        panic!("batch result variant changed")
    };
    assert_fact_identity(capture, &results[0].envelope.as_ref().unwrap().message);
}

#[test]
fn captured_history_has_the_declared_version_coverage() {
    let (dir, manifest) = corpus();
    let mut pages = BTreeSet::new();
    let mut transactions = BTreeSet::new();
    let mut proofs = BTreeSet::new();
    let mut spends = BTreeSet::new();
    let mut lock_proofs = BTreeSet::new();
    let mut included_transaction_ids = BTreeSet::new();
    let mut captured_transaction_ids = BTreeSet::new();
    let mut captured_pages = BTreeMap::new();
    for capture in &manifest.captures {
        let (_, slab, root) = load(&dir, capture);
        let space = slab.noun_space();
        match capture.kind {
            Kind::Page => {
                let fields = page_fields(root.in_space(&space));
                check_page_manifest(capture, &fields);
                pages.insert(fields.version);
                proofs.extend(fields.pow_version);
                included_transaction_ids.extend(fields.transaction_ids.iter().cloned());
                captured_pages.insert(fields.id.clone(), fields);
            }
            Kind::Transaction => {
                let fields = transaction_fields(root.in_space(&space));
                check_transaction_manifest(capture, &fields);
                transactions.insert(fields.version);
                spends.extend(fields.spend_versions);
                lock_proofs.extend(fields.lock_merkle_proof_kinds);
                captured_transaction_ids.insert(fields.id);
            }
        }
    }
    assert!(
        captured_transaction_ids.is_subset(&included_transaction_ids),
        "every historical transaction fixture must appear in a captured page: missing {:?}",
        captured_transaction_ids
            .difference(&included_transaction_ids)
            .collect::<Vec<_>>(),
    );
    for checkpoint in &manifest.public_checkpoints {
        let page = captured_pages
            .get(&checkpoint.id_base58)
            .expect("public checkpoint page must be represented in the corpus");
        assert_eq!(page.height, checkpoint.height, "public checkpoint height");
    }
    for capture in manifest
        .captures
        .iter()
        .filter(|capture| capture.kind == Kind::Transaction)
    {
        assert!(
            !capture.included_in_pages.is_empty(),
            "{}: transaction inclusion provenance",
            capture.file
        );
        for inclusion in &capture.included_in_pages {
            let page = captured_pages
                .get(&inclusion.id_base58)
                .expect("transaction inclusion must name a captured page");
            assert_eq!(
                page.height, inclusion.height,
                "{}: inclusion height",
                capture.file
            );
            assert!(
                page.transaction_ids.contains(&capture.id_base58),
                "{}: inclusion page transaction set",
                capture.file
            );
        }
    }
    for version in manifest.required.page_versions {
        assert!(pages.contains(&version), "missing captured page v{version}");
    }
    for version in manifest.required.transaction_versions {
        assert!(
            transactions.contains(&version),
            "missing captured transaction v{version}"
        );
    }
    for version in manifest.required.pow_versions {
        assert!(
            proofs.contains(&version),
            "missing captured PoW version {version}"
        );
    }
    for version in manifest.required.spend_versions {
        assert!(
            spends.contains(&version),
            "missing captured spend version {version}"
        );
    }
    for kind in manifest.required.lock_merkle_proof_kinds {
        assert!(
            lock_proofs.contains(&kind),
            "missing captured lock-merkle-proof {kind}"
        );
    }
    let missing_proofs = (0..=5)
        .filter(|version| !proofs.contains(version))
        .collect::<Vec<_>>();
    println!("captured page versions={pages:?}, transaction versions={transactions:?}, PoW versions={proofs:?}, spend versions={spends:?}, lock proofs={lock_proofs:?}; unrepresented supported PoW tags={missing_proofs:?}");
}

#[tokio::test]
async fn captured_pages_preserve_original_jam_and_identities() {
    let (dir, manifest) = corpus();
    let mut tested = 0;
    for capture in manifest
        .captures
        .iter()
        .filter(|capture| capture.kind == Kind::Page)
    {
        let (original, slab, root) = load(&dir, capture);
        let space = slab.noun_space();
        let expected = page_fields(root.in_space(&space));
        check_page_manifest(capture, &expected);
        let checked = PeerPage::from_noun(root.in_space(&space))
            .unwrap_or_else(|error| panic!("{}: {error}", capture.file));
        let protobuf = checked.to_proto().encode_to_vec();
        let checked = PeerPage::from_proto(
            pb::Page::decode(protobuf.as_slice()).expect("page protobuf decode"),
        )
        .expect("checked page protobuf");
        let mut destination: NounSlab = NounSlab::new();
        let rebuilt = checked.to_noun(&mut destination).expect("page noun bridge");
        destination.set_root(rebuilt);
        assert_eq!(
            destination.jam().as_ref(),
            original.as_slice(),
            "{}: original captured bytes",
            capture.file
        );
        assert_eq!(
            page_fields(rebuilt.in_space(&destination.noun_space())),
            expected,
            "{}: independent consensus fields",
            capture.file
        );
        assert_response_roundtrip(capture, &original).await;
        tested += 1;
    }
    assert!(tested > 0, "no real captured pages tested");
}

#[tokio::test]
async fn captured_transactions_preserve_original_jam_and_identities() {
    let (dir, manifest) = corpus();
    let mut tested = 0;
    for capture in manifest
        .captures
        .iter()
        .filter(|capture| capture.kind == Kind::Transaction)
    {
        let (original, slab, root) = load(&dir, capture);
        let space = slab.noun_space();
        let expected = transaction_fields(root.in_space(&space));
        check_transaction_manifest(capture, &expected);
        let checked = Transaction::from_noun(root.in_space(&space))
            .unwrap_or_else(|error| panic!("{}: {error}", capture.file));
        let protobuf = checked.to_proto().encode_to_vec();
        let checked = Transaction::from_proto(
            pb::Transaction::decode(protobuf.as_slice()).expect("transaction protobuf decode"),
        )
        .expect("checked transaction protobuf");
        let mut destination: NounSlab = NounSlab::new();
        let rebuilt = checked
            .to_noun(&mut destination)
            .expect("transaction noun bridge");
        destination.set_root(rebuilt);
        assert_eq!(
            destination.jam().as_ref(),
            original.as_slice(),
            "{}: original captured bytes",
            capture.file
        );
        assert_eq!(
            transaction_fields(rebuilt.in_space(&destination.noun_space())),
            expected,
            "{}: independent consensus fields",
            capture.file
        );
        assert_response_roundtrip(capture, &original).await;
        tested += 1;
    }
    assert!(tested > 0, "no real captured transactions tested");
}

#[test]
fn reconstructed_gossip_authenticates_each_captured_consensus_version() {
    let (dir, manifest) = corpus();
    let sender = libp2p::identity::Keypair::ed25519_from_bytes([7_u8; 32])
        .expect("test sender key")
        .public()
        .to_peer_id();
    let receiver = libp2p::identity::Keypair::ed25519_from_bytes([9_u8; 32])
        .expect("test receiver key")
        .public()
        .to_peer_id();
    let mut builder = equix::EquiXBuilder::new();
    let mut represented = BTreeSet::new();
    // Four representatives keep proof solving bounded. Identities, nonce and
    // proof belong to this reconstructed v3 envelope, not the historical source.
    for capture in &manifest.captures {
        if !represented.insert((capture.kind, capture.consensus_version)) {
            continue;
        }
        let (original, _, _) = load(&dir, capture);
        let expected_message = original_fact(&original, capture.kind);
        let request = NockchainRequest::authenticated_gossip_from_message(
            &mut builder,
            &sender,
            &receiver,
            expected_message.clone(),
        )
        .expect("solve reconstructed gossip proof");
        let decoded = decode_request(&encode_request(&request).expect("gossip encode"))
            .expect("gossip decode");
        assert_eq!(decoded, request, "{}", capture.file);
        decoded
            .verify_pow(&mut builder, &receiver, &sender)
            .expect("received gossip proof verifies with the original peer binding");
        let NockchainRequest::AuthenticatedGossip { message, .. } = decoded else {
            panic!("gossip variant changed")
        };
        assert_eq!(message, expected_message, "{}", capture.file);
        assert_fact_identity(capture, &message);
    }
    for version in manifest.required.page_versions {
        assert!(represented.contains(&(Kind::Page, version)));
    }
    for version in manifest.required.transaction_versions {
        assert!(represented.contains(&(Kind::Transaction, version)));
    }
}

#[tokio::test]
async fn reconstructed_complete_block_bundles_preserve_captured_payloads() {
    let (dir, manifest) = corpus();
    let transactions = manifest
        .captures
        .iter()
        .filter(|capture| capture.kind == Kind::Transaction)
        .map(|capture| {
            let (original, slab, root) = load(&dir, capture);
            let fields = transaction_fields(root.in_space(&slab.noun_space()));
            check_transaction_manifest(capture, &fields);
            (fields.id, original_fact(&original, Kind::Transaction))
        })
        .collect::<BTreeMap<_, _>>();
    let mut complete_bundles = 0;
    let mut nonempty_versions = BTreeSet::new();
    for capture in manifest
        .captures
        .iter()
        .filter(|capture| capture.kind == Kind::Page)
    {
        let (original, slab, root) = load(&dir, capture);
        let fields = page_fields(root.in_space(&slab.noun_space()));
        check_page_manifest(capture, &fields);
        if !fields
            .transaction_ids
            .iter()
            .all(|id| transactions.contains_key(id))
        {
            continue;
        }
        let tx_envelopes = fields
            .transaction_ids
            .iter()
            .map(|id| BundledTxEnvelope {
                tx_id: id.clone(),
                message: transactions[id].clone(),
            })
            .collect();
        let response = response_with(ResponseEnvelope::heard_block_with_txs(
            fields.id,
            original_fact(&original, Kind::Page),
            tx_envelopes,
            Vec::new(),
        ));
        let decoded = framed_response_roundtrip(&response).await;
        assert_eq!(
            decoded, response,
            "{}: all original page/transaction message bytes and identities",
            capture.file,
        );
        complete_bundles += 1;
        if !fields.transaction_ids.is_empty() {
            nonempty_versions.insert(fields.version);
        }
    }
    assert!(complete_bundles > 0, "no complete captured block bundle");
    for version in manifest.required.nonempty_bundle_page_versions {
        assert!(
            nonempty_versions.contains(&version),
            "missing nonempty complete captured bundle for page v{version}",
        );
    }
    println!("reconstructed complete bundles={complete_bundles}, nonempty page versions={nonempty_versions:?}");
}

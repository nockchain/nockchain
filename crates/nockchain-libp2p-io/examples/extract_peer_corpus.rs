//! Extract bounded, standalone consensus nouns for a peer conformance corpus.
//!
//! Inputs are local JSONL exports, never live databases or node state. A caller
//! must supply a sanitized logical `source_label`, not a host, path or peer ID.
//! Ordinary event jobs contain entropy, timestamps and peer routing metadata;
//! only the page/transaction noun is retained. Captured facts are observations,
//! not evidence of consensus acceptance. A direct page peek must include its
//! requested height; raw transaction peeks still need an inclusion cross-check.
//!
//! This tool deliberately does not use the v3 conversion under test. It reads
//! only enough of the original consensus noun to describe its coverage, then
//! jams that exact noun. It does not validate proofs, signatures or commitments.

use std::collections::{BTreeSet, HashSet};
use std::fs::{self, File, OpenOptions};
use std::io::{self, BufRead, BufReader, BufWriter, Write};
use std::path::Path;

use bytes::Bytes;
use nockapp::noun::slab::NounSlab;
use nockchain_libp2p_io::tip5_util::tip5_hash_to_base58;
use nockvm::noun::{NounAllocator, NounHandle};
use serde::{Deserialize, Serialize};

const MAX_JAM_BYTES: usize = 16 * 1024 * 1024;
const MAX_LINE_BYTES: usize = MAX_JAM_BYTES * 2 + 4096;
const MAX_NODES: usize = 1_000_000;
const MAX_RECORDS: usize = 20_000;
const MAX_OUTPUT_BYTES: usize = 512 * 1024 * 1024;
const PRIME: u64 = 0xffff_ffff_0000_0001;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Input {
    source_label: String,
    event_num: Option<u64>,
    job_jam_hex: Option<String>,
    payload_kind: Option<String>,
    payload_jam_hex: Option<String>,
    peek_result_jam_hex: Option<String>,
    requested_height: Option<u64>,
    requested_id: Option<String>,
}

#[derive(Serialize, Debug)]
struct Capture {
    kind: &'static str,
    consensus_version: u64,
    id_base58: String,
    height: Option<u64>,
    pow_version: Option<u64>,
    transaction_count: Option<usize>,
    tx_ids_base58: Option<Vec<String>>,
    spend_versions: Vec<u64>,
    inputs_or_spends_count: Option<usize>,
    event_num: Option<u64>,
    source_label: String,
    validation: &'static str,
    file: String,
}

fn invalid(message: &'static str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}

fn tuple(mut noun: NounHandle<'_>, len: usize) -> io::Result<Vec<NounHandle<'_>>> {
    let mut fields = Vec::with_capacity(len);
    for _ in 1..len {
        let cell = noun.as_cell().map_err(|_| invalid("expected tuple cell"))?;
        fields.push(cell.head());
        noun = cell.tail();
    }
    fields.push(noun);
    Ok(fields)
}

fn scalar(noun: NounHandle<'_>) -> io::Result<u64> {
    noun.as_atom()
        .map_err(|_| invalid("expected scalar atom"))?
        .as_u64()
        .map_err(|_| invalid("scalar exceeds u64"))
}

fn zero(noun: NounHandle<'_>) -> io::Result<()> {
    if scalar(noun)? != 0 {
        return Err(invalid("expected null"));
    }
    Ok(())
}

/// Walk the public consensus z-map/z-set representation without recursion.
fn tree_entries(root: NounHandle<'_>) -> io::Result<Vec<NounHandle<'_>>> {
    let mut pending = vec![root];
    let mut entries = Vec::new();
    let mut visits = 0;
    while let Some(noun) = pending.pop() {
        visits += 1;
        if visits > MAX_NODES {
            return Err(invalid("tree traversal exceeds node limit"));
        }
        if noun.is_atom() {
            zero(noun)?;
            continue;
        }
        let fields = tuple(noun, 3)?;
        entries.push(fields[0]);
        pending.push(fields[2]);
        pending.push(fields[1]);
    }
    Ok(entries)
}

fn hash(noun: NounHandle<'_>, slab: &NounSlab) -> io::Result<String> {
    for limb in tuple(noun, 5)? {
        if scalar(limb)? >= PRIME {
            return Err(invalid("hash limb outside base field"));
        }
    }
    tip5_hash_to_base58(noun.noun(), &slab.noun_space())
        .map_err(|_| invalid("cannot encode consensus id"))
}

fn version_and_body(noun: NounHandle<'_>) -> io::Result<(u64, NounHandle<'_>)> {
    let fields = tuple(noun, 2)?;
    if fields[0].is_atom() {
        if scalar(fields[0])? != 1 {
            return Err(invalid("unsupported consensus version"));
        }
        Ok((1, fields[1]))
    } else {
        Ok((0, noun))
    }
}

fn proof_version(pow: NounHandle<'_>) -> io::Result<Option<u64>> {
    if pow.is_atom() {
        zero(pow)?;
        return Ok(None);
    }
    let unit = tuple(pow, 2)?;
    zero(unit[0])?;
    let artifact = tuple(unit[1], 2)?;
    if artifact[0].eq_bytes(b"ai-pow") {
        return Ok(Some(4));
    }
    // This is the advertised discriminator, not proof verification. Versions
    // 0, 1, 2, 3 and 5 use [version objects hashes read-index].
    let proof = tuple(unit[1], 4)?;
    let version = scalar(proof[0])?;
    if !matches!(version, 0 | 1 | 2 | 3 | 5) {
        return Err(invalid("unknown proof artifact discriminator"));
    }
    Ok(Some(version))
}

fn inspect_payload(
    kind: &'static str,
    noun: NounHandle<'_>,
    slab: &NounSlab,
    input: &Input,
    validation: &'static str,
) -> io::Result<Capture> {
    let (consensus_version, body) = version_and_body(noun)?;
    let mut capture = Capture {
        kind,
        consensus_version,
        id_base58: String::new(),
        height: None,
        pow_version: None,
        transaction_count: None,
        tx_ids_base58: None,
        spend_versions: Vec::new(),
        inputs_or_spends_count: None,
        event_num: input.event_num,
        source_label: input.source_label.clone(),
        validation,
        file: String::new(),
    };
    if kind == "page" {
        let fields = tuple(body, 11)?;
        capture.id_base58 = hash(fields[0], slab)?;
        capture.height = Some(scalar(fields[9])?);
        if input.requested_height.is_some() && input.requested_height != capture.height {
            return Err(invalid("peek page height differs from requested height"));
        }
        capture.pow_version = proof_version(fields[1])?;
        let ids = tree_entries(fields[3])?
            .into_iter()
            .map(|id| hash(id, slab))
            .collect::<io::Result<Vec<_>>>()?;
        if ids.iter().collect::<HashSet<_>>().len() != ids.len() {
            return Err(invalid("duplicate transaction id in page"));
        }
        capture.transaction_count = Some(ids.len());
        capture.tx_ids_base58 = Some(ids);
    } else {
        let fields = tuple(body, if consensus_version == 0 { 4 } else { 2 })?;
        capture.id_base58 = hash(fields[0], slab)?;
        let entries = tree_entries(fields[1])?;
        capture.inputs_or_spends_count = Some(entries.len());
        if consensus_version == 1 {
            let mut versions = BTreeSet::new();
            for entry in entries {
                let spend = tuple(entry, 2)?[1];
                let version = scalar(tuple(spend, 2)?[0])?;
                if version > 1 {
                    return Err(invalid("unknown spend version"));
                }
                versions.insert(version);
            }
            capture.spend_versions = versions.into_iter().collect();
        }
    }
    capture.file = format!(
        "{}-v{}-{}.jam",
        capture.kind, capture.consensus_version, capture.id_base58
    );
    if input
        .requested_id
        .as_ref()
        .is_some_and(|id| id != &capture.id_base58)
    {
        return Err(invalid("peek transaction id differs from requested id"));
    }
    Ok(capture)
}

fn extract(input: Input) -> io::Result<Option<(Capture, Bytes)>> {
    if input.source_label.is_empty()
        || input.source_label.len() > 80
        || !input
            .source_label
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
    {
        return Err(invalid("source_label must be a sanitized logical label"));
    }
    let peek_result = input.peek_result_jam_hex.is_some();
    let direct = input.payload_jam_hex.is_some() || peek_result;
    let hex = match (
        &input.job_jam_hex, &input.payload_jam_hex, &input.peek_result_jam_hex,
    ) {
        (Some(hex), None, None)
            if input.event_num.is_some()
                && input.payload_kind.is_none()
                && input.requested_height.is_none()
                && input.requested_id.is_none() =>
        {
            hex
        }
        (None, Some(hex), None) | (None, None, Some(hex)) if input.event_num.is_none() => hex,
        _ => {
            return Err(invalid(
                "expected either an event job or a direct peek payload",
            ))
        }
    };
    if hex.len() > MAX_JAM_BYTES * 2 {
        return Err(invalid("input jam exceeds byte limit"));
    }
    let jam = hex::decode(hex).map_err(|_| invalid("invalid jam hex"))?;
    let mut slab: NounSlab = NounSlab::new();
    let root = slab
        .cue_into_with_max_nodes(Bytes::from(jam), MAX_NODES)
        .map_err(|_| invalid("input jam cannot be decoded within limits"))?;
    let space = slab.noun_space();
    let mut noun = root.in_space(&space);
    if peek_result {
        // PrivateNockApp.Peek uses NockAppHandle.peek, returning the raw
        // (unit (unit payload)) result. NockApp.peek_handle is a different API
        // that strips these wrappers. The input field makes this distinction
        // explicit instead of guessing from a payload's first atom.
        let outer = tuple(noun, 2)?;
        zero(outer[0])?;
        if outer[1].is_atom() {
            zero(outer[1])?;
            return Ok(None);
        }
        let inner = tuple(outer[1], 2)?;
        zero(inner[0])?;
        noun = inner[1];
    }
    let (kind, payload, validation) = if direct {
        match input.payload_kind.as_deref() {
            Some("page") if input.requested_height.is_some() && input.requested_id.is_none() => {
                ("page", noun, "accepted-chain-page-peek")
            }
            Some("transaction") if input.requested_height.is_none() => {
                ("transaction", noun, "raw-transaction-peek")
            }
            _ => return Err(invalid("direct page peek needs a requested height")),
        }
    } else {
        let job = tuple(noun, 6)?;
        if Some(scalar(job[0])?) != input.event_num {
            return Err(invalid("job event number differs from export metadata"));
        }
        let wire = tuple(job[1], 2)?;
        // An accepted crud replacement retains the original SQL wire metadata,
        // but has a different tuple shape. Never treat it as an ordinary poke.
        if !wire[0].eq_bytes(b"poke") {
            return Ok(None);
        }
        let driver_wire = tuple(wire[1], 3)?;
        if !driver_wire[0].eq_bytes(b"libp2p") {
            return Ok(None);
        }
        if scalar(driver_wire[1])? != 1 {
            return Err(invalid("unsupported peer driver wire version"));
        }
        for field in &job[2..5] {
            field
                .as_atom()
                .map_err(|_| invalid("invalid poke header"))?;
        }
        let cause = tuple(job[5], 2)?;
        if !cause[0].eq_bytes(b"fact") {
            return Ok(None);
        }
        let fact = tuple(cause[1], 2)?;
        zero(fact[0])?;
        let heard = tuple(fact[1], 2)?;
        let kind = if heard[0].eq_bytes(b"heard-block") {
            "page"
        } else if heard[0].eq_bytes(b"heard-tx") {
            "transaction"
        } else {
            return Ok(None);
        };
        (kind, heard[1], "captured-peer-fact")
    };
    let capture = inspect_payload(kind, payload, &slab, &input, validation)?;
    // copy_into uses the original noun, without converting through peer types.
    let mut output: NounSlab = NounSlab::new();
    let root = output.copy_into(payload.noun(), &space);
    output.set_root(root);
    let jam = output.jam();
    if jam.len() > MAX_JAM_BYTES {
        return Err(invalid("output jam exceeds byte limit"));
    }
    Ok(Some((capture, jam)))
}

fn read_line_bounded(reader: &mut impl BufRead, line: &mut Vec<u8>) -> io::Result<bool> {
    line.clear();
    loop {
        let available = reader.fill_buf()?;
        if available.is_empty() {
            return Ok(!line.is_empty());
        }
        let len = available
            .iter()
            .position(|byte| *byte == b'\n')
            .map_or(available.len(), |index| index + 1);
        if line.len() + len > MAX_LINE_BYTES {
            return Err(invalid("JSONL record exceeds byte limit"));
        }
        let complete = available[len - 1] == b'\n';
        line.extend_from_slice(&available[..len]);
        reader.consume(len);
        if complete {
            return Ok(true);
        }
    }
}

fn run(input_path: &Path, output_dir: &Path) -> io::Result<()> {
    let mut input = BufReader::new(File::open(input_path)?);
    fs::create_dir_all(output_dir)?;
    let mut manifest = BufWriter::new(
        OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(output_dir.join("captures.jsonl"))?,
    );
    let mut line = Vec::new();
    let (mut records, mut captured, mut skipped, mut rejected, mut duplicates) = (0, 0, 0, 0, 0);
    let mut output_bytes = 0;
    while read_line_bounded(&mut input, &mut line)? {
        records += 1;
        if records > MAX_RECORDS {
            return Err(invalid("input exceeds record limit"));
        }
        let result = serde_json::from_slice::<Input>(&line)
            .map_err(|_| invalid("invalid input JSON schema"))
            .and_then(extract);
        let (capture, jam) = match result {
            Ok(Some(capture)) => capture,
            Ok(None) => {
                skipped += 1;
                continue;
            }
            Err(error) => {
                rejected += 1;
                if rejected <= 10 {
                    // Errors contain fixed descriptions, never source noun data.
                    eprintln!("record {records}: {error}");
                }
                continue;
            }
        };
        let destination = output_dir.join(&capture.file);
        match OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&destination)
        {
            Ok(mut file) => {
                output_bytes += jam.len();
                if output_bytes > MAX_OUTPUT_BYTES {
                    return Err(invalid("corpus exceeds output byte limit"));
                }
                file.write_all(&jam)?;
            }
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
                if fs::metadata(&destination)?.len() > MAX_JAM_BYTES as u64
                    || fs::read(destination)?.as_slice() != jam.as_ref()
                {
                    return Err(invalid("same consensus id has different captured payloads"));
                }
                duplicates += 1;
                continue;
            }
            Err(error) => return Err(error),
        }
        serde_json::to_writer(&mut manifest, &capture)?;
        manifest.write_all(b"\n")?;
        captured += 1;
    }
    manifest.flush()?;
    eprintln!(
        "records={records} captured={captured} duplicates={duplicates} skipped={skipped} rejected={rejected} output_bytes={output_bytes}"
    );
    if rejected != 0 {
        return Err(invalid(
            "some records were rejected; captured records remain available",
        ));
    }
    Ok(())
}

fn main() -> io::Result<()> {
    let args = std::env::args_os().collect::<Vec<_>>();
    if args.len() == 2 && (args[1] == "--help" || args[1] == "-h") {
        println!(
            "Usage: extract_peer_corpus INPUT.jsonl OUTPUT_DIR\n\
             OUTPUT_DIR/captures.jsonl must not already exist.\n\
             Input forms (one JSON object per line):\n\
             {{\"source_label\":\"snapshot-a\",\"event_num\":1,\"job_jam_hex\":\"...\"}}\n\
             {{\"source_label\":\"chain-a\",\"payload_kind\":\"page\",\"requested_height\":1,\"payload_jam_hex\":\"...\"}}\n\
             {{\"source_label\":\"chain-a\",\"payload_kind\":\"transaction\",\"payload_jam_hex\":\"...\"}}\n\
             Use peek_result_jam_hex instead of payload_jam_hex for a raw PrivateNockApp.Peek double-unit result.\n\
             Review captured payloads and provenance before publication. Never commit raw job exports.\n\
             Limits: 16 MiB JAM, 1,000,000 decoded nodes, 20,000 records, 512 MiB output."
        );
        return Ok(());
    }
    if args.len() != 3 {
        return Err(invalid("usage: extract_peer_corpus INPUT.jsonl OUTPUT_DIR"));
    }
    run(Path::new(&args[1]), Path::new(&args[2]))
}

#[cfg(test)]
mod tests {
    use nockvm::noun::{D, T};

    use super::*;

    fn fixture_input() -> Input {
        Input {
            source_label: "public-v1-fixture".into(),
            event_num: None,
            job_jam_hex: None,
            payload_kind: Some("transaction".into()),
            payload_jam_hex: Some(hex::encode(include_bytes!(
                "../../nockchain-types/jams/v1/raw-tx.jam"
            ))),
            peek_result_jam_hex: None,
            requested_height: None,
            requested_id: None,
        }
    }

    #[test]
    fn extracts_original_public_transaction_and_spend_metadata() {
        let (capture, jam) = extract(fixture_input()).unwrap().unwrap();
        assert_eq!(capture.consensus_version, 1);
        assert_eq!(capture.spend_versions, [1]);
        assert_eq!(capture.inputs_or_spends_count, Some(1));
        assert_eq!(capture.validation, "raw-transaction-peek");
        assert!(capture.event_num.is_none());
        let (again, second) = extract(Input {
            payload_jam_hex: Some(hex::encode(jam)),
            ..fixture_input()
        })
        .unwrap()
        .unwrap();
        assert_eq!(again.id_base58, capture.id_base58);
        assert!(!second.is_empty());
    }

    #[test]
    fn bounds_input_and_rejects_ambiguous_provenance() {
        let mut input = fixture_input();
        input.source_label = "/private/path".into();
        assert!(extract(input).is_err());
        let mut input = fixture_input();
        input.event_num = Some(1);
        assert!(extract(input).is_err());
        let mut input = fixture_input();
        input.payload_jam_hex = Some("z0".into());
        assert!(extract(input).is_err());
    }

    #[test]
    fn raw_grpc_peek_results_require_explicit_double_unit_mode() {
        let (_, expected) = extract(fixture_input()).unwrap().unwrap();
        let mut slab: NounSlab = NounSlab::new();
        let tx = slab
            .cue_into_with_max_nodes(expected.clone(), MAX_NODES)
            .unwrap();
        let root = T(&mut slab, &[D(0), D(0), tx]);
        slab.set_root(root);
        let wrapped = hex::encode(slab.jam());
        let mut input = fixture_input();
        input.payload_jam_hex = Some(wrapped.clone());
        assert!(extract(input).is_err());
        let mut input = fixture_input();
        input.payload_jam_hex = None;
        input.peek_result_jam_hex = Some(wrapped);
        let (_, actual) = extract(input).unwrap().unwrap();
        assert_eq!(actual, expected);
    }

    #[test]
    fn extracts_only_the_payload_from_an_ordinary_peer_job() {
        let mut input = fixture_input();
        let (_, expected) = extract(fixture_input()).unwrap().unwrap();
        let mut slab: NounSlab = NounSlab::new();
        let tx = slab
            .cue_into_with_max_nodes(expected.clone(), MAX_NODES)
            .unwrap();
        let heard = T(&mut slab, &[D(nockvm_macros::tas!(b"heard-tx")), tx]);
        let fact = T(&mut slab, &[D(nockvm_macros::tas!(b"fact")), D(0), heard]);
        let driver_wire = T(&mut slab, &[D(nockvm_macros::tas!(b"libp2p")), D(1), D(0)]);
        let wire = T(&mut slab, &[D(nockvm_macros::tas!(b"poke")), driver_wire]);
        let root = T(&mut slab, &[D(9), wire, D(123), D(0), D(456), fact]);
        slab.set_root(root);
        input.event_num = Some(9);
        input.job_jam_hex = Some(hex::encode(slab.jam()));
        input.payload_kind = None;
        input.payload_jam_hex = None;
        let (capture, jam) = extract(input).unwrap().unwrap();
        assert_eq!(jam, expected);
        assert_eq!(capture.event_num, Some(9));
        assert_eq!(capture.validation, "captured-peer-fact");
    }

    #[test]
    fn accepted_crud_job_is_not_mistaken_for_peer_fact() {
        let mut slab: NounSlab = NounSlab::new();
        let wire = T(&mut slab, &[D(0), D(nockvm_macros::tas!(b"arvo")), D(0)]);
        let root = T(&mut slab, &[D(9), wire, D(0), D(0), D(0), D(0), D(0)]);
        slab.set_root(root);
        let input = Input {
            source_label: "test-events".into(),
            event_num: Some(9),
            job_jam_hex: Some(hex::encode(slab.jam())),
            payload_kind: None,
            payload_jam_hex: None,
            peek_result_jam_hex: None,
            requested_height: None,
            requested_id: None,
        };
        assert!(extract(input).unwrap().is_none());
    }
}

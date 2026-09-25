# Historical peer corpus

These are real consensus nouns captured on 2026-09-25: 31 pages and the 19 raw
transactions included in those pages. No consensus payload was synthesized.
The test harness constructs v3 transport envelopes around the captured data;
those envelopes are not recorded network traffic.

The reference definitions are the public repository at
`cbd9298f96584b14ab93074ecdf32d0ec212e50e`. The transport specification is
[Ecclesia](../../../../../changelog/protocol/018-ecclesia.md).

## Source and verification

Frozen node event logs from July and September provided candidate historical
heights and transaction IDs. Event-log facts alone do not establish consensus
acceptance. Every published page was subsequently fetched using the node's
read-only `%heavy-n` peek, which selects a page on its accepted heaviest chain.
Every raw transaction was fetched by `%raw-transaction` using an ID contained
in one of the captured pages.

The pages at heights 144, 4032, and 16128 match the checkpoint hashes in the
public `hoon/apps/dumbnet/lib/consensus.hoon`. The manifest pins those hashes,
each payload's SHA-256 and size, independently extracted IDs and versions, and
each transaction's inclusion pages. Tests verify all these fields before
performing v3 conversion.

The private gRPC `Peek` response contains `[0 [0 payload]]`. The extraction
tool verifies both unit tags, copies only `payload`, and serializes that noun
using the existing JAM implementation. It does not use the v3 conversion to
generate expected values. No raw event jobs, entropy, routing metadata, node
configuration, or host identifiers are included in this corpus.

The checked-in files are the serialization oracle: the v3 domain conversion,
protobuf encode/decode, and locally reconstructed noun must preserve their JAM
bytes exactly. Existing `nockchain-types` readers independently decode the
transactions, and their v1 content-ID implementation checks every captured v1
transaction. The suite does not rerun historical proof or signature validation.

## Reproduce

Use an operator-controlled node that retains the historical pages and raw
transactions. The capture tool supports a local private gRPC listener or SSH
to run `grpcurl` on that node. It only calls `Peek`.

From the repository root, capture the page heights listed in `manifest.json`:

```sh
python3 crates/nockchain-libp2p-io/examples/capture_peer_corpus.py \
  --ssh YOUR_NODE --grpcurl /path/to/grpcurl \
  --height 1 144 4032 6749 6750 11999 12000 16128 19996 20010 20011 \
    37349 37350 38999 39000 41568 41878 53999 54000 65499 65500 \
    119399 119400 125999 126000 147499 147500 148868 148869 148899 153864 \
  --output /tmp/peer-v3-pages.jsonl

cargo run -p nockchain-libp2p-io --example extract_peer_corpus -- \
  /tmp/peer-v3-pages.jsonl /tmp/peer-v3-pages
```

Use the extracted pages' `tx_ids_base58` values for a second capture:

```sh
python3 crates/nockchain-libp2p-io/examples/capture_peer_corpus.py \
  --ssh YOUR_NODE --grpcurl /path/to/grpcurl \
  --tx-id TRANSACTION_ID ... --output /tmp/peer-v3-transactions.jsonl

cargo run -p nockchain-libp2p-io --example extract_peer_corpus -- \
  /tmp/peer-v3-transactions.jsonl /tmp/peer-v3-transactions
```

Use a fresh output directory. The extractor refuses to overwrite a capture
manifest and rejects different payloads claiming the same ID within an export.
Missing history is reported as a skipped peek; it is not filled with generated
data. Raw event export is an alternative input mode documented by
`extract_peer_corpus --help`; such exports must remain outside the repository.

For fixture updates, first regenerate into scratch directories and compare
against the pinned checksums. For intentional additions, retain only consensus
payloads, add the extraction metadata and SHA-256/size to `manifest.json`, and
record inclusion pages for every transaction. Review the changed manifest and
payloads before staging them. JAM files are ignored by default, so stage only
the reviewed fixture paths explicitly with `git add -f`.

```sh
cargo test -p nockchain-libp2p-io --lib v3::
cargo test -p nockchain-libp2p-io --example extract_peer_corpus
```

See [COVERAGE.md](COVERAGE.md) for the scope of these checks and the remaining
validation work.

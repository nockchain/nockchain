# AI-PoW verifier setup lifecycle

The compact recursive verifier uses one proof-independent context for each reachable `(trace_height, sx_bound)` shape key. These contexts are large, but the set of keys and each verifier-key digest are consensus-known.

## Boot path

1. Enumerate every production shape key admitted by the Pearl parameter envelope.
2. Load or deterministically rebuild each context from the production circuit parameters.
3. Recompute and compare its verifier-key digest with the committed digest table.
4. Serialize validated contexts beneath the node's data directory with a local file checksum.
5. Install the complete bucket table into `ai-pow-jets` exactly once.

A digest mismatch aborts startup. A node must not continue with a locally derived setup that differs from the consensus-known table.

## Memory-bounded cache bootstrap

Generating all production proofs in one process can retain more allocator
working memory than a constrained validator host can provide. The setup-cache
utility isolates each bucket in its own child process, retains completed shards
across retries, verifies the assembled table against the committed consensus
digest, and writes the canonical seed cache:

```sh
cargo run --release -p ai-pow-jets --bin ai-pow-setup-cache -- \
  generate-all /path/to/nockchain-data
```

The resulting cache is
`/path/to/nockchain-data/ai-pow/verifier-setup-seeds-v1.bin`. Starting a node
with that data directory still validates the consensus digest and builds any
missing disk contexts before networking starts. Re-running `generate-all`
reuses every complete, checksummed shard and fails on malformed shard data.

## Verification path

The jet resolves the certificate's required trace-height bucket after setup-free
statement checks. A cache miss reads and deserializes the prebuilt file, verifies
both the file checksum and verifier-key digest, and inserts the context into the
resident table.

## Failure classes

- **Unknown shape key:** deterministic invalid block. Every conforming node has the same committed bucket set.
- **Known bucket cannot be read, decoded, or authenticated:** local node fault. The jet fails rather than rejecting a potentially valid block.
- **Certificate digest differs from the selected setup:** deterministic invalid block.
- **Setup table missing or empty:** initialization fault; the node must not validate AI blocks.

This distinction is consensus-critical. Local disk corruption must never become a different acceptance decision from healthy peers.

## Resource invariant

All production buckets are committed and present. The default cap retains all 14
shape keys across seven trace heights after first use. Operators can lower the cap
to trade RSS for synchronous page-ins; that setting is unsuitable for adversarial
validators unless the operator accepts the latency risk.

## Cryptographic dependency

The setup digest must commit to every preprocessed value, circuit parameter, FRI profile, and public-value layout used during verification. The certificate-carried digest is only a selector and consistency check; trust comes from the verifier-owned committed table, never from metadata supplied by the miner.

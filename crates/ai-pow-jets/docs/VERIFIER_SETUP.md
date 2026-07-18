# AI-PoW verifier setup lifecycle

The compact recursive verifier uses one proof-independent context for each reachable Layer-0 trace-height bucket. These contexts are large, but the set of buckets and each verifier-key digest are consensus-known.

## Boot path

1. Enumerate every production trace-height bucket admitted by the Pearl parameter envelope.
2. Load or deterministically rebuild each context from the production circuit parameters.
3. Recompute and compare its verifier-key digest with the committed digest table.
4. Serialize validated contexts beneath the node's data directory with a local file checksum.
5. Install the complete bucket table into `ai-pow-jets` exactly once.

A digest mismatch aborts startup. A node must not continue with a locally derived setup that differs from the consensus-known table.

## Verification path

The jet recomputes the certificate's required trace height from the block-bound statement and resolves exactly that bucket. A cache miss reads and deserializes the prebuilt file, verifies both the file checksum and verifier-key digest, and inserts the context into a bounded LRU.

Untrusted blocks cannot trigger setup generation. Page-in cost is bounded by one existing context file; circuit construction remains a boot-time operation.

## Failure classes

- **Unknown trace height:** deterministic invalid block. Every conforming node has the same committed bucket set.
- **Known bucket cannot be read, decoded, or authenticated:** local node fault. The jet fails rather than rejecting a potentially valid block.
- **Certificate digest differs from the selected setup:** deterministic invalid block.
- **Setup table missing or empty:** initialization fault; the node must not validate AI blocks.

This distinction is consensus-critical. Local disk corruption must never become a different acceptance decision from healthy peers.

## Resource invariant

All production buckets are committed and present, while only a configured number are resident. Eviction changes latency and RSS, not verifier behavior or accepted proofs. Each returned context is reference-counted so concurrent eviction cannot invalidate an in-flight verification.

The default cache should cover the production bucket set where operator memory permits. Lowering it trades RSS for attacker-influenceable page-in latency; it does not reduce the setup table or accepted parameter envelope.

## Cryptographic dependency

The setup digest must commit to every preprocessed value, circuit parameter, FRI profile, and public-value layout used during verification. The certificate-carried digest is only a selector and consistency check; trust comes from the verifier-owned committed table, never from metadata supplied by the miner.

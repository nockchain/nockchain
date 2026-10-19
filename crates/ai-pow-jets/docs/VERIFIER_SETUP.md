# AI-PoW verifier setup lifecycle

The compact recursive verifier uses one proof-independent native context for each `(rules, trace_height, sx_bound)` key. The production table contains 28 keys: the 14 canonical representatives under both `Legacy` and `Hardened` rules, covering trace heights `2^13..=2^19` and both shape classes.

## Boot path

`install_or_build_verifier_setup` installs the complete production table exactly once:

1. Validate the canonical representative parameters and computed trace heights. A cached descriptor cannot choose different parameters, AIRs, circuit layouts, or proof rules.
2. Load the shape-descriptor cache, or derive every context from the canonical program and recursive circuit structure.
3. For a new context, preprocess the deterministic 22-column Layer-0 program without constructing the main witness. Plan proof targets from the native AIR, degrees, FRI/opening layout, and circuit table operations.
4. Compile Layer 1 with the production profile, plugins, table order, and state/public-value bindings. Compute its real preprocessing commitment and metadata.
5. Compile Layer 2 with the same rule-selected verifier constraints. Under `Hardened`, bind the real computed Layer-1 preprocessing digest. Compute the real Layer-2 preprocessing commitment and metadata, and retain only native verifier data.
6. Serialize compact native contexts with file checksums. On cache load, validate every native context by page-in; reconstruct missing contexts from canonical shape.
7. Require all 28 observed native keys and both committed table fingerprints before installing the table.

The Legacy subset must match `AI_POW_V0_VERIFIER_SETUP_TABLE_DIGEST`; the complete versioned table must match `AI_POW_V1_VERIFIER_SETUP_TABLE_DIGEST`. Stored descriptor digests are not substitutes for validated native keys. A fresh derivation mismatch aborts startup; an unusable descriptor cache is replaced only after a complete validated derivation.

Setup does not execute a witness, call a reference prover, or manufacture a proof object to obtain metadata. Certificate generation still executes witnesses and produces genuine proofs using its separate prover setup.

`derive_compact_verifier_context_for_shape` is the native shape-only factory. `AiPowCompactVerifierSetupDescriptor::derive_context` invokes that factory and compares its computed key with the descriptor's recorded key.

## Shape cache and native files

The shape cache is `<datadir>/ai-pow/verifier-setup-shapes-v3.bin`. Its envelope is:

| Offset | Length | Encoding |
| --- | --- | --- |
| 0 | 8 | ASCII `NCVPSHAP` |
| 8 | 4 | Little-endian `u32` format version `3` |
| 12 | 8 | Little-endian `u64` payload length |
| 20 | 32 | Raw BLAKE3 checksum of the payload |
| 52 | Payload length | Bincode 2 serde standard encoding of the descriptor vector |

Each `AiPowCompactVerifierSetupDescriptor` contains, in field order:

1. `rules`: `Legacy` or `Hardened`.
2. `zk_params`: `m`, `k`, `n`, `noise_rank`, `tile`, and `difficulty_bits`, all `u32`.
3. `trace_height`: `usize`.
4. `verifier_key_digest_bytes`: a byte vector containing exactly 40 canonical digest bytes.

The loader requires complete framing, a matching checksum, complete payload consumption, canonical field limbs, and valid bounded geometry. Production loading additionally requires the exact canonical representatives and complete rule/shape coverage before allocating contexts.

Native files use `<datadir>/ai-pow/ctx-v4-{rules}-2p{log2}-{sx|rb}-{tag}.bin`, where `{rules}` is `Legacy` or `Hardened` and `{tag}` is the first eight digest bytes in lowercase hexadecimal. The file contains the Bincode 2 serde standard encoding of `AiPowVerifierSetup`. Append `.blake3` to the complete filename for its checksum sidecar; that sidecar contains exactly 32 raw BLAKE3 bytes.

Neither format contains a captured Layer-0 or Layer-1 proof. Previous proof-bearing cache files are not loaded or converted. `build_and_cache_verifier_setup_descriptors` writes descriptors and compact native files; `rebuild_verifier_setup_from_descriptor` recomputes canonical setup and rejects a different key. `AiPowCompactVerifierSetupDescriptor::from_context` validates a native context before recording its shape and digest.

## Verification path

After setup-free statement checks, the jet resolves the certificate's `(rules, trace_height, sx_bound)` key. A resident-cache miss reads the prebuilt native file and checks its checksum, complete decoding, metadata/common-data binding, FRI profile, rule/shape identity, and recomputed verifier-key digest against the already pinned table entry.

The native verifier then performs the actual certificate and statement-binding checks. Descriptors and native contexts do not supply accepted certificate values or replace proof verification. Paging changes only where a context resides.

## Failure classes

- **Unknown shape key:** deterministic invalid block. Every conforming node has the same committed bucket set.
- **Known bucket cannot be read, decoded, or authenticated:** local node fault. The jet fails rather than rejecting a potentially valid block.
- **Certificate digest differs from the selected setup:** deterministic invalid block.
- **Setup table missing or empty:** initialization fault; the node must not validate AI blocks.

This distinction is consensus-critical. Local disk corruption must never become a different acceptance decision from healthy peers.

## Resource invariant

All 28 production buckets must be present and validated before table installation. The resident-context cap is independent of that complete key set; reducing residence does not reduce the table or change verification rules.

Verifier contexts retain common verifier data, not raw preprocessing columns or PCS prover trees. Actual proof generation may retain those prover resources in its separate reusable prover cache.

## Cryptographic dependency

The setup digest binds the preprocessing commitment, circuit metadata, FRI profile, and public-value layout used by the native verifier. The certificate-carried digest is a selector and consistency check; trust comes from the verifier-owned committed table, never from miner-supplied metadata.

Serialized common data omits lookup descriptors. Native verification reconstructs lookup/AIR structure from the trusted verifier configuration; serialized context equality alone is not an equality check over that omitted in-memory structure.

This setup/cache cutover does not change certificate encoding, historical rule selection, either committed table fingerprint, or actual proof acceptance. Legacy certificates continue to use their Legacy native keys and checks.

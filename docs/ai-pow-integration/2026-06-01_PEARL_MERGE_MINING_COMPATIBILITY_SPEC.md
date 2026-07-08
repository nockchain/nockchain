# Pearl-Compatible Nockchain AI-PoW Submission Specification

Date: 2026-06-01
Status: Pearl-compatible dual submission plumbing in progress

## Goal

Nockchain AI-PoW should support a Pearl-format-compatible work attempt so one
miner can commit to a Pearl block candidate and a Nockchain block candidate,
evaluate one Pearl-style useful-work attempt, and submit the result to the
chain whose target was satisfied. A Nockchain hit submits the canonical
recursive `%ai-pow` certificate to the Nockchain node. A Pearl hit submits a
Pearl `PlainProof` to Pearl Gateway via `submitPlainProof`.

The chains do not need to share proof systems. Nockchain should use its own
recursive certificate and verifier. Compatibility is at the mineable work layer:
the same Pearl-style `sigma`, `mu`, matrix commitments, noise seeds, ticket
tile state, and jackpot digest must be used.

For the current milestone, the Hoon/kernel surface remains Nockchain-only while
the Rust miner also wires Pearl Gateway submission. That means:

- Nockchain accepts only the canonical `%ai-pow` block proof arm.
- Hoon does not define Pearl-specific molds or dispatch arms.
- Pearl-format details live inside Rust-owned nonce bytes.
- Nockchain acceptance requires the shared jackpot digest to satisfy the
  Nockchain target after Pearl's pattern-size work-factor adjustment.
- Nockchain acceptance does not require the shared jackpot digest to satisfy
  Pearl's `nbits` target.
- Pearl submission does not build or submit a Nockchain recursive certificate
  unless the same attempt also satisfies the Nockchain target.
- The attempt does not need to satisfy both targets. If only Pearl's adjusted
  target is hit, submit only to Pearl. If only Nockchain's target is hit,
  submit only to Nockchain. If both are hit, submit both.

## Canonical Hoon Artifact

The only production AI-PoW block artifact is:

```hoon
[%ai-pow nonce=ai-pow-nonce cert=ai-pow-certificate]
```

`ai-pow-nonce` is opaque to Hoon:

```hoon
+$  ai-pow-nonce  [len=@ud data=@uxaipownonce]
```

The recursive certificate remains structured:

```hoon
+$  ai-pow-certificate
  $:  version=@ud
      params=[m=@ud k=@ud n=@ud noise-rank=@ud tile=@ud difficulty-bits=@ud]
      found-idx=@ud
      trace-height=@ud
      commitments=ai-pow-commitments
      public-inputs=ai-pow-public-inputs
      certificate=ai-recursive-certificate
  ==
```

There is no Hoon `%ai-pmp` arm. There are no Hoon `pearl-*` types. The Hoon
kernel should not know whether the nonce bytes contain Pearl-compatible
material, a future native encoding, or another Rust-owned encoding. It stores
and hashes the artifact as `%ai-pow` and calls a Rust verifier for semantics.

## Opaque Nonce Bytes

The current Rust-owned nonce byte envelope for Pearl-format-compatible
Nockchain submission is:

```text
ai_pow_nonce_v1 =
    magic[4]              = "AIP1"
    statement_len[2]      = little-endian u16
    statement[statement_len]
    coinbase_tx_len[4]    = little-endian u32
    coinbase_tx[coinbase_tx_len]
    merkle_branch_len[1]
    merkle_branch[merkle_branch_len][32]
```

`statement` is the existing Rust `PMP1` Pearl-compatible public statement:

```text
PMP1 =
    magic[4]                    = "PMP1"
    pearl_incomplete_header[76]
    pearl_public_data[164]
    expected_aux_commitment[32]
    aux_len[2]
    aux_bytes[aux_len]
```

`aux_bytes` is the existing `NPA1` Nockchain aux envelope:

```text
NPA1 =
    magic[4]                    = "NPA1"
    chain_id_len[1]
    chain_id[chain_id_len]
    nock_block_commitment[32]
    target_epoch_or_height[8]
    extra_domain_data_len[2]
    extra_domain_data[extra_domain_data_len]
```

Rust miner default policy for this milestone:

- `chain_id` is `nockchain` for this milestone. The final protocol
  network/domain string can be changed centrally in the Rust miner default
  without changing Hoon types or the `%ai-pow` noun shape.
- `target_epoch_or_height` is zero for this milestone.
- `extra_domain_data` is empty for this milestone. Future replay-protection
  extensions should be constructor-owned protocol changes, not operator CLI
  knobs.
- Matrix inputs use the fixed recursive profile and deterministic local
  smoke-profile synthesis with seed `ai-pow-prod-v1`. Operator matrix-file
  inputs are not part of this milestone's CLI surface.
- Pearl work headers come from Pearl Gateway miner RPC `getMiningInfo` over
  Unix socket `/tmp/pearlgw.sock`, matching Pearl Gateway's default miner-RPC
  configuration. TCP gateway mode is available through the unified
  `--pearl-gateway` endpoint. Manual Pearl header CLI flags were removed; the
  production miner obtains Pearl work through Gateway only.
- Pearl Gateway requests use a bounded 2000 ms request timeout so a silent,
  wedged, or malicious local Gateway cannot block candidate processing
  indefinitely. Operator timeout tuning is not part of this milestone's CLI
  surface.
- Pearl Gateway JSON-RPC responses are read as a single bounded line, capped at
  160 KiB before JSON parsing. This covers a base64-encoded maximum-size
  coinbase aux inclusion plus the Pearl header, target, and JSON-RPC envelope
  while preventing a local Gateway from forcing unbounded miner allocation by
  streaming data without a newline.
- In Gateway mode, the miner refreshes `getMiningInfo` every 1000 ms while a
  Nockchain candidate remains current. Operator refresh tuning is not part of
  this milestone's CLI surface. If the
  refreshed Pearl incomplete header changes, the miner cancels the current
  ticket loop and restarts work for the same Nockchain candidate using the new
  Pearl header. After a solution is turned into a Nockchain poke attempt, the
  cached candidate is cleared and Gateway refresh does not redispatch that
  solved candidate; the miner waits for the node to emit a new candidate. A
  Pearl-only Gateway hit with a successful synchronous `submitPlainProof`
  response does not clear the Nockchain candidate: identical refreshed Pearl
  work is skipped, and a later changed Pearl header restarts ticket search for
  the same still-unsolved Nockchain candidate. If the synchronous
  `submitPlainProof` RPC/transport fails, the miner keeps the Nockchain
  candidate but clears the solved Pearl-header marker so the next Gateway
  refresh can retry the same header. There is no manual or static header mode.
- Pearl Gateway's work cache treats the full incomplete header bytes as the
  base-template freshness key. Same-parent updates that change timestamp,
  target bits, transaction merkle root, or version replace the current template
  and clear derived aux-bearing templates. This prevents stale same-parent
  Gateway work from surviving until the next previous-block change.
- Gateway-backed merge work asks `getMiningInfo` for a Pearl block template
  that already contains the Nockchain aux commitment. The request carries
  generic Pearl-side `coinbase_aux_flags`, encoded as standard base64 of
  `NOCKCHAIN-AI-POW-AUX || aux_commitment`, plus
  `return_aux_inclusion=true`. Gateway caps decoded aux flags at 256 bytes to
  keep miner-controlled coinbase rewriting bounded and caps cached derived
  aux-bearing templates at 1024 entries per current base template. A compatible
  Gateway returns an `aux_inclusion` object containing standard-base64
  `coinbase_tx` and a standard-base64 `merkle_branch` list. The Rust miner
  requires that returned proof in Gateway mode and verifies it against the
  returned incomplete header before using the work. Hoon still receives only
  the opaque nonce and recursive certificate.
- Pearl Gateway submission uses `submitPlainProof` with the Pearl wire format:
  `plain_proof` is standard base64 of Pearl's `bincode 1.3.3`
  `PlainProof`, and `mining_job` contains the base64 incomplete header bytes
  plus the exact target JSON integer returned by Gateway's `getMiningInfo`.
  Miners use the adjusted Pearl target for local hit detection, but the
  `submitPlainProof` `mining_job` echoes Gateway's original job target. The
  Rust client rejects Gateway work if that target is not a uint256 decimal JSON
  integer, if it is encoded as a negative number, float, string, array, object,
  boolean, or null, or if it does not equal the compact target encoded by the
  returned header's `nbits`. The proof contains the two matrix Merkle proofs
  for `A` and `B^T`; it is not a Nockchain block artifact and is never
  serialized into Hoon.
- Gateway acceptance is header-template sensitive. Pearl Gateway's async
  handler compares `mining_job.incomplete_header_bytes` with its current block
  template's `serialize_without_proof_commitment()` and silently skips old or
  different headers after returning `"submitted"`. Therefore production
  merge-mining requires the Pearl Gateway template to already commit to the
  same aux-bearing Pearl header that the Nockchain attempt used. The Rust miner
  now requests that aux-bearing work through the `getMiningInfo` extension
  above. If a Gateway accepts the request but omits `aux_inclusion`, the miner
  rejects the work before mining. This avoids locally mutating Gateway-issued
  work into a header that Nockchain can self-verify but Pearl Gateway cannot
  accept, and avoids false success from Gateway's pre-async `"submitted"`
  acknowledgement.

The `coinbase_tx` and `merkle_branch` prove that
`"NOCKCHAIN-AI-POW-AUX" || expected_aux_commitment` appears in the
txid-committed coinbase input script and that the coinbase txid is committed by
the Pearl header transaction merkle root. Nockchain can use a coinbase-only
Pearl block profile, so the current production verifier requires
`merkle_branch_len = 0`. The branch-length byte remains in the Rust-owned nonce
format for forward compatibility, but any nonzero branch is rejected until a
future milestone deliberately supports Pearl transaction merkle trees. A
witness-only occurrence or an output script occurrence is not sufficient for
Nockchain acceptance.

The current maximum nonce size is pinned by Rust tests at 101,424 bytes
(approximately 100.0 KiB):

```text
4 + 2 + PEARL_MERGE_PUBLIC_STATEMENT_MAX_SIZE
+ 4 + PEARL_AUX_INCLUSION_MAX_COINBASE_TX_BYTES
+ 1 + 32 * PEARL_AUX_INCLUSION_MAX_MERKLE_BRANCH
```

In the current production profile `PEARL_AUX_INCLUSION_MAX_MERKLE_BRANCH = 0`,
so the branch component contributes no bytes. Any Gateway-returned branch and
any nonce carrying `merkle_branch_len > 0` is rejected before mining or
verification work.

This is separate from recursive proof bytes. Hoon sees only `[len data]` for
the nonce and a recursive certificate artifact. As of the 2026-06-07 route
decision, the active production recursive-proof candidate is compact
batch-STARK over a fast statement-bound L1 proof. Native terminal compression
experiments were removed from the AI-PoW API, and the older large batch-STARK
checkpoint noun remains a hardened regression/checkpoint object rather than the
production wire artifact.

## Work Unit Terminology

Pearl-compatible Nockchain mining uses Pearl's work vocabulary:

- A **work instance** is the transcript and computation defined by
  `sigma`, `mu`, trusted `A`/`B`, `kappa`, `H_A`, `H_B`, `s_A`, `s_B`, and the
  noised tiled matmul.
- A **tile ticket** is a public proof-parameter selection, such as
  `t_rows`/`t_cols`, plus the jackpot digest for the opened tile state from
  that work instance.
- A single work instance may expose many valid tile tickets. That is Pearl's
  intended lottery model, not a shortcut.

The forbidden shortcut is different: a miner must not introduce a
Nockchain-only nonce or hash loop that produces many Nockchain target trials
without producing Pearl-valid tile tickets bound to the committed work
instance.

## Pearl Puzzle Requirements

Nockchain's Pearl-compatible puzzle requirement set is Pearl's requirement set,
not a narrower native square-tile subset.

Normative requirements:

1. The mining configuration `mu` is Pearl's 52-byte `MiningConfiguration`:
   `common_dim`, `rank`, `mma_type`, `rows_pattern`, `cols_pattern`, and
   all-zero reserved bytes.
2. `rows_pattern` and `cols_pattern` are Pearl 6-byte `PeriodicPattern`
   values. They may describe any canonical Pearl periodic pattern, including
   non-contiguous multi-dimensional patterns, not only `[0, 1, ..., t - 1]`.
3. The ticket public parameters are Pearl's `(t_rows, t_cols)` offsets. Each
   offset must be valid for its pattern, and the shifted pattern indices must
   remain inside the committed matrix dimensions:
   `t_rows + max(rows_pattern) < m` and
   `t_cols + max(cols_pattern) < n`.
4. The opened ticket rows are
   `rows_pattern.indices_with_offset(t_rows)`. The opened ticket columns are
   `cols_pattern.indices_with_offset(t_cols)`. These exact row/column sets
   define the tile state being proven and hashed.
5. The mineable work instance computes the full Pearl noised matmul schedule
   for the configured pattern partition. A submitted Nockchain ticket must be
   one of the Pearl-valid tickets from that work instance.
6. The target-price adjustment is Pearl's pattern-size rule:
   `target * rows_pattern.size() * cols_pattern.size() * dot_product_length`,
   where `dot_product_length = common_dim - common_dim % rank`, with the
   jackpot digest interpreted as Pearl's little-endian `uint256`. When
   Nockchain supplies an arbitrary chain target, that target replaces Pearl's
   `nbits` base target but the same pattern-size work factor applies.
7. The recursive certificate statement must bind the same public
   `MiningConfiguration`, `(t_rows, t_cols)`, `H_A`, `H_B`, `s_A`, tile state,
   and jackpot digest that a Pearl verifier would derive for the ticket.

Any implementation that cannot prove or verify an otherwise valid Pearl
periodic-pattern ticket must fail closed for that configuration until support
exists. It must not silently rewrite Pearl's puzzle into square-contiguous
tiles, verifier-selected tiles, a single-tile lottery, or a Nockchain-only
nonce loop.

## Work Transcript

For Pearl-format-compatible mode, Rust derives the mineable attempt with Pearl's
transcript:

```text
kappa = BLAKE3(sigma || mu)
H_A   = BLAKE3(pad(A_row_major), key=kappa)
H_B   = BLAKE3(pad(B_col_major), key=kappa)
s_B   = BLAKE3(kappa || H_B)
s_A   = BLAKE3(s_B || H_A)
hash  = BLAKE3(tile_state, key=s_A)
```

The nonce must not add a second Nockchain-only nonce into the attempt state.
Changing the Pearl header, mining configuration, aux commitment, or Nockchain
block commitment changes the work instance. Changing public proof parameters,
including a selected ticket offset, changes the public ticket and jackpot
statement for that work instance; it does not derive a fresh `kappa` or fresh
noised matmul.

Pearl-style tile-ticket reuse is intentional. Cache-friendly Nockchain-only
retry loops are the soundness risk. A miner may not run one Pearl work instance
and then grind many independent Nockchain nonces against it.

## Nockchain Acceptance Contract

This section records the future acceptance contract so the current data shape
does not become under-specified. It is not part of the current implementation
milestone. At present Hoon remains fail-closed for `%ai-pow`, and the Rust
boundary used by this branch performs metadata-only decode/precheck work before
submission.

When the real verifier is explicitly in scope, Nockchain-side verification must
perform these checks before recursive proof verification:

1. Bound the jammed block artifact before cueing.
2. Cue canonically and reject noncanonical jam.
3. Decode only `%ai-pow`.
4. Decode `ai-pow-nonce` as `[len data]` and reject length mismatches,
   oversized nonce bytes, malformed `AIP1`, malformed `PMP1`, malformed
   `NPA1`, malformed coinbase bytes, oversized coinbase bytes, oversized merkle
   branches, and trailing bytes.
5. Verify the aux inclusion proof against the Pearl header merkle root.
6. Verify the `NPA1` aux block commitment equals the trusted candidate
   Nockchain block commitment.
7. Recompute Pearl-compatible `kappa`, `H_A`, `H_B`, noise seeds, tile state,
   and jackpot digest from trusted matrices, `sigma`, `mu`, and the submitted
   public ticket parameters.
8. Verify the submitted ticket parameters are valid for Pearl's full puzzle
   configuration: canonical periodic row/column patterns, valid `t_rows` and
   `t_cols` offsets, in-bounds shifted pattern indices, and the exact opened
   row/column sets used by the recursive statement.
9. Verify the jackpot digest satisfies the Nockchain target after applying the
   same Pearl pattern-size work factor used for Pearl's target check.
10. Do not require the jackpot digest to satisfy Pearl's target for Nockchain
   acceptance.
11. Verify recursive certificate metadata matches the recomputed work:
    `zk_params`, `found_idx`, `trace_height`, `h_a_chunk`, `h_b_chunk`,
    `JOB_KEY = kappa`, `COMMITMENT_HASH = s_A`, `JACKPOT_MSG = tile_state`,
    and `HASH_JACKPOT = jackpot_hash`.
12. Only after those cheap checks, reconstruct and verify the recursive
    certificate.

For multi-tile and non-contiguous periodic-pattern configurations, Nockchain
verification must support Pearl's tile-ticket semantics or fail closed. It must
not accept a one-verifier-selected-tile statement as the final consensus rule
while claiming Pearl-compatible puzzle requirements.

## Current Implemented Surface

Implemented in this branch:

- `ai_pow::pearl_compat` serializes and parses Pearl header/config/public data,
  `NPA1` aux, and `PMP1` public statements.
- Pearl ticket tile-state computation uses the shared dimension-general
  matmul helper for `rows_pattern.size() × cols_pattern.size()` tickets and
  Pearl's rank-aligned `dot_product_length`; the old square `params.tile ×
  params.tile` tile path delegates to the same helper for the contiguous
  subset.
- The Layer-0 trace generator has dimension-general useful-work sweep and
  noised-chunk/store-layout entrypoints for `h × w` Pearl tile shapes.
  Canonical program reconstruction and the recursive bridge use an explicit
  strip-schedule entrypoint that sizes strip openings and sweep rows from the
  public A-row/B-column sets. The recursive prover must bind those exact
  shifted Pearl pattern indices; it must not prove a square-contiguous
  surrogate.
- The Rust precheck now treats Pearl and Nockchain targets independently for
  Nockchain submission: Nockchain-side precheck requires only the Nockchain
  target, adjusted by Pearl's
  `dot_product_length * rows_pattern.size() * cols_pattern.size()` work factor.
- `ai_pow_miner::pearl_mining` evaluates explicit Pearl tile tickets and
  returns when either the Pearl adjusted target or the Pearl-priced Nockchain
  target is hit.
  Attempt counters in this path count ticket evaluations, not necessarily
  fresh `sigma || mu` noised-matmul work instances. The returned ticket records
  `pearl_target_hit` and `nockchain_target_hit`, so the run loop can submit to
  the appropriate chain without treating one chain's target as the other's
  admission rule.
- `ai_pow_miner::run` now requires an explicit Rust-only
  `PearlMergeSubmissionConfig`. The connected node run loop derives the
  Nockchain candidate commitment, builds a coinbase-only Pearl-format aux
  inclusion, mines Pearl-compatible ticket attempts, submits Pearl hits to
  Gateway, and constructs the recursive proof-node payload only
  after a ticket hits the Nockchain target.
- The miner's internal Pearl plain-proof plumbing builds Pearl's `PlainProof`
  from the mined attempt, the trusted matrices, and Pearl matrix Merkle proofs,
  then serializes it as standard base64 of Pearl-compatible `bincode 1.3.3`
  bytes for Gateway `submitPlainProof`. This helper is intentionally not a
  public Nockchain proof API; public Nockchain submission goes through
  `PearlMergeSubmissionConfig::new_recursive` and the recursive `%ai-pow`
  certificate path. The resulting submission config exposes read-only accessors
  for inspection; callers cannot mutate the Gateway, Pearl mining config, aux
  template, mining options, or recursive certificate builder fields in place.
- The connected run loop submits the Nockchain `%ai-pow` command only for
  Pearl-priced Nockchain target hits. Pearl-only hits do not build a recursive
  certificate and do not poke Hoon. The Hoon kernel still receives no
  Pearl-specific fields beyond the Rust-owned opaque nonce bytes.
- The Pearl-compatible run loop accepts recursive certificate data only through
  a wrapper constructible by public callers from the opaque
  `AiPowRecursiveCertificateRun` returned by the recursive prover. Downstream
  crates cannot synthesize this run object directly. Before wrapping the
  command, the miner rechecks the run's `zk_params`, `found_idx`,
  exact A-row/B-column strip schedule, `trace_height`, commitments, and bound
  public inputs against the ticket-derived metadata, so a stale or wrong-ticket
  recursive run is rejected before it is submitted to the node.
- `ai-pow-mine` no longer has a submission-mode switch. It always builds
  canonical Pearl-format-compatible Nockchain `%ai-pow` submissions. It fetches
  the Pearl incomplete block header from Pearl Gateway miner RPC
  `getMiningInfo`. The visible operator CLI surface is intentionally
  small: node private gRPC address, required v1 `--mining-pkh` or
  `--mining-pkh-adv` reward configuration, unified `--pearl-gateway`
  endpoint, and log filter. The legacy split Gateway
  transport/socket/host/port flags were removed; Gateway location is configured
  through the one endpoint string. Matrix-shape, custom synthetic-seed, Gateway
  timing, and reconnect flags were removed. Gateway fetches use an explicit TCP
  connect timeout plus socket read/write timeouts so local
  Gateway failure is a skipped candidate, not an unbounded miner stall. The
  miner also polls Gateway while a Nockchain candidate is current and
  redispatches the ticket loop if the Pearl header changes. The miner derives
  the Rust-only Pearl mining config from the recursive AI-PoW params. The Rust
  submission config carries a direct Pearl Gateway RPC config.
  The CLI uses the default `ai-pow-prod-v1` local smoke-profile matrices; the
  remaining required local operator input is a v1 pubkey-hash reward
  configuration. v0 public-key mining configs are not accepted, are not
  defaulted, and are sent to the node as an empty legacy list. Once the miner
  builds a
  `%ai-pow` poke for a candidate and attempts to send it to the node, it clears
  the cached candidate so later Pearl Gateway template changes cannot produce
  duplicate submissions for the same Nockchain candidate. Pearl-only Gateway
  hits retain the cached Nockchain candidate, remember the solved Pearl header,
  and resume only after Gateway advertises changed Pearl work. Synchronous
  Gateway submit RPC/transport failures are retried on the next refresh.
  The legacy NCMN miner and prover-only smoke CLI were removed so downstream
  callers cannot accidentally treat them as production submission APIs.
- `AiPuzzleInputs` carries a required `PearlMergeSubmissionConfig`; the missing
  Pearl submission configuration state is no longer representable through the
  production miner API. There is no mixed-mode branch in the connected run loop,
  and the submission config is constructor-owned rather than a bag of mutable
  public fields.
- `ai-pow-miner` and `zk-pow-miner` require v1 mining reward configs. Public
  constructors and CLIs require at least one nonempty `MiningPkhConfig` with a
  nonzero share; connected run loops submit `set-mining-key-advanced` with an
  empty legacy public-key config list and a nonempty PKH config list.
- Pearl-compatible miner preflight rejects Rust-side submission configs whose
  `common_dim`, `rank`, recursive params, or row/column patterns do not match
  the configured AI params or Pearl's bounded public-parameter envelope. This
  is an implementation guard, not the protocol requirement set.
  The protocol requirement set is the Pearl periodic-pattern ticket semantics
  above. The current prover subset requires `difficulty_bits = 0` because the
  Nockchain target is verifier-supplied, and `spot_checks = 1` because the
  current recursive statement proves one explicit ticket. Contiguous,
  non-contiguous, rectangular, and non-native-grid Pearl schedules are
  represented by an explicit strip schedule derived from the public ticket.
- `ai_pow_miner::certificate_noun` emits `%ai-pow` artifacts with an opaque
  `[len data]` nonce and structured recursive certificate. The active
  production candidate is compact final-layer batch-STARK over a fast
  statement-bound L1 proof. The older large batch-STARK checkpoint certificate
  exceeds the wire-size budget and must not be treated as the production
  artifact.
- Public certificate-noun construction is typed around
  the opaque `AiPowRecursiveCertificateRun`; generic serde proof-node
  serializers and raw-node certificate builders are crate-internal
  test/plumbing helpers, not production submission APIs.
- Hoon-shaped Pearl public-statement noun builders/decoders are test-only
  Rust plumbing. Production block submission carries the Pearl statement only
  inside the opaque `AIP1` nonce bytes.
- The Rust metadata APIs parse the opaque nonce back into Pearl-format
  statement and aux inclusion evidence before any recursive proof-node work.
- The Rust noun boundary exposes a metadata-only `%ai-pow` decode/precheck
  path for Pearl-format-compatible artifacts, so malformed nonce bytes,
  candidate-block replay, aux inclusion tamper, target misses, and recursive
  metadata drift are rejected before recursive proof-node traversal.
- The Rust noun boundary also exposes a metadata-only command-shape entrypoint
  for the exact Nockchain submission payload
  `[%command %pow [%ai-pow nonce cert]]`. This is deliberately not a real
  verifier: it parses the command wrapper and runs cheap metadata checks only,
  leaving recursive proof verification disabled in Hoon for now.
- The Rust noun boundary also exposes a slab-level metadata-precheck entrypoint
  for future verifier integration. It accepts a `%ai-pow` artifact noun
  plus trusted Nockchain context and performs the same cheap metadata checks.
  This branch does not wire or pursue the real verifier.
- The trusted Nockchain verifier context is a named Rust struct rather than a
  loose argument list. It contains the candidate block commitment, matrix
  operands, Nockchain target, and Pearl pattern bound, all of which must be
  derived outside the miner-controlled artifact.
- Size-budget tests pin the maximum `AIP1` nonce envelope at 101,424 bytes,
  reject nonce bytes above that cap, reject any nonempty aux merkle branch in
  the current production profile, and assert a worst-case nonce plus small
  structured checkpoint certificate jams below 110 KiB.
- Historical batch-STARK recursive certificate harnesses measured
  representative structured certificate nouns above the production target. They
  remain useful checkpoint measurements, but the active production recursive
  proof candidate is the compact final-layer batch-STARK route over a fast
  statement-bound L1 proof.
- Hoon exports only `ai-pow-nonce`, `ai-pow-certificate`, and
  `ai-pow-artifact` concepts. No Pearl-specific molds are exported.
- Dumbnet consensus recognizes only `%ai-pow` for AI proof version `%3` and
  remains fail-closed. Real verifier wiring is out of scope for this
  milestone.

Release checks run for this state:

```text
GNORT_DISABLE=1 cargo test -p ai-pow-miner --release --features node -- --nocapture
GNORT_DISABLE=1 cargo test -p zk-pow-miner --release -- --nocapture
GNORT_DISABLE=1 cargo test -p ai-pow --release --features zk --test pearl_merge_compat -- --nocapture
```

## Remaining Fix Plan

1. Keep Hoon `%ai-pow` fail-closed and keep real verifier work out of this
   milestone.
2. Keep any future verifier call surface generic in the design: opaque nonce
   bytes, structured certificate, trusted candidate block commitment, target,
   params, and verifier context flow into Rust when that work is explicitly
   scheduled.
3. Done for this milestone: `NPA1.chain_id` is the Rust-owned `nockchain`
   default, `target_epoch_or_height` is zero, and `extra_domain_data` is empty.
   These aux metadata fields are not operator CLI knobs.
4. Done for this milestone: Nockchain production requires coinbase-only
   Pearl-format block templates (`merkle_branch_len = 0`). Revisit only if a
   future milestone deliberately supports Pearl transaction merkle trees.
5. Done for this milestone: `ai-pow-mine` uses Pearl Gateway miner RPC as its
   only Pearl work-header source.
6. Done for this milestone: Pearl Gateway header fetches have bounded request
   timeouts, bounded response-line reads, strict uint256 target parsing, header
   `nbits` target cross-checks, and bounded aux-inclusion decoding to avoid
   local Gateway denial of service during candidate processing.
7. Done for this milestone: Pearl Gateway work is refreshed while a Nockchain
   candidate remains current, and changed Pearl headers supersede stale ticket
   loops for that candidate. Pearl Gateway itself keys base-template freshness
   by the full incomplete header bytes, not only the previous block hash.
8. Done for this milestone: `ai-pow-mine` uses the fixed recursive profile and
   the `ai-pow-prod-v1` local smoke-profile synth seed. Operator matrix-file
   inputs were removed from this milestone's CLI surface.
9. Done for this milestone: `ai-pow-mine` and `zk-pow-mine` require v1
   pubkey-hash reward configs and submit an empty legacy public-key config list
   to the node.
10. Done for this milestone: after a successful Nockchain `%ai-pow` submission,
   the run loop clears the cached candidate so Pearl Gateway refresh cannot
   redispatch solved work. Only a fresh Nockchain candidate restarts mining.
11. Done for the Rust client path: Pearl Gateway `submitPlainProof` plumbing,
    Pearl-compatible `PlainProof` serialization, aux-bearing `getMiningInfo`
    requests, and returned `aux_inclusion` verification exist in Rust. Complete
    production Pearl-side acceptance requires deploying a Pearl Gateway server
    with the matching `getMiningInfo` extension so it issues the exact
    aux-bearing incomplete header used by the Nockchain attempt.
12. Re-run and tighten real recursive certificate size-budget caps after the
   final production proof shape is fixed.
13. Keep metadata-precheck tests covering malformed `AIP1`, `PMP1`, `NPA1`,
   candidate-block replay, aux inclusion tamper, target miss, metadata drift,
   and proof-node DoS limits without wiring Hoon acceptance.
14. Keep the explicit strip-schedule bridge separate from native square-tile
   helper paths. The legacy `tile` field remains in serialized ZK params as
   metadata, but Pearl ticket rows/columns are bound by the explicit schedule,
   not by `tile_i/tile_j`.

## Non-Negotiable Requirements

- The persisted and wire-transmitted Nockchain artifact is `%ai-pow`.
- Hoon must not define or dispatch on Pearl concepts.
- Hoon must not accept a Pearl ZKP, raw `MatmulProof`, or nonrecursive proof as
  the production AI-PoW certificate.
- A Pearl work instance may expose many tile tickets; each submitted ticket
  checks the same Pearl jackpot digest against the relevant chain target. No
  grinding a fresh Nockchain nonce against cached Pearl work.
- Pearl-priced Nockchain target satisfaction is sufficient for Nockchain
  submission; Pearl target satisfaction must not be enforced by Nockchain-side
  submission.
- Cheap replay, target, aux inclusion, metadata, and size checks must happen
  before recursive proof reconstruction or verification.

+++
version = "0.1.15"
status = "draft"
consensus_critical = true

# Activation (filled in after coordination)
activation_height = 114300  # ai-pow-activation-height (mainnet default; fakenet-overridable)

# Dates
published = "2026-07-17"
activation_target = "2026-07-31"

# People
authors = ["Logan Allen (National Compute Co)"]
reviewers = ["@nockchain-core"]

supersedes = "0.1.14"
superseded_by = ""
+++

# Logos

Adds a second, proof-of-useful-work mining puzzle — AI-PoW, a Pearl-compatible
tiled INT8 matmul with a compact recursive STARK certificate — that runs
*alongside* the existing ZK-PoW puzzle on one chain. Both puzzles contribute
equal fork-choice weight per unit of expected work, each retargets on its own
per-block ASERT subchain, and a mandatory Rust verify jet admits `%ai-pow`
blocks. The transition is gradual by construction: ZK-PoW is unchanged and keeps
producing blocks, while AI miners join as a second block source.

## Summary

Logos activates AI-PoW at height 114,300 (`ai-pow-activation-height`) as an
**additive** consensus change:

1. **A second block variant, `%ai-pow`.** Post-activation, a block may prove
   either the existing ZK-PoW puzzle (`%pow`) or the new AI-PoW puzzle
   (`%ai-pow`). AI-PoW is Pearl's proof-of-useful-work: a miner grinds noised
   INT8 tiled matmuls until a tile's keyed-hash *jackpot* clears the AI target,
   then proves that tile in-circuit with a compact final-layer batch-STARK
   recursive certificate. Consensus verifies the certificate with a mandatory
   Rust jet.

2. **Equal-weight heaviness across both puzzles.** The two puzzles live in
   different target spaces (ZK in the ~2³²⁰ tip5 space, AI in a 256-bit BLAKE3
   jackpot space). A single normalizer makes one unit of expected AI work weigh
   exactly the same as one unit of expected ZK work in fork choice, so neither
   puzzle can be "cheap-weighted" and the heaviest chain is a faithful sum of
   real work regardless of which puzzle mined which block.

3. **Independent per-puzzle ASERT, weighted 60/40 toward AI.** Each puzzle
   retargets on its own aserti3-2d subchain — the AI target reacts only to the
   interval between consecutive `%ai-pow` blocks, the ZK target only to
   consecutive `%pow` blocks. The per-puzzle ideal block times are set to give
   the AI puzzle **~60% of blocks and ZK ~40%** (AI 250 s, ZK 375 s;
   `1/250 : 1/375 = 60 : 40`), deliberately favouring AI to bootstrap and
   incentivize the new AI Compute Network. The two rates still sum to Aletheia's
   ~150 s combined cadence (`1/250 + 1/375 = 1/150`). ZK-PoW re-anchors at
   `ai-pow-activation-height − 1` onto a new post-AI ASERT regime
   (`zk-asert-post-ai`), leaving Aletheia's 150 s single-puzzle regime unchanged
   before activation.

4. **Single protocol-fund recipient.** Every post-activation block, whether
   `%pow` or `%ai-pow`, pays 20% of its newly issued coinbase reward to the
   existing protocol fund (`protocol-fund-address`). AI-PoW changes the puzzle
   and per-puzzle retargeting, not the coinbase recipient.

This upgrade does **not** change the ZK-PoW puzzle, the emissions schedule, the
80/20 split *ratio*, transaction formats, or any pre-activation behaviour. It
adds a puzzle; it does not replace one.

## Motivation

### Useful-work proof-of-work

Nockchain's long-run design goal is a proof-of-work whose expended energy
produces something beyond the hash itself. AI-PoW is Pearl's answer: the "work"
is inference over a caller-chosen INT8 model, and the puzzle is to find a noised
matmul tile whose committed output hashes below a difficulty target. A valid
`%ai-pow` block is a succinct attestation that a specific tile of a specific
(committed) model was actually evaluated. Introducing it as a live consensus
puzzle is the point of this upgrade.

### Why a dual puzzle, not a replacement

Swapping ZK-PoW out for AI-PoW in one step would strand the existing ZK-PoW
mining ecosystem and stake the chain's liveness on a brand-new, GPU-shaped
puzzle overnight. Running both puzzles on one chain instead lets AI-PoW prove
itself under real economic conditions while ZK-PoW continues to provide
liveness. The two puzzles are independent block sources that both extend the
same chain; a node validates each block against whichever puzzle it proves.

The one hard requirement this creates is fork-choice *fairness*: if AI blocks
weighed less (or more) than ZK blocks per unit of expected work, miners would
pile onto the cheaper-weight puzzle and the "heaviest chain" would stop tracking
total work. The equal-weight normalizer (below) is what makes a dual puzzle
sound rather than a footgun.

### Why per-puzzle ASERT

A single shared difficulty across both puzzles cannot work: the two puzzles have
different, independently-varying hashrates (CPU/GPU vs the ZK prover fleet) and
live in different target magnitudes. Each puzzle therefore carries its own
aserti3-2d instance (Aletheia's algorithm, reused verbatim) that measures
*only the blocks of its own type* since its own anchor, and each self-stabilises
to its own ideal block time.

### Why weight the block share 60/40 toward AI

The per-puzzle ideal block times target an expected **~60% of blocks** for AI
and **~40%** for ZK — AI at a 250 s ideal, ZK at a 375 s ideal. This biases
expected block-reward opportunity toward AI miners while the market develops.
The weighting changes expected miner opportunity only: both puzzle types retain
Aletheia's protocol-fund recipient for the 20% new-issuance share. It grants no
fixed percentage of total supply or chain-wide issuance.

The weighting is expressed purely through the two ideal block times and falls out
of the arithmetic: at ideals `t_ai`, `t_zk`, each puzzle produces blocks at rate
`1/t`, so the AI share is `(1/t_ai) / (1/t_ai + 1/t_zk)`. With `t_ai = 250`,
`t_zk = 375`: `1/250 : 1/375 = 60 : 40`, and the combined rate
`1/250 + 1/375 = 1/150` keeps the **global cadence at 150 s (2.5 min)** —
unchanged from Aletheia. The 12 h half-life is retained for both (a per-puzzle
stability window of ~173 blocks for AI, ~115 for ZK).

The 60/40 tilt is a **bootstrapping measure, not the end state**. Once the
upgraded, fully-useful ZK-PoW puzzle ships in a later protocol upgrade, the split
is intended to move to a balanced **50/50** (equal ideal block times) — at which
point both puzzles perform useful work and neither warrants a launch subsidy over
the other.

## Technical Specification

### Blockchain-Constants

Three fields are added to `blockchain-constants` in
`hoon/common/tx-engine-1.hoon`, and the Rust mirror in
`crates/nockchain-types/src/blockchain_constants.rs` gains the corresponding
`AsertParams` instances. The v1 `blockchain-constants` noun grows to a 10-slot
layout; the round-trip encode/decode is pinned by a Rust test
(`blockchain_constants_roundtrip_from_noun_for_mainnet_and_fakenet`).

```hoon
ai-pow-activation-height=114.300   :: AI-PoW activation height

+$  ai-asert                      :: AI-puzzle aserti3-2d params
  phase=114.300                    :: == ai-pow-activation-height
  anchor-height=114.300            :: == phase (the first AI block IS the anchor)
  anchor-target-atom=^~((bex 227)):: 2^227 (< 2^256, see below)
  ideal-block-time=250            :: seconds; AI wins ~60% of blocks
  half-life=^~((mul 12 (mul 60 60))) :: 12 h

+$  zk-asert-post-ai              :: ZK-puzzle post-activation regime
  phase=114.300
  anchor-height=114.299            :: == phase - 1 (standard aserti3-2d anchor)
  anchor-target-atom=^~((bex 291)):: 2^291 (unchanged ZK target space)
  ideal-block-time=375            :: seconds; ZK wins ~40% of blocks
  half-life=^~((mul 12 (mul 60 60)))
```

Two anchor conventions coexist and differ deliberately:

- **ZK (standard aserti3-2d):** `anchor-height = phase − 1`. The anchor is the
  last pre-activation block; the first post-activation ZK block is one interval
  past it. This is the same convention Aletheia uses.
- **AI:** `anchor-height = phase`. There is no AI block before activation, so the
  *first* `%ai-pow` block becomes the AI puzzle's own anchor. Subsequent AI
  blocks measure their interval against it.

The CLI enforces this split: `--fakenet-ai-asert-*` validates `anchor-height ==
phase` and `--fakenet-asert-*`/`--fakenet-zk-asert-*` validates `anchor-height ==
phase − 1`. A `--fakenet-ai-pow-activation-height 0` is rejected (it would make
`phase − 1` underflow).

The AI anchor target is `2^227`, **not** `2^291`. The AI jackpot is a 256-bit
BLAKE3 keyed-hash, so an anchor `≥ 2^256` would be cleared by every jackpot —
i.e. no proof-of-work at the anchor. `2^227` is chosen so that, under the
equal-weight normalizer below, a block at `2^227` in the AI space contributes
exactly the same fork-choice weight as a block at `2^291` in the ZK space
(`291 − 227 = 64`).

### The AI-PoW puzzle

AI-PoW is the Pearl proof-of-useful-work puzzle (whitepaper §4). Per mining
attempt, a miner:

1. Derives per-attempt transcript commitments from the block commitment and an
   extranonce (the header timestamp): `κ = BLAKE3(σ‖μ)`, matrix commitments
   `H_A`/`H_B = BLAKE3(pad, key=κ)`, and noise seeds
   `s_B = BLAKE3(κ‖H_B)`, `s_A = BLAKE3(s_B‖H_A)`. The noise is commitment-keyed,
   so each attempt forces a *fresh* noised matmul — there is no extra nonce that
   lets a miner skip the inference.
2. Runs the noised INT8 tiled matmul over its own (miner-chosen, Pearl-parity)
   `A`/`B` matrices and computes each tile's keyed-hash *jackpot*
   `= keyed_hash(tile_state, s_A)`.
3. If a tile's jackpot `≤ ai-target`, proves *that tile* in-circuit and emits the
   `%ai-pow` block. Otherwise it advances the extranonce and repeats.

The proof is a **compact final-layer batch-STARK recursive certificate**
(Plonky3 over Goldilocks + Tip5 + FRI, recursion via the vendored
`plonky3-recursion` substrate). It carries only the small final compact body
plus an explicit verifier-key digest; it binds the opened tile to the
block-committed `H_A`/`H_B` (dense) or the routing/jackpot commitment (MoE)
in-circuit, so the matrices are miner-chosen but non-grindable — a proof over any
other matrices fails the public-input binding. The larger checkpoint
(non-compact) certificate is a regression/benchmark intermediate and is not a
block artifact.

The new consensus crates are `ai-pow` (the Pearl-compatible prover/verifier
glue), `ai-pow-zk` (the Plonky3 AIR + recursion), `ai-pow-jets` (the consensus
verify jet + verifier-setup residency), and the miner-side `ai-pow-miner` +
`zk-pow-miner` + shared `nockchain-mining-common`.

### Equal-weight heaviness

`block-compute-work` (in `consensus.hoon`) dispatches on the block's proven
puzzle-type and adds the puzzle's normalized work to the parent's accumulated
work. `check-heaviness` requires `accumulated-work == parent + block-compute-work`
exactly, and `check-target` requires the block's target to equal the
ASERT-recomputed target — both deterministic, so a forged easy target or inflated
work is rejected.

The two work functions (`+tx-engine-0`):

```
compute-work(T)    = max-target-atom / (T + 1)          :: expected ZK attempts
compute-work-ai(T) = max-target-atom / (T·2^64 + 1)     :: = 2^256 / T
                   = compute-work(T · 2^64)              :: exact integer identity
```

`compute-work-ai(T) == compute-work(T · 2^64)` is an exact integer identity, so
at AI-target `T` and ZK-target `T·2^64` the two puzzles have equal solve
probability and their work sums meaningfully. Work rises as the target tightens,
so mining the easier puzzle yields *less* work per block — there is no cheap-weight
coin-hopping. At the anchors, `2^227` (AI) and `2^291` (ZK) satisfy `227 + 64 =
291`, so each anchor block contributes identical weight.

### Per-puzzle ASERT

Each puzzle retargets with Aletheia's `+compute-target:asert` (unchanged
polynomial, `rbits = 16`, 12 h half-life). Every accepted post-activation block
stores parent-derived ZK/AI counts, heads, and anchors in
`puzzle-asert-states`, keyed by block ID. Target computation reads the candidate
parent's entry, so `height-diff` counts only that branch's blocks of the same
puzzle type; fork arrival order and the other puzzle's cadence are irrelevant.
The activation parent initializes the ZK lineage. The first accepted `%ai-pow`
block on each branch initializes that branch's AI anchor.

### Block variant and the verify jet

The block/effect types gain the `%ai-pow` arm
(`[%ai-pow nonce=ai-pow-nonce cert=ai-pow-certificate]`) and the `%mine-ai`
mining-candidate effect (see below). `check-pow` dispatches: a `%pow` block runs
the existing ZK verifier; an `%ai-pow` block runs the **mandatory Rust jet**
`~/ %ai-pow-verify` (`ai-pow-verify:mine`), whose Hoon arm is a stub that `!!`s if
the jet is absent — so a node without the jet fails closed rather than admitting
unverified AI blocks.

The jet (`crates/ai-pow-jets`) binds the certificate to
`(block-commitment, target)`, enforces `jackpot ≤ target`, verifies the compact
recursive certificate against a verifier-owned setup, and returns a loobean.
**It is impossible to panic the node from this jet**: the two attacker-controlled
steps (decoding the artifact and running the recursion verifier) are wrapped in
`catch_unwind`; a panic on crafted input is a deterministic invalid-block signal
(`NO`, which consensus turns into a `%liar-block-id`), not a crash. A build guard
(`#[cfg(panic = "abort")] compile_error!`) refuses to build the consensus
verifier under `panic = "abort"`, where `catch_unwind` would be a no-op.

### Verifier setup

The recursion verifier needs a preprocessed *verifier setup* keyed by the
certificate's Layer-0 trace height. Consensus admits seven trace-height buckets
(`2^13 .. 2^19`); a block claiming a height above `2^19` is invalid (the
top-of-envelope `2^20` bucket is deliberately not built — it needs a large-RAM
node). At boot, a node installs the setup from a disk cache if present (fast:
load seeds + rebuild, no proving) or **generates it once** (~15 minute one-time
boot delay; it logs this), caching it and validating it against a committed v0
consensus digest either way. A node with no valid setup cannot validate `%ai-pow`
blocks, so any failure is fatal — the node shuts down rather than run blind.

The setup is disk-paged: each bucket's context is read + checksum-verified on
first use and held in an LRU. The production default retains 13 shape keys across
seven trace heights, bounding remote-triggered page-ins to one per key.

### Miner and candidate emission

Post-activation, the node emits a `%mine-ai` candidate alongside `%mine-zk`
whenever the candidate block changes *and* on new-heaviest-block / born
(`do-mine`), so AI miners re-target immediately. `%mine-ai` carries the AI-puzzle
variant of the candidate: the same block re-targeted to the AI ASERT target
(`+build-ai-candidate`), with its own commitment. `do-pow` reconstructs the
identical variant from the same candidate + state and runs the verify jet.

The reference miner is `ai-pow-miner` (`ai-pow-mine`), with a self-contained
gateway-free `--canonical` CPU mode for fakenet. A submitted block is
`[%command %pow %ai-pow nonce cert]`.

### Pearl merge-mining compatibility

The certificate binds the *same parameters* a Pearl proof binds, so a proof may
in principle be accepted by both Pearl and Nockchain. The Nockchain block
commitment is embedded in the Pearl coinbase via a
`NOCKCHAIN-AI-POW-AUX` tag; `verify_pearl_aux_inclusion` requires the tag to occur
**exactly once**, so a merge-miner cannot bind two Nockchain commitments to one
Pearl proof-of-work (one PoW ⇒ one Nockchain block). MoE (grouped-GEMM /
sparse-matmul) models are supported via the compact MoE path, which binds the
routing commitment.

### Coinbase new-issuance recipient

Aletheia's coinbase rule stands: every post-activation block pays
`floor(emission / 5)` (20%) of that block's **newly issued coinbase reward** to
the existing protocol fund (`protocol-fund-address`) and the remainder to the
miner, both as standard-timelocked coinbase outputs, validated by
`+check-fund-split:consensus`. The percentage applies only to new issuance in
that block. It does not transfer or encumber total supply, circulating supply,
previously issued NOCK, or transaction fees.

The rule is independent of puzzle type: `%pow` and `%ai-pow` coinbases both use
the same protocol-fund key. `+check-fund-split:consensus` validates that single
slot and rejects any block that redirects the required share to a different key.

The public [Tokenomics — Issuance Schedule](https://docs.nockchain.org/usdnock-asset/overview#issuance-schedule)
distinguishes newly issued rewards from existing supply, while
[Coinbase Distribution](https://docs.nockchain.org/usdnock-asset/overview#coinbase-distribution)
documents Aletheia's baseline split. Those pages provide public background;
this specification and consensus code define the Logos rule.

`+check-fund-split` validates the single protocol-fund slot: the key must equal
`protocol-fund-address`, and its coins must equal `floor(emission / 5)`. The
miner-side (`+build-ai-candidate` / `+new:coinbase`) uses the same coinbase
builder for ZK and AI candidates. A miner who directs that share to any other
address produces a block that fails `check-fund-split`. The targeting rules aim
for an expected ~60% AI block share; all post-activation fund outputs still use
the protocol-fund recipient.

## Activation

- **Height**: 114,300 (`ai-pow-activation-height`). AI-PoW verification, the
  per-puzzle ASERT regimes, and `%mine-ai` emission all activate at this height.
- **AI anchor**: the first `%ai-pow` block at or after 114,300 becomes the AI
  puzzle's ASERT anchor (bootstrapped by `accept-block`).
- **ZK re-anchor**: block 114,299 is the anchor for the new
  `zk-asert-post-ai` 375 s regime. Target computation switches for candidate
  height 114,300; block 114,299 itself remains in the pre-activation 150 s regime.
- **Verifier setup**: every validating node must have the AI-PoW verifier setup
  installed before it needs to validate an `%ai-pow` block (first boot generates
  it, ~15 min; subsequent boots load the cache). A node without it shuts down on
  boot rather than run blind.
- **Coordination**: all nodes must upgrade — and have the verifier setup — before
  the first `%ai-pow` block. There is no separate runtime anchor-capture step.

## Migration

### Requirements

- Software version: 0.1.15+.
- All nodes must upgrade before `ai-pow-activation-height = 114,300`.
- Validating nodes must complete the one-time verifier-setup generation (or ship
  a cache) before the first `%ai-pow` block.

### Configuration

No mandatory configuration changes for mainnet. By default, validating nodes retain
the full 13-key verifier setup table across seven trace heights after first use.
Operators may lower `--ai-pow-verifier-cache-cap` to reduce RSS by paging verifier
contexts in and out; doing so trades memory for synchronous page-in latency under
adversarial traffic.
Fakenet operators may override the activation height and per-puzzle ASERT params
with `--fakenet-ai-pow-activation-height`, `--fakenet-ai-asert-*`, and
`--fakenet-zk-asert-*` (all `requires = "fakenet"`). The AI ASERT phase and AI
activation are one boundary: either flag family may set it, and conflicting
values are rejected.

### Data Migration

Kernel state advances to `kernel-state-12`; its `derived-state-11` replaces the
process-global anchor caches with the branch-local `puzzle-asert-states` map.
State 11 upgrades only before AI activation (or at genesis), when the lineage is
empty and deterministic; a post-activation state-11 load fails closed because
its missing fork lineage cannot be reconstructed. `blockchain-constants` keeps
the v1 10-slot layout, whose Rust encode/decode round-trip is regression-pinned.

### Steps

1. Stop the node.
2. Update to version 0.1.15+ before block 114,300.
3. Restart. On first boot the node generates + caches the AI-PoW verifier setup
   (~15 min, logged) and auto-upgrades kernel state.

### Rollback

Rollback to a pre-Logos binary is safe only before `ai-pow-activation-height`.
After the first `%ai-pow` block, a pre-Logos node will reject it (unknown block
variant / no verify jet) and compute divergent heaviness (no
`compute-work-ai` normalizer), so it forks off.

## Backward Compatibility

### Breaking Changes

This is a **consensus-critical** upgrade. After activation:

- Pre-0.1.15 nodes cannot decode or verify `%ai-pow` blocks (unknown block
  variant; no `%ai-pow-verify` jet) and will reject them.
- Heaviness diverges: pre-0.1.15 nodes lack `compute-work-ai`, so they cannot
  reproduce the accumulated work of any chain containing an `%ai-pow` block.
- Per-puzzle ASERT changes ZK target computation for candidate height 114,300,
  using block 114,299 as the new regime's anchor; pre-0.1.15 nodes will not
  reproduce it.
- The `blockchain-constants` noun gains the AI-PoW fields, causing decode
  failures on pre-0.1.15 software.

### Network Partition Risk

Any node not upgraded (and without the verifier setup) before the first
`%ai-pow` block will fork onto an incompatible chain and reject valid AI blocks.
**All node operators must upgrade before block 114,300.**

### Transaction Compatibility

This upgrade does not change transaction formats. Transactions remain
structurally valid across the boundary.

## Security Considerations

- **Fail-closed verification.** The Hoon `++ai-pow-verify` arm `!!`s if the jet
  is absent, and the jet returns a deterministic `NO` (not a crash) on any
  malformed/forged input. A stale, forged, or mis-shaped certificate is rejected.
- **No node-panic from untrusted input.** Decode + recursion-verify are wrapped
  in `catch_unwind`; a build guard forbids `panic = "abort"` for the verifier
  crate. Decode is bounded (proof-node depth/count/atom-byte caps), and a
  trace-height above the `2^19` accept-band is rejected before the setup lookup.
- **Invalid-proof spam escalation.** A `%failed-pow-check` liar effect blocks
  every peer that supplied the block and records objective cryptographic
  misbehavior against its authenticated connection address; repeated peer-ID
  rotation from one IP escalates through the bounded IP-exclusion policy. Other
  liar reasons remain peer-scoped because protocol-version skew can produce
  honest disagreement at an upgrade boundary.
- **Equal-weight soundness.** `compute-work-ai(T) == compute-work(T·2^64)` is an
  exact identity, so the two puzzles sum work faithfully; `check-heaviness` and
  `check-target` re-derive both deterministically, so a forged easy target or
  inflated work is rejected (`%page-target-invalid` / `%page-heaviness-invalid`).
- **256-bit AI target, no saturation.** The AI anchor is `2^227 < 2^256`. An
  anchor `≥ 2^256` would be cleared by every jackpot (trivial PoW); `2^227`
  keeps a real work margin and equal-weights the ZK anchor.
- **One PoW ⇒ one Nockchain block (merge mining).** `verify_pearl_aux_inclusion`
  enforces exactly one `NOCKCHAIN-AI-POW-AUX` tag in the Pearl coinbase, and the
  aux commitment must equal the consensus block commitment — so a single Pearl
  PoW binds exactly one Nockchain commitment (no two-forks-from-one-PoW).
- **Fresh inference per attempt.** The noise is commitment-keyed
  (`s_A`/`s_B` derived from `κ`/`H_A`/`H_B`), so changing the extranonce forces a
  fresh noised matmul; there is no separate nonce that lets a miner skip
  inference. Matrices are miner-chosen (arbitrary model, Pearl parity) but bound
  in-circuit to the committed `H_A`/`H_B` (dense) or routing/jackpot (MoE).
- **Cache-thrash DoS.** `trace_height` is an attacker-controlled cert field and a
  cache miss triggers a synchronous disk page-in before proof rejection. The default
  LRU cap covers all 13 committed shape keys across seven trace heights, so each
  key pages in at most once. Lower caps are an operator-selected
  memory-for-latency tradeoff.
- **Time-warp.** `check-timestamp` enforces BIP113 median-of-11 + max-future.
  Both puzzles read the same global median, so timestamp manipulation cannot
  create cross-puzzle difficulty asymmetry.
- **External audit dependency (open).** The recursion + composite-AIR soundness
  is the one place a false-accept could live and is *assumed*, not
  independently verified — every "sound" verdict above is conditional on a
  dedicated external ZK-circuit + recursion audit, which must complete before an
  adversarial mainnet. The verifier setup ships pinned to a committed v0 digest;
  the fund/anchor `min-timestamp` placeholders inherited from Aletheia's Phase 2
  are unaffected by this upgrade.

## Operational Impact

- **One-time boot delay.** First boot generates the AI-PoW verifier setup
  (~15 minutes, logged: "Generating the AI-PoW verifier-setup table…"). Do not
  kill a node that appears "hung" during this step; subsequent boots load the
  cache in seconds. Ship the cache to skip it.
- **Verifier-setup RSS.** The DoS-safe default retains all 13 contexts after first
  use, requiring the measured setup-table RSS budget. Lower caps reduce RSS by
  paging contexts in and out. jemalloc is required (not optional).
- **Dual mining.** Two miner processes can attach: `zk-pow-mine` and
  `ai-pow-mine` (the latter with a self-contained `--canonical` CPU mode). Each
  receives its own candidate effect (`%mine-zk` / `%mine-ai`) and submits its own
  block; both re-target immediately on a new heaviest block.
- **Block cadence.** The AI puzzle targets a 250 s per-puzzle ideal and the ZK
  puzzle 375 s, so their block rates sum to `1/250 + 1/375 = 1/150` — a ~150 s
  combined cadence (Aletheia's target) split 60% AI / 40% ZK. If only one puzzle
  is active, that puzzle's ASERT converges it toward its own ideal (250 s or
  375 s).
- **AI proving cost.** A canonical CPU proof takes ~30 s; the node's
  candidate-update interval must exceed the prove time (fakenet:
  `--fakenet-update-candidate-interval-secs`). GPU-shaped provers are the
  intended mainnet path.
- **Monitoring.** Operators should watch for: the first `%ai-pow` block at/after
  114,300 becoming the AI ASERT anchor; per-puzzle target drift converging toward
  250 s (AI) and 375 s (ZK); the verifier-setup digest matching the pinned v0
  constant at boot; and the combined chain settling near ~150 s (≈60% AI / 40%
  ZK) once both puzzles produce blocks.

## Testing and Validation

### Rust — consensus verifier + Pearl compatibility (`crates/ai-pow/tests`, `crates/ai-pow-jets`, `crates/ai-pow-miner`)

- Positive/negative accept KATs: a real compact MoE block verifies; tampered
  commitment/target/artifact is rejected (`pearl_moe_*`, `end_to_end.rs`).
- Adversarial suites: `adversarial.rs`, `soundness_sim.rs`, `block_noise.rs`,
  `pearl_moe_fail_closed.rs`, `pearl_moe_routing_binding.rs`, decode-DoS bounds
  (`decode_dos.rs`), byte-equivalence to Pearl (`m52_unit_of_work_byte_equiv.rs`,
  `pearl_model_compat.rs`, `llm_shape.rs`).
- Merge-mining aux inclusion: exactly-one-tag enforcement + the
  one-PoW-two-commitments rejection (`pearl_merge_compat.rs`:
  `pearl_aux_inclusion_rejects_double_tag_n5`).
- Verify jet: panic-safety (`catch_unwind`), deterministic-`NO`-vs-`%fail`
  split, the pinned v0 setup digest, and corrupt-cache recovery
  (`ai-pow-jets`).

### Rust — consensus wiring (`crates/nockchain`)

- `tests/ai_pow_accept_e2e.rs`: boots the real dumb kernel in-process, drives
  genesis + born + a timer candidate, and admits a valid `%ai-pow` block through
  `do-pow → heard-block` via the jet — and rejects a wrong-commitment cert.
- CLI validation unit tests for the `--fakenet-{zk,ai}-asert-*` /
  `--fakenet-ai-pow-activation-height` flags (anchor-height invariants,
  all-or-nothing trio, `activation-height ≥ 1`).
- `nockchain-types`: `blockchain_constants_roundtrip_from_noun_for_mainnet_and_fakenet`
  pins the v1 encode/decode identity.

### Hoon — kernel consensus (`hoon/tests/dumb/mod/unit`)

- `ai-pow-jet.hoon`: the `%ai-pow-verify` jet fires and the fail-closed Hoon arm
  behaves.
- `dual-puzzle.hoon` + the tandem ASERT unit tests: the two puzzles retarget
  independently over their own subchains; equal-weight heaviness scales the AI
  contribution to match the ZK contribution.

### Live fakenet

A dual-miner fakenet smoke harness runs a node with both `zk-pow-mine` and
`ai-pow-mine --canonical`, advancing a single chain with both puzzles winning
heights and each block carrying its own per-puzzle ASERT target (zero errors
over multi-minute runs).

## Future Work

The following are tracked residuals, not part of this upgrade's consensus rules:

- **External ZK-circuit + recursion audit (`§1.1`)** — the one assumption every
  soundness verdict rests on; required before an adversarial mainnet.
- **Coinbase new-issuance split reversion.** Aletheia deferred a possible
  reversion of its coinbase split to a later upgrade after a fully useful PoW
  puzzle ships. Logos ships that puzzle but does not add a reversion trigger.
  Until a subsequent upgrade changes the rule, each block's newly issued
  coinbase reward continues to split 80/20.

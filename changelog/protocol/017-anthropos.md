+++
version = "0.1.17"
status = "draft"
consensus_critical = true

# Activation
activation_height = 147500

# Dates
published = "2026-09-14"
activation_target = ""

# People
authors = ["Nock Community Co"]
reviewers = ["@nockchain-core"]

supersedes = "0.1.16"
superseded_by = ""
+++

# Anthropos

## Summary

Anthropos activates ZK proof version `%5` at block height 147,500. Version `%5`
keeps full hardened STARK verification but moves the ZK nonce lottery to a
Tip5 digest of proof object `0`, then recalibrates dual-puzzle fork-choice work
and ASERT intervals for 70% AI-PoW / 30% ZK-PoW.

Proof version `%4` remains the AI-PoW artifact discriminator. It is not a ZK
proof version and its verifier, mining kernel, and proof encoding do not change.

## Motivation

ZK proof versions `%0` through `%3` derive their mining digest from all seven
proof objects. A miner must therefore regenerate the complete proof before it
can test another nonce. Version `%5` separates those jobs: generate and verify
the full proof once, then grind the block-bound `%puzzle` object with Tip5.

That changes the hardware unit represented by one ZK attempt. The Logos
exchange rate compared Pearl MAC throughput with complete ZK proof throughput;
Anthropos instead compares Pearl MAC throughput with public RTX 5090 Tip5
hash-grinding throughput. The block allocation also changes from Logos's 30%
AI / 70% ZK target to 70% AI / 30% ZK while retaining the approximately 150 s
combined cadence.

## Technical Specification

### Proof-version schedule

Consensus selects the ZK proof version from block height as follows:

```text
height < 6,750                 -> %0
6,750 <= height < 12,000      -> %1
12,000 <= height < 119,400    -> %2
119,400 <= height < 147,500   -> %3
height >= 147,500             -> %5
```

`%4` remains reserved for structured AI-PoW artifacts. ZK encoders, decoders,
miners, and verifiers reject `%4` rather than interpreting it as a ZK proof.

### Version `%5` proof and mining digest

Version `%5` uses the version `%3` AIR, preprocessing tables, seven-object proof
sequence, canonical encodings, hardened DEEP post-commitment equation, Merkle
authentication, and FRI verification.

The submitted `%puzzle` object retains its actual nonce, but the `%5` prover and
verifier replace that nonce with zero when evaluating the puzzle statement and
when absorbing object `0` into the Fiat-Shamir transcript. Consequently the
proof suffix is independent of the mining nonce and remains valid while a miner
changes only object `0`'s nonce. Block admission still verifies the complete
proof and returns the submitted block commitment and nonce.

The mining projection hashes the raw submitted object `0` and discards objects
`1` through `6`:

```text
object-0 = %puzzle [block-commitment actual-nonce len p]
pow-v5   = Tip5([leaf+%zkpow-v5 hash+hash-proof([%5 [object-0] ~ 0])])
```

The winning digest therefore remains bound to both the candidate block and the
actual nonce. Mutating the nonce changes `pow-v5` without invalidating the
reusable proof suffix. Mutating any suffix object does not change `pow-v5`, but
admission still rejects an invalid suffix. The block ID independently commits
to the complete submitted proof envelope under the `%zkblk-v5` domain, so
accepted suffix changes cannot create an unbound or identifier-malleable block.

### Height-gated cross-puzzle work

A block contributes expected work at its own target. ZK work remains
`(p^5 - 1)/(target+1)`. AI work is `2^256/(target+1)` MAC-equivalents divided by
the exchange rate selected for that block height.

Historical Logos blocks retain their original rate:

```text
height < 147,500: 25,750,000,000 MAC / complete-proof attempt
```

For version `%5`, the reference RTX 5090 rates are 400 TMAC/s for Pearl and
388.8 Mguess/s for post-hardfork Tip5 grinding. The latter is public
Neptune Cash/OXZD miner data: <https://useneptune.org/faq/#mining>.

```text
400,000,000,000,000 / 388,800,000 = 1,028,806.584...
height >= 147,500: 1,028,807 MAC / Tip5 hash
```

The quotient is rounded to the nearest integer. The height gate is mandatory:
replacing the Logos value retroactively would change historical accumulated
work and fork choice.

### 70% AI / 30% ZK ASERT allocation

At height 147,500, both per-puzzle ASERT lineages re-anchor to the median
timestamp of block 147,499 and reset their virtual counts. Their ideal
inter-block times become:

```text
AI-PoW: 214 seconds
ZK-PoW: 500 seconds
```

The steady-state shares are:

```text
AI = (1/214) / (1/214 + 1/500) = 70.028%
ZK = (1/500) / (1/214 + 1/500) = 29.972%
combined interval = 1 / (1/214 + 1/500) = 149.86 seconds
```

The version `%5` ASERT lanes use independent reference-network capacities:

```text
ZK-PoW: 2,000 RTX 5090s * 388,800,000 Tip5 hashes/s
      = 777,600,000,000 Tip5 hashes/s
AI-PoW: 10 ExaMAC/s
      = 10,000,000,000,000,000,000 MAC/s
```

The resulting consensus anchor targets are:

```text
ZK = floor((p^5 - 1) / (777,600,000,000 * 500))
   = 5493793810273389665851259858908398676672107719039465617368341183437285024349963580
AI = floor(2^256 / (10,000,000,000,000,000,000 * 214))
   = 54108452914633736179238778041442947594985974142822693476
```

After integer flooring, the anchors contribute 777,599,999,999 ZK and
9,719,996,073,121 AI normalized work units per second. These independent
capacity assumptions intentionally make the AI rate about 12.5 times the ZK
rate. The cross-puzzle conversion remains 1,028,807 MAC per Tip5 hash.

### Unchanged rules

Anthropos does not change AI-PoW `%4`, the STARK AIR, transaction or note
encoding, emissions, the 80/20 miner/protocol-fund split, or any block below
height 147,500.

## Activation

- **Height**: `147500`
- **Coordination**: Nodes and both miner types must deploy the Anthropos release
  before the activation boundary. ZK miners must emit `%5` proofs beginning with
  block 147,500; `%3` is rejected at and after that height.

The dynamic ASERT timestamp cache accepts block 147,499 directly as the anchor,
so a node upgraded while parked at the predecessor does not need to replay it.

## Migration

### Requirements

- Node and miner software containing protocol version `0.1.17`.
- ZK mining software capable of encoding `%5` and grinding the object-`0` Tip5
  digest.

### Configuration

None. Activation height, exchange rates, ideal intervals, and anchor targets are
consensus constants.

### Data Migration

None. The derived per-puzzle ASERT counters reset deterministically when block
147,500 is accepted. Existing blocks, proofs, accumulated-work values, and
transaction state are retained.

### Steps

1. Stop block production before height 147,500 if the node or miner has not been
   upgraded.
2. Deploy the Anthropos node release to validators and both miner types.
3. Confirm ZK miners advertise proof version `%5` for candidate height 147,500.
4. Monitor the first post-activation ZK and AI targets, proof rejection reasons,
   and observed per-puzzle cadence.

### Rollback

Do not roll back to a pre-Anthropos binary after accepting block 147,500. It
will derive `%3`, use the Logos exchange rate and ASERT schedule, and split from
the upgraded chain.

## Backward Compatibility

This is a height-gated consensus break. Old nodes reject `%5` ZK proofs and
compute different post-activation targets and accumulated work. Old ZK miners
continue producing `%3`, which upgraded nodes reject at height 147,500.

Pre-activation proofs and blocks remain valid under their historical version,
work exchange rate, and ASERT schedule. Wallet and transaction formats do not
change.

## Security Considerations

- Full proof verification remains mandatory. The shortened mining projection is
  not a shortened proof and does not bypass transcript, Merkle, DEEP, or FRI
  checks.
- Proof object `0` includes the block commitment and nonce. A winning digest
  cannot be transplanted to a different candidate block.
- The complete proof remains block-ID-bound under `%zkblk-v5`; proof suffixes
  cannot be changed without changing the block ID, and invalid suffixes fail
  verification.
- `%4` is not accepted by the ZK codec. This preserves an unambiguous wire-level
  distinction between AI-PoW and ZK-PoW.
- The old work rate remains active below height 147,500. Historical chainwork is
  therefore invariant under the upgrade.

## Operational Impact

ZK miners no longer regenerate a complete proof for each nonce attempt. They
should generate one valid nonce-independent proof envelope, repeatedly replace
object `0`'s nonce while grinding its raw Tip5 digest, and submit the complete
proof with the winning object. Pools must update job/version negotiation and
must not submit `%3` at or after activation.

Operators should monitor the first `%5` block, invalid-version and invalid-proof
rates, both ASERT targets, and convergence toward the 70% AI / 30% ZK target.
The combined target cadence remains approximately 150 seconds.

## Testing and Validation

- Hoon proof tests pin that changing object `1` leaves `%5` PoW unchanged,
  changing object `0` changes it, and full block hashing binds every object.
  The Roswell proving scenario additionally verifies one complete `%5` proof,
  replaces object `0`'s nonce, and verifies the same proof suffix again.
- Rust codec tests pin `%5` noun round trips and canonical array encoding.
- Rust verifier tests reject `%2` and `%3` nonce-bound transcripts relabelled as
  `%5`; transcript tests pin that `%5` Fiat-Shamir excludes the mining nonce.
- The reference-miner worker test submits two `%5` nonce attempts for one
  candidate and pins identical proof suffixes and transcript hashes.
- Consensus boundary tests pin `%3` immediately before height 147,500, `%5` at
  and after it, and `%4` as AI-only.
- Dual-puzzle tests pin the historical/new exchange-rate boundary, the new ASERT
  rows, requested capacity anchors, and their exact normalized work rates.

## Reference Implementation

- `hoon/common/ztd/five.hoon`
- `hoon/common/stark/prover.hoon`
- `hoon/common/stark/verifier.hoon`
- `hoon/common/pow.hoon`
- `hoon/apps/dumbnet/miner.hoon`
- `hoon/common/tx-engine.hoon`
- `hoon/apps/dumbnet/lib/consensus.hoon`
- `hoon/apps/dumbnet/lib/derived.hoon`
- `crates/zkvm-jetpack/src/form/proof.rs`
- `crates/zkvm-jetpack/src/form/tog.rs`
- `crates/zkvm-jetpack/src/form/verify.rs`
- `crates/zk-pow-miner/src/worker.rs`

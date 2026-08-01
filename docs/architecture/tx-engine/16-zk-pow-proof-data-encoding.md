# ZK PoW Proof Data, Representation, and Encoding

## Scope

This document specifies the current Nockchain ZK proof-of-work proof transcript: the proof envelope, its ordered `proof-data` objects, their noun representation, the packed polynomial format, and the consensus canonicality rule for `%poly` objects.

Hoon defines the consensus types and validation rules. Rust mirrors the noun codec for proof tooling, proof-stream windows, and verification.

## Proof Envelope

A complete proof is a four-field noun:

```hoon
[version objects hashes read-index]
```

| Field | Type | Meaning |
|---|---|---|
| `version` | `%0`, `%1`, `%2`, or `%3` | Proof version. |
| `objects` | `(list proof-data)` | Ordered transcript objects. |
| `hashes` | `(list noun-digest:tip5)` | Incremental transcript-hash cache. |
| `read-index` | `@` | Number of transcript objects consumed. |

A verifier begins with an empty hash cache and a zero read index. It consumes objects in order, hashes each consumed object, and derives Fiat-Shamir challenges from the consumed transcript. The object order, tag, and payload are therefore commitment-critical.

Proof-stream transport uses a window envelope:

```hoon
[format proof-version range objects context]
```

```hoon
range   [start=@ end=(unit @)]
context [total=@ digest=noun-digest:tip5]
```

A window contains a contiguous range of `objects`; `context` commits to the complete object sequence that the window belongs to. The assembler accepts format `0` only, requires a common version and context, requires windows to start at zero and remain contiguous, requires the assembled length to equal `context.total`, and rehashes the assembled proof to match `context.digest`.

## Proof Data Union

Every transcript object is a tagged noun. Tuple notation below is Hoon noun notation.

| Tag | Noun form | Payload |
|---|---|---|
| `%m-root` | `[%m-root p]` | Merkle root `p`, a Tip5 noun digest. |
| `%puzzle` | `[%puzzle commitment nonce len p]` | Block commitment, mining nonce, puzzle length, and the raw puzzle-result noun. |
| `%codeword` | `[%codeword p]` | An `fpoly` FRI codeword. |
| `%terms` | `[%terms p]` | A `bpoly` of terminal values. |
| `%m-path` | `[%m-path [leaf path]]` | An `fpoly` Merkle opening and its sibling digest list. |
| `%m-pathbf` | `[%m-pathbf [leaf path]]` | A `bpoly` Merkle opening and its sibling digest list. |
| `%comp-m` | `[%comp-m p num]` | Composition-codeword Merkle root and number of composition pieces. |
| `%evals` | `[%evals p]` | An `fpoly` of evaluations. |
| `%heights` | `[%heights p]` | A list of table-height exponents. |
| `%poly` | `[%poly p]` | A base-field polynomial (`bpoly`). |

The `%puzzle` payload preserves the puzzle noun itself. Rust derives flattened leaf and Dyck-word caches when decoding it; those caches are not separate wire fields.

Each object is converted to a typed, tag-domain-separated Tip5 hashable value before it enters the Fiat-Shamir transcript. The transcript commits to the semantic object representation rather than to a jam byte slice.

## Object Sequence

The prover emits the following fixed prefix:

| Index | Object | Role |
|---:|---|---|
| 0 | `%puzzle` | Block-bound puzzle statement. |
| 1 | `%heights` | Trace table heights. |
| 2 | `%m-root` | Base-trace commitment. |
| 3 | `%m-root` | Extension-trace commitment. |
| 4 | `%terms` | Terminal values. |
| 5 | `%poly` | Extra-composition polynomial. |
| 6 | `%evals` | Extra trace evaluations. |
| 7 | `%m-root` | Mega-extension commitment. |
| 8 | `%comp-m` | Composition commitment and piece count. |
| 9 | `%evals` | Trace evaluations at the DEEP point. |
| 10 | `%evals` | Composition-piece evaluations at the DEEP point. |

The PoW digest commits to objects `0` through `6`, inclusive. In particular, the `%poly` object is within the mined prefix. Versions `%0` through `%2` use the raw transcript-prefix digest for consensus compatibility. Version `%3` hashes that digest once more as `[leaf+%zkpow-v3 hash+prefix-digest]`, separating mining output from Fiat-Shamir output without adding codeword commitments to the mining loop.

### Version 3 post-commitment binding

Version `%3` retains the same proof-object sequence and the version `%2` AIR. It adds a verifier equation at the existing DEEP challenge `y`, which is sampled only after the mega-extension and composition Merkle roots (objects `7` and `8`) have been absorbed:

```text
C_extra(trace_evaluations_at_y, y) = P(y)
```

Object `9` supplies `trace_evaluations_at_y`; the subsequent DEEP/FRI checks bind those claims to the committed trace codewords. This second equation prevents a miner from changing `P`, repairing object `6` at its pre-commitment challenge, and reusing the expensive trace work.

The Merkle roots themselves deliberately remain outside the PoW prefix. Putting a codeword root directly into the mining digest would permit cheap root grinding by changing one received-word leaf and updating one Merkle path; a small number of spot checks is unlikely to query that leaf. Version `%3` instead makes any fixed, inconsistent `P` satisfy a fresh extension-field equation only with the polynomial-identity error probability, while preserving winner-only Merkle construction.

Let `r` be the FRI round count and `s` the FRI spot-check count. The suffix contains:

1. `r` `%m-root` FRI commitments, followed by one `%codeword` final FRI codeword.
2. `r × s` `%m-path` FRI openings.
3. For every spot check, four `%m-pathbf` openings in this order: base trace, extension trace, mega-extension trace, and composition pieces.

The verifier requires exactly

```text
12 + r + r × s + 4 × s
```

objects. This excludes omitted, reordered, duplicated, and trailing objects.

## Packed Polynomial Representation

Base and extension polynomials use a pair of a logical coefficient count and an indirect atom:

```hoon
bpoly  [len=@ dat=@ux]
fpoly  [len=@ dat=@ux]
```

`len` must fit in 32 bits.

### Base Polynomials

A `bpoly` stores `len` 64-bit base-field coefficients in increasing coefficient order:

```text
[c₀, c₁, …, cₙ₋₁] = c₀ + c₁x + … + cₙ₋₁xⁿ⁻¹
```

The least-significant 64-bit word of `dat` is `c₀`; each successive word contains the next coefficient. The next word is a sentinel with value one:

```text
dat = c₀ + c₁·2⁶⁴ + … + cₙ₋₁·2⁶⁴⁽ⁿ⁻¹⁾ + 2⁶⁴ⁿ
```

The sentinel forces the atom to retain exactly `len` coefficient words, including trailing coefficient words whose values are zero.

### Extension Polynomials

An `fpoly` stores `len` extension-field coefficients. Each coefficient occupies three consecutive 64-bit base-field limbs, so the sentinel follows `3 × len` data words:

```text
dat = Σᵢ Σⱼ eᵢ,ⱼ·2⁶⁴⁽³ⁱ⁺ʲ⁾ + 2¹⁹²ⁿ,  where 0 ≤ j < 3
```

The packed-array shape check is:

```text
word_count(dat) - 1 = step × len
```

where `step` is one for a `bpoly` and three for an `fpoly`. This establishes that `len` matches the packed buffer. Field-membership and proof-specific length checks are separate verifier obligations.

## Canonical `%poly` Encoding

A valid packed `bpoly` need not be a canonical polynomial representative: a polynomial can have arbitrary zero coefficients appended at its highest degrees.

`bpcan` canonicalizes a `bpoly` by decoding its coefficient list, removing trailing zero coefficients, and rebuilding the packed representation. The zero polynomial has the single-coefficient canonical representation `[0]`; it is not represented by an empty list.

Examples:

```text
[3, 7]       canonical
[3, 7, 0]    non-canonical representation of the same polynomial
[0]          canonical zero polynomial
```

At block heights below `112,500`, `canonical-pow-proof` permits all `%poly` encodings. At height `112,500` and later, it requires every `%poly` object in the proof to equal `bpcan` of its payload. The rule applies only to `%poly` proof objects; it does not impose polynomial-degree canonicality on `%terms` or `%m-pathbf` payloads.

Block validation evaluates the puzzle binding and this canonicality predicate before running the full ZK verifier.

## Source of Truth

- Hoon proof-data types and transcript hashing: `hoon/common/ztd/four.hoon`
- Hoon STARK proof producer and verifier: `hoon/common/stark/prover.hoon`, `hoon/common/stark/verifier.hoon`
- Hoon FRI producer: `hoon/common/ztd/six.hoon`
- Hoon packed polynomial and canonicalization definitions: `hoon/common/ztd/one.hoon`
- Consensus proof admission: `hoon/apps/dumbnet/inner.hoon`, `hoon/apps/dumbnet/lib/types.hoon`
- Rust noun codec and stream-window types: `crates/zkvm-jetpack/src/form/proof.rs`, `crates/nockchain-math/src/convert.rs`

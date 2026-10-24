# Schnorr Signatures over the Cheetah Curve

## Overview

Nockchain uses **Schnorr signatures** over the **Cheetah elliptic curve** — a STARK-friendly curve defined over a sextic extension of the Goldilocks prime field. The Cheetah curve was designed by Toposware specifically for efficient verification both natively and inside zero-knowledge proof circuits.

The Rust implementation is in `crates/lib/nockchain-math/src/crypto/cheetah.rs` (~455 lines). The Hoon types are authoritative in `tx-engine-1.hoon`, with Rust serialization mirrors in `crates/lib/nockchain-types/src/tx_engine/`.

## Why Not secp256k1 or Ed25519?

Bitcoin uses secp256k1 (ECDSA, later Schnorr via Taproot). Ethereum and many other chains use Ed25519 or secp256k1. These curves operate over 256-bit prime fields that have no special relationship to Nockchain's arithmetic.

Nockchain's entire proof system operates over the **Goldilocks field** (p = 2^64 − 2^32 + 1). For STARK compatibility, all cryptographic operations — including signature verification — must be efficiently expressible as constraints over this field. A standard 256-bit curve would require multi-limb arithmetic inside the STARK circuit, making signature verification prohibitively expensive.

The Cheetah curve solves this: its base field *is* Goldilocks, and its extension field arithmetic reduces to `u64` operations over the Goldilocks prime. This means Schnorr signature verification inside a STARK circuit requires only native field operations.

## Cheetah Curve Specification

The Cheetah curve comes from Toposware's research on elliptic curves over sextic extensions of small prime fields (ePrint 2022/277).

### Field Tower

| Layer | Definition | Meaning |
|---|---|---|
| Base field Fp | p = 2^64 − 2^32 + 1 | Goldilocks prime, fits in one `u64` |
| Extension Fp^6 | Fp[X]/(X^6 − 7) | Degree-6 extension, implemented as `F6lt = [Belt; 6]` |

The extension field is constructed as Fp^6 = Fp[u] where u^6 = 7. Each element is a 6-tuple of Goldilocks field elements.

### Curve Parameters

| Parameter | Value |
|---|---|
| Curve equation | E: y² = x³ + x + b, where b = 395 + u (see `++b` in `ztd/three.hoon:1472`) |
| Prime field | Fp, p = 2^64 − 2^32 + 1 (Goldilocks) |
| Coordinate field | Fp^6 = Fp[u]/(u^6 − 7), each point's (x, y) lives here |
| Group order | `0x7af2599b3b3f22d0563fbf0f990a37b5327aa72330157722d443623eaed4accf` (~255 bits) |
| Scalar field | Z/nZ where n = group order (~255 bits) |
| Security level | ~128 bits (resistant to Pollard-Rho, twist, MOV, cover, decomposition attacks) |
| Generator | `a-gen` / `A_GEN` (predefined constant in `ztd/three.hoon:1535` / `cheetah.rs:22`) |
| Identity | `a-id` / `A_ID` (point at infinity: `[f6-zero f6-one %.y]`) |

### Why Sextic Extension?

The degree-6 extension is a carefully chosen balance:

1. **64-bit base field**: Each base element fits in a single machine word → fast native arithmetic
2. **Large group order**: The ~255-bit subgroup provides ~128 bits of security (comparable to secp256k1)
3. **Attack resistance**: Degree 6 is small enough to resist cover and decomposition attacks specific to extension-field curves, while large enough for security
4. **STARK efficiency**: All field operations reduce to `u64` arithmetic over Goldilocks → efficient as STARK AIR constraints

## Point Representation

```rust
// crates/lib/nockchain-math/src/crypto/cheetah.rs:60-65
pub struct CheetahPoint {
    pub x: F6lt,    // x-coordinate in Fp^6
    pub y: F6lt,    // y-coordinate in Fp^6
    pub inf: bool,  // point at infinity flag
}

pub struct F6lt(pub [Belt; 6]);  // 6 Goldilocks field elements
```

A point on the Cheetah curve is represented by its (x, y) coordinates in the sextic extension field, plus an infinity flag. Each coordinate is 6 × 64 = 384 bits, so a full point is 768 bits + flag.

## Extension Field Arithmetic

The `F6lt` type supports full field arithmetic, implemented via Karatsuba-style multiplication:

| Operation | Function | Notes |
|---|---|---|
| Addition | `f6_add(a, b)` | Component-wise addition mod p |
| Negation | `f6_neg(a)` | Component-wise negation mod p |
| Subtraction | `f6_sub(a, b)` | `f6_add(a, f6_neg(b))` |
| Multiplication | `f6_mul(a, b)` | Karatsuba via `karat3` — reduces to three degree-3 multiplications |
| Squaring | `f6_square(a)` | Currently `f6_mul(a, a)` (TODO: dedicated Karatsuba-square) |
| Inversion | `f6_inv(a)` | Extended GCD over polynomial ring Fp[X]/(X^6 − 7) |
| Division | `f6_div(a, b)` | `f6_mul(a, f6_inv(b))` |
| Scalar mult | `f6_scal(s, a)` | Multiply each component by base field element |

The Karatsuba multiplication (`karat3`) is the key optimization: it multiplies two degree-2 polynomials using 3 component multiplications instead of the naive 9, then composes two `karat3` calls to handle the full degree-5 multiplication with reduction by X^6 − 7.

The reduction by the irreducible polynomial X^6 − 7 appears in `f6_mul` as additions of `Belt(7) * (...)` terms — when a product exceeds degree 5, the X^6 term is replaced by 7 (since X^6 ≡ 7 mod (X^6 − 7)).

## Point Arithmetic

| Operation | Function | Notes |
|---|---|---|
| Point addition | `ch_add(p, q)` | Handles identity, negation, and doubling cases |
| Point doubling | `ch_double(p)` | Optimized doubling using tangent slope |
| Point negation | `ch_neg(p)` | Negate y-coordinate |
| Scalar mult (u64) | `ch_scal(n, p)` | Binary method for 64-bit scalars |
| Scalar mult (big) | `ch_scal_big(n, p)` | Binary method for arbitrary-size integers (UBig) |

The addition formula for non-special cases:

```rust
// crates/lib/nockchain-math/src/crypto/cheetah.rs:262-271
pub fn ch_add_unsafe(p: CheetahPoint, q: CheetahPoint) -> Result<CheetahPoint, JetErr> {
    let slope = f6_div(&f6_sub(&p.y, &q.y), &f6_sub(&p.x, &q.x))?;
    let x_out = f6_sub(&f6_square(&slope), &f6_add(&p.x, &q.x));
    let y_out = f6_sub(&f6_mul(&slope, &f6_sub(&p.x, &x_out)), &p.y);
    Ok(CheetahPoint { x: x_out, y: y_out, inf: false })
}
```

The doubling formula uses the curve equation's derivative (3x² + a where a = 1 for Cheetah):

```rust
// crates/lib/nockchain-math/src/crypto/cheetah.rs:228-240
pub fn ch_double_unsafe(x: &F6lt, y: &F6lt) -> Result<CheetahPoint, JetErr> {
    let slope = f6_div(
        &f6_add(&f6_scal(Belt(3), &f6_square(x)), &F6_ONE),  // 3x² + 1
        &f6_scal(Belt(2), y),                                  // 2y
    )?;
    let x_out = f6_sub(&f6_square(&slope), &f6_scal(Belt(2), x));
    let y_out = f6_sub(&f6_mul(&slope, &f6_sub(x, &x_out)), y);
    Ok(CheetahPoint { x: x_out, y: y_out, inf: false })
}
```

## Curve Validation

```rust
// crates/lib/nockchain-math/src/crypto/cheetah.rs:114-121
pub fn in_curve(&self) -> bool {
    if *self == A_ID { return true; }
    let scaled = ch_scal_big(&G_ORDER, self)
        .expect("scalar multiplication should succeed");
    scaled == A_ID  // [n]G = ∞ iff G has order dividing n
}
```

Validation checks that `[G_ORDER]P = ∞` — the point has order dividing the group order. This is sufficient because the subgroup has prime order, so any non-identity point with this property is a valid group element.

## Schnorr Signature Scheme (Authoritative Hoon)

The Schnorr scheme is defined in `hoon/common/ztd/three.hoon` within the `++schnorr` core of the `++cheetah` library.

### Signing (`++sign`, `three.hoon:1628-1661`)

```hoon
++  sign
  |=  [sk-as-32-bit-belts=(list belt) m=noun-digest:tip5]
  ^-  [c=@ux s=@ux]
```

1. Derive public key: `pubkey = [sk]G`
2. Compute nonce deterministically: `nonce = trunc-g-order(hash-varlen(pubkey_x || pubkey_y || m || sk_belts))`
3. Compute commitment point: `R = [nonce]G`
4. Compute challenge: `chal = trunc-g-order(hash-varlen(R_x || R_y || pubkey_x || pubkey_y || m))`
5. Compute response: `sig = (nonce + chal · sk) mod g-order`
6. Return `[chal, sig]`

The nonce is derived deterministically from the secret key and message (like RFC 6979), avoiding the need for a random number generator.

### Verification (`++verify`, `three.hoon:1663-1686`)

```hoon
++  verify
  |=  [pubkey=a-pt:curve m=noun-digest:tip5 chal=@ux sig=@ux]
  ^-  ?
```

1. Check `0 < chal < g-order` and `0 < sig < g-order`
2. Recover commitment: `R = [sig]G − [chal]pubkey`
3. Recompute challenge: `chal' = trunc-g-order(hash-varlen(R_x || R_y || pubkey_x || pubkey_y || m))`
4. Accept if `chal == chal'`

This is a standard Schnorr verification: if `sig = nonce + chal·sk`, then `[sig]G − [chal]pubkey = [nonce]G + [chal·sk]G − [chal·sk]G = [nonce]G = R`, recovering the original commitment point.

### `trunc-g-order`: Hash-to-Scalar (`three.hoon:1695-1706`)

```hoon
++  trunc-g-order
  |=  a=(list belt)
  (mod (add (snag 0 a) (mul p (snag 1 a)) ...) g-order:curve)
```

Converts a Tip5 hash output (list of belts) into a scalar in `[0, g-order)` by interpreting the first 4 elements as a base-p number and reducing modulo the group order. This is the bridge between the hash function's Goldilocks output and the elliptic curve's scalar field.

### Batch Verification

```hoon
++  batch-verify
  |=  batch=(list [pubkey=a-pt:curve m=noun-digest:tip5 chal=@ux sig=@ux])
  (levy batch verify)
```

Currently verifies each signature independently. Schnorr's linearity enables future optimization via randomized batch verification.

## Schnorr Signature Types (Rust Serialization Mirror)

### SchnorrPubkey

```rust
// crates/lib/nockchain-types/src/tx_engine/common/mod.rs:15-16
pub struct SchnorrPubkey(pub CheetahPoint);
```

A public key is a point on the Cheetah curve. It derives `NounEncode`/`NounDecode` automatically from `CheetahPoint`.

### SchnorrSignature

```rust
// crates/lib/nockchain-types/src/tx_engine/common/mod.rs:30-34
pub struct SchnorrSignature {
    pub chal: [Belt; 8],  // Challenge scalar as 8 base field elements
    pub sig: [Belt; 8],   // Response scalar as 8 base field elements
}
```

In Hoon, challenge and response are raw `@ux` atoms. The Rust serialization represents each as 8 Goldilocks field elements (512 bits), which accommodates the ~255-bit group order. The `[Belt; 8]` encoding matches how Hoon nouns serialize large integers as sequences of 64-bit words.

### PkhSignatureEntry

```rust
// crates/lib/nockchain-types/src/tx_engine/v1/tx.rs:296-301
pub struct PkhSignatureEntry {
    pub hash: Hash,                   // The pubkey hash being satisfied
    pub pubkey: SchnorrPubkey,        // The actual public key
    pub signature: SchnorrSignature,  // Schnorr signature
}
```

When satisfying a `Pkh` lock primitive, the witness provides a `PkhSignatureEntry` for each signing key. The entry binds together:
1. The pubkey hash (committed in the lock)
2. The actual public key (revealed at spend time)
3. The Schnorr signature over the transaction's sig-hash

### Sig-Hash: What Gets Signed

The message signed by a Schnorr signature is the **sig-hash** of the transaction — a hash of the seeds (outputs) and fee, but *excluding* the witness data. This is the same SegWit-inspired design described in file 02: the witness does not sign itself, enabling witness malleability prevention.

## Key Encoding

Public keys use Base58 serialization:

```rust
// crates/lib/nockchain-math/src/crypto/cheetah.rs:67-81
const BYTES: usize = 97;  // 1 prefix + 12 Belt elements × 8 bytes each

pub fn into_base58(&self) -> Result<String, CheetahError> {
    let mut bytes = Vec::new();
    bytes.push(0x1);  // Prefix byte
    for belt in self.y.0.iter().rev().chain(self.x.0.iter().rev()) {
        bytes.extend_from_slice(&belt.0.to_be_bytes());
    }
    Ok(bs58::encode(bytes).into_string())
}
```

The encoding is: `[0x01 | y₅..y₀ | x₅..x₀]` in big-endian, producing a 97-byte value that encodes to a ~132-character Base58 string. The leading `0x01` byte serves as a version/format prefix.

Decoding includes `in_curve()` validation to reject points not on the curve.

## Signature Collection

The `Signature` type in the witness is a z-map (hash-ordered persistent tree) of `(SchnorrPubkey → SchnorrSignature)` pairs:

```rust
// crates/lib/nockchain-types/src/tx_engine/common/mod.rs:36-48
pub struct Signature(pub Vec<(SchnorrPubkey, SchnorrSignature)>);

impl NounEncode for Signature {
    fn to_noun<A: NounAllocator>(&self, stack: &mut A) -> Noun {
        self.0.iter().fold(D(0), |map, (pubkey, sig)| {
            let mut key = pubkey.to_noun(stack);
            let mut value = sig.to_noun(stack);
            zmap::z_map_put(stack, &map, &mut key, &mut value, &DefaultTipHasher)
                .expect("z-map put for signature should not fail")
        })
    }
}
```

Signatures are stored in a z-map keyed by public key, ensuring deterministic ordering regardless of the order signatures were collected. This is critical for consensus — the noun representation of the witness must be identical across all nodes.

## The `trunc_g_order` Function

```rust
// crates/lib/nockchain-math/src/crypto/cheetah.rs:332-339
pub fn trunc_g_order(a: &[u64]) -> UBig {
    let mut result = UBig::from(a[0]);
    result += &*P_BIG * UBig::from(a[1]);
    result += &*P_BIG_2 * UBig::from(a[2]);
    result += &*P_BIG_3 * UBig::from(a[3]);
    result % &*G_ORDER
}
```

This converts a 4-element array of `u64` values into a scalar modulo the group order, interpreting the array as a base-p number: `a[0] + a[1]·p + a[2]·p² + a[3]·p³ mod G_ORDER`. This is used to convert Tip5 hash outputs (which are in the Goldilocks field) into scalars suitable for elliptic curve operations.

## Also Available: Ed25519 Jets

The Nock VM also includes Ed25519 jets for non-consensus cryptographic operations:

| Jet | Location | Purpose |
|---|---|---|
| `jet_sign` | `crates/lib/nockvm/rust/nockvm/src/jets/lock/ed.rs` | Ed25519 signing |
| `jet_veri` | same | Ed25519 verification |
| `jet_puck` | same | Public key derivation |
| `jet_shar` | same | Shared secret (X25519) |

These are available in the VM for application-level cryptography (e.g., key derivation, off-chain signatures) but are not used in the consensus-critical transaction engine. The transaction engine exclusively uses Schnorr over Cheetah for on-chain authentication.

Additionally, `crates/lib/nockchain-math/src/crypto/argon2.rs` provides Argon2 key derivation — used for wallet password hashing, not consensus operations.

## Comparison with Bitcoin's Signature Evolution

| Aspect | Bitcoin (pre-Taproot) | Bitcoin (Taproot) | Nockchain |
|---|---|---|---|
| Signature scheme | ECDSA | Schnorr | Schnorr |
| Curve | secp256k1 (256-bit) | secp256k1 (256-bit) | Cheetah (Fp^6 over Goldilocks) |
| Key size | 33 bytes (compressed) | 32 bytes (x-only) | 97 bytes (full point) |
| Signature size | ~72 bytes (DER) | 64 bytes | 128 bytes (8+8 Belt elements) |
| Multisig | OP_CHECKMULTISIG (N pubkeys + N sigs) | MuSig2 (single aggregated key) | Native M-of-N via Pkh primitive |
| ZK-friendly | No | No | Yes (native field arithmetic in STARKs) |
| Batch verification | No | Yes (Schnorr linearity) | Possible (Schnorr linearity) |

### Key Design Differences

1. **Larger keys and signatures**: Cheetah operates over Fp^6, so points and scalars are larger than secp256k1. The trade-off is STARK-circuit efficiency.

2. **No x-only pubkeys**: Bitcoin Taproot uses x-only public keys (32 bytes) where the y-coordinate is implicitly even. Nockchain encodes the full (x, y) point, avoiding the parity ambiguity at the cost of larger keys.

3. **Hash-based key commitment**: Like Bitcoin P2PKH but unlike Taproot's key-path, Nockchain's Pkh stores `Hash(pubkey)` rather than the pubkey itself. The actual key is revealed only at spend time.

4. **Schnorr from inception**: Bitcoin migrated from ECDSA to Schnorr over a decade. Nockchain chose Schnorr from V1 inception, benefiting from linearity (enabling potential future MuSig-style aggregation) without legacy compatibility concerns.

# Goldilocks Field Arithmetic (ztd one and two)

## Overview

All cryptographic operations in Nockchain — hashing (Tip5), signatures (Cheetah/Schnorr), Merkle trees, STARK proofs — operate over the **Goldilocks prime field**: p = 2^64 − 2^32 + 1 = 18,446,744,069,414,584,321.

The foundational arithmetic is defined in two Hoon files:
- `hoon/common/ztd/one.hoon` (~1390 lines): base field types, operations, array utilities
- `hoon/common/ztd/two.hoon` (~1716 lines): extension field, polynomial arithmetic, NTT/FFT

With Rust mirrors in `crates/lib/nockchain-math/src/belt.rs`, `felt.rs`, `bpoly.rs`, and `poly.rs`.

## Why Goldilocks?

The choice of p = 2^64 − 2^32 + 1 is driven by four properties:

1. **Fits in a single `u64`**: Despite being a 64-bit prime, it's slightly less than 2^64, so every field element is a native machine word. No multi-limb arithmetic needed for base field operations.

2. **Efficient modular reduction**: The special form p = 2^64 − 2^32 + 1 means modular reduction can use shifts and additions instead of general-purpose division. For a product `a * b mod p`, the 128-bit result can be reduced via the identity `2^64 ≡ 2^32 − 1 (mod p)`.

3. **Large power-of-two subgroup**: The multiplicative group has order p − 1 = 2^64 − 2^32 = 2^32 · (2^32 − 1), containing a subgroup of order 2^32. This enables efficient NTT (Number Theoretic Transform) / FFT for polynomial operations — critical for STARK proof generation.

4. **Ecosystem adoption**: The same field is used by Plonky2 (Polygon), Polygon Miden, and other ZK systems, enabling shared tooling and research.

## ztd/one.hoon: Base Field Types and Operations

### Core Types

```hoon
+$  belt  @                     :: Base field element in [0, p)
+$  felt  @ux                   :: Extension field element (packed atom)
+$  melt  @                     :: Montgomery-space element
+$  bpoly  [len=@ dat=@ux]     :: Base field polynomial
+$  fpoly  [len=@ dat=@ux]     :: Extension field polynomial
+$  poly   (list @)             :: Coefficient list polynomial
+$  array  [len=@ dat=@ux]     :: Fixed-stride u64 array
+$  mary   [step=@ =array]     :: Multi-stride array (step = element width in u64 words)
```

**Belt** is the fundamental type — a single Goldilocks field element stored as a `u64` atom. All arithmetic ensures results stay in `[0, p)`.

**Felt** is a cubic extension field element — three `belt` values packed into a single atom with a high-bit marker. The extension is Fp[x]/(x³ − x − 1), so each felt represents `a₀ + a₁x + a₂x²` where the aᵢ are belts.

**Melt** is a belt in Montgomery representation (multiplied by R = 2^64 mod p), used for efficient repeated multiplication inside Tip5.

**Mary** is a strided array — a contiguous block of `u64` words where each element occupies `step` words. Used for polynomials, execution traces, and Merkle tree heaps.

### Base Field Operations

```hoon
++  badd   :: a + b mod p
++  bneg   :: p - a (additive inverse)
++  bsub   :: a - b mod p
++  bmul   :: a * b mod p (128-bit intermediate, then reduce)
++  bpow   :: a^n mod p (repeated squaring)
++  binv   :: a^(-1) mod p (Fermat: a^(p-2))
++  ordered-root  :: primitive root of unity of given order
```

Rust mirror (`crates/lib/nockchain-math/src/belt.rs`):

```rust
pub const PRIME: u64 = 18446744069414584321;

pub struct Belt(pub u64);

impl Add for Belt { ... }   // badd
impl Sub for Belt { ... }   // bsub
impl Mul for Belt { ... }   // bmul (via u128 intermediate)
impl Neg for Belt { ... }   // bneg
impl Belt {
    pub fn inv(&self) -> Belt { ... }  // binv
    pub fn pow(&self, exp: u64) -> Belt { ... }  // bpow
}
```

The Rust `Belt` type implements standard Rust operator traits, making field arithmetic look like natural integer arithmetic: `a + b`, `a * b`, `-a`.

### Montgomery Arithmetic

For Tip5's permutation (7 rounds of repeated field multiplications), Montgomery multiplication avoids per-operation modular division:

```hoon
++  montify        :: belt → melt: multiply by R mod p
++  mont-reduction :: melt → belt: multiply by R^(-1) mod p
++  montiply       :: a * b * R^(-1) mod p (single operation)
```

Montgomery multiplication `montiply(a, b)` computes `a · b · R^(-1) mod p` using only shifts and additions (no division), roughly 4× faster than standard modular multiplication for sequences of multiplications.

Rust constants:

```rust
pub const PRIME: u64 = 18446744069414584321;
const RP: u128 = 340282366841710300967557013911933812736;  // R·p
pub const R2: u128 = 18446744065119617025;                  // R² mod p
pub const H: u64 = 20033703337;                              // Montgomery reduction constant
pub const ORDER: u64 = 2_u64.pow(32);                       // Order of multiplicative subgroup
```

### Array Utilities (mary)

The `mary` type and its `ave` core provide array operations central to the STARK proof system:

| Arm | Purpose |
|---|---|
| `snag` | Index into array |
| `scag` | Take first N elements |
| `slag` | Drop first N elements |
| `weld` | Concatenate arrays |
| `flop` | Reverse array |
| `change-step` | Reinterpret element stride |
| `snag-as-bpoly` | Extract element as bpoly |
| `snag-as-fpoly` | Extract element as fpoly |
| `snag-as-mary` | Extract element as sub-mary |

These operate on packed binary data with stride-based indexing, providing efficient bulk data manipulation for polynomial evaluation tables and execution traces.

### Multivariate Polynomials

ztd/one also defines the `mp-mega` and `mp-graph` types for STARK constraint polynomials:

```hoon
+$  mp-mega  (map bpoly belt)    :: Sparse monomial map
+$  mp-graph                      :: Expression graph
  $%  [%con a=belt]               :: Constant
      [%var col=@]                :: Variable (column index)
      [%rnd t=term]               :: Random challenge
      [%add a=mp-graph b=mp-graph] :: Addition
      [%mul a=mp-graph b=mp-graph] :: Multiplication
      ...
  ==
```

The `mp-graph` type represents STARK constraint polynomials as expression trees, preserving semantic structure for efficient evaluation. Variables reference columns in execution trace tables, and random challenges come from the Fiat-Shamir transcript.

### Base58 Encoding

```hoon
++  en-base58  :: atom → Base58 string
++  de-base58  :: Base58 string → atom
```

Used for human-readable encoding of hashes, public keys, and addresses throughout the system.

## ztd/two.hoon: Extension Field and Polynomial Math

ztd/two imports ztd/one and builds the extension field and polynomial infrastructure needed for the STARK proof system.

### Extension Field Fp³

```hoon
|_  deg=_`@`3  :: Extension degree (default 3)
```

The extension field is Fp[x]/(x³ − x − 1), configured by the `deg-to-irp` map:

```hoon
++  deg-to-irp
  %-  ~(gas by *(map @ bpoly))
  :~  [1 (init-bpoly ~[0 1])]                          :: x
      [2 (init-bpoly ~[2 p-1 1])]                      :: x² + (p-1)x + 2
      [3 (init-bpoly ~[1 p-1 0 1])]                    :: x³ − x − 1 → x³ + (p-1)x + 1
  ==
```

The irreducible polynomial is x³ − x − 1 (stored as `[1, p-1, 0, 1]` using the additive inverse of 1 since we're in Fp).

### Extension Field Operations

| Arm | Purpose |
|---|---|
| `fadd` | Addition (component-wise mod p) |
| `fneg` | Negation (component-wise) |
| `fsub` | Subtraction |
| `fmul` | Multiplication with reduction by irreducible poly |
| `finv` | Inversion via extended GCD |
| `fdiv` | Division (`fmul(a, finv(b))`) |
| `fpow` | Exponentiation (repeated squaring) |
| `lift` | Belt → Felt (embed base element) |
| `drop` | Felt → Belt (project to base element) |
| `frip` | Felt → list of Belts (decompose) |
| `frep` | List of Belts → Felt (compose) |

The Rust mirror in `crates/lib/nockchain-math/src/felt.rs` provides equivalent operations. Note that the Cheetah curve uses a *different* extension — Fp^6 = Fp[X]/(X^6 − 7) — implemented directly in `cheetah.rs` rather than through the general felt machinery.

### Polynomial Arithmetic

ztd/two provides full polynomial arithmetic over both base and extension fields:

| Category | Arms |
|---|---|
| **Basic** | `fpadd`, `fpsub`, `fpmul`, `fpdiv`, `fpmod`, `fpscal` |
| **Evaluation** | `fpeval` (evaluate polynomial at a point) |
| **Interpolation** | `interpolate`, `intercosate` (cosine-domain interpolation) |
| **NTT/FFT** | `fp-ntt`, `bp-ntt`, `fp-fft`, `fp-ifft` |
| **Domain** | `zerofier`, `coseword` (evaluation domains) |
| **GCD** | `fpgcd`, `bpegcd` (extended GCD for inversion) |

### NTT/FFT

The Number Theoretic Transform is the finite-field analogue of FFT:

```hoon
++  fp-ntt   :: Forward NTT over extension field polynomials
++  bp-ntt   :: Forward NTT over base field polynomials
++  fp-fft   :: Forward FFT
++  fp-ifft  :: Inverse FFT
```

NTT converts between coefficient and evaluation representations of polynomials in O(n log n) field operations. This is critical for STARK proof generation:
- Polynomial multiplication via NTT: O(n log n) instead of O(n²)
- Low-degree extension (LDE): evaluate constraint polynomials on larger domains
- FRI queries: evaluate committed polynomials at specific points

The Goldilocks field's 2^32-order subgroup enables NTT up to length 2^32, sufficient for practical STARK proof sizes.

### u32 Arithmetic

```hoon
++  u32-add  :: 32-bit addition with overflow check
++  u32-sub  :: 32-bit subtraction
++  u32-mul  :: 32-bit multiplication
++  u32-dvr  :: 32-bit division with remainder
```

Used for the Tip5 S-box lookup table addressing, where bytes are manipulated as 32-bit values.

### Bignum Arithmetic

```hoon
++  chunk   :: Split atom into multi-word representation
++  merge   :: Combine multi-word back to atom
++  valid   :: Validate bignum representation
```

Used for operations that exceed a single field element, such as scalar multiplication on the Cheetah curve.

### Shape Utilities

```hoon
++  shape
  ++  grow            :: Expand a Dyck word
  ++  leaf-sequence   :: Extract leaf atoms from a noun tree
  ++  num-of-leaves   :: Count leaves in a noun tree
```

These tree structure utilities are used for noun hashing (extracting the leaf sequence and Dyck word from a noun for Tip5 `hash-noun-varlen`) and for note-data size validation (counting leaves to enforce `max-size` limits).

## Rust Mirror Architecture

| Hoon | Rust File | Key Types/Functions |
|---|---|---|
| Belt operations | `crates/lib/nockchain-math/src/belt.rs` | `Belt`, `PRIME`, Add/Sub/Mul/Neg impls, `inv()`, `pow()` |
| Felt operations | `crates/lib/nockchain-math/src/felt.rs` | `Felt`, Karatsuba multiplication |
| Polynomial ops | `crates/lib/nockchain-math/src/poly.rs`, `bpoly.rs` | `bpoly_to_vec`, `init_bpoly`, polynomial arithmetic |
| Mary/array | `crates/lib/nockchain-math/src/structs.rs` | Array manipulation utilities |

The Rust implementations are jets — they accelerate the Hoon computations while maintaining bit-exact compatibility. The Hoon definitions remain authoritative.

## Usage in the Transaction Engine

The field arithmetic from ztd/one and ztd/two underpins every cryptographic operation in the tx-engine:

| Operation | Types Used | Source |
|---|---|---|
| Tip5 hashing | Belt (field elements), Melt (Montgomery form) | ztd/one |
| Z-map/z-set ordering | Belt (hash digest comparison) | ztd/one |
| Schnorr signatures | Belt (scalars), F6lt (Cheetah point coords) | ztd/one + cheetah.rs |
| Merkle tree hashing | Belt (Tip5 digests) | ztd/one |
| Note-data size limits | Shape utilities (num-of-leaves) | ztd/two |
| STARK proofs | Felt, Bpoly, Fpoly, Mary, NTT/FFT | ztd/one + ztd/two |
| Transaction ID computation | Belt (hash output) | ztd/one |
| Base58 addresses | Belt → Base58 encoding | ztd/one |

## Comparison with Other Blockchain Fields

| Blockchain | Field | Size | Special Properties |
|---|---|---|---|
| Bitcoin | Binary (SHA-256) | 256-bit | No algebraic structure |
| Ethereum | Binary (Keccak) | 256-bit | No algebraic structure |
| Mina (Poseidon) | Pasta curves (Fp, Fq) | 255-bit | SNARK-friendly |
| Polygon Miden | Goldilocks (2^64 − 2^32 + 1) | 64-bit | Same field as Nockchain |
| Nockchain | Goldilocks (2^64 − 2^32 + 1) | 64-bit | STARK-friendly, native u64 |

The Goldilocks field is becoming a standard in the ZK space. Its 64-bit size trades off raw security margin (compensated by the cubic extension for signatures and the sponge capacity for hashing) for dramatic improvements in computational efficiency — every field operation is a single machine instruction rather than multi-limb arithmetic.

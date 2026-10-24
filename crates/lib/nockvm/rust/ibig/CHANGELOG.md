# Changelog

This project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## 0.3.6 - 2022-09-18

### Features
* GCD, greatest common divisor.
* Extended GCD (GCD with Bézout coefficients).
* Modular inverse.
* Modular division.

## 0.3.5 - 2022-03-12

### Bugfixes
* Corrected too-strict lifetimes in modular exponentiation.

### Dependencies
* Removed the dependency on `const_fn_assert`.

## 0.3.4 - 2021-11-03

### Features
* Optional `serde` support for `UBig` and `IBig`.

### Toolchain
* Rust 1.49+ is now required.

### Dependencies
* Added an optional dependency on `serde`.

## 0.3.3 - 2021-10-28

### Features
* Mixed-type arithmetic with primitive integer types.

  Allows `x + 1` instead of `x + ubig!(1)`.
  
  This breaks with the convention that arithmetic operators require same type on both sides. A better alternative would be user-defined custom integer literals, so that `1` could be inferred to have type `UBig`. But Rust does not support this yet. So this is a workaround for the sake of ergonomics.

## 0.3.2 - 2021-05-02

### Toolchain
* Rust 1.47+ is now supported.

### Dependencies
* Added a dependency on `cfg-if`.

## 0.3.1 - 2021-04-03

### Features
* Maximum supported length in bits: `UBig::MAX_BIT_LEN`.

### Fixes
* Broken build for `aarch64`, `mips64` and` powerpc64` fixed.

### Dependencies
* Added a dependency on `const_fn_assert`.

## 0.3.0 - 2021-03-29

### Breaking changes
* Removed `prelude`.
* Split into modules:
  * Moved `InRadix` to `fmt`.
  * Moved operator traits to `ops`.
  * Moved errors to `error`.
  * Moved distributions to `rand`.
* Removed deprecated `IBig::is_positive`, `IBig::is_negative`.
  Just compare with `ibig!(0)` instead.
* Shift left and right now only accepts `usize` for the number of bits, for consistency
  with other bit addressing operations and exponents.

## 0.2.2 - 2021-03-28

### Features
* Modular arithmetic: `ModuloRing`, `Modulo`.
* Conversions to floating point: `to_f32`, `to_f64`. Rounds to nearest, breaking ties to even.
* `From<bool>` for `IBig`.

## 0.2.1 - 2021-03-14

### License

* Loosened the license to either MIT or Apache-2.0.

### Features
* Implemented num-traits traits.

### Deprecated features
* `IBig::is_positive`, `IBig::is_negative`. Just use `> ibig!(0)`, `< ibig!(0)`.

### Dependencies
* Added optional dependency on `num-traits 0.2.14`.
* Removed dependency on `ascii`.

## 0.2.0 - 2021-03-11

### Removed features
* Removed deprecated functions `to_str_radix`, `to_str_radix_uppercase`, `ilog2`.

## 0.1.2 - 2021-03-09

### New features
* `bit_len`

### Deprecated features
* `to_str_radix`, `to_str_radix_uppercase`. Use `in_radix(...)` instead.
* `ilog2`. Use `bit_len` instead.

### Dependencies
* Added a dependency on `static_assertions 1.1`.
* Bumped `rand` to `0.8.3`.

### Performance
* Large division improved. Now uses a divide and conquer algorithm, O(n^1.47).
* Large `parse` improved using a divide and conquer algorithm, O(n^1.47).
* Large `to_string` improved using a divide and conquer algorithm, O(n^1.47).
* Other minor performance improvements.

## 0.1.1 - 2021-03-03

### New features
* Hashing.
* Exponentiation.
* Random sampling (optional dependency on `rand 0.8`).

### Performance
* Multiplication improved, now uses Karatsuba and Toom-Cook-3 algorithms, O(n^1.47).

### Examples
* `factorial` prints 1000000! in hexadecimal.

## 0.1.0 - 2021-02-25

The initial usable version.

### Features
* All basic arithmetic and bitwise operations.
* Parsing and formatting.
* Constructor macros.

### Performance
* Operations on very large numbers are still slow.

# Native Hoon Parser (`hatch`)

Rust parser for Hoon source. It turns source text into the typed AST in `src/ast/hoon.rs`, can emit JSON for inspection, and is used by the native compiler parity path.

For parser-only notes, see `../../docs/native-parser/README.md`. Parser behavior that affects native compiler artifact parity is documented with the compiler notes in `../../../docs/native-compiler/`.

## High-level architecture

The parser is a direct parser-combinator implementation built on `chumsky`. There is no separate lexer pass: rune parsers consume source text, whitespace, comments, atoms, cords, wings, and nested expressions directly.

Important source areas:

| Path | Role |
| --- | --- |
| `src/main.rs` | CLI plus the top-level recursive parser builders. |
| `src/lib.rs` | Library wrapper exposing `native_parser`. |
| `src/ast/hoon.rs` | Serializable AST definitions for Hoon, specs, skins, types, nouns, nock, wings, and source spots. |
| `src/runes/*.rs` | Per-rune-family parsers (`bar`, `buc`, `cen`, `col`, `dot`, `fas`, `ket`, `mic`, `sail`, `sig`, `tis`, `wut`, `zap`). |
| `src/utils.rs` | Shared token/atom/cord parsing, source maps, doc handling, spot generation, AST↔noun conversion, and tests. |

## Parse flow

At a high level, parsing a file does this:

1. Read source text.
2. Build a `LineMap`, which maps byte spans to Hoon source spots.
3. Build mutually-recursive parsers for tall and wide Hoon forms.
4. Dispatch rune families by leading rune character.
5. Parse constants, wings, specs, skins, nock literals, Sail/XML, and irregular syntax through shared helpers.
6. Attach optional `dbug` source spots and docs.
7. Return a `Hoon` AST, or serialize it to JSON/jam for tooling.

The top-level parser handles both tall and wide syntax. Rune-family modules usually expose separate helpers for wide and tall forms, while `src/main.rs` assembles them into the recursive grammar.

## AST model

The main AST is `Hoon`. Related syntax families have separate enums:

- `Spec` for molds/specs.
- `Skin` for pattern/skin syntax.
- `Type`, `Coil`, `Garb`, `Tome`, and related types for parsed type nouns.
- `Nock` for parsed nock formulas.
- `NounExpr` and `ParsedAtom` for source-level nouns and arbitrary-size atoms.
- `Spot` and `Pint` for source path and line/column ranges.

`ParsedAtom` stores small atoms inline and uses `BigUint` when an atom does not fit in the small representation. This matters for Hoon syntax that can contain large numeric atoms, including large axes in nock-like positions.

## Source spots and `dbug`

When `dbug` is enabled, parsed Hoon, spec, and skin nodes can be wrapped with source locations. These locations are not just presentation data: when the parser feeds the native compiler, spot choices can affect byte-for-byte artifact parity.

`LineMap` is responsible for translating spans into Hoon-style spots and for the small amount of canonical range expansion needed around comments and docs. Keep changes to this logic structural and covered by tests. If a parser spot change is made for compiler parity, document it under `../../../docs/native-compiler/` rather than as parser-only behavior.

## Import handling

This crate parses source text and directory inputs for the CLI. The native compiler has its own import resolver in `open/crates/honk/src/pipeline.rs` that uses this parser for individual files, resolves Hoon imports, detects cycles, and assembles the ASTs needed by the compiler build path.

## Build

From the repository root:

```bash
cargo build --release -p hatch
```

The binary is written to:

```text
target/release/hatch
```

## Basic usage

Parse a Hoon file to JSON:

```bash
target/release/hatch file_to_parse.hoon --out out.json
```

Parse a directory:

```bash
target/release/hatch /path/to/hoon-dir --out out.json
```

Print to stdout when `--out` is omitted:

```bash
target/release/hatch file_to_parse.hoon
```

Disable `dbug` source spots:

```bash
target/release/hatch --no-dbug file_to_parse.hoon
```

## Tests

Run parser tests in release mode:

```bash
cargo test --release -p hatch --lib
```

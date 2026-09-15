# Phase 0: PATH/COMMAND RECONCILIATION (RT-18)

**Status:** HISTORICAL RECONCILIATION RECORD; decisions and commands updated
after implementation on 2026-08-06.

## Overview

This document reconciles stale `open/` paths and outdated commands in the native-compiler documentation against the current repository checkout. The migration plan and supporting docs were written relative to an external `open/` tree; this checkout uses `crates/{honk,hatch,hoonc}`, `hoon/common/hoon.hoon`, and justfile recipes. All stale references have been catalogued with corrected in-repo equivalents.

---

## Historical Stale Path Inventory

| Stale Reference | File:Line | In-Repo Equivalent | Type |
|---|---|---|---|
| `open/crates/honk/` | README.md:11 | `crates/honk/` | directory |
| `open/crates/hoonc/hoon/hoon-138.hoon` | README.md:12 | `crates/hoonc/hoon/hoon-138.hoon` | file |
| `open/crates/hatch/` | README.md:13 | `crates/hatch/` | directory |
| `open/hoon/common/hoon.hoon` | README.md:14 | `hoon/common/hoon.hoon` | file |
| `open/docs/native-parser/` | README.md:5 | `docs/native-parser/` (if exists) | directory reference |
| `open/crates/hoonc/hoon/hoon-138.hoon` | artifact-parity.md:9 | `crates/hoonc/hoon/hoon-138.hoon` | file |
| `open/hoon/common/hoon.hoon` | artifact-parity.md:10 | `hoon/common/hoon.hoon` | file |
| `open/crates/honk/test-assets/BUILD.bazel` | artifact-parity.md:11 | `crates/honk/test-assets/` (check existence) | directory |
| `open/crates/honk/test-assets/compiler_artifact_parity_test.sh` | artifact-parity.md:12 | `crates/honk/test-assets/compiler_artifact_parity_test.sh` | file |
| `open/crates/hoonc/hoon/hoon-138.hoon` | artifact-parity.md:14 | `crates/hoonc/hoon/hoon-138.hoon` | file |
| `open/hoon/common/hoon.hoon` | artifact-parity.md:14 | `hoon/common/hoon.hoon` | file |
| `open/crates/honk/assets/hoonc-octs-type-138.jam` | README.md:44 | `crates/honk/assets/hoonc-octs-type-138.jam` | file |
| `open/crates/hatch/src/utils.rs` | source-spots.md:25 | `crates/hatch/src/utils.rs` | file |
| `open/crates/hoonc/hoon/hoon-138.hoon` | performance.md:26 | `crates/hoonc/hoon/hoon-138.hoon` | file |

---

## Corrected Bazel Targets

| Historical stale target | Current target | Status |
|---|---|---|
| `//open/crates/honk/test-assets:hoon_138_arbitrary_parity_test` | `//crates/honk/test-assets:hoon_138_arbitrary_parity_test` | strict byte-parity target |
| `//open/assets/native:kernel_parity_test` | `//assets/native:kernel_parity_test` | six strict byte-parity targets |

The current targets exist in this checkout and use `cmp`; there is no
directory-hash waiver in either Bazel acceptance gate.

---

## Canonical Current Commands for Implementers

### Build Commands

**Build honk binary (native compiler):**
```bash
cargo build --release -p honk
# or via Bazel:
bazel build //crates/honk:honk
```

**Build hatch (parser):**
```bash
cargo build --release -p hatch
# or via Bazel:
bazel build //crates/hatch
```

**Build hoonc (canonical reference compiler):**
```bash
cargo build --release -p hoonc
# or via Bazel:
bazel build //crates/hoonc
```

---

### Parity Test Commands

#### 1. Single-File Compile (arbitrary-mode hoon-138)

**Test:** Does honk's native mint of the hoon-138 prelude byte-match hoonc's?

**Cargo:**
```bash
just honk-138-parity
```

**Bazel:**
```bash
bazel test //crates/honk/test-assets:hoon_138_arbitrary_parity_test
```

**What it does:**
- Invokes `crates/honk/test-assets/honk_138_native_parity.sh`
- Compiles `hoon/common/hoon.hoon` (= `crates/hoonc/hoon/hoon-138.hoon`) with both compilers
- Compares artifacts with strict `cmp` (byte-exact)
- Runs under RSS guard (75% physical RAM ceiling) to catch memory blowup safely
- **Current status:** the native mint completes under the RSS guard and must be
  byte-identical to hoonc; the pre-arena OOM recorded on 2026-06-14 is resolved

**Reference command:**
```bash
# Manual invocation (if not using justfile):
HONK_NATIVE_PARITY=1 cargo run --release -p honk -- \
  --arbitrary --output /tmp/native.jam \
  --prelude hoon/common/hoon.hoon \
  hoon/common/hoon.hoon hoon

cargo run --release -p hoonc -- \
  --new --arbitrary --output /tmp/ref.jam \
  hoon/common/hoon.hoon hoon

cmp /tmp/native.jam /tmp/ref.jam
```

---

#### 2. Kernel Parity (Six reference kernels)

**Test:** Does each honk-compiled kernel byte-match the hoonc reference?

**Cargo workflow:**
```bash
# 1. Build the compilers and reference kernels (hoonc-built)
just build
just build-kernel-assets

# 2. Build kernels natively with honk
just honk-kernel-jams

# 3. Compare with diagnostic structural diff (dir-hash tolerant)
just honk-parity
```

**Authoritative Bazel gate:**
```bash
bazel test //assets/native:kernel_parity_test
```

**Kernels tested:**
- dumb (Dumbnet outer kernel)
- wal (Wallet kernel)
- miner (Dumbnet miner kernel)
- peek (Peek kernel)
- bridge (Bridge kernel)
- roswell (Roswell main app)

**Gate semantics:** each Bazel test uses `cmp` on artifacts built from identical
declared dependency trees. `jam-diff --kernel-parity` remains a local diagnostic
tool, not the acceptance oracle.

---

#### 3. Full Corpus Parity (Compiler mint tests)

**Test:** Do compiler unit tests pass on both paths?

**Cargo:**
```bash
# honk path
cargo test --release -p honk

# hatch parser (including source-spot tests)
cargo test --release -p hatch --lib

# hoonc canonical reference
cargo build --release -p hoonc
```

---

### Performance Gate: Roswell <60s

**Test:** Does the roswell kernel compile to native artifact in under 60 seconds (honk binary build excluded)?

**Cargo:**
```bash
just honk-roswell-timed
```

**Manual equivalent:**
```bash
cargo build --release -p honk
start=$(date +%s)
target/release/honk --new --output assets/native/roswell.jam \
  --prelude hoon/common/hoon.hoon \
  hoon/apps/roswell/roswell.hoon hoon
end=$(date +%s)
elapsed=$((end - start))
echo "roswell native compile: ${elapsed}s"
test "$elapsed" -lt 60 && echo "PASS" || echo "FAIL"
```

**Gate status:** an isolated post-rebase production build completed in 63.71 s
on an Apple M5 Max. The historical 71–76 s result is obsolete, but the `<60s`
target remains an open near miss.

---

## Documentation Classification

### Documents with corrected `open/` references

1. **docs/native-compiler/README.md** (lines 3–14, 28–34, 44)
   - Context: Open-tree policy and primary code references
   - Correction: Replace all `open/` prefixes with direct crate/hoon paths

2. **docs/native-compiler/artifact-parity.md** (lines 7–27)
   - Context: Target files and test invocation
   - Correction: Replace `open/` paths with in-repo equivalents; note that Bazel targets do not exist

3. **docs/native-compiler/source-spots.md** (lines 25, 33)
   - Context: Code reference and test invocation
   - Correction: Update path and note that the test is a bash script, not a Bazel target

4. **docs/native-compiler/performance.md** (line 26)
   - Context: Workload description
   - Correction: Update `open/` path; note it is the same as `hoon/common/hoon.hoon`

---

### Documents that historically referenced exported-tree targets

1. **docs/native-compiler/NATIVE-TYPES-MIGRATION-RT.md** (section RT-18, line 117–119)
   - Finding: Acknowledges the stale `open/` reference problem explicitly
   - Status: this document records the Phase-0 resolution

2. **docs/native-compiler/NATIVE-TYPES-MIGRATION.md** (section 9, lines 604–608)
   - Context: Branch hygiene section requiring reconciliation
   - Status: Addressed by this document

3. **docs/native-compiler/TODOS.md** (line 18)
   - Context: References `just honk-parity` which exists but uses a tolerant gate (RT-02)
   - Status: Noted below in RT-02 decision

---

## Chunked Mint Classification (RT-18 resolved)

**Current status:** PRODUCTIZED for the `HONK_NATIVE_PARITY=1` self-mint route.

- **Where:** `crates/honk/src/bin/honk.rs` lines 2603–2617 (canonical prelude routing when peeled root is `=<`)
- **Parity configuration:** the prelude is intentionally parsed with
  `dbug=false`, and the whole hoon-138 artifact is byte-exact against hoonc.
- **Remaining limitation:** a future caller enabling prelude `dbug` must
  preserve the peeled `Dbug`/`Note` wrappers.

The rejected delete/quarantine options remain documented in
`PHASE0-CHUNKED-DECISION.md`; `NATIVE_HOON_NO_CHUNK=1` retains the monolithic
diagnostic path.

---

## RT-02 Decision: Parity Gate Strictness (resolved)

**Finding:** `just honk-parity` currently passes byte equality OR a dir-hash-only difference, while the migration plan requires byte-for-byte artifact equality.

**Current Bazel acceptance behavior:**
```bash
bazel test //crates/honk/test-assets:hoon_138_arbitrary_parity_test
bazel test //assets/native:kernel_parity_test
# Both compare with cmp and reject every byte difference.
```

`jam-diff --kernel-parity` is still useful for localizing a failure, but it does
not decide acceptance. A legitimate environment-dependent difference would
require a named waiver rather than a silent pass.

---

## Commands Summary Table

| Task | Cargo | Bazel | Status |
|---|---|---|---|
| **Build honk binary** | `cargo build --release -p honk` | `bazel build //crates/honk:honk` | canonical |
| **Build hatch parser** | `cargo build --release -p hatch` | `bazel build //crates/hatch` | canonical |
| **Build hoonc (reference)** | `cargo build --release -p hoonc` | `bazel build //crates/hoonc` | canonical |
| **Arbitrary hoon-138 parity** | `just honk-138-parity` | `bazel test //crates/honk/test-assets:hoon_138_arbitrary_parity_test` | strict byte parity; RSS ceiling in local script |
| **Kernel parity (dumb only)** | direct native build + `cmp` | `bazel test //assets/native:dumb_parity_test` | strict byte parity |
| **All six kernel parity** | local build + diagnostics | `bazel test //assets/native:kernel_parity_test` | strict byte parity |
| **Roswell timing gate** | `just honk-roswell-timed` | `bazel test //assets/native:roswell_compile_bench` | manual benchmark; 63.71 s isolated post-rebase |
| **Unit tests (honk)** | `cargo test --release -p honk` | — | canonical Cargo suite |
| **Unit tests (hatch)** | `cargo test --release -p hatch --lib` | — | canonical Cargo suite |

---

## File Existence Verification

| Path | Exists | Notes |
|---|---|---|
| `crates/honk/` | ✓ | primary native compiler |
| `crates/hatch/` | ✓ | Hoon parser |
| `crates/hoonc/` | ✓ | canonical reference compiler (Hoon source, Rust wrapper) |
| `hoon/common/hoon.hoon` | ✓ | prelude (symlinked to `crates/hoonc/hoon/hoon-138.hoon` in some contexts) |
| `crates/honk/assets/hoonc-octs-type-138.jam` | ✓ | canonical `$octs` type for data imports |
| `crates/honk/test-assets/` | ✓ | contains parity test scripts |
| `crates/honk/test-assets/compiler_artifact_parity_test.sh` | ✓ | single-file compile parity |
| `crates/honk/test-assets/honk_138_native_parity.sh` | ✓ | hoon-138 arbitrary parity (with memory guard) |
| `crates/honk/test-assets/wrapper_asset_parity_test.sh` | ✓ | wrapper asset parity |
| `assets/native/kernel_parity_test.sh` | ✓ | kernel parity harness (bash script) |
| `docs/native-parser/` | ? | external tree reference; not in this checkout |

---

## Documentation Update Outcome

The active README, parity, source-spot, and performance documents now use
repository-relative paths and real Bazel targets. Strict acceptance and
diagnostic structural comparison are separate, and the chunked self-mint is
classified. The stale-path table above is retained only as review provenance.

---

## References

- **NATIVE-TYPES-MIGRATION.md § 2.2:** Parity gate strictness (RT-02)
- **NATIVE-TYPES-MIGRATION.md § 3.10:** Non-final noun-boundary matrix (RT-10)
- **NATIVE-TYPES-MIGRATION.md § 9:** Branch hygiene (RT-18, chunked decision)
- **NATIVE-TYPES-MIGRATION-RT.md RT-18:** Stale docs/commands finding
- **TODOS.md:** Resolved items section lists H0 kernel-parity harness
- **TODOS-PERF.md:** Current native compiler performance work
- **justfile:** Canonical cargo recipes
- **bazel.just:** Bazel equivalents
- **artifact-parity.md:** Parity test policy and workflow

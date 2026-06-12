# Phase 0: PATH/COMMAND RECONCILIATION (RT-18)

## Overview

This document reconciles stale `open/` paths and outdated commands in the native-compiler documentation against the current repository checkout. The migration plan and supporting docs were written relative to an external `open/` tree; this checkout uses `crates/{honk,hatch,hoonc}`, `hoon/common/hoon.hoon`, and justfile recipes. All stale references have been catalogued with corrected in-repo equivalents.

---

## Stale Path References

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

## Stale Bazel Targets

| Stale Target | File:Line | In-Repo Equivalent | Status |
|---|---|---|---|
| `//open/crates/honk/test-assets:hoon_138_arbitrary_parity_test` | README.md:33, artifact-parity.md:20, source-spots.md:33 | See "Commands" section below | test script (Bazel target does NOT exist in this checkout) |
| `//open/assets/native:kernel_parity_test` | README.md:34, artifact-parity.md:27 | See "Commands" section below | test script (Bazel target does NOT exist in this checkout) |

**Important:** The referenced Bazel targets (`//open/crates/honk/test-assets:hoon_138_arbitrary_parity_test`, `//open/assets/native:kernel_parity_test`) do not exist in this checkout. They are referenced in the exported-tree documentation but the canonical test scripts are standalone bash scripts; see the "Commands" section for the correct invocation method.

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

**What it does:**
- Invokes `crates/honk/test-assets/honk_138_native_parity.sh`
- Compiles `hoon/common/hoon.hoon` (= `crates/hoonc/hoon/hoon-138.hoon`) with both compilers
- Compares artifacts with strict `cmp` (byte-exact)
- Runs under RSS guard (75% physical RAM ceiling) to catch memory blowup safely
- **Current status:** honk's native mint OOMs before completing; test reports memory blowup rather than parity result (as of 2026-06-14)

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

**Bazel workflow:**
```bash
# 1. Build everything and generate reference kernels
just bazel build
just bazel build-assets

# 2. Build kernels natively with honk (Bazel-built binary)
just bazel honk-kernel-jams

# 3. Compare
just bazel honk-parity
```

**Kernels tested:**
- dumb (Dumbnet outer kernel)
- wal (Wallet kernel)
- miner (Dumbnet miner kernel)
- peek (Peek kernel)
- bridge (Bridge kernel)
- roswell (Roswell main app)

**Comparison tool:** `jam-diff --kernel-parity` (in `crates/honk-tools`)

**Gate semantics:** As of this reconciliation, `honk-parity` uses `jam-diff --kernel-parity` which permits a dir-hash-only difference (see RT-02 concern below). For strict byte-equality acceptance, use `cmp` directly on the `.jam` files.

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

**Bazel:**
```bash
bazel test //crates/honk:all
bazel test //crates/hatch:all
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

**Gate status:** Currently ~71–76s (exceeds 60s); blocked on memory bounding and type interning (see TODOS-PERF.md).

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

### Documents referencing Bazel targets that do not exist

1. **docs/native-compiler/NATIVE-TYPES-MIGRATION-RT.md** (section RT-18, line 117–119)
   - Finding: Acknowledges the stale `open/` reference problem explicitly
   - Status: This very document is the Phase-0 resolution

2. **docs/native-compiler/NATIVE-TYPES-MIGRATION.md** (section 9, lines 604–608)
   - Context: Branch hygiene section requiring reconciliation
   - Status: Addressed by this document

3. **docs/native-compiler/TODOS.md** (line 18)
   - Context: References `just honk-parity` which exists but uses a tolerant gate (RT-02)
   - Status: Noted below in RT-02 decision

---

## Chunked Mint Classification (RT-18 decision required)

**Current status:** The chunked-prelude mint (splitting compilation by core) is ACTIVE, not obsolete.

- **Where:** `crates/honk/src/bin/honk.rs` lines 2603–2617 (canonical prelude routing when peeled root is `=<`)
- **Issue:** Not byte-exact under `--dbug=true` (`honk.rs:2486–2500`)
- **Gate impact:** It currently gates native-parity memory experiments because output consistency cannot be guaranteed

**Phase 0 decision required:**
1. **DELETE:** Remove chunked mint entirely; force sequential compilation
2. **QUARANTINE:** Disable chunked behind a feature flag; do not use for parity evidence
3. **PRODUCTIZE:** Fix dbug exactness; validate it byte-matches sequential path under all modes

**Recommendation:** QUARANTINE pending Memory Thesis (Phase 3). The chunked path is a plausible optimization but is not byte-exact on the critical observables (dbug spots), so do not use its output as Phase-3 evidence that memory is bounded. Mark it clearly as "experimental, non-parity-checked" to prevent accidental misuse.

---

## RT-02 Decision: Parity Gate Strictness

**Finding:** `just honk-parity` currently passes byte equality OR a dir-hash-only difference, while the migration plan requires byte-for-byte artifact equality.

**Current gate behavior:**
```bash
# justfile honk-parity recipe uses:
target/release/jam-diff --kernel-parity assets/dumb.jam assets/native/dumb.jam
# which permits dir-hash-only diffs
```

**Phase 0 decision required:**

1. **Acceptance gate (strict):** Use `cmp` for byte-equality, failing if any bit differs
   ```bash
   cmp -s assets/dumb.jam assets/native/dumb.jam
   ```

2. **Diagnostic gate (tolerant):** Use `jam-diff --kernel-parity` only for localizing differences
   ```bash
   jam-diff --kernel-parity assets/dumb.jam assets/native/dumb.jam
   ```

3. **Waiver:** Any kernel legitimately differing only by sandbox dir-hash is a named exception with a written waiver, not a silent pass

**Recommendation:** Implement separate harnesses:
- `honk-parity-strict` (acceptance, `cmp`-based)
- `honk-parity-diagnostic` (localizing, `jam-diff`-based)
- Keep `honk-parity` as the current behavior for backward compatibility during transition, but mark it explicitly as "tolerant; use honk-parity-strict for Phase 0 gates"

---

## Commands Summary Table

| Task | Cargo | Bazel | Status |
|---|---|---|---|
| **Build honk binary** | `cargo build --release -p honk` | `bazel build //crates/honk:honk` | canonical |
| **Build hatch parser** | `cargo build --release -p hatch` | `bazel build //crates/hatch` | canonical |
| **Build hoonc (reference)** | `cargo build --release -p hoonc` | `bazel build //crates/hoonc` | canonical |
| **Arbitrary hoon-138 parity** | `just honk-138-parity` | (no Bazel target) | **OOMs; memory issue** |
| **Kernel parity (dumb only)** | `cargo build --release -p honk` + `cargo run --release -p honk -- --new --output assets/native/dumb.jam --prelude hoon/common/hoon.hoon hoon/apps/dumbnet/outer.hoon hoon` + `cmp assets/dumb.jam assets/native/dumb.jam` | `just bazel honk-kernel-jams` + `just bazel honk-parity` | canonical |
| **All six kernel parity** | `just honk-kernel-jams` + `just honk-parity` | `just bazel honk-kernel-jams` + `just bazel honk-parity` | canonical |
| **Roswell timing gate** | `just honk-roswell-timed` | (no Bazel target) | **~71–76s; exceeds 60s** |
| **Unit tests (honk)** | `cargo test --release -p honk` | `bazel test //crates/honk:all` | canonical |
| **Unit tests (hatch)** | `cargo test --release -p hatch --lib` | `bazel test //crates/hatch:all` | canonical |

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

## Recommendations for Documentation Updates

1. **Immediate (before Phase 0 execution):**
   - Update all `open/` paths to in-repo equivalents in:
     - `README.md` (lines 3–14, 28–34, 44)
     - `artifact-parity.md` (lines 7–27)
     - `source-spots.md` (lines 25, 33)
     - `performance.md` (line 26)

   - Replace Bazel target references with accurate script/command alternatives

   - Mark sections 2.2 (RT-02 parity gate) and 9 (chunked mint) as "Phase 0 decision required"

2. **Phase 0 deliverables (per NATIVE-TYPES-MIGRATION.md § 6):**
   - Strict byte-equality harness + `just native-parity-dual` (dual-run, `cmp`)
   - Diagnostic structural diff kept separate (§2.2 recommendation above)
   - Chunked-mint classification (delete/quarantine/productize decision)
   - Updated command registry with all canonical equivalents

3. **Long-term (Phase 6, before retiring noun `ut`):**
   - Remove all references to `open/` tree or mark explicitly as "exported-tree doc"
   - Finalize parity gate semantics (strict acceptance criteria)
   - Clean up deprecated/obsolete recipes

---

## References

- **NATIVE-TYPES-MIGRATION.md § 2.2:** Parity gate strictness (RT-02)
- **NATIVE-TYPES-MIGRATION.md § 3.10:** Non-final noun-boundary matrix (RT-10)
- **NATIVE-TYPES-MIGRATION.md § 9:** Branch hygiene (RT-18, chunked decision)
- **NATIVE-TYPES-MIGRATION-RT.md RT-18:** Stale docs/commands finding
- **TODOS.md:** Resolved items section lists H0 kernel-parity harness
- **TODOS-PERF.md:** Native prelude mint memory blowup + roswell <60s gate
- **justfile:** Canonical cargo recipes
- **bazel.just:** Bazel equivalents
- **artifact-parity.md:** Parity test policy and workflow

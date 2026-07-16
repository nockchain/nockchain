# Disk-paged residency for the AI-PoW verifier-setup table

## Problem

The boot verifier-setup table rebuilt all 7 compact verifier contexts (trace heights
2^13..2^19) at boot and held them resident for the node's lifetime — measured ~8 GB.
A node verifies at the *current* difficulty's trace height (stable — ASERT adjusts
slowly), so the realistic working set is 1–2 heights; the other ~5–6 GB is idle.

## Rejected alternative: lazy rebuild

Building a bucket's context on demand (from its seed) costs a **~12 s circuit
rebuild** (measured). Paying that on the verify path is unacceptable ("long latency
verification"). So contexts are NOT rebuilt on demand.

## Design: build once, page from disk

Build every context ONCE at boot and serialize it to a per-bucket file; at runtime
hold only a bounded working set in memory and page the rest in/out from disk. A verify
pays at most a fast disk **page-in** (read + deserialize a prebuilt context —
**measured ~0.6 s worst-case for the 2^19 bucket**, vs ~12 s to rebuild), and usually
nothing (the active height stays resident in the LRU).

### Boot — `install_or_build_verifier_setup`

1. Ensure the seed cache exists (generate if absent — the existing ~5-min step) and
   validate it against the committed consensus digest via the SEED path
   (`verify_verifier_setup_seed_table_digest`, cached digests, no rebuild). Corrupt /
   mismatched cache ⇒ delete + regenerate.
2. **Build every bucket's context AT THE OUTSET** and serialize it to
   `data_dir/ai-pow/ctx-2p<log2>-<digest8>.bin` (the committed digest is in the
   filename, so a table change ⇒ new file ⇒ rebuild; a stale file is never reused).
   A freshly-built context is digest-validated before it is written. First boot builds
   all 7 (~1–2 min); later boots find the files and skip straight to paging.
3. Inject `init_ai_pow_verifier_setup_disk(disk_buckets, cap)` — only the small
   `(trace_height, committed_digest, context_path)` metadata is resident.

### Runtime — `ai_pow_verifier_setup_for(h) -> Option<Arc<AiPowVerifierSetup>>`

1. Lock the LRU; if `h` is resident, bump recency and return `Arc::clone` (instant).
2. Miss: read + deserialize `context_path` OUTSIDE the lock (~0.6 s), validate the
   deserialized context's verifier-key digest == the committed value (fail-safe `None`
   on read/deserialize error or mismatch — a divergent/corrupt context must not
   verify), then insert into the LRU (evicting the least-recently-used beyond `cap`).

The jet holds the returned `Arc` for the verify's duration, so an eviction on another
thread cannot free the context mid-verify. "Page out" = the LRU drops an idle context
(its heap freed); the file stays on disk for the next page-in.

### Consensus safety

- Verification RESULTS are independent of residency: contexts are deterministic
  (proof-independent, digest-stable — audited) and verify is a pure function of
  `(context, artifact, commit, target)`. Paging only changes WHEN a context is in RAM.
- The consensus parameter (per-bucket verifier-key digest) is committed and checked at
  boot (seed cached digests hash to `AI_POW_V0_VERIFIER_SETUP_TABLE_DIGEST`), when each
  context is built (built digest == committed), and on every page-in (deserialized
  digest == committed).
- Fail modes: unknown in-band height ⇒ BAIL (as before); a missing/corrupt/divergent
  context file ⇒ fail-safe reject + loud log.

### Eager mode (tests) preserved

`init_ai_pow_verifier_setup(setups)` still pins the given contexts in the LRU with
`cap >= len` and no disk buckets, so they never evict or page — the all-resident
behavior. `install_verifier_setup_disk_from_setups(setups, dir, cap)` is the
disk-paged analog for tests / the accept e2e (serialize the built contexts, inject
disk-paged, so the jet actually pages them in).

### Config / tradeoff

`cap` (env `AI_POW_VERIFIER_CACHE_CAP`, default 2) bounds resident contexts. Blocks are
admitted in height-contiguous order and the trace height changes slowly, so the working
set is ≤ 2 and a page-in only happens on a rare height shift (prefetchable). Raising
`cap` trades RSS for fewer page-ins; `cap >= 7` keeps every touched bucket resident.
Cost: ~8 GB of context files on disk (the size we removed from RAM).

## Staging

1. **Structure** (committed): global + generic LRU + `Arc` lookup, eager-populated
   (no behavior change).
2. **Disk-paged path**: committed seed-digest boot check, build-all-at-boot +
   serialize, page-in on miss, `cap` config. Validated: LRU units, disk page-in KAT,
   page-in-latency probe (~0.6 s vs 12 s rebuild), lazy boot digest check accepts the
   real cache, and the accept e2e (real block admitted / wrong-commit rejected through
   the live kernel, paging the context in from disk).

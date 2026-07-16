# Lazy residency for the AI-PoW verifier-setup table

## Problem

The boot verifier-setup table rebuilds all 7 compact verifier contexts (trace
heights 2^13..2^19) at boot and holds them resident for the node's lifetime —
measured ~8 GB (each context ~0.9–2.7 GB; the preprocessed LDE Merkle tree
dominates). A node verifies at the *current* difficulty's trace height, which is
stable (ASERT adjusts slowly), so the realistic working set is 1–2 heights — the
other ~5–6 GB is idle.

## Design

Keep only the small SEEDS resident (~39 MB total); materialize a bucket's heavy
context on first verify at that height; cache it in a bounded LRU. Standing RSS
becomes `seeds + up to cap resident contexts`.

### Resident state (process-global)

- `seeds: Map<trace_height, BucketSeed>` — immutable after boot.
  `BucketSeed { trace_height, committed_digest: [u8;40], seed_bytes: Vec<u8> }`,
  where `seed_bytes` is the bincode of one `AiPowCompactVerifierSetupSeed`
  (~1–5 MB). The seed is not `Clone` (its metadata's `stark_common` isn't), so we
  store bytes and deserialize per rebuild — cheap next to the ~10 s circuit rebuild.
- `resident: Mutex<Lru>` — LRU of `Arc<AiPowVerifierSetup>`, capped at `cap`.
- `cap: usize` — max resident contexts (env `AI_POW_VERIFIER_CACHE_CAP`, default 2).

### Lookup — `ai_pow_verifier_setup_for(h) -> Option<Arc<AiPowVerifierSetup>>`

1. Lock the LRU; if `h` is resident, bump recency and return `Arc::clone`.
2. Miss: find `seeds[h]`. `None` ⇒ return `None` (the jet then BAILs, exactly as
   today for an unknown in-band height / uninjected table).
3. Rebuild OUTSIDE the lock (~6–15 s): deserialize `seed_bytes` →
   `rebuild_verifier_setup_from_seed`. Validate the rebuilt context's verifier-key
   digest == `committed_digest[h]`; on mismatch log a loud error and return `None`
   (fail-safe — a divergent verifier must reject, never accept).
4. Re-lock, insert (dedup if another thread filled `h` meanwhile), evict the LRU
   entry if over `cap`, return the `Arc`.

The jet holds the returned `Arc` for the verify's duration, so an eviction on
another thread cannot drop the context mid-verify.

### Boot — `install_or_build_verifier_setup` (lazy)

- Load the seed cache (as today). Do NOT rebuild any context.
- Validate the cached per-bucket digests against the committed
  `AI_POW_V0_VERIFIER_SETUP_BUCKET_DIGESTS` (fast, no rebuild) — catches a
  wrong/corrupt cache at boot. The full-table digest (`e7eef3f4…`, blake3 over the
  committed per-bucket digests) is preserved as the cross-check.
- Corrupt / undecodable / digest-mismatched cache ⇒ delete + regenerate (existing
  recovery), then re-validate.
- `init_ai_pow_verifier_setup_lazy(seed bytes + committed digests, cap)`.

### Consensus safety

- Verification RESULTS are independent of residency: rebuild is deterministic
  (proof-independent, digest-stable — audited) and verify is a pure function of
  `(context, artifact, commit, target)`. The LRU only changes WHEN a context is
  built, never the accept/reject outcome.
- The consensus parameter (per-bucket verifier-key digest) is committed and checked
  at boot (cached digest) and on every rebuild (actual digest). Same trust as the
  eager table.
- Fail modes: unknown in-band height ⇒ BAIL (as today); rebuild digest mismatch ⇒
  fail-safe reject + loud log.

### Eager mode (tests) preserved

`init_ai_pow_verifier_setup(setups)` still works: it pins the given contexts in the
LRU with `cap ≥ len` and no seeds, so they never evict and are never rebuilt —
identical to the pre-lazy behavior. The accept e2e and jet KATs use it unchanged.

### Config / tradeoff

`cap` (default 2) bounds RSS to ~`cap` contexts. Blocks are admitted in
height-contiguous order and trace-height changes slowly (ASERT), so temporal
locality keeps the working set ≤ 2 and thrashing (a ~10 s rebuild when the height
shifts) is rare. Raising `cap` trades RSS for fewer rebuilds; `cap ≥ 7` ≈ the old
all-resident behavior (built on demand, no upfront boot rebuild). The cap is
count-based (buckets vary ~10× in size) — a deliberate simplicity choice.

## Staging

1. **Structural refactor** — lazy global + LRU + `Arc` lookup, eager-populated (no
   behavior change). Validate against the existing fast tests, jet KATs, and the
   accept e2e.
2. **Lazy production path** — committed per-bucket digests, boot loads seeds without
   rebuild, on-demand rebuild + digest validation, RSS measurement. Validate
   exhaustively (build-on-demand verifies identically, eviction+rebuild, RSS win,
   accept e2e, roswell).

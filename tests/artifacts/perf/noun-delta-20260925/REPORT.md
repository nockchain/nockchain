The direct answer from the completed chain benchmark is **243 ms median to diff the newer saved state against a snapshot reconstructed from an epoch plus a delta**, producing a **35.94 MiB** next delta. Encoding added 2.61 ms; decoding and validating the resulting virtual state added 54.36 ms. The combined measured operations took 300.37 ms median over 20 repetitions. **These timings require prepared content indexes.** Indexing the newer saved state from its existing PMA first took 210.014 seconds.

The best measured full-scan alternative uses exact subtree interning while retaining 256-bit portable fingerprints. It took **164.335 seconds to prepare the epoch dictionary and 160.268 seconds to ingest the next snapshot and identify its new records**. Its serialized delta is **79.07 MiB** for a 26.67 GiB target. Including a full independent reconstruction check, the experiment took 397.115 seconds and peaked at **69.23 GiB RSS**. This is a separate A/B experiment; it does not replace the preparation requirement behind the 243 ms chain result. The measurements favor maintaining reusable noun metadata in the runtime if fast routine deltas are required.

The chain used A as an epoch, reconstructed B from A plus the serialized A→B delta, and compared stopped operative state C (event 7,433,651) against that reconstructed B. Both deltas were written, fsynced and reread; new atom payloads in the reconstructed state come from decoded delta files. The physical B snapshot/index was released before C was indexed. Full independent comparison of reconstructed C against the original C passed: 417,390,995 node pairs and 18,667,145,447 atom bytes, taking 220.022 seconds. No event replay was involved.

| Chain operation, 128-bit experimental fingerprints | Seconds |
|---|---:|
| Index A and B concurrently |178.351|
| Build epoch content lookup |30.246|
| Generate A→B delta |0.219|
| Mount decoded B |0.051|
| Derive B hash index from epoch cache plus new records |0.057|
| Extend B lookup |0.028|
| Index C from existing PMA |210.014|
| First diff C against reconstructed B |0.236|
| Write and fsync B→C delta |0.022|
| Read B→C delta |0.008|
| Validate/mount C through two deltas |0.049|
| Full independent C comparison |220.022|
| Complete experiment including 20 warm iterations |647.585|

A→B occupies 83,184,471 bytes and B→C 37,687,373 bytes: **115.27 MiB combined**, in addition to the epoch. C has 26.687 GiB of used PMA bytes; its new delta is 0.132% of that. The chain cache reuses epoch allocations and adds approximately 15.50 MiB of index/lookup allocations for B. Its optional incremental fingerprint sidecar would be 4.50 MiB; the epoch index would be 9.32 GiB. Large sidecars were not persisted or loaded. The full process peaked at **92.11 GiB RSS**, including preparation and mapped files.

Prepared C diff timings across 20 repeated comparisons were 238.40–302.36 ms, with empirical nearest-rank p95 of 249.53 ms. Decode/validated virtual mount median was 54.36 ms, p95 54.77 ms. These are a single unchanged state pair and a shallow chain; they are not evolving workload or arbitrary-depth latency guarantees. The saved C kernel hash differs from A/B (`0440db5b…8e921554` versus `3c3921d7…c3085470`), so value reconstruction establishes no cross-kernel replay compatibility.

The following separate baseline retains 256-bit fingerprints and uses the same A/B inputs. Its two indexes were built sequentially; preparation elapsed times therefore must not be compared with the parallel chain run as an isolated hashing speedup.

Two saved Nockchain states demonstrate substantial noun-level reuse: the later state occupies **26.67 GiB of used PMA bytes**, while its base-relative delta is **79.33 MiB** (83,184,519 bytes), or **0.29045%** of that size. This is the incremental payload; the base state and the machinery needed to resolve its references remain necessary. Preparing that machinery is currently expensive: the baseline took **531.93 seconds** to index both states and construct the base lookup, with **104.54 GiB peak RSS**.

The inputs are immutable copies of rotating snapshots at events **7,373,518 (A)** and **7,392,318 (B)**, separated by 18,800 events. Their used prefixes are 28,556,535,944 and 28,639,773,280 bytes. Each PMA has 64 GiB capacity plus its footer; capacity and used bytes are different measures. The run used a Ryzen 9 9950X host with 32 logical CPUs and approximately 249 GiB RAM, alongside the running production leader. No production noun was modified, no node was booted, and this benchmark did not replay events or scan SQLite.

The persisted root is **kernel state, corresponding to Arvo axis 6**, rather than the complete live subject with its battery. A read-only wrapper exposes atoms and ordered cell children without modifying their values, allocation bytes, or cached mugs. Reconstructed graphs preserve noun values but may share equal subtrees differently. Addresses, mugs, unreachable allocations, and allocation topology are excluded from value identity.

The following preparation and validation stages were measured once:

| Stage | Seconds |
|---|---:|
| Index A: 417,117,172 reachable allocated nodes | 254.967 |
| Index B: 417,342,305 reachable allocated nodes | 259.169 |
| Build base content lookup | 17.795 |
| Build delta after preparation | 0.212 |
| Write delta and fsync | 0.048 |
| Read delta and create validated virtual view | 0.095 |
| Full independent structural comparison | 180.817 |
| Complete benchmark, including warm repetitions | 720.227 |

“Cold preparation” here means indexes were absent, not that the operating-system page cache was emptied. Cache dropping was not performed. The separately measured 20 warm iterations retained both indexes and the base lookup; encoding and decoding used memory buffers rather than durable disk writes.

| Warm operation | Median | Nearest-rank p95 |
|---|---:|---:|
| Build delta | 210.09 ms | 211.83 ms |
| Encode | 5.74 ms | 5.89 ms |
| Decode and validate virtual mount | 85.59 ms | 86.36 ms |
| Combined measured operations | 301.35 ms | 304.19 ms |

These are repeated comparisons of the same A/B pair, not measurements of 20 evolving production states. Twenty observations and production co-tenancy do not establish stable tail latency. A virtual mount is not a cold-process restore or materialization of a new bootable PMA.

The delta contains 285,992 new cells, 8,911 new atoms, and 121,783 references to base subtrees. New atoms contribute 78,242,336 bytes. **Changed atoms are stored in full**; there is no byte-level patching within an atom. The independent full comparison passed, checking 417,349,828 node pairs and 18,654,018,430 atom bytes. It compares actual values and structure, rather than accepting a matching root digest as sufficient evidence.

The baseline uses domain-separated **BLAKE3-256, with 32-byte fingerprints**. Iterative postorder traversal hashes canonical atom values and ordered child fingerprints, memoizing shared nodes. Base lookup identifies reusable subtrees independent of relocation; matches compare all 256 fingerprint bits. The delta binds references to the particular base index/layout and records new nodes in dependency order.

Direct atoms and unreachable allocations are excluded from the indexed-node counts. For V reachable allocated nodes and L atom bytes, initial indexing takes O(V + L) time and O(V) auxiliary space. Base lookup construction is expected O(V). Prepared diffing visits changed nodes and the boundary of reused subtrees, but its worst case remains a full target traversal. Full verification maintains a visited-pair set and reads compared atom contents. Neither cheap warm diffing nor the small payload eliminates initial indexing.

Each optional serialized index would occupy approximately **15.54 GiB**, using 40 bytes per allocated noun plus a header. These sizes are calculated from the format, not large sidecars written and reread during this run. In-memory index allocations were estimated at 28.75 GiB each; measured peak RSS also includes mapped source pages and other working data. Existing snapshots contain no such maintained per-noun index.

A byte-layout control shows why raw block reuse is insufficient for this pair. At 64 KiB chunks it took **16.13 seconds**: new payload was **99.5128%** of the target, or **99.5494%** including the estimated piece table. At 1 MiB it took **13.27 seconds**, with **99.8851%** including the table. These passes retained exact pointer bytes, confirmed candidate matches by byte equality, checked source manifest hashes, and verified the virtual target checksum. They counted payload rather than writing a huge delta; pieces containing new bytes borrow those bytes from the original target. Thus the control validates a virtual piece reconstruction, not a restored persisted delta. Their root walk was bounded; it was not the full semantic validation above.

Runtime integration would need fingerprints maintained when nouns become final, associations preserved or rebuilt across GC relocation, durable base identities, crash-safe publication, and retention rules for every referenced epoch. Chained deltas need rebasing to release obsolete ancestor data and bound lookup depth. A loader must either support the derived representation or materialize valid PMA allocations; neither integration cost was measured here.

Three bounded serial pilots used the same first 100 million physical nodes and the same revised engine. Each was measured once:

| Variant | Traversal seconds | Peak RSS GiB |
|---|---:|---:|
|256-bit streaming cell hash |44.846|8.09|
|256-bit single-call cell hash |44.896|8.09|
|128-bit single-call cell hash |35.625|6.60|

Single-call hashing showed no benefit in that sample. Truncation reduced measured traversal time by 20.6% and index-record width from 40 to 24 bytes. It changes cell hash inputs and root identities and has a smaller collision margin; it is an explicit experiment with distinct artifact magic, not a recommended production security choice. Both widths passed the independent full-value checks described above. The baseline and chain use different width/parallelism/cache histories, so their whole-run times do not isolate any one optimization.

The final optimization exploits the difference between physical nodes and distinct noun values. A contains 417,117,172 reachable allocated nodes but only 59,187,793 distinct allocated values. A shared dictionary interns atoms by normalized bytes and cells by exact ordered pairs of canonical child references. Atom digest candidates require full byte equality. The implementation computes a portable cell fingerprint only when its canonical value is new, and releases each physical traversal's raw-address memo afterwards. It leaves every original noun unchanged. Both resulting 256-bit root fingerprints exactly match the original baseline.

| Exact interning A→B operation | Seconds |
|---|---:|
| Intern A and build shared dictionary |164.335|
| Traverse all of B, reuse dictionary and identify new records |160.268|
| Serialize delta, write and fsync |0.078|
| Read delta, discard construction maps and validate reconstructed view |0.174|
| Full independent B comparison, including verifier cleanup |72.259|
| Complete experiment |397.115|

The persisted delta is 82,911,906 bytes (79.07 MiB, 0.28950% of B's used bytes), with 271,456 new canonical cells and 7,410 new canonical atoms. Its reconstruction uses the A dictionary and owned records decoded from that file; new atom payloads do not borrow B. Independent comparison passed for all 417,342,305 physical node pairs and 18,654,015,166 atom bytes. Portable fingerprints match the baseline, while the more thoroughly shared canonical representation slightly reduces delta size. Dictionary capacity accounting estimates 5.18 GiB, excluding mapped atom storage and the transient physical-address memo; this must not be confused with the measured 69.23 GiB process peak.

Total A+B preparation was 324.603 seconds versus 531.931 seconds for the original sequential 256-bit approach. The exact-interning run measured only one whole-state pair, after the backup integrity checker had finished, with a warm host page cache. It incurred zero major page faults, versus 119,487 in the baseline; reported filesystem-input counts were 8 versus 105,789,152. Earlier baseline preparation had different cache history and partial overlap with that checker. These observations show a promising improvement, not an isolated or repeatable percentage speedup. The interner still reads all physical nodes and hashes all encountered atom bytes when ingesting a fresh PMA; it does not deliver a subsecond delta from unindexed input. No separate prepared interner diff, persistent dictionary loader or long-chain interner benchmark was measured.

Twelve independent oracle tests passed under all four combinations of hash width and hashing method; five reader/block-control tests also passed. Tests cover relocation, different sharing, numeric atom normalization, large changed atoms, deep iterative traversal, corrupt payloads, wrong bases/layouts, actual serialization lengths and A→B→C→A recovery. Guarded chain tests verify that deriving an index and lookup makes zero base-noun reads. Sidecar readers and production runtime integration remain unimplemented.

Five additional interning cases passed, covering forced atom-candidate collisions, canonical sharing and atom normalization, deep traversal, retained dictionary identity, and reconstruction after the target mapping/file is removed and its encoded input buffer overwritten. The complete final all-features run passed 23 test executions covering 22 distinct cases; the collision case runs in both the binary and integration-test targets.

The production leader stayed running on the same image and PID, while raw benchmark binaries used read-only backup mounts, inaccessible production paths and separate network namespaces. A separate low-priority read-only SQLite integrity check began at 18:08:12 UTC; it overlapped later baseline stages and subsequent experiments. Host governor/cache settings were unchanged. No prototype code was added to the runtime PR.

Evidence: `artifacts/remote-sequence/sequence-128.jsonl`, its `.time` and `.stderr` files and `summary.json`; `artifacts/remote-baseline/` contains the 256-bit pair, block control, pilot results and host fingerprint. `artifacts/remote-hashcons/` contains the exact-interning pilot and complete A/B run. Source/build identities and independent test logs are under `artifacts/`; original baseline source is preserved separately in `artifacts/baseline-source/`.

The fallback backup is complete at `/opt/nockcloud/backups/pma-storage-efficiency-20260925T1635Z/data`. All 40 copied files retained their original sizes. A separate read-only full SQLite integrity check returned `ok` at 2026-09-25 18:39:53 UTC for the 426,889,695,232-byte database, after 1901.111 seconds. Its result and logs are preserved in `artifacts/backup/`. The prior candidate process remains stopped.

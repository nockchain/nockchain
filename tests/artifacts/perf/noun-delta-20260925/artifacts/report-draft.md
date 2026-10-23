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

These results belong to the saved baseline source and 256-bit variant. Planned hashing and 128-bit truncated-fingerprint experiments require separate results. A forthcoming comparison also uses stopped operative state C at event 7,433,651: its saved kernel hash differs from A/B (`0440db5b…8e921554` versus `3c3921d7…c3085470`). Comparing those saved noun values does not establish cross-kernel replay compatibility.

Evidence: `remote-baseline/pair-baseline.jsonl`, `pair-baseline.time`, `summary.json`, `page-ab.jsonl`, and `fingerprint.json`; benchmark implementation preserved under `baseline-source/`. C metadata was separately inspected; chain and optimized-variant results are pending.

Noun-value contract: atoms are normalized unsigned little-endian integers and cells preserve ordered head/tail values. Cache mugs, arena offsets, unreachable arena allocations, and allocation-sharing topology are excluded from value identity. Original files are opened read-only and bound read-only in a restricted benchmark unit. Delta/envelope outputs are separate. Kernel hash/event metadata is recorded alongside measured roots.

Golden correctness: independent synthetic logical model covers relocation, cached-mug changes, direct/indirect equivalent atoms, large atoms, repeated DAG children, changed leaves, sharing differences, a deep list, and chained views. A full target-vs-reconstructed structural comparison validates real data without treating a matching root digest alone as proof. BLAKE3-256 is used for content lookup; candidate bucket matches are checked against all256 digest bits. Base node IDs are bound to the exact base index/layout, not just its logical root.

Hypotheses before measurement:
1. Cold index construction dominates initial elapsed time because every reachable node needs reading and hashing. Unmeasured.
2. A maintained Merkle sidecar permits delta discovery proportional to new/changed reachable nodes. Unmeasured.
3. GC relocation prevents safe raw-address identity reuse, but should not destroy noun-value subtree reuse. Synthetic tests intended; real data unmeasured.
4. A virtual reconstructed epoch+delta view can mount cheaply, while validating/materializing it has separate costs. Unmeasured.
5. Warm root comparison alone is not an end-to-end benchmark; all preparation and runtime-side maintenance requirements must be included explicitly.

Optimization policy: preserve ordering and integer values; no floating-point or RNG semantic changes. Change one measured bottleneck at a time and retain source revisions, input fingerprints, root values, full reconstruction validation, and timings. No host-global tuning or cache dropping. Production co-tenancy remains a limitation of tail-latency claims.

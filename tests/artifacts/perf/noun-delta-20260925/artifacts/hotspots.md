# Baseline profile and optimization hypotheses

Host nockcloud-control; 100 million unique indirect nouns from older rotating snapshot. Actual sample: 41.646 s traversal, 8,479,328 KiB peak RSS. Node budget stops deliberately; this is a prefix, not a whole-snapshot result. 1,487 perf cycles samples over 15 seconds; no lost samples. Filesystem cache uncontrolled and production leader colocated; no kernel/governor changes.

| Rank | Symbol | Self sample fraction | Evidence |
|---|---|---:|---|
| 1 | BLAKE3 AVX512 compression | 36.24% | remote-baseline/pilot-100m-profile.txt |
| 2 | build_index (includes inlined map/traversal) | 21.80% | same |
| 3 | BLAKE3 ChunkState::update | 16.68% | same |
| 4 | BLAKE3 Hasher::update | 6.97% | same |
| 5 | Snapshot::word_range | 5.74% | same |

Optimization hypotheses: one-shot fixed-size cell hashing can remove repeated streaming update overhead while preserving exact fingerprints; incremental external hashes avoid revisiting old DAG on subsequent deltas; dense offset IDs may reduce map memory but cost 4 bytes per allocated PMA word regardless of reachability; byte-block delta is an independent throughput/storage control and may fail after relocation. Full comparison running before adopting changes.

Sample count is insufficient for tail estimates; no p99 claims. Repeating full billion-node ingestion 20 times would conflict with the requested modest benchmark effort. Warm delta trials have a 60-second aggregate budget and report their actual count.

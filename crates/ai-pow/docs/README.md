# ai-pow documentation

The Pearl-compatible mining implementation is documented by the crate README
and by the retained fixture tests under `tests/`.

The current production certificate pipeline is owned by `ai-pow-zk`:

- [`../../ai-pow-zk/docs/2026-06-07_COMPACT_RECURSIVE_PRODUCTION_PIPELINE.md`](../../ai-pow-zk/docs/2026-06-07_COMPACT_RECURSIVE_PRODUCTION_PIPELINE.md)

Older Pearl audit and divergence-tracking documents were removed from this
branch. They remain available in git history, while the current source of truth
for Pearl byte-equivalence is the fixture set in `tests/fixtures/pearl.rs` and
the tests that exercise it.

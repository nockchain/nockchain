# `ai-pow` documentation

The stable documentation set is:

- [`ARCHITECTURE.md`](ARCHITECTURE.md) — ownership boundaries and end-to-end attempt/certificate flow.
- [`SECURITY.md`](SECURITY.md) — maintained invariants, attacker model, cryptographic assumptions, and consensus dependencies.
- [`2026-07-17_DUAL_PUZZLE_CONSENSUS_AUDIT.md`](2026-07-17_DUAL_PUZZLE_CONSENSUS_AUDIT.md) — dated findings, remediation record, and remaining independent-review gate.

The crate-level [`README`](../README.md) is the entry point for APIs and system integration. Cross-implementation behavior is pinned by fixtures and tests, especially `tests/fixtures/pearl.rs`; historical roadmaps, progress logs, and superseded residual lists belong in git history rather than the active documentation set.

Proof-stack details live in [`../../ai-pow-zk/docs/`](../../ai-pow-zk/docs/). Verifier setup lifecycle details live in [`../../ai-pow-jets/docs/VERIFIER_SETUP.md`](../../ai-pow-jets/docs/VERIFIER_SETUP.md).

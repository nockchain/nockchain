# PMA Documentation

This directory contains PMA documentation that is useful for production testing, public release review, and future PMA-sensitive development.

## Documents

- [`DESIGN.md`](./DESIGN.md) — implementation-oriented PMA design, durability model, snapshot model, GC model, and known limits.
- [`DURABILITY-OPERATIONS.md`](./DURABILITY-OPERATIONS.md) — operator and production-testing guide for PMA durability, boot recovery, snapshots, event logs, and GC.
- [`NOUN-PROVENANCE-AND-BRANDED-HANDLES.md`](./NOUN-PROVENANCE-AND-BRANDED-HANDLES.md) — context for alien noun / NounSpace bugs and why `NounHandle` / branded handles exist.
- [`NOCKSTACK-ATTRIBUTION.md`](./NOCKSTACK-ATTRIBUTION.md) — memory attribution guide for PMA, NockStack, and heap/anonymous memory.
- [`FAST-HINT-REGISTRATION.md`](./FAST-HINT-REGISTRATION.md) — current-code guide to `%fast` hint registration, cold/warm/hot state, and persistence implications.
- [`NOCK-PMA.md`](./NOCK-PMA.md) — lower-level historical PMA design notes for NockVM internals.
- [`PMA-NOW.md`](./PMA-NOW.md) — historical implementation-plan/status notes retained for context.

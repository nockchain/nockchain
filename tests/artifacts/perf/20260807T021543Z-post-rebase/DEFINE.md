# DEFINE — Honk post-rebase compiler profile

## Scenario

Compile the production Dumbnet entry with Honk's native compiler and compile
the canonical hoon-138 prelude with `HONK_NATIVE_PARITY=1`. Dumbnet supplies a
repeatable production baseline; hoon-138 exercises the bounded self-mint path.
An isolated Roswell build revalidates the historical 60-second secondary gate.

## Metric

Dumbnet warm-cache wall-time distribution and throughput; peak resident memory
for Dumbnet, hoon-138, and Roswell; sampled CPU stacks and native phase timings
for hoon-138.

## Budget

- Dumbnet p95 must stay below 45 seconds on this Apple M5 Max host.
- Hoon-138 native self-mint must complete below the parity script's 75%-of-RAM
  ceiling and remain byte-identical to hoonc.
- Roswell should compile in under 60 seconds.

## Golden output

The Bazel `hoon_138_arbitrary_parity_test` must pass exact byte comparison.
The final Dumbnet output and its separate resident-memory run must produce the
SHA-256 digest recorded in `golden_checksums.txt`. Roswell must match the strict
production-kernel parity gate.

## Scope boundary

This run profiles native Honk compilation. It does not compare PGO, tune global
macOS settings, or treat the slower hoonc reference compiler as a performance
baseline.

## Variance envelope

- At most 10% p95 drift on this host is noise.
- More than 10% requires investigation.
- More than 20%, or three consecutive results over 10%, escalates.

## Stakeholder / requester

Requested by the branch owner to decide what optimization should follow the
arena migration after rebasing onto current master.

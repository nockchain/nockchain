# Post-rebase baseline

This run used `target/profile-post-rebase/release/honk`, built from rebased
commit `268f33b983c13754707b776d53251ca2c73b36da` with the release profile and
`-C force-frame-pointers=yes`. See `fingerprint.json` for the complete host and
toolchain fingerprint.

## Results

| Workload | Runs | Wall time | Peak RSS | Output |
| --- | ---: | ---: | ---: | --- |
| Dumbnet production entry | 20, after 3 warmups | mean 32.783 s; median 32.485 s; p95 35.632 s; min 28.915 s; max 38.721 s | 4.92 GiB in a separate timed run | 20,010,224 bytes; SHA-256 `79dad9653af051ba335243581c14010ddc49eba265fd7fcf417bbba94f042d95` |
| hoon-138 native parity build | 1 isolated timed run | 58.48 s | 10.29 GiB | 2,286,744 bytes; SHA-256 `b156034a5d5d158c133fad6591a71b96def4339e49faba4773b9f9b68e1c1741` |
| Roswell production entry | 1 isolated production run | 63.71 s | 8.93 GiB | 26,612,048 bytes; SHA-256 `8c47a2b9af208d190f2f151bcc299a494a67431afef21221a203538ca3b08cef` |

The Dumbnet p95 is below the 45-second budget. The 20-run coefficient of
variation is about 5.6%; hyperfine reported statistical outliers, so deltas
smaller than the variance envelope in `DEFINE.md` should not be treated as
improvements.

The hoon-138 output hash matches the pre-rebase result. The authoritative Bazel
parity target also passed its exact byte comparison against hoonc. The measured
process incurred no page faults or swaps.

Roswell missed the existing 60-second target by 3.71 seconds (6.2%). A second
run using the frame-pointer profiling binary took 63.82 seconds and emitted the
same bytes, so profiling instrumentation was not a material confounder here.

## hoon-138 native phase timings

| Phase | Time | Share of isolated wall time |
| --- | ---: | ---: |
| Load honc, cold | 0.472 s | 0.8% |
| Load musk, cold | 0.548 s | 0.9% |
| Mint shared honc formula | 20.493 s | 35.0% |
| Parse target leaf | 0.098 s | 0.2% |
| Swet `hoon/common/hoon.hoon` | 35.749 s | 61.1% |

All eight target layers were native-minted. The phase total shows that parsing
and filesystem I/O are not useful optimization targets for this workload.

## Evidence

- `dumbnet_hyperfine.json`: raw 20-run wall-time distribution.
- `dumbnet_time_l.txt`: isolated Dumbnet CPU and resident-memory counters.
- `hoon138_time_l.txt`: isolated hoon-138 CPU and resident-memory counters.
- `hoon138_trace.log`: native phase trace.
- `roswell_time_l.txt`: isolated production Roswell CPU and memory counters.
- `hoon138_cpu_symbolicated.json.gz` and its symbol sidecar: sampled CPU profile.

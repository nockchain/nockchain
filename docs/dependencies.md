# Dependency maintenance

The workspace and CUDA miner have separate active Cargo dependency roots. The
CUDA miner is excluded from the main workspace and uses
`crates/apps/ai-pow-miner-cuda/Cargo.lock`.

`crates/tools/nockup/Cargo.lock` is not used by workspace builds; nockup resolves
through the root lockfile. AWS dependencies resolve from published crates,
without a local runtime patch.

Cargo and Bazel lockfile writers must run sequentially. After dependency updates,
run `bazel mod show_repo @crates` and confirm that `MODULE.bazel.lock` and
`Cargo.lock` resolve the same updated packages. Verify consumers with
`cargo check --locked --workspace --all-targets`, relevant regressions, and a
runtime exercise of the affected APIs. CUDA compilation requires its separate
toolchain and driver environment.

## Consumer constraints

The OpenTelemetry API/exporter family uses 0.32 with SDK ≥0.32.1 and
tracing-opentelemetry 0.33. `nockapp::observability::init_tracing` exports OTLP gRPC
using the existing `DD_*` endpoint/resource settings and
`OTEL_TRACES_SAMPLE_RATE`; exporter configuration failures return
`ExporterBuildError` to the caller.

Bazel, the Nix Foundry input, and the bridge installer pin Foundry 1.8.6. The
bridge compiler is Solidity 0.8.38 with the Prague EVM target, optimizer enabled
at 200 runs, and via-IR compilation. Pin the EVM target independently of Forge's
defaults. Compiler updates must preserve contract ABIs and storage layouts and
pass the bridge authorization, replay, mint/burn, and upgrade checks.

Networking uses libp2p 0.57.1 and Hickory 0.26. System DNS settings take precedence
over the Cloudflare UDP/TCP fallback. `MultiaddrExt::canonical_ip_addr` normalizes
IPv4-mapped IPv6 addresses before exclusion and per-IP accounting; the upstream
`ip_addr` method does not.

Cache consumers use Ratatui 0.30.2, Alloy 2.5, op-alloy 2.0, and AWS S3 ≥1.144.0.
Active lockfiles resolve lru 0.18.5. Keep Alloy and op-alloy compatible when
updating provider fillers and wallets.

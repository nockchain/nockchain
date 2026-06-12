# Create .env file if it doesn't exist
$(shell [ ! -f .env ] && touch .env)

# Load environment variables from .env file
include .env

# Set default env variables if not set in .env
export RUST_BACKTRACE ?= full
export RUST_LOG ?= info,nockchain=info,nockchain_libp2p_io=info,libp2p=info,libp2p_quic=info
export MINIMAL_LOG_FORMAT ?= true
export MINING_PKH ?= 9yPePjfWAdUnzaQKyxcRXKRa5PpUzKKEwtpECBZsUYt9Jd7egSDEWoV
export

# ---------------------------------------------------------------------------
# Cross-compilation (cargo-zigbuild) configuration.
# See the "zig-build targets for cross-compilation" section near the bottom.
# ---------------------------------------------------------------------------
# Linux x86_64 release target. The .2.39 suffix is the glibc floor (modern
# Ubuntu/Debian); override ZIGBUILD_TARGET for a different arch or glibc floor.
ZIGBUILD_TARGET ?= x86_64-unknown-linux-gnu.2.39

# aws-lc-sys and jemalloc cross-compile through these zig wrappers. aws-lc-sys
# honors AWS_LC_SYS_CC; tikv-jemalloc-sys builds via its own autotools configure
# which reads plain AR/RANLIB, so without the wrappers it falls back to the host
# (e.g. macOS) ar and silently drops cross-compiled ELF objects -> empty
# libjemalloc.a -> undefined mallocx/rallocx/sdallocx at link time.
ZIGBUILD_AWS_LC_CC ?= $(CURDIR)/tools/zig/zig_cc_linker.sh
ZIGBUILD_AR ?= $(CURDIR)/tools/zig/zig_ar.sh
ZIGBUILD_RANLIB ?= $(CURDIR)/tools/zig/zig_ranlib.sh
zigbuild_aws_lc_env = ZIG_TARGET=$(1) AWS_LC_SYS_CC=$(ZIGBUILD_AWS_LC_CC) AR=$(ZIGBUILD_AR) RANLIB=$(ZIGBUILD_RANLIB)

DOCKER_IMAGE ?= nockchain-local
DOCKER_MEM ?= 32g
# DOCKER_MEM_SWAP ?= 32g
DOCKER_P2P_PORT ?= 30000
DOCKER_DATA_DIR ?= $(CURDIR)/.data.nockchain
DOCKER_NOCKCHAIN_ENVS ?=
DOCKER_NOCKCHAIN_ARGS ?=
DOCKER_METRICS_COMPOSE ?= docker/docker-compose.metrics.yml
DOCKER_METRICS_NETWORK ?= nockchain-metrics
INFLUXDB_VERSION ?= 2.7
TELEGRAF_VERSION ?= 1.30
STATSD_HOST ?= telegraf
STATSD_PORT ?= 8125
GENESIS_SYNC_RUST_LOG ?= info,nockapp::kernel::form=info,nockapp::kernel::boot=info,nockapp::utils::durability=info
GENESIS_SYNC_MINIMAL_LOG_FORMAT ?= true
GENESIS_SYNC_PEER ?=
GENESIS_SYNC_COMMON_ARGS ?=
GENESIS_SYNC_EXTRA_ARGS ?=
GENESIS_SYNC_NODE_CMD ?= cargo run --release --bin nockchain --
GENESIS_SYNC_DATA_DIR_ON ?= $(CURDIR)/.data.nockchain-sync-fsync-on
GENESIS_SYNC_DATA_DIR_OFF ?= $(CURDIR)/.data.nockchain-sync-fsync-off
GENESIS_SYNC_BIND_PORT_ON ?= 31000
GENESIS_SYNC_BIND_PORT_OFF ?= 31001
ifneq (,$(wildcard ../Cargo.toml))
HONK_BIN ?= ../target/release/honk
else
HONK_BIN ?= target/release/honk
endif
HONK_RUN_ENV ?= RUST_LOG=warn

.PHONY: build
build: build-hoon-all build-rust
	$(call show_env_vars)

## Build all rust
.PHONY: build-rust
build-rust:
	cargo build --release

.PHONY: contracts-deps
contracts-deps: ## Install Solidity dependencies for bridge crate
		$(MAKE) -C crates/bridge/contracts deps

.PHONY: build-nockchain-jemalloc
build-nockchain-jemalloc:
	cargo build --release --features jemalloc --bin nockchain

.PHONY: build-nockchain-bridge-tui
build-nockchain-bridge-tui:
	cargo build --release --bin nockchain-bridge-tui

## Run all tests
.PHONY: test
test:
	cargo test --release

.PHONY: bench-nockchain-kernel
bench-nockchain-kernel:
	cargo run --release -p nockchain --bin bench_nockchain_kernel -- --skip-mining

.PHONY: bench-nockchain-checkpoint-block
bench-nockchain-checkpoint-block:
	cargo run --release -p nockchain --bin bench_nockchain_checkpoint_block --

.PHONY: docker-nockchain
docker-nockchain: docker-nockchain-build docker-nockchain-run

.PHONY: docker-nockchain-pma-persist
docker-nockchain-pma-persist: docker-nockchain-build
	$(MAKE) docker-nockchain-run \
		DOCKER_NOCKCHAIN_ENVS="-e NOCK_PMA_PERSIST=1" \
		DOCKER_NOCKCHAIN_ARGS="--pma-persist $(DOCKER_NOCKCHAIN_ARGS)"

.PHONY: docker-nockchain-build
docker-nockchain-build:
	docker build -f docker/Dockerfile -t $(DOCKER_IMAGE) .
# --checkpoint-mode stream \
.PHONY: docker-nockchain-run
docker-nockchain-run:
	mkdir -p $(DOCKER_DATA_DIR)
	@docker network inspect $(DOCKER_METRICS_NETWORK) >/dev/null 2>&1 || docker network create $(DOCKER_METRICS_NETWORK)
	docker run --rm -it --name nockchain \
		--network $(DOCKER_METRICS_NETWORK) \
		--memory $(DOCKER_MEM) \
		-e RUST_BACKTRACE=1 \
		-e NOCK_PMA_TIMING=1 \
		-e NOCK_PMA_TIMING_DETAIL=1 \
		-e NOCK_STACK_TIMING_DETAIL=1 \
		-e STATSD_HOST=$(STATSD_HOST) \
		-e STATSD_PORT=$(STATSD_PORT) \
		$(DOCKER_NOCKCHAIN_ENVS) \
		-p $(DOCKER_P2P_PORT):$(DOCKER_P2P_PORT)/udp \
		-v $(DOCKER_DATA_DIR):/data/.data.nockchain \
		$(DOCKER_IMAGE) \
		--fast-sync --num-threads 0 \
		--save-interval 300000 \
		--gc-interval 900 \
		--data-dir /data/.data.nockchain \
		--identity-path /data/.data.nockchain/.nockchain_identity \
		--bind /ip4/0.0.0.0/udp/$(DOCKER_P2P_PORT)/quic-v1 \
		$(DOCKER_NOCKCHAIN_ARGS)

.PHONY: docker-metrics
docker-metrics:
	@docker network inspect $(DOCKER_METRICS_NETWORK) >/dev/null 2>&1 || docker network create $(DOCKER_METRICS_NETWORK)
	docker compose -f $(DOCKER_METRICS_COMPOSE) up -d

.PHONY: run-genesis-sync-fsync-on
run-genesis-sync-fsync-on:
	NOCK_PMA_TIMING=1 NOCK_PMA_TIMING_DETAIL=1 RUST_LOG="$(GENESIS_SYNC_RUST_LOG)" MINIMAL_LOG_FORMAT="$(GENESIS_SYNC_MINIMAL_LOG_FORMAT)" $(GENESIS_SYNC_NODE_CMD) \
		--data-dir "$(GENESIS_SYNC_DATA_DIR_ON)" \
		--identity-path "$(GENESIS_SYNC_DATA_DIR_ON)/.nockchain_identity" \
		--bind "/ip4/0.0.0.0/udp/$(GENESIS_SYNC_BIND_PORT_ON)/quic-v1" $(if $(GENESIS_SYNC_PEER),--peer "$(GENESIS_SYNC_PEER)") $(GENESIS_SYNC_COMMON_ARGS) $(GENESIS_SYNC_EXTRA_ARGS)

.PHONY: run-genesis-sync-fsync-off
run-genesis-sync-fsync-off:
	RUST_LOG="$(GENESIS_SYNC_RUST_LOG)" MINIMAL_LOG_FORMAT="$(GENESIS_SYNC_MINIMAL_LOG_FORMAT)" $(GENESIS_SYNC_NODE_CMD) \
		--disable-fsync \
		--data-dir "$(GENESIS_SYNC_DATA_DIR_OFF)" \
		--identity-path "$(GENESIS_SYNC_DATA_DIR_OFF)/.nockchain_identity" \
		--bind "/ip4/0.0.0.0/udp/$(GENESIS_SYNC_BIND_PORT_OFF)/quic-v1" $(if $(GENESIS_SYNC_PEER),--peer "$(GENESIS_SYNC_PEER)") $(GENESIS_SYNC_COMMON_ARGS) $(GENESIS_SYNC_EXTRA_ARGS)

.PHONY: fmt
fmt:
	cargo fmt

.PHONY: check-cargo-fmt
check-cargo-fmt:
	@cargo fmt --check || (echo "Hint: run 'make fmt' to format Rust code." >&2; exit 1)

.PHONY: clippy
clippy: contracts-deps ## Run clippy with the same flags as the upstream repo
	@echo "Running clippy..."
	@cargo clippy --all-targets -- -Dclippy::unwrap_used -Aclippy::missing_safety_doc

.PHONY: lint-local
lint-local: contracts-deps ## Run local cargo clippy with warnings denied
	cargo clippy --all-targets -- -Dclippy::unwrap_used -Aclippy::missing_safety_doc -Dwarnings

.PHONY: clippy-fix
clippy-fix: contracts-deps ## Apply clippy autofixes (same flags as `clippy`)
	@echo "Applying clippy autofixes..."
	cargo clippy --fix --all-targets --allow-dirty --allow-staged -- -Dclippy::unwrap_used -Aclippy::missing_safety_doc

.PHONY: install-hooks
install-hooks: ## Install git pre-commit hooks (cargo fmt + clippy) from .githooks
	git config core.hooksPath .githooks
	@echo "Installed git hooks: core.hooksPath -> .githooks"

.PHONY: docs-check
docs-check:
	./scripts/docs/check_docs_metadata.sh
	./scripts/docs/check_canonical_links.sh
	./scripts/docs/check_nous_validation_entrypoints.sh

.PHONY: build-hoonc
build-hoonc: nuke-hoonc-data ## Build hoonc from this repo
	$(call show_env_vars)
	cargo build --release --locked --bin hoonc

.PHONY: build-hoonc-tracing
build-hoonc-tracing: nuke-hoonc-data ## Build hoonc with tracing
	$(call show_env_vars)
	cargo build --release --bin hoonc --features tracing-tracy

.PHONY: install-hoonc
install-hoonc: nuke-hoonc-data ## Install hoonc from this repo
	$(call show_env_vars)
	cargo install --locked --force --path crates/hoonc --bin hoonc

.PHONY: update-hoonc
update-hoonc:
	$(call show_env_vars)
	cargo install --locked --path crates/hoonc --bin hoonc

.PHONY: build-nockchain
build-nockchain: assets/dumb.jam assets/miner.jam
	$(call show_env_vars)
	cargo build --release --bin nockchain --features tracing-tracy

.PHONY: install-nockchain
install-nockchain: assets/dumb.jam assets/miner.jam
	$(call show_env_vars)
	cargo install --locked --force --path crates/nockchain --bin nockchain

.PHONY: install-nockchain-wallet
install-nockchain-wallet: assets/wal.jam
	$(call show_env_vars)
	cargo install --locked --force --path crates/nockchain-wallet --bin nockchain-wallet

.PHONY: install-nockchain-peek
install-nockchain-peek: assets/peek.jam
	$(call show_env_vars)
	cargo install --locked --force --path crates/nockchain-peek --bin nockchain-peek


HOONC ?= hoonc
HOONC_FLAGS ?=

# Optional root for per-kernel data/PMA directories. When set, each kernel build
# is handed its own `--data-dir $(HOONC_PMA_ROOT)/<name>` so several hoonc runs
# can proceed in parallel without sharing (or clobbering) PMA/checkpoint state.
# Left empty for local builds so the default shared ~/.nockapp/hoonc cache is
# reused across kernels. See the `build-kernels-ci` target below.
HOONC_PMA_ROOT ?=
hoonc_data_flag = $(if $(HOONC_PMA_ROOT),--data-dir $(HOONC_PMA_ROOT)/$(1))

.PHONY: ensure-dirs
ensure-dirs:
	mkdir -p hoon
	mkdir -p assets
	mkdir -p assets/native

.PHONY: build-trivial
build-trivial: ensure-dirs
	$(call show_env_vars)
	echo '%trivial' > hoon/trivial.hoon
	$(HOONC) $(HOONC_FLAGS) --arbitrary hoon/trivial.hoon

HOON_TARGETS=assets/dumb.jam assets/wal.jam assets/miner.jam assets/peek.jam assets/bridge.jam assets/roswell.jam
HONK_HOON_TARGETS=assets/native/dumb.jam assets/native/wal.jam assets/native/miner.jam assets/native/peek.jam assets/native/bridge.jam


HOON_SRCS := $(find hoon -type file -name '*.hoon')

HONK_ASSET_WORKDIR=target/honk-assets
HONK_WRAPPER_ASSET_DIR=$(HONK_ASSET_WORKDIR)/wrapper-assets
HONK_HOONC_OCTS_TYPED_DYNOCK=$(HONK_ASSET_WORKDIR)/data-import-typed-dynock.jam
HONK_HOONC ?= cargo run --release -p hoonc --bin hoonc --
HONK_ASSET_TARGETS=assets/honc-cold-138.jam crates/honk/assets/hoonc-octs-type-138.jam

.PHONY: build-honk-assets
build-honk-assets: $(HONK_ASSET_TARGETS) ## Build cached honk compiler assets

assets/honc-cold-138.jam: ensure-dirs honk-cargo-build hoon/common/hoon.hoon $(HOON_SRCS)
	$(call show_env_vars)
	rm -rf $(HONK_WRAPPER_ASSET_DIR)
	$(HONK_RUN_ENV) $(HONK_BIN) --new --dump-wrapper-assets $(HONK_WRAPPER_ASSET_DIR) --prelude hoon/common/hoon.hoon hoon
	cp $(HONK_WRAPPER_ASSET_DIR)/honc-cold-138.jam $@

crates/honk/assets/hoonc-octs-type-138.jam: ensure-dirs hoon/probes/hoon-compiler/hoonc_octs_type_probe.hoon hoon/jams/small-blocks.jam $(HOON_SRCS)
	$(call show_env_vars)
	mkdir -p crates/honk/assets $(HONK_ASSET_WORKDIR)
	rm -f data-import-typed-dynock.jam $(HONK_HOONC_OCTS_TYPED_DYNOCK)
	$(HONK_HOONC) $(HOONC_FLAGS) --dynock-typed --output data-import-typed-dynock.jam hoon/probes/hoon-compiler/hoonc_octs_type_probe.hoon hoon
	mv data-import-typed-dynock.jam $(HONK_HOONC_OCTS_TYPED_DYNOCK)
	cargo run --release -p honk-tools --bin extract-hoonc-octs-type -- $(HONK_HOONC_OCTS_TYPED_DYNOCK) $@

.PHONY: honk-cargo-build
honk-cargo-build:
	@if [ -x "$(HONK_BIN)" ] || command -v "$(HONK_BIN)" >/dev/null 2>&1; then \
		echo "Using honk at $(HONK_BIN)"; \
	elif [ -f ../Cargo.toml ]; then \
		$(MAKE) -C .. honk-cargo-build; \
	elif [ -f Cargo.toml ]; then \
		HONC_COLD_138_ALLOW_MISSING=1 HONK_HOONC_OCTS_TYPE_138_ALLOW_MISSING=1 cargo build --release -p honk --bin honk; \
	else \
		echo "No honk binary at $(HONK_BIN). Set HONK_BIN=/path/to/honk, or run from the full workspace so ../Cargo.toml exists." >&2; \
		exit 1; \
	fi

.PHONY: build-hoon-honk build-hoon-honk-cached build-honk-kernel-assets nuke-honk-assets
build-hoon-honk: ## Build open kernel JAM artifacts with honk into assets/native from scratch
	$(MAKE) nuke-honk-assets
	$(MAKE) build-hoon-honk-cached

build-hoon-honk-cached: ensure-dirs honk-cargo-build ## Build open kernel JAM artifacts with honk into assets/native without nuking existing native assets
	$(call show_env_vars)
	$(HONK_RUN_ENV) $(HONK_BIN) --new --output assets/native/dumb.jam --prelude hoon/common/hoon.hoon hoon/apps/dumbnet/outer.hoon hoon
	$(HONK_RUN_ENV) $(HONK_BIN) --new --output assets/native/miner.jam --prelude hoon/common/hoon.hoon hoon/apps/dumbnet/miner.hoon hoon
	$(HONK_RUN_ENV) $(HONK_BIN) --new --output assets/native/peek.jam --prelude hoon/common/hoon.hoon hoon/apps/peek/peek.hoon hoon
	$(HONK_RUN_ENV) $(HONK_BIN) --new --output assets/native/wal.jam --prelude hoon/common/hoon.hoon hoon/apps/wallet/wallet.hoon hoon
	$(HONK_RUN_ENV) $(HONK_BIN) --new --output assets/native/bridge.jam --prelude hoon/common/hoon.hoon hoon/apps/bridge/bridge.hoon hoon
	@ls -alh $(HONK_HOON_TARGETS)

build-honk-kernel-assets: build-hoon-honk ## Alias for building open kernel JAM artifacts with honk into assets/native

nuke-honk-assets: ## Remove honk-built open kernel JAM artifacts from assets/native only
	rm -f assets/native/*.jam

.PHONY: nuke-hoonc-data
nuke-hoonc-data:
	rm -rf .data.hoonc
	rm -rf ~/.nockapp/hoonc

.PHONY: nuke-assets
nuke-assets:
	rm -f assets/*.jam

.PHONY: build-hoon-all
build-hoon-all: nuke-assets update-hoonc ensure-dirs build-trivial $(HOON_TARGETS)
	$(call show_env_vars)

.PHONY: build-hoon
build-hoon: ensure-dirs update-hoonc $(HOON_TARGETS)
	$(call show_env_vars)

.PHONY: build-assets
build-assets: ensure-dirs $(HOON_TARGETS)
	$(call show_env_vars)

# Number of kernel builds to run concurrently in `build-kernels-ci`.
KERNEL_JOBS ?= 2

# Build every kernel in parallel, each in its own ephemeral PMA dir. Used by CI
# to cut the previously-serial kernel build time. Each hoonc run is `--ephemeral`
# (no PMA/event-log/checkpoint writes) and gets a unique `--data-dir` plus a
# unique output filename, so KERNEL_JOBS builds can run at once without sharing
# state or clobbering each other's output. Override concurrency with KERNEL_JOBS.
.PHONY: build-kernels-ci
build-kernels-ci: ensure-dirs
	$(call show_env_vars)
	$(MAKE) -j$(KERNEL_JOBS) \
		HOONC_FLAGS="$(HOONC_FLAGS) --ephemeral" \
		HOONC_PMA_ROOT="$(CURDIR)/.hoonc-pma" \
		$(HOON_TARGETS)


## Build dumb.jam with hoonc
assets/dumb.jam: ensure-dirs hoon/apps/dumbnet/outer.hoon $(HOON_SRCS)
	$(call show_env_vars)
	rm -f assets/dumb.jam
	$(HOONC) $(HOONC_FLAGS) $(call hoonc_data_flag,dumb) --output $(@F) hoon/apps/dumbnet/outer.hoon hoon
	mv $(@F) $@

## Build wal.jam with hoonc
assets/wal.jam: ensure-dirs hoon/apps/wallet/wallet.hoon $(HOON_SRCS)
	$(call show_env_vars)
	rm -f assets/wal.jam
	$(HOONC) $(HOONC_FLAGS) $(call hoonc_data_flag,wal) --output $(@F) hoon/apps/wallet/wallet.hoon hoon
	mv $(@F) $@

## Build mining.jam with hoonc
assets/miner.jam: ensure-dirs hoon/apps/dumbnet/miner.hoon $(HOON_SRCS)
	$(call show_env_vars)
	rm -f assets/miner.jam
	$(HOONC) $(HOONC_FLAGS) $(call hoonc_data_flag,miner) --output $(@F) hoon/apps/dumbnet/miner.hoon hoon
	mv $(@F) $@

## Build peek.jam with hoonc
assets/peek.jam: ensure-dirs hoon/apps/peek/peek.hoon $(HOON_SRCS)
	$(call show_env_vars)
	rm -f assets/peek.jam
	$(HOONC) $(HOONC_FLAGS) $(call hoonc_data_flag,peek) --output $(@F) hoon/apps/peek/peek.hoon hoon
	mv $(@F) $@

## Build bridge.jam
assets/bridge.jam: ensure-dirs hoon/apps/bridge/bridge.hoon $(HOON_SRCS)
	$(call show_env_vars)
	rm -f assets/bridge.jam
	$(HOONC) $(HOONC_FLAGS) $(call hoonc_data_flag,bridge) --output $(@F) hoon/apps/bridge/bridge.hoon hoon
	mv $(@F) $@

## Build roswell.jam
assets/roswell.jam: ensure-dirs hoon/apps/roswell/roswell.hoon $(HOON_SRCS)
	$(call show_env_vars)
	rm -f assets/roswell.jam
	$(HOONC) $(HOONC_FLAGS) $(call hoonc_data_flag,roswell) --output $(@F) hoon/apps/roswell/roswell.hoon hoon
	mv $(@F) $@

# ---------------------------------------------------------------------------
# zig-build targets for cross-compilation
# ---------------------------------------------------------------------------
# Cross-compile Linux x86_64 release binaries from any host (notably macOS)
# with cargo-zigbuild: https://github.com/rust-cross/cargo-zigbuild
#
# Install the toolchain once with `make install-cargo-zigbuild`. The zig
# wrappers under tools/zig/ are wired in via zigbuild_aws_lc_env (defined near
# the top of this file) so aws-lc-sys and jemalloc cross-compile correctly. The
# wrappers resolve a Zig executable from $ZIG_EXE, then a Bazel-staged
# hermetic Zig, then `zig` on PATH; cargo-zigbuild otherwise fetches its own.
#
# All targets build against ZIGBUILD_TARGET (glibc 2.39, the floor Ubuntu 24.04
# LTS ships, matching the ubuntu:24.04 runtime Dockerfile).

.PHONY: install-cargo-zigbuild
install-cargo-zigbuild: ## Install cargo-zigbuild for cross-compilation
	cargo install --locked cargo-zigbuild

## Cross-compile the main release binaries (node, wallet, peek) for Linux x86_64.
## nockchain is built with jemalloc.
.PHONY: zig-build
zig-build: ## Cross-compile nockchain (jemalloc), nockchain-wallet and nockchain-peek for Linux x86_64
	$(call zigbuild_aws_lc_env,$(ZIGBUILD_TARGET)) cargo zigbuild --release --features jemalloc --target $(ZIGBUILD_TARGET) --bin nockchain
	$(call zigbuild_aws_lc_env,$(ZIGBUILD_TARGET)) cargo zigbuild --release --target $(ZIGBUILD_TARGET) --bin nockchain-wallet
	$(call zigbuild_aws_lc_env,$(ZIGBUILD_TARGET)) cargo zigbuild --release --target $(ZIGBUILD_TARGET) --bin nockchain-peek

# nockchain-api uses jemalloc as its default global allocator on Linux: there is
# no `jemalloc` feature to pass, it is only disabled by the `malloc`/
# `tracing-heap` features or on Apple/Miri. So this plain cross-build is already
# a jemalloc build.
.PHONY: zig-build-nockchain-api
zig-build-nockchain-api: ## Cross-compile nockchain-api (jemalloc) for Linux x86_64
	$(call zigbuild_aws_lc_env,$(ZIGBUILD_TARGET)) cargo zigbuild --release -p nockchain-api --target $(ZIGBUILD_TARGET) --bin nockchain-api

.PHONY: zig-build-nockchain-bridge-sequencer
zig-build-nockchain-bridge-sequencer: ## Cross-compile the bridge sequencer binaries for Linux x86_64
	$(call zigbuild_aws_lc_env,$(ZIGBUILD_TARGET)) cargo zigbuild --release -p nockchain-bridge-sequencer --target $(ZIGBUILD_TARGET) --bin nockchain-bridge-sequencer
	$(call zigbuild_aws_lc_env,$(ZIGBUILD_TARGET)) cargo zigbuild --release -p nockchain-bridge-sequencer --target $(ZIGBUILD_TARGET) --bin nockchain-bridge-sequencer-ctl

.PHONY: zig-build-bridge
zig-build-bridge: ## Cross-compile the bridge for Linux x86_64
	$(call zigbuild_aws_lc_env,$(ZIGBUILD_TARGET)) cargo zigbuild --release --target $(ZIGBUILD_TARGET) --bin bridge

.PHONY: zig-build-roswell
zig-build-roswell: ## Cross-compile roswell for Linux x86_64
	$(call zigbuild_aws_lc_env,$(ZIGBUILD_TARGET)) cargo zigbuild --release --target $(ZIGBUILD_TARGET) --bin roswell

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

ZIGBUILD_TARGET ?= x86_64-unknown-linux-gnu.2.39
DOCKER_IMAGE ?= nockchain-local
DOCKER_MEM ?= 32g
# DOCKER_MEM_SWAP ?= 32g
DOCKER_P2P_PORT ?= 30000
DOCKER_DATA_DIR ?= $(CURDIR)/.data.nockchain
DOCKER_NOCKCHAIN_ENVS ?=
DOCKER_NOCKCHAIN_ARGS ?=
DOCKER_METRICS_COMPOSE ?= docker-compose.metrics.yml
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

.PHONY: install-cargo-zigbuild
install-cargo-zigbuild:
	cargo install --locked cargo-zigbuild

.PHONY: zig-build-bridge
zig-build-bridge:
	cargo zigbuild --release --target $(ZIGBUILD_TARGET) --bin bridge

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
	docker build -t $(DOCKER_IMAGE) .
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

.PHONY: ensure-dirs
ensure-dirs:
	mkdir -p hoon
	mkdir -p assets

.PHONY: build-trivial
build-trivial: ensure-dirs
	$(call show_env_vars)
	echo '%trivial' > hoon/trivial.hoon
	$(HOONC) $(HOONC_FLAGS) --arbitrary hoon/trivial.hoon

HOON_TARGETS=assets/dumb.jam assets/wal.jam assets/miner.jam assets/peek.jam assets/bridge.jam

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

HOON_SRCS := $(find hoon -type file -name '*.hoon')

## Build dumb.jam with hoonc
assets/dumb.jam: ensure-dirs hoon/apps/dumbnet/outer.hoon $(HOON_SRCS)
	$(call show_env_vars)
	rm -f assets/dumb.jam
	$(HOONC) $(HOONC_FLAGS) hoon/apps/dumbnet/outer.hoon hoon
	mv out.jam assets/dumb.jam

## Build wal.jam with hoonc
assets/wal.jam: ensure-dirs hoon/apps/wallet/wallet.hoon $(HOON_SRCS)
	$(call show_env_vars)
	rm -f assets/wal.jam
	$(HOONC) $(HOONC_FLAGS) hoon/apps/wallet/wallet.hoon hoon
	mv out.jam assets/wal.jam

## Build mining.jam with hoonc
assets/miner.jam: ensure-dirs hoon/apps/dumbnet/miner.hoon $(HOON_SRCS)
	$(call show_env_vars)
	rm -f assets/miner.jam
	$(HOONC) $(HOONC_FLAGS) hoon/apps/dumbnet/miner.hoon hoon
	mv out.jam assets/miner.jam

## Build peek.jam with hoonc
assets/peek.jam: ensure-dirs hoon/apps/peek/peek.hoon $(HOON_SRCS)
	$(call show_env_vars)
	rm -f assets/peek.jam
	$(HOONC) $(HOONC_FLAGS) hoon/apps/peek/peek.hoon hoon
	mv out.jam assets/peek.jam

## Build bridge.jam
assets/bridge.jam: ensure-dirs hoon/apps/bridge/bridge.hoon $(HOON_SRCS)
	$(call show_env_vars)
	rm -f assets/bridge.jam
	$(HOONC) $(HOONC_FLAGS) hoon/apps/bridge/bridge.hoon hoon
	mv out.jam assets/bridge.jam

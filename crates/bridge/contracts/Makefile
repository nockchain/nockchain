# Makefile for Foundry + Tenderly integration

.PHONY: help build test clean verify install deps deploy upgrade validate integration-test list-deployments emergency-disable emergency-enable transfer-ownership

help:
	@echo "Available commands:"
	@echo "  build              - Build the project"
	@echo "  test               - Run tests"
	@echo "  clean              - Clean build artifacts"
	@echo "  verify             - Verify contracts on Tenderly"
	@echo "  deploy             - Deploy contracts using Tenderly RPC"
	@echo "  upgrade            - Upgrade MessageInbox implementation"
	@echo "  validate           - Validate deployed contract configuration"
	@echo "  integration-test   - Run integration tests against deployed contracts"
	@echo "  list-deployments   - List all deployment files (use HISTORY=1 for history)"
	@echo "  emergency-disable  - Disable withdrawals (emergency only)"
	@echo "  emergency-enable   - Re-enable withdrawals after emergency"
	@echo "  transfer-ownership - Transfer ownership to new address (e.g., multisig)"
	@echo "  install            - Install forge, tenderly, and deps"
	@echo "  deps               - Install forge dependencies"

build:
	forge build --optimize true

build-strict:
	forge build --optimize true --deny-warnings

test:
	forge test --gas-report -vvv

clean:
	forge clean
	rm -f deployments.json
	rm -rf deployments/*.json

verify:
	@if [ -f .env ]; then \
		set -a; \
		. ./.env; \
		set +a; \
	fi; \
	./scripts/verify_contracts.sh $(DEPLOYMENTS_PATH)

deploy:
	@if [ -f .env ]; then \
		set -a; \
		. ./.env; \
		set +a; \
	fi; \
	./scripts/deploy_tenderly.sh $(EXTRA_FLAGS)

upgrade:
	@if [ -f .env ]; then \
		set -a; \
		. ./.env; \
		set +a; \
	fi; \
	./scripts/upgrade_tenderly.sh $(DEPLOYMENTS_PATH)

validate:
	@if [ -f .env ]; then \
		set -a; \
		. ./.env; \
		set +a; \
	fi; \
	./scripts/validate_deployment.sh $(DEPLOYMENTS_PATH)

integration-test:
	@if [ -f .env ]; then \
		set -a; \
		. ./.env; \
		set +a; \
	fi; \
	forge script forge/IntegrationTest.s.sol:IntegrationTest \
		--rpc-url "$${TENDERLY_RPC_URL}" \
		--private-key "$${TEST_ACCOUNT_PRIVATE_KEY:-$${TENDERLY_PRIVATE_KEY}}" \
		--broadcast \
		--slow

list-deployments:
	@./scripts/list_deployments.sh $(if $(HISTORY),--history,)

emergency-disable:
	@if [ -f .env ]; then \
		set -a; \
		. ./.env; \
		set +a; \
	fi; \
	./scripts/emergency_disable_withdrawals.sh $(DEPLOYMENTS_PATH)

emergency-enable:
	@if [ -f .env ]; then \
		set -a; \
		. ./.env; \
		set +a; \
	fi; \
	./scripts/emergency_enable_withdrawals.sh $(DEPLOYMENTS_PATH)

transfer-ownership:
	@if [ -z "$(NEW_OWNER)" ]; then \
		echo "Error: NEW_OWNER not set. Usage: make transfer-ownership NEW_OWNER=0x..." >&2; \
		exit 1; \
	fi; \
	if [ -f .env ]; then \
		set -a; \
		. ./.env; \
		set +a; \
	fi; \
	./scripts/transfer_ownership.sh $(NEW_OWNER) $(DEPLOYMENTS_PATH)

install:
	@echo "Installing Foundry (forge/cast) if missing..."
	@if command -v foundryup >/dev/null 2>&1; then \
		true; \
	else \
		curl -L https://foundry.paradigm.xyz | bash; \
	fi
	@if [ -x "$$HOME/.foundry/bin/foundryup" ]; then \
		"$$HOME/.foundry/bin/foundryup"; \
	elif command -v foundryup >/dev/null 2>&1; then \
		foundryup; \
	else \
		echo "foundryup not found after install" >&2; exit 1; \
	fi
	@echo "Installing Tenderly CLI if missing..."
	@command -v tenderly >/dev/null 2>&1 || ( \
		if [ "$$(uname)" = "Darwin" ]; then \
			curl -s https://raw.githubusercontent.com/Tenderly/tenderly-cli/master/scripts/install-macos.sh | sh; \
		elif [ "$$(uname)" = "Linux" ]; then \
			curl -s https://raw.githubusercontent.com/Tenderly/tenderly-cli/master/scripts/install-linux.sh | sh; \
		else \
			echo "Unsupported OS for Tenderly installer" >&2; exit 1; \
		fi \
	)
	$(MAKE) deps

deps:
	@echo "Installing forge dependencies..."
	@mkdir -p lib
	@test -d lib/forge-std || forge install --no-git --shallow foundry-rs/forge-std@v1.12.0
	@test -d lib/openzeppelin-contracts || forge install --no-git --shallow OpenZeppelin/openzeppelin-contracts@v5.5.0
	@test -d lib/openzeppelin-contracts-upgradeable || forge install --no-git --shallow OpenZeppelin/openzeppelin-contracts-upgradeable@v5.5.0
	@echo "Dependencies are ready."

# Bridge Release Punchlist

Status: Draft
Owner: Nockchain Maintainers
Last Reviewed: 2026-05-06

This punchlist tracks release-blocking bridge launch checks that are easy to miss
when moving from fakenet to mainnet.

## Mainnet / Fakenet Configuration

1. Remove the Hoon mainnet bridge-lock-root default before launch.
   - `bridge-lock-root` should be required state, not a mold fallback.
   - The active root must come from `bridge_lock_root` in bridge config.
   - Silent fallback to the mainnet root is unsafe for fakenet and future
     network-specific deployments.
2. Render `bridge_lock_root` explicitly in every production bridge config.
   - Mainnet must use the signer-derived canonical mainnet bridge multisig root.
   - Fakenet / bridge-dev must use the signer-derived testing root.
   - Startup should continue to reject mismatches between the configured root,
     signer-derived root, and expected network root.
3. Verify Solidity contract addresses against the selected Base network.
   - Confirm `MessageInbox` proxy address, not implementation address.
   - Confirm `MessageInbox.nock()` matches the configured `Nock` token.
   - Confirm `Nock.inbox()` matches the configured `MessageInbox`.
   - Confirm on-chain bridge node addresses and threshold match bridge config.
4. Add an explicit network / Base chain-id guard before mainnet launch.
   - Mainnet Base must be chain id `8453`.
   - Fakenet / VNET / Base Sepolia configs must not be accepted as mainnet.
5. Confirm withdrawal launch toggles.
   - `withdrawalsEnabled()` may be false pre-launch.
   - Production readiness validation should fail until withdrawals are enabled.

## Rendered Production Config

1. Render the exact production bridge config from deployment automation.
   - Do not rely on template inspection alone.
   - Parse the rendered TOML with the bridge binary or a config parser test.
2. Confirm all required mainnet fields are present.
   - `bridge_lock_root`
   - `inbox_contract_address`
   - `nock_contract_address`
   - `base_confirmation_depth`
   - `nockchain_confirmation_depth`
   - `withdrawal_activation_nock_next_height`
   - all five `[[nodes]]` entries
3. Confirm production values are not inherited from fakenet / bridge-dev.
   - No Base Sepolia, Tenderly VNET, localhost, or dev-only endpoints unless
     explicitly part of the target deployment.
   - No dev signer keys, placeholder node PKHs, or generated bridge-dev roots.
4. Confirm secrets are injected from secret management.
   - Private keys and object-store credentials must not be committed in rendered
     configs or checked-in vars.

## R2 / Object-Store Journal Configuration

1. Configure the sequencer journal for production before serving sequencer RPCs.
   - `[sequencer_journal].enabled` defaults to true.
   - Production should fail closed if the R2 / S3-compatible mirror is enabled
     but unavailable.
   - `bridge-dev` may explicitly disable the mirror.
2. Use a dedicated Cloudflare R2 bucket for the sequencer journal.
   - Do not share the bucket with unrelated logs, build artifacts, or backups.
   - Use a deployment-specific `journal_id`, for example
     `base-mainnet-bridge-<deployment-id>`.
   - Use a stable prefix such as `withdrawal-sequencer`.
   - Set `[sequencer_journal].verifier_address` to the public Ethereum address
     of the dedicated journal signing key.
3. Required object-store settings:
   - `endpoint = "https://<account-id>.r2.cloudflarestorage.com"`
   - `bucket = "<dedicated-journal-bucket>"`
   - `region = "auto"`
   - `prefix = "withdrawal-sequencer"`
   - `journal_id = "base-mainnet-bridge-<deployment-id>"`
   - `access_key_id` and `secret_access_key` should come from environment or
     secret management, not checked-in config.
4. Bucket retention policy:
   - Do not configure lifecycle expiration or manual deletion for journal
     objects until explicit checkpoint / compaction tooling has landed and all
     sequencer binaries can recover from those checkpoints.
   - Treat journal objects as append-only recovery data with indefinite
     retention for launch.
   - Current startup recovery expects the local cursor's remote event object to
     remain readable, so deleting old objects can strand otherwise valid local
     sequencer state.
5. Bucket access policy:
   - Sequencer credentials need read, list, and write access for the journal
     prefix.
   - Prefer credentials scoped to the dedicated bucket or journal prefix.
   - Avoid broad account-level object-store credentials on production hosts.
6. Recovery expectations:
   - The local sequencer DB is a projection.
   - The remote journal is the durable source for exact sequencer resume.
   - If the local cursor is ahead of R2, recovery must fail closed.
   - If R2 is ahead of the local cursor, startup recovery must replay successors
     before serving sequencer RPCs.

## Recovery Drill

1. Run an empty-DB sequencer recovery drill before launch.
   - Start from an empty sequencer SQLite DB.
   - Replay from the configured R2 / S3-compatible journal.
   - Verify recovered withdrawals, reserved inputs, raw transaction artifacts,
     and journal cursor.
2. Run a behind-DB recovery drill.
   - Start from a DB whose cursor is behind the remote journal.
   - Verify startup replays successors before serving sequencer RPCs.
3. Run a fail-closed recovery drill.
   - Local cursor ahead of R2 must fail closed.
   - Missing cursor with non-empty sequencer state must fail closed.
   - Corrupted cursor event or mismatched projection row must fail closed.
4. Confirm recovery never reconstructs unconfirmed authorized raw tx bytes from
   chain data.
   - In-flight unconfirmed withdrawals require `authorized_raw_tx` from SQLite or
     the remote journal.

## Nockchain / Kernel State

1. Confirm every production node is running the intended bridge kernel jam.
   - Check reboot logs or startup output for the kernel version / jam identity.
   - All bridge nodes should agree before withdrawals are enabled.
2. Confirm kernel projection cursors are coherent.
   - Existing kernel-derived local tables must have a usable projection cursor.
   - Missing cursor plus non-empty kernel-derived rows is fail-closed.
   - Empty local tables may initialize only at the configured withdrawal
     activation cutoffs.
3. Confirm mainnet confirmation settings.
   - `nockchain_confirmation_depth` should match the production value.
   - Sequencer confirmation depth should be explicitly configured and reviewed if
     it diverges from the bridge kernel depth.
4. Confirm safe-origin liquidity filtering is active.
   - Withdrawal input selection must exclude notes whose origin is newer than the
     safe Nockchain tip.
   - Recent refund notes become selectable only after the confirmation window.

## RPC And Fee-Method Checks

1. Confirm Base RPC points at the intended production network.
   - `eth_chainId` must return Base mainnet `8453`.
   - `eth_getBlockByNumber`, `eth_getLogs`, and contract calls must succeed.
2. Confirm transaction fee RPC methods work on the production endpoint.
   - `eth_feeHistory` should work for EIP-1559 fee estimation.
   - `eth_gasPrice` should not hang.
   - If an endpoint requires legacy gas pricing or explicit gas overrides, record
     that exception before launch.
3. Confirm the production Base RPC is not accidentally using a Tenderly VNET
   endpoint.
   - Any `.rpc.tenderly.co` or virtual endpoint must be intentional and reviewed.
4. Confirm Nockchain public API and gRPC endpoints are caught up.
   - The sequencer should not serve fresh withdrawal work if its Nockchain or
     Base view is behind the journal / required recovery frontier.

## Liquidity And Withdrawal Readiness

1. Confirm the bridge has enough confirmed spendable Nockchain notes for expected
   withdrawal demand.
   - Use the safe-origin filtered balance, not raw tip balance.
   - Reserved inputs must be subtracted from available liquidity.
2. Confirm planner idle behavior when liquidity is unsafe or insufficient.
   - The bridge should wait and retry later, not construct spends from unsafe
     notes.
3. Confirm withdrawal burn inputs are validated before users submit.
   - Destination lock root must fit the Solidity `bytes32` withdrawal field.
   - Amounts must satisfy minimum event and fee policy expectations.

## Networking And Operations

1. Confirm production firewall and peer reachability.
   - Bridge ingress ports are reachable only by intended peers.
   - Sequencer gRPC is reachable by bridge nodes.
   - Base RPC, Nockchain public API, and object-store endpoints are reachable
     from production hosts.
2. Confirm systemd restart behavior.
   - Restart should preserve local DBs and logs.
   - Startup recovery must complete before the sequencer serves RPCs.
3. Confirm production observability labels.
   - Logs and metrics should use `mainnet` / production environment labels, not
     `testnet`.
4. Snapshot local state before enabling withdrawals.
   - Capture bridge DBs, sequencer DB, rendered configs, binary versions, and
     kernel jam identity.

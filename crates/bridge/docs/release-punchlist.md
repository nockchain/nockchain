# Bridge Release Punchlist

Status: Draft
Owner: Nockchain Maintainers
Last Reviewed: 2026-08-27

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
   - `MessageInbox.withdrawalsEnabled()` controls whether Base accepts new burns.
   - `withdrawal_processing_enabled` controls whether bridge nodes assemble,
     sign, exchange, and submit withdrawal proposals.
   - Keep both gates false while recovery, monitoring, SDK, frontend, and
     certification gates are incomplete.
   - Enable both only during the controlled cutover after every bridge node and
     the sequencer report the certified readiness frontier.

## Rendered Production Config

1. Render the exact production bridge config from deployment automation.
   - Do not rely on template inspection alone.
   - Parse the rendered TOML with the bridge binary or a config parser test.
2. Confirm all required mainnet fields are present.
   - `bridge_lock_root`
   - `inbox_contract_address`
   - `nock_contract_address`
   - `base_chain_id`
   - `base_confirmation_depth`
   - `nockchain_confirmation_depth`
   - `withdrawal_processing_enabled`
   - `withdrawal_activation_nock_next_height`
   - `withdrawal_policy = "withdrawal-policy-v1"`
   - all five `[[nodes]]` entries
3. Confirm production values are not inherited from fakenet / bridge-dev.
   - No Base Sepolia, Tenderly VNET, localhost, or dev-only endpoints unless
     explicitly part of the target deployment.
   - No dev signer keys, placeholder node PKHs, or generated bridge-dev roots.
4. Confirm secrets are injected from secret management.
   - Private keys and object-store credentials must not be committed in rendered
     configs or checked-in vars.

## Withdrawal Contract Compatibility

1. Confirm the production contract identity and configuration.
   - Record Base chain id, `MessageInbox` proxy, `Nock` token, code hashes,
     reciprocal pairing, owner, five signers, and threshold.
2. Confirm the official SDK implements `WithdrawalWireV1`.
   - Exact calldata length is 116 bytes.
   - Bytes `68..76` are ASCII `NOCKWD1!`.
   - Bytes `76..116` are the five unsigned 64-bit big-endian Tip5 limbs.
   - Commitment vectors match
     `test-fixtures/withdrawal_wire_v1_vectors.json` byte for byte.
   - The published SDK version is immutable and installable without Git SSH
     credentials or a moving branch.
3. Confirm NockSwap exclusively uses the official codec.
   - No generated `writeContract` call invokes
     `burn(uint256,bytes32)`.
   - The exact same raw calldata is locally decoded, simulated, gas-estimated,
     and submitted.
   - Account, chain, token, amount, or destination changes invalidate the
     previous calldata.
4. Confirm malformed-burn quarantine is operational.
   - Every excluded `BurnForWithdrawal` persists one immutable
     `sequencer_base_burn_rejections` row.
   - Alerts include chain/deployment, tx hash, log index, `base_event_id`,
     burner, amount, commitment, rejection code, and detail.
   - A malformed burn followed by a valid burn must advance the scanner and
     admit only the valid burn.
   - Zero unresolved malformed-burn incidents remain at cutover.
5. Confirm the compensation process is rehearsed.
   - Ordinary 68-byte burns cannot be replayed because the destination is
     absent.
   - Governance and independent-verifier roles are assigned.
   - Every compensation has an exact coordinate-validated config entry on all
     bridge nodes and the sequencer.
   - Sequencer startup persists those entries before Base scanning or RPC.
   - Public lookup returns `COMPENSATED`; registration and Base recovery reject
     the same identity before any compensation transfer.
   - A compensated `base_event_id` cannot later enter withdrawal state.

## NockSwap End-to-End Product Gate

1. Pin NockSwap to one immutable, publicly fetchable Iris SDK release.
   - The local `0.3.3` candidate is not a release until it is published and a
     clean install resolves it without a sibling checkout, moving branch, or
     Git SSH credentials.
   - Run the published Rust parity vectors from the installed package.
2. Keep the Base-to-Nockchain route flag disabled until every item in this
   section passes together.
   - A direction selector or Base wallet connection is not launch proof.
   - Runtime/UI state manipulation must not bypass the same flag.
3. Verify the exact client value boundary.
   - User amount remains a decimal string plus bigint nicks/Base units.
   - Unsupported nick precision is rejected before the wallet opens.
   - Displayed, simulated, and encoded amounts are identical.
4. Verify the Base transaction lifecycle.
   - Resolve a v1 PKH or explicit lock root to one normalized five-limb value
     and show it before confirmation.
   - Self-validate one 116-byte payload and pass those exact bytes to simulation,
     gas estimation, and raw submission.
   - Wait for a successful Base receipt and verify the exact
     `BurnForWithdrawal` log before entering `withdrawal_pending`.
   - Persist chain id, tx hash, log index, `base_event_id`, amount, destination,
     calldata identity, and timestamps before polling.
5. Verify the public withdrawal lifecycle.
   - Reload resumes the existing pending record and never offers an automatic
     second burn.
   - Poll only the public deployment-bound lookup/history API.
   - Render success only after confirmed Nockchain settlement evidence.
   - Delayed, invalidated, reorg-held, corrupt, and unavailable states provide
     safe next action and support references.
6. Verify fees against the submitted transaction.
   - Estimate the exact 116-byte Base request.
   - Include or explicitly disclose the OP Stack L1 data fee.
   - Label Nockchain payout as an estimate until confirmed.
7. Run browser E2E from Base wallet through reload to confirmed settlement.
   - Assert exactly one burn and one payout.
   - Assert ordinary/malformed ABI paths cannot be initiated.
   - Record exact NockSwap, SDK, backend, contracts, policy, and deployment
     revisions in the result.

## Private Sequencer, Reorg, And Behavioral Gates

1. Run private sequencer smoke from all five intended production bridge hosts.
   - The intended private network or tunnel path must succeed.
   - The public listener must not serve private `WithdrawalSequencer` methods.
2. Certify confirmation assumptions and post-confirmation recovery.
   - Lock the approved Base and Nockchain confirmation depths in every
     production node; zero or reduced depths require a new safety review.
   - Ordinary shallow forks are withheld from Hoon. The real-kernel mismatch
     tests model history changing only after it crossed that buffer.
   - A post-confirmation mismatch must stop every bridge, block sequencer
     mutation/readiness, and trigger closure of the Base burn gate.
   - Exercise the audited ancestor restore/rebuild, canonical replay, and Rust
     reconciliation procedure. Automatic Hoon rewind is optional when this
     procedure satisfies the same safety invariant.
3. Keep the pinned Forge gate required on contract, decoder, fixture, or
   withdrawal changes.
   - The current required suite is 57 tests, including 13 withdrawal
     compatibility/fail-closed tests and explicit 68-vs-116 behavior.
   - Forge success does not imply Rust, Hoon, Anvil observer, R2, or browser
     success.
4. Run a deterministic nonignored local Anvil smoke using the exact published
   SDK artifact and production Rust decoder.
5. Run real-R2, real-chain restart, degraded-quorum, and reorg suites with
   revision-bound redacted artifacts.
6. Do not enable the on-chain gate or frontend flag while any P0, unresolved
   malformed burn, unapproved compensation case, recovery hold, or proof gap
   remains.

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
3. Confirm official withdrawal input validation before users submit.
   - Destination parses to five bounded Tip5 limbs and serializes to 40 bytes.
   - Amount satisfies `withdrawal-policy-v1`, including exact nick divisibility
     and the inclusive 100,000-NOCK gross minimum.
   - A current quote reports a positive estimated payout.
   - Final 116-byte calldata passes local SDK decode/self-validation.

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

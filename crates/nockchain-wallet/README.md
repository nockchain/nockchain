# Nockchain Wallet

Status: Active
Owner: Nockchain Maintainers
Last Reviewed: 2026-04-01
Canonical/Legacy: Canonical (Tier 1 scoped authority for wallet CLI behavior and operational usage; protocol authority remains in [`PROTOCOL.md`](../../PROTOCOL.md))

`nockchain-wallet` manages local keys and watch-only identifiers, synchronizes
notes through Nockchain gRPC, plans and signs transactions, and submits them to a
node. It is a client of consensus, not a consensus implementation.

## Place in the system

```text
local key store + user request + synchronized notes
                         |
                 nockchain-wallet
              plan -> encode -> sign
                         |
                  public/private gRPC
                         |
                node and Hoon validation
```

`wallet-tx-builder` performs deterministic note selection, fee estimation, lock
resolution, and value allocation. `nockchain-types` provides the Rust mirrors of
transaction nouns. The wallet owns local signing and files; the node owns
admission, mempool state, chain inclusion, and finality.

## Maintained invariants and security properties

- Private keys and seed phrases remain local unless the user explicitly exports
  them. Watch-only state never acquires signing authority.
- Addresses, public keys, and payee hashes are decoded as explicit versions;
  visually similar strings do not bypass key-type checks.
- Transaction planning conserves value, uses checked arithmetic, and represents
  gifts, bridge amounts, fees, and refund separately.
- Fee and timelock decisions use synchronized chain context. A stale view may
  produce a transaction the node rejects, never authority to spend a different
  note.
- The wallet signs the exact canonical transaction noun it displays and saves.
  Changing recipients, fees, note data, locks, or witnesses changes the signed
  digest.
- Manual legacy-v0 and v1 flows retain their distinct ownership and refund
  rules; mixed-version inputs are rejected where the kernel cannot validate
  them as one transaction.
- gRPC acknowledgement is not confirmation. Pending transactions may be
  replaced or rejected, and clients must wait for canonical-chain inclusion.
- Imported key material, exported key files, saved transactions, and local
  wallet state are sensitive operator data and require filesystem protection
  and backups.

Cryptographic safety relies on the Cheetah/Schnorr implementation, secure
entropy for key generation, deterministic derivation, canonical noun hashing,
and `nockchain-types` matching Hoon transaction molds. The node rechecks every
signature, lock, fee, and conservation rule.

## Canonical Scope

This document is Tier 1 canonical for:
- wallet CLI behavior and user-facing operational workflows.
- key import/export/watch workflows and endpoint-selection behavior.
- transaction-construction invocation patterns for supported CLI surfaces.

This document is NOT canonical for:
- protocol/consensus rule interpretation (use [`PROTOCOL.md`](../../PROTOCOL.md)).
- node/runtime architecture policy (use [`ARCHITECTURE.md`](../../ARCHITECTURE.md)).

## Failure Modes And Limits

- CLI flags and command behavior can change as wallet internals evolve.
- Examples can drift if CLI changes are not documented in lockstep.
- This doc cannot adjudicate protocol disputes; where protocol semantics matter, Tier 0 protocol docs win.

## Verification Contract

When wallet CLI behavior or flags change, update this doc in the same change.

Minimum validation:
- `make docs-check`
- `cargo check -p nockchain-wallet`

## Setup

### Generate New Key Pair

```bash
# Generate a new key pair with random entropy. If no active master key set, switches the active key
# to the new key. Otherwise, the active key remains the same
nockchain-wallet keygen
```

### Importing and Exporting Keys

The wallet supports importing and exporting keys:

```bash
# Export all wallet keys to a file (default: keys.export)
nockchain-wallet export-keys

# Import keys from the exported file
nockchain-wallet import-keys --file keys.export

# Import an extended key string
nockchain-wallet import-keys --key "zprv..."

# Generate master private key from seed phrase (version required)
# If you generated your seedphrase before October 2025 then it’s probably version 0
# If you import with version 0 and find that you cannot spend your notes, try
# importing the seed phrase again with version 1.
nockchain-wallet import-keys --seedphrase "your seed phrase here" --version <version | 1 or 0>

# Import watch-only identifiers
nockchain-wallet watch address <base58-pkh-or-pubkey>
nockchain-wallet watch pubkey <base58-pubkey>
nockchain-wallet watch multisig --threshold <M> --participants "<pkh-a>,<pkh-b>,..."

# Import a master public key from exported file
nockchain-wallet import-master-pubkey keys.export

# Show the active master extended private key
nockchain-wallet show-master-zprv

# Show the active master raw private key as base58
nockchain-wallet show-master-prv
```

The exported keys file contains all wallet keys as a `jam` file that can be imported on another instance.
Once imported, watch-only identifiers are kept in sync automatically, so their balances show up in every sync-heavy command without extra flags.

Can be used for:
- Backing up your wallet
- Migrating to a new device
- Sharing public keys with other users

### Connecting to a Nockchain API server

The wallet talks to the gRPC APIs exposed by a running nockchain instance. You can target either the **public** API (default) or the **private** API that is typically bound to `localhost`. You must run a nockchain instance to connect to the private API. The wallet connects to a public Nockchain API server at `23.252.122.18:5556` by default.

#### Public API (default)

```bash
# Use the default public endpoint (23.252.122.18:5556)
nockchain-wallet list-notes

# Or point at a different remote public listener
nockchain-wallet \
  --client public \
  --public-grpc-server-addr https://public-node.example.com \
  list-notes
```
- The wallet syncs its balance based on the pubkeys that are stored in it. Make sure your wallet is loaded with your keys before running sync-heavy commands such as `list-notes`, `list-notes-by-address`, `create-tx`, and `send-tx`. If you do not have pubkeys, import them with `import-keys` (see [Importing and Exporting Keys](#importing-and-exporting-keys)).
- `--public-grpc-server-addr` accepts a bare `host:port` or a full URI (e.g. `http://host:port`).
- If you omit the port, the wallet assumes **80** for `http://` and **443** for `https://` URLs.
- Watch-only pubkeys and addresses are synced automatically alongside your signing keys, so watch-only balances appear in all sync-heavy commands without additional flags.

#### Private API

```bash
# Talk to a private listener running on localhost:5555 (default)
nockchain-wallet --client private list-notes

# Override the private port if your setup uses a different port forward
nockchain-wallet \
  --client private \
  --private-grpc-server-port 6000 \
  list-notes
```

When `--client private` is selected, the wallet spins up the private listener driver so subsequent operations (balance sync and transaction submission) use the private interface automatically. You must have a
nockchain instance running locally to use the private client.

> **Tip:** Ensure the corresponding NockApp gRPC server is running and reachable before issuing wallet commands; otherwise the wallet will fail when attempting to synchronize state.



# Advanced Options

### Derive Child Key

```bash
# Derive child key with index as positional argument
nockchain-wallet derive-child <0-2147483647> --hardened --label <label>

# Examples:
nockchain-wallet derive-child 42
nockchain-wallet derive-child 42 --hardened
nockchain-wallet derive-child 42 --hardened --label "my-key"
```

Derives a child public or private key at the given index from the current master key.

### Managing Addresses

```bash
# List active addresses, shows the current active master addresses and all of its child addresses:
nockchain-wallet list-active-addresses

# List all stored master addresses and see which one is active
nockchain-wallet list-master-addresses

# Promote an existing address (pubkey or pkh) to be the active master
nockchain-wallet set-active-master-address <address-b58>

```

- `%set-active-master-address` accepts either the base58-encoded master pubkey (v0 wallets) or the base58-encoded payee hash address (v1+ wallets) already present in your key store.
- `%list-master-addresses` prints every tracked master address and highlights the one currently in use, making it easy to confirm which derivation tree future operations will follow.
- Both commands operate purely on local state; no network sync is required.




## Listing Notes

### List All Notes

```bash
nockchain-wallet list-notes
```

Displays all notes (UTXOs) currently managed by the wallet, sorted by assets.

### List Notes by Public Key

```bash
nockchain-wallet list-notes-by-address <base58-address>
```

Shows only the notes associated with the specified public key. Useful for filtering wallet contents by address or for multisig scenarios.

### Watch-Only Tracking

Use `nockchain-wallet watch <subcommand>` to track external identifiers without importing private keys.

- `watch address <base58>` – accepts either a schnorr pubkey (v0) or a pay-to-pubkey-hash (v1) string
- `watch pubkey <base58-pubkey>` – shortcut when you know you’re tracking a raw schnorr key
- `watch multisig --threshold <M> --participants "<pkh-a>,<pkh-b>,..."` – records multisig locks so their balances appear in sync results

Once added, run `list-notes`, `list-notes-by-address`, or any other sync-heavy command to see the balances.

Shows only the notes associated with the specified public key. Useful for filtering wallet contents by address or for multisig scenarios.

You must add the watch-only identifier to the wallet before it will be recognized.

### List Notes by Public Key (CSV format)

```bash
nockchain-wallet list-notes-by-address-csv <address>
```

Outputs matching notes in CSV format suitable for analysis or reporting. The output csv has the format: `notes-<public-key>.csv`.

### Show Wallet Data

```bash
nockchain-wallet show-balance
```

Displays the aggregate wallet balance, including the total number of notes and the total nicks held. Additional `%show` paths are not exposed via the CLI.

## Transaction Creation

#### Components of transaction creation

1. **Seeds**: Define where funds are going and how much
2. **Inputs**: Specify which notes (UTXOs) to spend
3. **Transaction**: Combine inputs into a complete transaction
4. **Sign**: Authorize the transaction with private keys
5. **Send Transaction**: Send the final transaction for broadcasting

### Create a Transaction

We support transactions with any amount of input notes going to any number of recipients.

For the common case — paying one or more single-signer p2pkh addresses — you can skip the JSON entirely and pass paired `--to`/`--amount` flags, where **amounts are in nocks**:

```bash
# Ergonomic: send 100 nocks to a p2pkh address (amount is in whole NOCKS, not nicks).
# The wallet prints the saved ./txs/<name>.tx path and the exact send-tx command.
nockchain-wallet create-tx --to <p2pkh-b58> --amount 100

# Fan out to several p2pkh recipients; each --to is paired with one --amount.
# Amounts are whole nocks only (no decimals). --fee is a fee override in nocks.
nockchain-wallet create-tx \
  --to <p2pkh-a> --amount 100 \
  --to <p2pkh-b> --amount 5 \
  --fee 1

# Prefer raw nicks? Use --amount-nicks / --fee-nicks instead (nicks, not nocks).
nockchain-wallet create-tx --to <p2pkh-b58> --amount-nicks 6553600 --fee-nicks 65536

# Deposit onto the Base bridge: --bridge-deposit (nocks) paired with --to-evm-address.
# Minimum deposit is 100,000 nocks; the bridge also charges a 0.3% fee.
nockchain-wallet create-tx --bridge-deposit 100000 --to-evm-address 0x0c8d9cf278d4f3e23b00ea0a16bba2d05c07a7b6
```

`--to A --amount 100` is exactly equivalent to `--recipient '{"kind":"p2pkh","address":"A","amount":6553600}'` (100 × 65536 nicks). Amounts are whole nocks only — there is no decimal input. `--to`/`--amount` and `--recipient` may be combined in one command. Use `--amount-nicks` to give a `--to` amount in raw nicks (mutually exclusive with `--amount`), `--fee` for a fee override in nocks, and `--fee-nicks` for a fee override in nicks (mutually exclusive with `--fee`). To move funds onto the Base bridge instead of paying a p2pkh, pass `--bridge-deposit <nocks> --to-evm-address <0x...>` (only one bridge deposit is allowed per transaction).

The full JSON form is still available for multisig and bridge outputs, and for entering amounts directly in nicks:

```bash
# Auto-select spendable notes and compute fee
nockchain-wallet create-tx \
  --recipient '{"kind":"p2pkh","address":"<p2pkh-b58>","amount":10000}'

# Send to a single P2PKH recipient
nockchain-wallet create-tx \
  --names "[first1 last1],[first2 last2]" \
  --recipient '{"kind":"p2pkh","address":"<p2pkh-b58>","amount":10000}' \
  --fee-nicks 10

# Send to a multisig recipient
nockchain-wallet create-tx \
  --names "[first1 last1],[first2 last2]" \
  --recipient '{"kind":"multisig","threshold":2,"addresses":["<pkh-a>","<pkh-b>","<pkh-c>"],"amount":9000}' \
  --fee-nicks 10
```

`--recipient` gifts are denominated in nicks (65536 nicks = 1 nock); the ergonomic `--amount` and `--fee` are denominated in whole nocks, while `--amount-nicks` and `--fee-nicks` are in nicks.

#### Common Parameters

- The optional `names` argument is a list of `[first-name last-name]` pairs for manual note selection; omit it to auto-select spendable notes
- Auto-selection remains v1-only
- Manual `--names` selection may spend either an all-v1 set or an all-v0 set; mixed-version manual sets are rejected
- The optional `--fee` argument overrides the planner-computed fee, denominated in whole nocks (65536 nicks = 1 nock); `--fee-nicks` is the same override in nicks (mutually exclusive with `--fee`)
- Provide multiple `--recipient` flags (or multiple paired `--to`/`--amount` flags) to fan out to several outputs
- Each `--recipient` is either a JSON object (preferred) or a legacy `<p2pkh>:<amount>` string
- `--to <p2pkh-b58> --amount <nocks>` is a shorthand for a p2pkh `--recipient`; amounts are whole nocks (use `--amount-nicks` for raw nicks), and each `--to` must be paired with exactly one `--amount`/`--amount-nicks`
- `--bridge-deposit <nocks> --to-evm-address <0x...>` is a shorthand for a Base bridge deposit output (one per transaction); see [Bridge Deposits](#bridge-deposits)
- `address`/`addresses` fields expect base58-encoded pay-to-pubkey-hash values
- Provide `--sign-key <index[:hardened]>` multiple times to explicitly choose signing keys. If omitted, the wallet uses the master key or the `--index/--hardened` pair.
- `--refund-pkh` is required when manually spending legacy v0 notes. For v1 notes, refund defaults to the note owner.

### Migrating Legacy V0 Notes

Use `migrate-v0-notes` when you want to sweep spendable legacy v0 notes into a v1 pay-to-pubkey-hash address.

```bash
nockchain-wallet migrate-v0-notes --destination <v1-p2pkh-b58>
```

What the command does:

- Syncs the wallet and finds spendable v0 notes for the active v0 master and any active v0 child signers under that master
- Ignores v1 notes
- Computes the required fee for each signer bucket
- Builds one migration transaction per spend-capable signer bucket
- Writes each saved transaction to `./txs` in the current working directory
- Uses the destination as the refund target, so any leftover value also comes back as v1
- Prints a signer-by-signer summary with the saved tx path, selected inputs, fee, expected migrated amount, and the exact `send-tx` command for each created transaction

Typical migration flow:

```bash
# 1. Pull the latest wallet code
git pull origin master

# 2. Rebuild the wallet jams and binary
make install-nockchain-wallet

# 3. Import the legacy seed if you have not already done so
nockchain-wallet import-keys \
  --seedphrase "your legacy seed phrase here" \
  --version 0

# 4. Confirm the legacy master address is present
nockchain-wallet list-master-addresses

# 5. Switch the active master to the legacy v0 master key that owns the signer tree
nockchain-wallet set-active-master-address <legacy-v0-master-address>

# 6. Run the migration sweep into your v1 P2PKH destination
nockchain-wallet migrate-v0-notes --destination <v1-p2pkh-b58>

# 7. Submit each saved transaction shown in the migration summary
nockchain-wallet send-tx <path-to-tx-file>
```

Notes:

- The destination must be a v1 pay-to-pubkey-hash address
- The command is full-sweep only in the current release
- The command may create multiple transactions, not just one: it creates up to one migration tx per active local v0 signer under the active master
- Inspect the migration summary before submitting anything. It tells you which signer each tx belongs to, how many notes were selected, the fee, the expected migrated amount, where the tx was saved, and how to submit it
- Watch-only imports are not enough; the wallet must hold the matching v0 signing key
- Import the matching v0 signing key before migrating legacy v0 notes.

### Manual V0 Fan-In With `create-tx`

If you need to pin the exact legacy inputs instead of sweeping every spendable v0 note, you can still use `create-tx` with a manual `--names` set, as long as every selected note is v0 and you provide `--refund-pkh`.

```bash
nockchain-wallet create-tx \
  --names "[first1 last1],[first2 last2]" \
  --recipient '{"kind":"p2pkh","address":"<v1-p2pkh-b58>","amount":10000}' \
  --refund-pkh <v1-p2pkh-b58>
```

Rules for manual legacy spends:

- Every selected note must be v0
- Mixed v0/v1 manual sets are rejected
- `--refund-pkh` is required
- Fee may be planner-computed or overridden with `--fee`
- Omit `--names` if you want normal auto-selection; auto-selection does not pick v0 notes

#### Recipient JSON Format

`--recipient` accepts JSON objects in addition to the legacy `<p2pkh>:<amount>` syntax (legacy supports simple 1-of-1 P2PKH locks only). Wrap JSON in single quotes (or escape the quotes) when invoking the CLI. The supported shapes are:

```json
{"kind":"p2pkh","address":"<base58-pkh>","amount":10000}
{"kind":"multisig","threshold":2,"addresses":["<pkh-a>","<pkh-b>","<pkh-c>"],"amount":9000}
{"kind":"bridge-deposit","evm-address":"0x0123abcd...","amount":6553600000}
```

- `kind` must be `p2pkh`, `multisig`, or `bridge-deposit`
- `amount` is specified in nicks
- Multisig objects also require a `threshold` (m) and at least one `addresses` entry
- Bridge deposits route funds to the Base bridge; `evm-address` expects a 20-byte hex string (40 hex chars, case-insensitive) with or without the `0x` prefix. Only one `%bridge-deposit` output is allowed per transaction. The bridge enforces a minimum deposit of **100,000 nocks** (6,553,600,000 nicks) and charges a **0.3% fee** on the deposited amount.

Provide multiple `--recipient` flags to fan out to several recipients in one transaction.

### Multisig Recipients

Multisig outputs are expressed via the JSON form. Supply each output as:

```json
{"kind":"multisig","threshold":<M>,"addresses":["<pkh-a>", ...],"amount":<nicks>}
```

- `threshold` defines the `m` value (must be ≥1 and ≤ number of addresses)
- `addresses` is the list of base58 payee hashes that define the lock
- `amount` is denominated in nicks

### Bridge Deposits

Send Nockchain assets to the **Base** bridge. The Nockchain-side output is locked to the canonical bridge lock root by default; the deposit mints the wrapped-NOCK ERC-20 token on Base, whose contract is at [`0x9B5E262cF9bb04869ab40b19AF91D2dc85761722`](https://basescan.org/address/0x9B5E262cF9bb04869ab40b19AF91D2dc85761722).

The bridge enforces a **minimum deposit of 100,000 nocks** (6,553,600,000 nicks) and charges a **0.3% fee** on the deposited amount, so keep every deposit at or above the minimum.

The ergonomic form is a `--bridge-deposit`/`--to-evm-address` pair (amount in whole nocks):

```bash
# Deposit 100,000 nocks (the minimum) onto the Base bridge, credited to the given Base address.
nockchain-wallet create-tx \
  --bridge-deposit 100000 \
  --to-evm-address 0x0c8d9cf278d4f3e23b00ea0a16bba2d05c07a7b6
```

The equivalent explicit JSON form (amount in nicks) also works:

```bash
nockchain-wallet create-tx \
  --names "[first1 last1]" \
  --recipient '{"kind":"bridge-deposit","evm-address":"0x0c8d9cf278d4f3e23b00ea0a16bba2d05c07a7b6","amount":6553600000}' \
  --fee-nicks 60000000
```

- The `--to-evm-address` / `evm-address` is the Base recipient; provide exactly 20 bytes of hex (`0x` prefix optional). The deposit is credited as the wrapped-NOCK ERC-20 token on Base (contract `0x9B5E262cF9bb04869ab40b19AF91D2dc85761722`).
- Only a single bridge deposit output is allowed per transaction.
- The bridge enforces a minimum deposit of **100,000 nocks** (6,553,600,000 nicks) and takes a **0.3% fee** on the deposited amount; treat [`PROTOCOL.md`](../../PROTOCOL.md) as protocol authority, and use [`crates/bridge/docs/README.md`](../bridge/docs/README.md) for current bridge operations.

```bash
nockchain-wallet create-tx \
  --names "[first1 last1],[first2 last2]" \
  --recipient '{"kind":"multisig","threshold":2,"addresses":["<pkh-a>","<pkh-b>","<pkh-c>"],"amount":750000000}' \
  --fee-nicks 60000000
```

- `--sign-key` is optional and lets you pick which derived keys sign the bundle when the command runs. Each entry is `index:hardened` (for example, `5:true` signs with hardened child 5). If omitted, the active master key provides the initial signature.
- `--refund-pkh` is optional; when omitted, change returns to the default refund target for the spent notes.

### Signing Multisig Transactions

Every multisig bundle is validated and saved to `./txs/<transaction-name>.tx`. Share this file with the remaining signers. Additional signatures are appended with:

```bash
nockchain-wallet sign-multisig-tx ./txs/<transaction-name>.tx --sign-keys "1:false"
```

`sign-multisig-tx` accepts the same `index:hardened` pairs (or defaults to the active master key). Use `show-tx` to inspect the current signature set before broadcasting:

```bash
nockchain-wallet show-tx ./txs/<transaction-name>.tx
```

Once enough signatures are collected, broadcast the transaction with:

```bash
nockchain-wallet send-tx ./txs/<transaction-name>.tx
```

### Make Transaction from Transaction File

```bash
# Display transaction contents
nockchain-wallet show-tx txs/transaction.tx

# Make and broadcast the signed transaction
nockchain-wallet send-tx txs/transaction.tx
```

Note: The transaction file will be saved in `./txs/` directory with a `.tx` extension.

### Check whether a transaction was accepted (public API only)

```bash
# Query the public API for acceptance status
nockchain-wallet \
  --client public \
  tx-accepted <base58-tx-id>
```

- The wallet asks the Nockchain node whether it has validated the transaction (consistency check). A `true` response means the node accepted the transaction, not that it currently resides in the mempool. You can use this command to check whether a transaction was accepted by the network; it is necessary for inclusion in a block but not sufficient when timelocks are present.
- Currently, the private API cannot be queried with this request
- The command is lightweight and does not perform a full balance sync.


## Message Signing and Verification

### Sign Message

Signs arbitrary bytes with the wallet's key. By default, the signature is written to `message.sig`.

Short flags:

```bash
nockchain-wallet sign-message -m "hello"
nockchain-wallet sign-message -m "hello" --index 5 --hardened
```

Positional message (equivalent to `-m/--message`):

```bash
nockchain-wallet sign-message "hello"
```

From file:

```bash
nockchain-wallet sign-message --message-file ./payload.bin
```

### Verify Message

Verifies a signature against a message and a base58-encoded schnorr public key.

Short flags:

```bash
nockchain-wallet verify-message -m "hello" -s message.sig -p <BASE58_PUBKEY>
```

Positional-only form (message, signature file, pubkey):

```bash
nockchain-wallet verify-message "hello" message.sig <BASE58_PUBKEY>
```

Named/positional mixed examples:

```bash
nockchain-wallet verify-message --message-file ./payload.bin message.sig <BASE58_PUBKEY>
nockchain-wallet verify-message "hello" -s message.sig -p <BASE58_PUBKEY>
```

Notes:
- The positional forms are equivalent to the named flags (`--message`, `--signature`, `--pubkey`).

### Sign Hash

Signs a precomputed tip5 hash (base58 string). Writes signature to `hash.sig`.

```bash
nockchain-wallet sign-hash <BASE58_TIP5_HASH>
nockchain-wallet sign-hash <BASE58_TIP5_HASH> --index 5 --hardened
```

### Verify Hash

Verifies a signature against a precomputed tip5 hash (base58 string) and pubkey.

```bash
nockchain-wallet verify-hash <BASE58_TIP5_HASH> hash.sig <BASE58_PUBKEY>
nockchain-wallet verify-hash <BASE58_TIP5_HASH> -s hash.sig -p <BASE58_PUBKEY>
```

# Nockchain Wallet

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

# Import a watch-only address or pubkey
nockchain-wallet watch-address <base58-pkh-or-pubkey>

# Import a master public key from exported file
nockchain-wallet import-master-pubkey keys.export
```

The exported keys file contains all wallet keys as a `jam` file that can be imported on another instance.

Can be used for:
- Backing up your wallet
- Migrating to a new device
- Sharing public keys with other users

### Connecting to a Nockchain API server

The wallet talks to the gRPC APIs exposed by a running nockchain instance. You can target either the **public** API (default) or the **private** API that is typically bound to `localhost`. You must run a nockchain instance to connect to the private API. Zorp runs its own public Nockchain API server at `https://nockchain-api.zorp.io`, and the wallet connects to it by default.

#### Public API (default)

```bash
# Use the default public endpoint (https://nockchain-api.zorp.io)
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
- By default, we do not sync notes attached to watch-only pubkeys. Pair sync-heavy commands with `--include-watch-only` when you want watch-only pubkeys included in balance updates.

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

### List Arbitrary Notes by Public Key (Watch-Only)

```bash
nockchain-wallet watch-address <address>
nockchain-wallet list-notes-by-address <address> --include-watch-only
```

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

```bash
# Send to a single recipient
nockchain-wallet create-tx \
  --names "[first1 last1],[first2 last2]" \
  --recipient "<pkh-b58>:<amount>" \
  --fee 10

# Send to multiple recipients
nockchain-wallet create-tx \
  --names "[first1 last1],[first2 last2]" \
  --recipient "<pkh-b58>:<amount>" \
  --recipient "<pkh-b58>:<amount>" \
  ...
  --fee 10
```

Gifts and fees are denominated in nicks (65536 nicks = 1 nock).

#### Common Parameters

- The `names` argument is a list of `[first-name last-name]` pairs specifying funding notes
- The `fee` argument is the transaction fee to pay (in nicks, 65536 nicks to 1 nock)

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

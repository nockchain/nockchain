# Nockchain Wallet

## Setup

### Generate New Key Pair

```bash
# Generate a new key pair with random entropy
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

# Generate master private key from seed phrase
nockchain-wallet import-keys --seedphrase "your seed phrase here"

# Generate master public key from private key and chain code
nockchain-wallet import-keys --master-privkey <private-key> --chain-code <chain-code>

# Import a master public key from exported file
nockchain-wallet import-master-pubkey keys.export
```

The exported keys file contains all wallet keys as a `jam` file that can be imported on another instance.

Can be used for:
- Backing up your wallet
- Migrating to a new device
- Sharing public keys with other users

### Connecting to Nockchain

The wallet connects to the NockApp gRPC server exposed by a running nockchain instance to perform operations like checking balances, broadcasting transactions, etc.

```bash
# Connect to nockchain using gRPC
nockchain-wallet --grpc-address http://localhost:5555 <command>
```

Note: Make sure nockchain is running and the grpc address matches your nockchain configuration.



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




## Listing Notes

### List All Notes

```bash
nockchain-wallet list-notes
```

Displays all notes (UTXOs) currently managed by the wallet, sorted by assets.

### List Notes by Public Key

```bash
nockchain-wallet list-notes-by-pubkey <public-key>
```

Shows only the notes associated with the specified public key. Useful for filtering wallet contents by address or for multisig scenarios.

### List Notes by Public Key (CSV format)

```bash
nockchain-wallet list-notes-by-pubkey-csv <public-key>
```

Outputs matching notes in CSV format suitable for analysis or reporting.


## Transaction Creation

#### Components of transaction creation

1. **Seeds**: Define where funds are going and how much
2. **Inputs**: Specify which notes (UTXOs) to spend
3. **Transaction**: Combine inputs into a complete transaction
4. **Sign**: Authorize the transaction with private keys
5. **Send Transaction**: Send the final transaction for broadcasting

### Create a Transaction

The create-tx command supports two modes: single recipient and multiple recipients.

#### Single Recipient Transaction

```bash
# Send to a single recipient
nockchain-wallet create-tx \
  --names "[first1 last1]" \
  --recipient "[1 pk1]" \
  --gift 100 \
  --fee 10
```

For single recipient transactions:
- `--recipient` specifies one recipient as `[<num-of-signatures> <public-key-1>,<public-key-2>,...]`
- `--gift` specifies the amount to send to that recipient
- Multiple names can still be provided to use funds from multiple notes

#### Multiple Recipients Transaction

```bash
# Send to multiple recipients
nockchain-wallet create-tx \
  --names "[first1 last1],[first2 last2]" \
  --recipients "[1 pk1],[2 pk2,pk3]" \
  --gifts "100,200" \
  --fee 10
```

For multiple recipient transactions:
- `--recipients` specifies a list of recipients, each as `[<num-of-signatures> <public-key-1>,<public-key-2>,...]`
- `--gifts` specifies a list of amounts, one for each recipient (must match the number of recipients)

#### Common Parameters

- The number of signatures required is specified as the first number in each recipient specification
- The `names` argument is a list of `[first-name last-name]` pairs specifying funding notes
- The `fee` argument is the transaction fee to pay
- For multisig recipients, list multiple public keys after the signature count

### Make Transaction from Transaction File

```bash
# Sign the transaction
nockchain-wallet sign-tx txs/transaction.tx

# Optionally specify a key index for signing
nockchain-wallet sign-tx txs/transaction.tx --index 5

# Display transaction contents
nockchain-wallet show-tx txs/transaction.tx

# Make and broadcast the signed transaction
nockchain-wallet send-tx txs/transaction.tx
```

Note: The transaction file will be saved in `./txs/` directory with a `.tx` extension.


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

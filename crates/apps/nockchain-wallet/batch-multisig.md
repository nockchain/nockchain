# Batch multisig: create & sign

Two `rust-script` helpers that drive `nockchain-wallet` for high-volume multisig
flows. They add no signing/crypto logic of their own — they only orchestrate
existing wallet subcommands.

- **`scripts/batch_multisig_create.rs`** — repeatedly create multisig
  transactions, draining a notes CSV until it is exhausted.
- **`scripts/batch_multisig_sign.rs`** — sign every `.tx` in a folder (work
  queue), moving each signed tx into `done/`, optionally broadcasting it.

Both are self-contained (`#!/usr/bin/env rust-script`, std-only). The wallet
binary is resolved from `$NOCKCHAIN_WALLET`, the `--wallet <path>` flag, else
`nockchain-wallet` on `PATH`.

Everything after a literal `--` is passed verbatim to `nockchain-wallet`
**before** the subcommand — that is where wallet *global* flags live
(`--data-dir`, `--fakenet`, `--client`, `--private-grpc-server-port`,
`--public-grpc-server-addr`, …).

## Prerequisites

1. Wallet has keys imported and has been **synced** at least once (the note data
   for the multisig must be in local wallet state).
2. The multisig is watched:
   ```
   nockchain-wallet ... watch multisig --threshold <M> --participants <pkh1>,<pkh2>,<pkh3>
   ```
3. A notes CSV for that multisig has been generated:
   ```
   nockchain-wallet ... list-notes-by-multisig-csv <multisig-first-name> > notes-multisig-<root>.csv
   ```

`create-multisig-tx --notes-csv` and `sign-multisig-tx` both run **offline**, so
the create/sign loops do not re-sync each iteration. `send-tx` (via `--send`)
needs node connectivity.

## 1. Create — drain a notes CSV

Each iteration runs `create-multisig-tx --notes-csv <CSV> ...`, which selects
notes from the CSV, writes `./txs/<name>.tx`, and **removes the spent notes from
the CSV**. The loop stops when the CSV has no data rows left, or when an
iteration spends no notes and writes no tx (the remaining notes can't fund
another payment at the given amount/fee) — so it never spins forever.

```
scripts/batch_multisig_create.rs \
  --notes-csv notes-multisig-<root>.csv \
  --threshold 2 --participants <pkh1>,<pkh2>,<pkh3> \
  --recipient '{"kind":"p2pkh","address":"<dest-b58>","amount":1000000}' \
  --fee 65536 \
  --sign-key 0:false \
  -- --data-dir ./test_run_data/wallet --fakenet
```

Key flags:

| flag | meaning |
|---|---|
| `--notes-csv <PATH>` | multisig notes CSV to drain (mutated in place; backed up to `<csv>.bak` unless `--no-backup`) |
| `--threshold <M>` / `--participants <pkh,...>` | the multisig lock being spent (same values used with `watch multisig`) |
| `--recipient <SPEC>` | output per tx; repeatable. `<p2pkh-b58>:<amount>` or a JSON object (`p2pkh`, `multisig`, `bridge-deposit`) |
| `--fee <NICKS>` | optional fee override (else the planner computes it) |
| `--sign-key <INDEX:HARDENED>` | initial signature(s) the creator contributes; repeatable |
| `--refund-pkh <PKH>` | send change to a single-signer address (default: back to the multisig) |
| `--txs-dir <DIR>` | where the wallet writes tx files (default `txs`) |
| `--max-txs <N>` | stop after N transactions |
| `--dry-run` | print the exact per-iteration command, execute nothing |
| `--no-backup` | skip the `<csv>.bak` copy |

> Each tx consolidates as many input notes as fit under the tx-size limit, so a
> large CSV naturally takes several transactions to drain — that is the point of
> the loop.

## 2. Sign — a folder of txs as a work queue

Signs every top-level `*.tx` in `--tx-dir`, moving each signed file into
`--done-dir` (default `<tx-dir>/signed`). "Done" means *this signer has signed*;
for an m-of-n that still needs signatures, hand the `done/` folder to the next
signer as their `--tx-dir`.

```
scripts/batch_multisig_sign.rs \
  --tx-dir txs \
  --sign-keys 1:true,2:false \
  -- --data-dir ./test_run_data/wallet --fakenet
```

With `--send` (online), each tx is broadcast right after signing:

- validates & submits → moves to `--sent-dir` (default `<tx-dir>/sent`);
- does not validate (typically threshold not met yet) → stays signed, moves to
  `--done-dir` to await more signatures. **Not** treated as an error — only a
  failure to *sign* is.

```
scripts/batch_multisig_sign.rs --tx-dir txs --sign-keys 2:false --send \
  -- --client public --public-grpc-server-addr http://127.0.0.1:5001
```

Key flags:

| flag | meaning |
|---|---|
| `--tx-dir <DIR>` | folder of `.tx` files to sign (the queue) |
| `--sign-keys <INDEX:HARDENED,...>` | key indices to sign with (omit → master key) |
| `--done-dir <DIR>` | where signed txs go (default `<tx-dir>/signed`) |
| `--send` | broadcast each tx after signing |
| `--sent-dir <DIR>` | where successfully-sent txs go (default `<tx-dir>/sent`) |
| `--continue-on-error` | skip a tx that fails to sign and keep going (default: stop) |
| `--dry-run` | print the sign (and send) commands, execute nothing |

Exit status is non-zero if any tx failed to **sign**.

## End-to-end (single-party, e.g. fakenet)

```
# generate the notes CSV
nockchain-wallet ... list-notes-by-multisig-csv <first-name> > notes-multisig-<root>.csv

# drain it into ./txs (creator contributes signature 0)
scripts/batch_multisig_create.rs --notes-csv notes-multisig-<root>.csv \
  --threshold 2 --participants <pkh1>,<pkh2> \
  --recipient '{"kind":"p2pkh","address":"<dest>","amount":1000000}' \
  --sign-key 0:false -- --data-dir ./wallet

# second signer signs the folder and broadcasts whatever reaches threshold
scripts/batch_multisig_sign.rs --tx-dir txs --sign-keys 1:false --send \
  -- --data-dir ./wallet --client public
```

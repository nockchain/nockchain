# Nockchain Block Explorer TUI

Status: Active
Owner: Nockchain Maintainers
Last Reviewed: 2026-02-20
Canonical/Legacy: Legacy (crate-level reference; canonical docs spine starts at [`START_HERE.md`](../../START_HERE.md))

A terminal user interface for exploring the Nockchain blockchain via the gRPC Block Explorer API.

## ALPHA SOFTWARE, IF YOU ARE NOT AN ADEPT USER YOU WILL NOT FIND THIS USEFUL

**This is pre-release/alpha/pre-alpha grade stuff. If you don't know how to write code without an LLM doing most of the work turn back now.**

## OK you made it this far

- There is no public instance for this to connect to.
- No, not Zorp's public API instance that the wallet uses either.
- You must run your own instance. We will not help you. Ask the community and they may have time to help you. We do not.
- If the state gets hinky, just restart the TUI app
- This is mostly for enabling systems integrators to debug the gRPC blocks and transactions endpoints
- I also like to use it because it makes me happy. YMMV.

## Features

- **Blocks tab**: browse blocks by height with cached pagination, range sync, and detail prefetch.
- **Block details**: inspect a block and drill into transactions.
- **Transactions tab**: browse known transactions and open transaction details.
- **Wallets tab**: build and sort an address summary index from cached transaction details.
- **Metrics tab**: inspect explorer metrics from the metrics gRPC service.
- **Transaction search**: search by transaction ID prefix and inspect status.
- **Connection resilience**: retry on disconnects by default, or use `--fail-fast` for strict startup behavior.

## Usage

```bash
# Connect to default server (localhost:50051)
cargo run --release -p nockchain-explorer-tui

# Connect to custom server
cargo run --release -p nockchain-explorer-tui -- --server http://my-server:50051

# Exit immediately if initial connect fails
cargo run --release -p nockchain-explorer-tui -- --fail-fast

# Or use the binary directly
./target/release/nockchain-explorer-tui --server http://localhost:50051
```

## Key Bindings

`?` opens the in-app help overlay with the authoritative, view-specific shortcut list.

Common shortcuts:

- Global: `Tab`/`Shift+Tab` switch top-level tabs, `?` toggles help, `q` quits.
- Blocks tab: `↑/↓` move selection, `PgUp/PgDn` jump, `Enter` block details, `c` copy block ID, `t` open TX search, `r` refresh newest page, `n` fetch next page, `s` sync pages.
- Block details: `ESC` back to list, `PgUp/PgDn` prev/next block, `Tab` toggle tx focus, `Enter` open highlighted transaction, `n/p` next/prev transaction.
- Transactions tab: `Enter` open details, `n/p` next/prev transaction details, `s` sync pages.
- Wallets tab: `b/r/e/t` sort by balance/received/sent/tx-count, `o` toggles sort order, `s` sync pages.
- Search view: `Enter` search, `Ctrl+V` paste, `Ctrl+C` clear input, `ESC` back.

## UI Notes

The UI layout and status text are still evolving.  
Use `?` in-app for the current, runtime-accurate controls and view semantics.

## Requirements

- A running `nockchain-api` with public v2 block explorer services enabled.
- gRPC endpoint reachable at the configured `--server` URI (default `http://localhost:50051`).

# Nockchain Block Explorer TUI

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

- **Blocks List View**: Browse blocks in descending order by height with pagination
- **Block Details View**: Drill down into individual blocks to see all transactions
- **Transaction Search**: Look up transactions by ID to see if they're confirmed or pending
- **Auto-refresh**: Automatically updates the blockchain state every 10 seconds

## Usage

```bash
# Connect to default server (localhost:50051)
cargo run --release -p nockchain-explorer-tui

# Connect to custom server
cargo run --release -p nockchain-explorer-tui -- --server http://my-server:50051

# Or use the binary directly
./target/release/nockchain-explorer-tui --server http://localhost:50051
```

## Key Bindings

### Blocks List View
- `↑/k`: Move up
- `↓/j`: Move down
- `Enter`: View block details
- `t`: Search for a transaction
- `r`: Manually refresh blocks
- `a`: Toggle auto-refresh on/off
- `n`: Load next page of blocks
- `q`: Quit

### Block Details View
- `↑/k`: Navigate to previous block
- `↓/j`: Navigate to next block
- `ESC`: Return to blocks list
- `q`: Quit

### Transaction Search View
- Type: Enter transaction ID (base58-encoded)
- `Enter`: Perform search
- `Ctrl+C`: Clear input and results
- `Backspace`: Delete character
- `ESC`: Return to blocks list
- `q`: Quit

## Example

```
┌Info─────────────────────────────────────────────────────────────┐
│Nockchain Block Explorer | Height: 12345                          │
│Server: http://localhost:50051                                    │
└─────────────────────────────────────────────────────────────────┘
┌Blocks (50)──────────────────────────────────────────────────────┐
│>> Height:  12345 | TXs:   3 | Block ID: 0001020...0304050607   │
│   Height:  12344 | TXs:   1 | Block ID: 0011121...1314151617   │
│   Height:  12343 | TXs:   5 | Block ID: 0021222...2324252627   │
│   ...                                                            │
└─────────────────────────────────────────────────────────────────┘
┌Status───────────────────────────────────────────────────────────┐
│Ready | Auto-refresh: ON (last: 3s ago) | More pages available   │
└─────────────────────────────────────────────────────────────────┘
↑/k: Up | ↓/j: Down | Enter: Details | t: Search TX | q: Quit
```

## Requirements

- Running nockchain-api server with block explorer API enabled
- gRPC server must be accessible at the specified URI

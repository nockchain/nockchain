# Nockchain

**Nockchain is programmable sound money that scales.**

Nockchain is a ZK-Proof of Work L1 that combines sound money incentives with modern research into data availability, app-rollups, and intent-based composability.


*Nockchain is entirely experimental and many parts are unaudited. We make no representations or guarantees as to the behavior of this software.*


## Setup

Install `rustup` by following their instructions at: [https://rustup.rs/](https://rustup.rs/)

Ensure you have these dependencies installed if running on Debian/Ubuntu:
```
sudo apt update
sudo apt install clang llvm-dev libclang-dev make protobuf-compiler
```
Clone the repo and cd into it:
```
git clone https://github.com/zorp-corp/nockchain.git && cd nockchain
```

Copy the example environment file and rename it to `.env`:
```
cp .env_example .env
```

### For Linux

Linux users **must** manually set their memory overcommit status:

```
# Enable always-overcommit:
echo 'vm.overcommit_memory=1' | sudo tee /etc/sysctl.d/99-overcommit.conf

# Reload kernel parameters:
sudo sysctl --system
# or:
sudo sysctl -p /etc/sysctl.d/99-overcommit.conf
```

## Install Hoon Compiler

Install `hoonc`, the Hoon compiler:

```
make install-hoonc
export PATH="$HOME/.cargo/bin:$PATH"
```

(If you build manually with `cargo build`, be sure to use `--release` for `hoonc`.)

## Install Wallet

After you've run the setup and build commands, install the wallet:

```
make install-nockchain-wallet
export PATH="$HOME/.cargo/bin:$PATH"
```

See the nockchain-wallet [README](./crates/nockchain-wallet/README.md) for more information.


## Install Nockchain

After you've run the setup and build commands, install Nockchain:

```
make install-nockchain
export PATH="$HOME/.cargo/bin:$PATH"
```

## Setup Keys

To generate a new key pair:

```
nockchain-wallet keygen
```

This will print a new public/private key pair + chain code to the console, as well as the seed phrase for the private key.

To track a watch-only address or pubkey without importing private material:

```
nockchain-wallet watch-address <base58-pkh-or-pubkey>
```

The wallet normalizes the identifier so you can supply either a v1 payee hash or a schnorr pubkey.

Use `.env_example` as a template and copy your pkh to the `.env` file.

Then, in your `.env` file, set the `MINING_PKH` variable to the address of the v1 key you generated.

```
MINING_PKH=<address>
```

The miner will generate v1 coinbases spendable by the `MINING_PKH`.

## Backup Keys

To backup your keys, run:

```
nockchain-wallet export-keys
```

This will save your keys to a file called `keys.export` in the current directory.

They can be imported later with:

```
nockchain-wallet import-keys --file keys.export
```

## Running Nodes

Make sure your current directory is nockchain.

To run a Nockchain node without mining.

```
bash ./scripts/run_nockchain_node.sh
```

To run a Nockchain node and mine to a pkh:

```
bash ./scripts/run_nockchain_miner.sh
```

For launch, make sure you run in a fresh working directory that does not include a .data.nockchain file from testing.

## FAQ

### What is a pkh?

A pkh is a "pubkey hash", which is a shorter representation of a public key.  v1 pkhs are base58-encoded.

### Can I use same pkh if running multiple miners?

Yes, you can use the same pkh if running multiple miners.

### How do I change the mining pkh?

Run `nockchain-wallet keygen` to generate a new key pair.

If you are using the Makefile workflow, copy the pkh to the `.env` file.

### How do I run a testnet?

To run a testnet on your machine, follow the same instructions as above, except use the fakenet scripts provided in the `scripts` directory.

Here's how to set it up:

```bash
Make sure you have the most up-to-date version of Nockchain installed.

Inside of the nockchain directory:

# Create directories for each instance
mkdir fakenet-hub fakenet-node

# Copy .env to each directory
cp .env fakenet-hub/
cp .env fakenet-node/

# Run each instance in its own directory with .env loaded
cd fakenet-hub && sh ../scripts/run_nockchain_node_fakenet.sh
cd fakenet-node && sh ../scripts/run_nockchain_miner_fakenet.sh
```

The hub script is bound to a fixed multiaddr and the node script sets that multiaddr as an initial
peer so that nodes have a way of discovering eachother initially.

You can run multiple instances using `run_nockchain_miner_fakenet.sh`, just make sure that
you are running them from different directories because the checkpoint data is located in the
working directory of the script.

### What are the networking requirements?

Nockchain requires:

1. Internet.
2. If you are behind a firewall, you need to specify the p2p ports to use and open them..
   - Example: `nockchain --bind /ip4/0.0.0.0/udp/$PEER_PORT/quic-v1`
3. **NAT Configuration (if you are behind one)**:
   - If behind NAT, configure port forwarding for the peer port
   - Use `--bind` to specify your public IP/domain
   - Example: `nockchain --bind /ip4/1.2.3.4/udp/$PEER_PORT/quic-v1`

### Why aren't Zorp peers connecting?

Common reasons for peer connection failures:

1. **Network Issues**:
   - Firewall blocking P2P port
   - NAT not properly configured
   - Incorrect bind address

2. **Configuration Issues**:
   - Invalid peer IDs

3. **Debug Steps**:
   - Check logs for connection errors
   - Verify port forwarding

### What do outgoing connection failures mean?

Outgoing connection failures can occur due to:

1. **Network Issues**:
   - Peer is offline
   - Firewall blocking connection
   - NAT traversal failure

2. **Peer Issues**:
   - Peer has reached connection limit
   - Peer is blocking your IP

3. **Debug Steps**:
   - Check peer's status
   - Verify network connectivity
   - Check logs for specific error messages

### How do I know if it's mining?

You can check the logs for mining activity.

If you see a line that looks like:

```sh
[%mining-on 12.040.301.481.503.404.506 17.412.404.101.022.637.021 1.154.757.196.846.835.552 12.582.351.418.886.020.622 6.726.267.510.179.724.279]
```

### How do I check block height?

You can check the logs for a line like:

```sh
block Vo3d2Qjy1YHMoaHJBeuQMgi4Dvi3Z2GrcHNxvNYAncgzwnQYLWnGVE added to validated blocks at 2
```

That last number is the block height.

### What do common errors mean?

Common errors and their solutions:

1. **Connection Errors**:
   - `Failed to dial peer`: Network connectivity issues, you may still be connected though.
   - `Handshake with the remote timed out`: Peer might be offline, not a fatal issue.

### How do I check wallet balance?

To check your wallet balance:

```bash
# List all notes by pkh
nockchain-wallet list-notes-by-address <your-base58-address>
```

### How do I configure logging levels?

To reduce logging verbosity, you can set the `RUST_LOG` environment variable before running nockchain:

```bash
# Show only info and above
RUST_LOG=info nockchain

# Show only errors
RUST_LOG=error nockchain

# Show specific module logs (e.g. only p2p events)
RUST_LOG=nockchain_libp2p_io=info nockchain

# Multiple modules with different levels
RUST_LOG=nockchain_libp2p_io=info,nockchain=warn nockchain
```

Common log levels from most to least verbose:
- `trace`: Very detailed debugging information
- `debug`: Debugging information
- `info`: General operational information
- `warn`: Warning messages
- `error`: Error messages

You can also add this to your `.env` file if you're running with the Makefile:
```
RUST_LOG=info
```

### How do profile for performance?

Here's a demo video for the Tracy integration in Nockchain: https://x.com/nockchain/status/1948109668171051363

The main change since the video is tracing is now enabled by default. If you want to disable it you can [disable](https://doc.rust-lang.org/cargo/reference/features.html#the-default-feature) the `tracing-tracy` feature here. The tracing is [inhibited](https://www.google.com/search?q=inhibit+definition&sca_esv=677f3ddbc8bf65e8&ei=hnSCaJuSGIGlqtsP96qM0QE&ved=0ahUKEwib7Z60idaOAxWBkmoFHXcVIxoQ4dUDCBA&uact=5&oq=inhibit+definition&gs_lp=Egxnd3Mtd2l6LXNlcnAiEmluaGliaXQgZGVmaW5pdGlvbjITEAAYgAQYkQIYsQMYigUYRhj5ATIGEAAYFhgeMgYQABgWGB4yBhAAGBYYHjIGEAAYFhgeMgYQABgWGB4yBhAAGBYYHjIGEAAYFhgeMgYQABgWGB4yBhAAGBYYHjItEAAYgAQYkQIYsQMYigUYRhj5ARiXBRiMBRjdBBhGGPkBGPQDGPUDGPYD2AEBSIgaUMgFWIgZcAR4AJABAJgBeaAB7AqqAQQxOS4yuAEDyAEA-AEBmAIZoAKpC8ICDhAAGIAEGLADGIYDGIoFwgILEAAYgAQYsAMYogTCAhAQABiABBiRAhiKBRhGGPkBwgIKEAAYgAQYQxiKBcICCxAAGIAEGJECGIoFwgILEAAYgAQYsQMYgwHCAg4QABiABBixAxiDARiKBcICBRAuGIAEwgIREC4YgAQYsQMY0QMYgwEYxwHCAioQABiABBiRAhiKBRhGGPkBGJcFGIwFGN0EGEYY-QEY9AMY9QMY9gPYAQHCAg8QABiABBhDGIoFGEYY-QHCAikQABiABBhDGIoFGEYY-QEYlwUYjAUY3QQYRhj5ARj0Axj1Axj2A9gBAcICCBAuGIAEGLEDwgIIEAAYgAQYsQPCAi0QABiABBiRAhixAxiKBRhGGPkBGJcFGIwFGN0EGEYY-QEY9AMY9QMY9gPYAQHCAg0QABiABBixAxhDGIoFwgIFEAAYgATCAhEQABiABBiRAhixAxiDARiKBcICChAuGIAEGLEDGArCAgcQABiABBgKwgIKEAAYgAQYsQMYCsICBxAuGIAEGArCAg0QABiABBixAxiDARgKwgIOEAAYgAQYkQIYsQMYigXCAggQABgWGAoYHpgDAIgGAZAGBboGBggBEAEYE5IHBDIzLjKgB_ePArIHBDE5LjK4B6ALwgcGMC4yNC4xyAc5&sclient=gws-wiz-serp) by default, it only collects traces when a [Tracy profiler client](https://github.com/wolfpld/tracy) connects to the application. This means minimal (9% or less for the nockvm, shouldn't impact jetted mining) performance impact but the profiling data is available any time you'd like to connect your Nockchain instance.

There are two main kinds of performance data Tracy will gather from your application. Instrumentation and samples. Instrumentation comes from the [tracing crate's](https://docs.rs/tracing/latest/tracing/) spans. The integration with [nockvm](https://github.com/zorp-corp/nockchain/tree/master/crates/nockvm) is via the same `tracing` spans. Samples are _stack samples_, so it's not a perfectly and minutely traced picture of where your time was spent. However, the default sampling rate for Tracy is _very_ high but very efficient. You should expect a problematic performance impact from connecting Tracy to an instance if every single core and hyperthread is maxed out on your machine. You should be leaving some spare threads unoccupied even on a mining instance for the Serf thread and the kernel anyway. We (Zorp Corp) generally left 4 threads unused on each mining server.

Stack samples are roughly speaking the "native" or Rust part of the application whereas instrumentation is the nockvm spans showing how much time you're spending in your Hoon arms plus any Rust functions that were also instrumented. You can tell them apart because the spans for Hoon will have weird paths like `blart/boop/snoot/goof/slam/woof` and no source location in the Tracy profiler UI. The Rust spans will have much plainer names mapping onto whatever the function was named, so a function like `fn slam()` will show up in the instrumentation as `slam` and have a source location path ending in a `*.rs` file.

What makes this especially powerful is:

- The profiling data is now unified into a single tool. Previously we used `samply` for Rust/native code and the JSON (nockvm) traces for the Hoon.
- Tracy can attribute what native stack samples

#### OK, but how do I get started?

Build the application like normal, it's enabled by default. No special CLI arguments, it's _enabled by default_.

Get a copy of the Tracy profiler client, it's a GUI C++ application that uses dear imgui. They may not have a release binary for your platform so you may need to build it yourself. Here's a tip: steal the build commands for Linux/macOS [from here](https://github.com/wolfpld/tracy/blob/6f3a023df871e180151d2e86fb656e8122e274eb/.github/workflows/linux.yml#L24-L25).

They're using Arch Linux, so make sure you have these equivalent packages installed for your platform:

```
pacman -Syu --noconfirm && pacman -S --noconfirm --needed freetype2 debuginfod wayland dbus libxkbcommon libglvnd meson cmake git wayland-protocols nodejs
```

Then to build the GUI app:

```
cmake -B profiler/build -S profiler -DCMAKE_BUILD_TYPE=Release -DGIT_REV=${{ github.sha }}
cmake --build profiler/build --parallel
```

Note that your Tracy profiler GUI version and the [Tracy client library version _must_ match](https://github.com/nagisa/rust_tracy_client?tab=readme-ov-file#version-support-table) or it will not work and will refuse to connect. It'll tell you why too.

If your Nockchain instance is running locally on the same machine as your Tracy profiler client GUI it will pop up in the connect menu automatically. If you're using `ssh` to connect to a remote server (see below re: security) then you will need to add the port to `127.0.0.1` like so: `127.0.0.1:8087` and hit connect after establishing the ssh connection with the port mapping.

By default you'll only get instrumentation, not the stack samples, generally speaking. To enable stack samples you need to change some security parameters on your Linux server. Stack sampling doesn't work on macOS natively at all, if you want to profile something with stack samples on macOS, run it in Docker with a permissive seccomp profile.

Here's what you'd need to do on a typical Linux server running Ubuntu LTS to enable stack samples:

```
echo 0 | sudo tee /proc/sys/kernel/yama/ptrace_scope
echo 1 | sudo tee /proc/sys/kernel/perf_event_paranoid
```

Nerfing `perf_event_paranoid` might be enough by itself, ymmv. This commands only set these `sysctl` parameters until the server gets rebooted. Create a permanent `sysctl` config in `/etc` if you want to make it permanent. You might need to restart the process after changing the parameters as well.

#### Tracy, profiling, and security

Do _not_ expose the port the tracy server binds to from the Nockchain application instance to servers or networks you do not control end-to-end. If you have a server hosted in the cloud or a leased dedicated server, leave it private and use `ssh` to create a local proxy for the port on your dev machine.

When I demo'd Tracy here's the ssh command I used:

```
ssh -L 8087:backbone-us-south-mig:8086 backbone-us-south-mig
```

`backbone-us-south-mig` is a hostname reference from my `~/.ssh/config` that we generate from a script I wrote to inject an Ansible inventory into the local SSH configuration. I'm using port `8087` for the local binding because running the Tracy profiler GUI binds port 8086.

### Troubleshooting Common Issues

1. **Node Won't Start**:
   - Check port availability
   - Verify .env configuration
   - Check for existing .data.nockchain file
   - Ensure proper permissions

2. **No Peers Connecting**:
   - Verify port forwarding
   - Check firewall settings

3. **Mining Not Working**:
   - Verify mining pkh
   - Check --mine flag
   - Ensure peers are connected
   - Check system resources

4. **Wallet Issues**:
   - Verify key import/export
   - Check socket connection
   - Ensure proper permissions

# Contributing

Unless you explicitly state otherwise, any contribution intentionally submitted for inclusion in the work by you, as defined in the Apache-2.0 license, shall be dual licensed as below, without any additional terms or conditions.

# License

Licensed under either of

Apache License, Version 2.0 (LICENSE-APACHE or https://www.apache.org/licenses/LICENSE-2.0)
MIT license (LICENSE-MIT or https://opensource.org/licenses/MIT)
at your option.

# {{project_name}}

gRPC demo template with two binaries:

- `listen`: gRPC server listener.
- `talk`: client-side talker that emits `%grpc` effects.

## Files

- `src/listen.rs`, `src/talk.rs`: Rust binaries.
- `hoon/app/listen.hoon`, `hoon/app/talk.hoon`: matching kernels.
- `src/lib.rs`: shared helpers.

## Build

From the workspace directory that contains `nockapp.toml`:

```sh
nockup project build {{project_name}}
```

This compiles Hoon apps and writes `listen.jam` and `talk.jam`.

## Run

`nockup project run` is single-binary oriented. For this template, run binaries explicitly:

```sh
cd {{project_name}}
cargo run --release --bin listen
cargo run --release --bin talk
```

# {{project_name}}

Stateless HTTP template that returns static HTML for `GET` requests.

## Files

- `src/main.rs`: Rust runtime with `http_driver()`.
- `hoon/app/app.hoon`: static response kernel.
- `hoon/common/wrapper.hoon`: wrapper core.

## Build and run

From the workspace directory that contains `nockapp.toml`:

```sh
nockup project build {{project_name}}
nockup project run {{project_name}}
```

From this project directory directly:

```sh
cargo build --release
hoonc hoon/app/app.hoon
cargo run --release
```

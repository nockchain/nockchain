# {{project_name}}

Stateful HTTP NockApp template (counter-style request handling).

## Files

- `src/main.rs`: Rust runtime with `http_driver()`.
- `hoon/app/app.hoon`: request routing and state updates.
- `hoon/lib/http.hoon`: HTTP nouns/helpers.
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

# `nockapp-grpc-proto`

`nockapp-grpc-proto` owns Nockchain's protobuf schemas and generated Rust types. It contains message and service definitions, not server policy or kernel logic.

## Place in the system

- `proto/nockchain/private/v1` defines trusted-operator NockApp `Peek`, `Poke`, and `WatchEffects` RPCs.
- `proto/nockchain/public` defines public Nockchain query services.
- `proto/nockchain/common` defines shared primitives and error shapes.
- `proto/nockchain/monitoring` defines monitoring services.
- `nockapp-grpc` implements the services and re-exports the generated package tree.
- Wallets, miners, API clients, and nodes consume these types.

## Maintained invariants

- Existing protobuf field numbers and meanings are stable within a version. Fields are added with new numbers; incompatible shapes require a new package version.
- Binary nouns crossing RPC boundaries remain canonical JAM bytes. Protobuf frames them but does not redefine the noun.
- Private and public services remain separate trust domains. A private message becoming serializable does not make it safe for public exposure.
- `WatchEffects` messages carry one JAM-encoded effect. `head_filter` compares raw effect-head bytes and an empty filter means all live effects.
- Generated descriptors match the checked-in schema set so reflection and clients observe the same API.
- Conversion failures are explicit; malformed external data must not be interpreted as a partial protocol object.

## Security and distributed-systems properties

Protobuf provides framing and compatibility, not authentication, authorization, replay protection, consensus validity, or durable delivery. Deployments must apply those properties at the service and network layers. In particular, the private API permits kernel pokes and is equivalent to privileged node access.

Consensus objects decoded from these messages still require normal Hoon and Rust validation. No generated type is trusted merely because it passed protobuf decoding.

## Validation

```sh
cargo test -p nockapp-grpc-proto
cargo check -p nockapp-grpc-proto --all-features
```

# Historical AI-PoW compatibility fixture

`legacy-moe.jam` is a frozen, complete MoE block artifact generated with the
historical AI-PoW implementation. It is a synthetic honest fixture, not a block
captured from mainnet. It is retained to check compatibility with historical
verification rules below activation height 154,500.

The generating implementation is source-equivalent to public revision
`cbd9298f96584b14ab93074ecdf32d0ec212e50e`. This identifies the equivalent
implementation, not the original generation checkout.

`commit.bin` contains its Nockchain commitment, and `verifier-key-digest.bin`
contains its verifier key digest. The artifact has an 8192-row Layer-0 trace.
The fixture's auxiliary metadata is synthetic and does not establish a chain
height. No verifier context is loaded from the artifact: compatibility checks
build trusted setup independently.

`provenance.txt` records the byte length, BLAKE3 hashes, generation toolchain,
and original verification result. Preserve these frozen bytes when updating
compatibility checks. This single fixture does not establish full historical
conformance or cover replay of mainnet blocks.

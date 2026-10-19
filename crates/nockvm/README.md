# NockVM

Status: Experimental
Owner: Nockchain Runtime Maintainers
Last Reviewed: 2026-02-19
Canonical/Legacy: Legacy (crate-level runtime reference; canonical docs spine starts at [`START_HERE.md`](../../START_HERE.md))

*Trust posture: NockVM is experimental. Treat this README as contextual guidance, not canonical protocol or architecture policy.*

An experimental Nock runtime library plus historical New Mars/Ares design material.

## Build And Tests

See [DEVELOPERS.md](DEVELOPERS.md) for crate-local build/test commands and historical development context.

## Atom Construction

Empty byte input produces the zero atom. Atom-returning byte helpers use its direct representation without allocating.

Raw indirect constructors retain one initialized zero data word for an empty request so the result can be normalized safely. Returned byte, bit, and fixed-array views still have the requested length; an empty request does not expose additional writable data.

# `nockchain-libp2p-io`

`nockchain-libp2p-io` connects a NockApp kernel to Nockchain's peer-to-peer network. It implements libp2p behavior, gossip and request/response transport, catch-up requests, peer state, traffic prioritization, metrics, and peer/IP abuse controls.

## Place in the system

The crate sits between untrusted peers and the node's kernel:

```text
remote peers <-> QUIC/libp2p <-> nockchain-libp2p-io <-> NockApp effects/pokes
                                                           |
                                                     Hoon validation
```

It transports blocks and chain data but does not decide their validity or fork-choice weight.

## Maintained invariants

- Peer input is untrusted and bounded before it can consume unbounded memory, work, or queue capacity.
- Request/response messages retain request identity, expected peer, range, and response-size constraints across retries and fallbacks.
- Duplicate suppression and batching may change transport work, never the logical block or transaction presented to the kernel.
- Catch-up prefetch is advisory. Missing, reordered, duplicated, or stale responses cannot bypass kernel validation.
- Peer IDs, connection IDs, and network addresses remain distinct identities. Address/IP exclusions are applied only when the address is actually resolved for the offending connection.
- Objective cryptographic abuse may escalate to peer and address/IP exclusion. Reasons that can arise from protocol-version disagreement remain peer-scoped so an upgrade boundary cannot incorrectly ban an address shared by honest peers.
- Slow or malicious peers cannot hold global driver state locks across outbound channel waits.
- Transport penalties and peer selection affect availability and resource use, not consensus acceptance.

Configured peers retry recognized temporary transport outages through the existing
configured-peer backoff rather than discovery quarantine. This policy applies
only to the exact configured endpoint and its expected identity, including live
DNS aliases when applicable. Permission failures retain their dedicated cooldown;
wrong-identity, protocol, and unclassified failures do not receive this exception.
Other address/IP exclusions, admission limits, and recovery-slot protection
remain effective.

Automatic configured startup, backbone, and recovery dialing is single-flight
per configured endpoint: bootstrap and backbone requests do not start a second
configured attempt while one is pending. Actual terminal connection events or
connection establishment update that tracking, allowing normal retries. Manual
and force dialing remain distinct paths; this policy does not promise
single-flight behavior for all manual dials or broaden the endpoint, identity,
or recovery exceptions above.

Missing configured endpoints retain the existing bounded exponential background
retry cadence, capped at one hour. That cadence is distinct from losing the last
live configured path: a connection-availability transition to no configured path
re-arms recovery for five seconds, even if a dead backbone endpoint had driven
the background timer to its cap. Another remaining configured path does not
trigger that reset, and repeated no-path events do not postpone its deadline.
Existing dial windows, capacity limits and reserved recovery slots still apply.

The path-loss timer reset is reported at DEBUG level. Native connection-closure
events separately report the authenticated peer, connection ID, endpoint, and
remaining established sessions; stopping a remote process is not itself a local
connection-closure event. Outgoing connection-error events report their native
connection ID, optional peer, and typed error at DEBUG level before recovery and
address-hygiene handling.

Consecutive ping failures belong to the authenticated connection ID, not its
IP or peer's other connections. Only a successful ping on that same connection
resets its failure count; reaching the configured threshold closes only that
connection. Closure removes the count, and stale or mismatched ping events
cannot alter another session. Shared-IP admission, request quotas, and address
hygiene remain separate.

## Response cache bounds

Ordinary block and transaction responses share the existing response-cache
budget with elders responses: at most 4096 retained entries and 100 MiB of
allocated `NounSlab` arena bytes across those caches. Insertion evicts entries
as needed; an individual response whose arena exceeds the byte budget is
served without being retained. Eviction changes later kernel-query work, not
consensus validation.

The byte budget measures allocated cache arenas, not serialized response size
or whole-process memory. In-flight responses, copies, cache metadata, and other
node allocations are outside this limit.

## Batch response sizing

Responders project successful result sizes from CBOR header widths, envelope
metadata, and estimated payload lengths without constructing dummy bodies.
Projection work and temporary memory do not scale with the estimated payload length.

The response budget reserves one result per request, including a backpressure
tail for unexecuted items. Estimates can stop execution early; actual results
also pass an exact encoded-size check before acceptance. `NotFound` results
leave successful-response size hints unchanged.

Exact range assembly keeps a checked running encoded-size total. It counts each
borrowed candidate block once and accounts separately for canonical CBOR array
header widths, rather than cloning and reserializing every growing prefix.
The same contiguous-prefix soft budget, first-oversized-block exception, and
final hard response cap apply. This makes payload-sizing work linear in the
serialized candidate bytes; it is not a bound on kernel queries or cache work.

## Catch-up accounting

Prefetch admission uses the configured rolling per-peer and numeric-IP byte
budgets, together with the per-peer in-flight ceiling. All active candidate
connections must have available budget before a request is submitted. Multiple
connections do not create independent budgets for the same peer, and peer IDs
sharing an IP share its aggregate budget.

These byte budgets apply only to range-prefetch responses, not ordinary fetches.
The rolling window is 60 seconds. The configured default is 200 MiB per peer;
each IP bucket has four times the per-peer budget (800 MiB by default). A zero
per-peer byte cap disables both byte-budget gates, not the in-flight ceiling.

IP buckets group IPv4 addresses by /32 and IPv6 addresses by /64. Aggregate
history uses fixed one-second bins whose end timestamps never move when more
responses enter the same bin. Within a retained bucket, bytes never expire
before 60 seconds and may remain charged for at most one additional second.
Each bucket retains at most 61 bins, independent of response count or rate.
The aggregate history table is capped at 4096 buckets, pruning expired buckets
before evicting the bucket with the oldest recorded bin end when full.

Background block-range requests are independently submitted and accounted for,
while ordinary requests retain transport batching. Existing caps and timers
remain unchanged.

Range prefetch also remains demand-driven when future gossip is deferred:
on a genuine kernel by-height request at the current non-genesis frontier,
a future gossip block still retained in the bounded pending pool may qualify
the existing depth threshold (eight heights by default). This is an unvalidated
catch-up hint, not accepted-chain progress or permission to validate a future
block. Prefetched fills, stale heights, evicted blocks, and drained flush leases
cannot supply the hint. The request still starts at the demanded height and
uses the standard window and peer selection; the normal singleton, response
validation, quotas, in-flight limits, and timers remain unchanged.

Singleton block-height demand and alternate-peer retries share the same source
preference after per-height attempt filtering and the existing address/IP and
request-health eligibility policy. Peers supplying newly accepted chain progress
within the existing 60-second bandwidth window rank first, followed by live
outbound connections to configured initial or backbone endpoints. Configured
matches require the actual numeric endpoint or its live dial alias and any
expected PeerId; identity alone is insufficient. Within the highest eligible
preference tier, a shared per-tier cursor rotates sources across independent
height demands and alternate retries. Native PeerId ordering defines a stable
traversal, not permanent priority for the smallest identity. Lower tiers do not
displace eligible higher-tier sources. Gossip acknowledgments and pending blocks
do not earn accepted-chain progress preference. Dispatch remains one peer per
height request, with the existing attempt recycling, failure budget, backoff,
quotas, and timers unchanged. Other request routing retains its existing policy.

A transport response is not necessarily useful request success. `NotFound` and
acknowledgment-only batch results retain transport completion/latency accounting
but neither earn useful-success credit nor clear an existing request-health
cooldown. A routed result or positively validated gossip acknowledgment can earn
that credit; only kernel-accepted chain progress earns the top height-source
preference. Mixed responses retain per-item retry decisions.

Response accounting uses application response-stream bytes actually consumed by
the codec's bounded read, including accepted trailing bytes and alternate CBOR
encodings, and the actual request/response connection's numeric endpoint. The
read stops at the configured byte cap: a truncated value fails decoding, but a
complete value before the cap may succeed even if the sender supplied more
bytes. Bytes beyond the cap and QUIC framing are not counted. Requests retain their
issue-time connection snapshots until completion, failure, or consumer
retirement. Disconnecting a peer does not reset aggregate IP-window history.
An exhausted candidate IP pauses that peer's prefetch while eligible peers on
other IPs remain available.

Response validation retains the responding connection's identity and numeric
endpoint through asynchronous consumption and connection closure. Address
cooldowns use that retained endpoint, not another address advertised by the
same peer. Without connection evidence, no address is inferred.

## Gossip producer context

The kernel's internal gossip effect is
`[%gossip context=[origin phase source=(unit peer-id)] p=fact]`. Context belongs
to one emitted effect; it is not persisted with a transaction or added to the
network fact. Outgoing fact version and payload encoding remain unchanged.

The first accepted peer transaction is forwarded to other peers, not back to
its causal source. Peer duplicates do not re-announce it. An explicit local
resubmission can immediately reach that previous source, and intentional
chain-progress and mining refreshes retain their normal fanout. Block gossip
continues to exclude its separately bounded set of known sources.

Origins are `peer`, `local`, `chain`, `mining`, or `kernel`. Phases distinguish
initial acceptance, local resend, a pending head, a new accepted head, and
mining regossip. A pending head is not a fully accepted block, and a mining
candidate's transaction is not necessarily already mined.

Debug observations carry the decoded content ID and a per-process effect
sequence through native request submission. The effect observation clock does
not claim the kernel's commit time. Issuance records the actual request ID and
candidate connections; the actual response event identifies the selected
connection.

## Address verification

External-address admission is passive: two distinct authenticated, live PeerIds
must report exactly the same numeric IP/nonzero UDP/QUIC-v1 endpoint. Each
report must come from its identified live connection with a resolved numeric
remote endpoint; unknown, foreign, closed, or unresolved connections cannot
vote. Repeated messages, parallel sessions, and reconnects from one identity
never count as additional observers.

Claims must use a unicast, non-unspecified IP. A loopback claim is eligible only
when its observer's actual connection endpoint is loopback. Observers need not
have different IPs or subnets: localhost, same-LAN, and shared-NAT peers retain
normal behavior.

The pending table retains at most 64 claims and eight peers per claim. Each
identity has a shared four-claim budget across pending votes and unexpired
active or retired admission receipts. Pending votes expire after ten minutes;
repeats, overlapping sessions, and reconnects retain the original timestamp.
An admission retains exactly the two selected live witness PeerIds. Its
immutable receipt lasts one hour from admission, independent of the pending-vote
lifetime. Repeated active observations, witness disconnections, replacement,
and external ownership or expiry notifications do not extend that hour or
refund its charges.

The manager owns at most 16 automatic advertisements. With receipt capacity
available, a fresh budget-eligible quorum immediately replaces the oldest
manager-owned advertisement when those slots are full. The removed address's
receipt remains charged until its original expiry, but does not ban the address.
A fresh quorum can readmit it with a new receipt, admission time, and charge;
every previous receipt remains charged independently until its own expiry.
Active and retired receipts together are bounded at 64; when this history is
full, new admissions wait for receipt expiry rather than forgetting charges or
withdrawing an existing advertisement. A fixed pair can sponsor at most four
unexpired admissions, not fill all 16 slots.

Expiry is lazy on valid observations: expired manager-owned advertisements are
withdrawn before any new admission is installed. Invalid claims or invalid
sessions do not trigger pruning. Existing independent/manual advertisements
are never acquired or removed by this manager, and independently confirmed
addresses relinquish manager ownership while retaining their receipts. The
16-advertisement bound covers only manager-owned automatic advertisements,
not the swarm's independent/manual advertisements. No expiry starts a probe.

With only one eligible observer, an address remains unconfirmed while the node
continues listening and making outbound connections. Remote listen-address
discovery, exclusions, and bootstrap/backbone scheduling remain independent of
this quorum, including discovery from heterogeneous peers. Initial and backbone
peers receive no trust bypass. Claimed addresses never earn IP-health credit;
only successful pings on validated connection endpoints provide that credit.
There are no address-check settings, probes, or verifier servers.

## Security and distributed-systems dependencies

QUIC/TLS and libp2p authenticate transport peers, not blocks. Block authenticity and validity come from consensus proofs, signatures, hashes, and the Hoon validation path. Kademlia and gossip are discovery/distribution mechanisms and may return adversarial data.

Network correctness assumes eventual access to at least one honest peer for synchronization. The implementation must remain safe under Byzantine peers, duplicated messages, partial responses, disconnects, reordering across connections, and local cache loss. Liveness policies must not create a second interpretation of protocol validity.

## Validation

```sh
cargo test -p nockchain-libp2p-io --lib
cargo check -p nockchain-libp2p-io
```

The integration harness under `test_support` is for tests and does not weaken production admission rules.

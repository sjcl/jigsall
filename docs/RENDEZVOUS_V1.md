# Rendezvous client v1

`game/src/network/gns/rendezvous/` implements the optional production routing
adapter for `sjcl/puzzella-rendezvous`. It is independent of Bevy runtime/UI,
gameplay authority, core, WireMessage, replication and Sync. The server protocol
and golden examples are canonical in that repository's `docs/PROTOCOL_V1.md`;
the mirrored schema is `protocol.rs`, with identical
`game/tests/fixtures/protocol_v1.jsonl` vectors.

## Features and use

`rendezvous` forwards `gns` and optional Tokio/tokio-tungstenite/rustls,
futures-util, URL and Base64 dependencies. Default still enables neither GNS nor
WebSocket dependencies. `gns` alone retains the existing Direct IP backend and
native P2P foundation without the WebSocket client. Root feature forwards both
game and UI GNS features as before.

```rust,ignore
use puzzella_game::network::gns::{IceConfig, rendezvous::{EndpointUrl, RendezvousAdapter}};
let url = EndpointUrl::production(configured_wss_url)?;
let (mut backend, mut adapter) = RendezvousAdapter::new(url, 0, configured_ice)?;
// Caller polls adapter and backend each frame, starting create/join after Welcome.
// On HostReady: backend.connect_peer(host_peer_id, remote_virtual_port)?;
// Then SecureTransport::new(backend) + existing password bootstrap, owned by caller.
```

The factory constructs `GnsP2p::new_routed()` itself; it cannot accept an
unverified backend or arbitrary public SignalingEndpoint. The former unverified
foundation constructor is named `new_unverified_for_test`; routed tests stay
unchanged. P2P singleton identity and GNS callback/poll model remain unchanged.
Configure endpoint and `IceConfig` in the caller. Public candidates and STUN are
separate GNS settings; no STUN address, game password or PAKE value enters
rendezvous.

## Poll / worker ownership

A dedicated named thread owns a Tokio current-thread runtime. DNS, TLS,
WebSocket handshake/read/write and heartbeat all execute there. The worker and
adapter exchange typed commands/events using separate bounded 32-slot channels;
there is no unbounded queue. Frame/message limits are 24 KiB, opaque signaling
is 16 KiB maximum. TLS connection deadline is 10 seconds, writes 2 seconds,
client Ping/matching-Pong deadline 15/15 seconds. A fixed window bounds inbound
frames at 512/second. Worker overflow reports Backpressure and closes only the
control plane; the terminal status uses one bounded slot.

`poll()` consumes at most 32 worker events and 32 GNS outbound signals per call.
It retains at most one unsent command under backpressure, including an ACK; the
route is installed first and that ACK is retried before reading more messages.
GNS's own fair bounded signaling mailbox and per-route limits still apply.
Inbound mailbox congestion emits SignalBackpressure and sheds that signal,
allowing other routes to continue. Unknown/unavailable senders, conflicting
route identity, invalid schema/version/Base64 or unexpected state fail closed.

Drop/shutdown signals the worker asynchronously and never joins it on the frame
thread. Cancellation covers pending connection and I/O futures; the worker's
runtime gets a bounded one-second shutdown budget for resolver work.
`worker_finished()` permits tests/owners to observe completion without blocking.

## Routing transitions

Host: Welcome → Idle → create_room → RoomCreated → Host. AuthorizePeer is accepted
only in Host and within a 64-route cap. It creates
`RouteOrigin::from_authenticated_route(authority_id, room_id, member_id)` and
calls authorize_peer before queuing AuthorizeAck. PeerJoined must match that
pending member identity before it becomes active and is exposed to the caller.
The server enqueues PeerJoined to the host before RoomJoined to the joiner,
serializing the state commit and both enqueues against concurrent relays.
Thus each socket receives its activation notification before any Signal,
including an immediate response from either peer. If the host's activation
notification cannot be queued, the server never releases the joiner. The adapter
keeps pending-sender rejection as a check of this authenticated-server contract;
an early Signal never implicitly activates a route.

Joiner: Welcome → Idle → join_room → Joining → RoomJoined. It installs the host
route from the server's authority/room/host MemberId, then emits HostReady.
Neither flow creates ClientBootstrap or starts SPAKE2/Sync/Ready. Membership and
native Transport::Connected confer no game authentication. Existing password
bootstrap remains the only secure-channel installer.

For every poll, `pop_outbound()` becomes Signal(to_peer_id, Base64 opaque bytes).
Server Signal.from_peer_id is checked against an active, available binding before
`SignalingEndpoint::receive` receives the opaque bytes. PeerId remains a routing
label. `RouteOrigin.account` is a **server-issued anonymous MemberId**, not a
Steam account, account authentication, or complete Sybil resistance. Future
account authentication must replace this field through a versioned trusted
server contract; the server also guards IP/admission/room/rate resources.

PeerUnavailable disables future control routing; RoomClosed/Disconnected disables
all further adapter routing. Active bindings remain installed, so GNS maintain
does not mistake WebSocket loss for gameplay disconnection. Pending/unactivated
bindings are revoked on cleanup. The caller may explicitly `release_route(peer)`
after established connection cleanup or a security decision. That operation can
close a live GNS route on its next poll, so it is never automatic for an active
route. Retained bindings still consume the finite 64-route cap until released.
v1 has no resume; create a new adapter/backend after owner-managed cleanup.

Production `EndpointUrl::production` requires `wss://.../v1/ws`, WebPKI-validated
certificates, and rejects credentials, query and fragment. The explicit
`loopback_for_test` path permits plaintext only for literal 127/8 or ::1 IPs;
DNS `localhost` and all remote `ws://` URLs are rejected. No insecure certificate
option exists.
The worker builds TLS with an explicit ring crypto provider and WebPKI roots,
without mutating global TLS configuration. A local in-memory TLS handshake test
asserts that this production configuration rejects an untrusted self-signed
certificate; the accompanying fixture private key is public test data.

## Local verification

```sh
cargo fmt --all --check
cargo check --workspace --locked
cargo check --workspace --locked --features gns
cargo check --workspace --locked --features rendezvous
cargo test --locked -p puzzella-game --features rendezvous rendezvous
cargo test --locked -p puzzella-game --features rendezvous gns_localhost -- --nocapture --test-threads=1
```

Local WS fixtures cover Welcome/create/join, route-before-ACK, HostReady binding,
opaque inbound/outbound, unknown/pending sender, mismatched member and conflicting origin,
bounded command/event queues and ACK retry, worker shutdown, pending revocation,
active route preservation and endpoint security. Both repositories parse the
same canonical golden messages and reject unknown version/type/sender fields.
Existing Direct IP and P2P native foundation tests remain in the suite. Linux
all-features CI and the Windows adapter fixture step exercise this feature without
checking out the server repository.

## Two-process real-server smoke

In the server repository, start the real binary:

```sh
cargo run --locked
```

In this repository, using PowerShell:

```powershell
$env:PUZZELLA_RENDEZVOUS_SMOKE_URL = 'ws://127.0.0.1:8080/v1/ws'
cargo test --locked -p puzzella-game --features rendezvous gns_localhost_real_rendezvous_native_ice_password_secure_lanes -- --ignored --nocapture --test-threads=1
Remove-Item Env:PUZZELLA_RENDEZVOUS_SMOKE_URL
```

The ignored test launches two independent native GNS process identities. The host
creates the room through the real server; the parent passes only its room code
to the joiner. Signaling goes exclusively through the WS adapter/server. Each
process asserts actual native ICE details, server-bound Origin, Connected before
channel installation, SPAKE2 Authenticated, no Ready player mapping, encrypted
Control/Transient/Bulk records and exact received payloads. Both WebSocket workers
then stop, and all three lanes are exchanged again before explicit game close.
The test uses private host candidates and no external STUN/network. It does not
establish real Internet NAT compatibility or cross-OS bit equivalence.

## Verification record (2026-10-04)

Windows x86_64, Rust 1.97.0, development/test profile, LLVM/libclang 18.1.8:

| Check | Result |
| --- | --- |
| Server fmt / all-target Clippy / tests / binary build | Passed; 15 tests |
| Puzzella default workspace check and tests | Passed |
| Direct-IP-only `gns` check and game tests | Passed; 720 game tests + 16 serial native localhost tests |
| `rendezvous` workspace check and all-target Clippy | Passed |
| Rendezvous adapter/protocol/worker/TLS fixtures | Passed; 13 tests |
| Rendezvous game regression + serial native localhost tests | Passed; 731 regression tests (before the final TLS/Drop fixture additions), plus 16 localhost tests |
| Real server + two independent routed GNS processes | Passed: native ICE, SPAKE2, encrypted 3-lane exchange, then 3-lane exchange after both WS workers stopped |
| Mirrored protocol schema and canonical golden JSON | Byte-identical across both repositories |

The MSVC multi-config GNS library search path was supplemented through the
command's `LIB` environment per WINDOWS_BUILD.md. Shared Rust cache and vcpkg
paths were preserved; build lock waits were allowed to finish. This is local
loopback validation; deployed WSS and real Internet NAT combinations remain to
be tested. Fixture keys are self-signed public test data, never trust roots.

## Next integration

Real Internet NAT trials still need a deployed WSS endpoint, caller-configured
STUN + public ICE candidates, runtime Host/Join ownership and SecureTransport/
bootstrap handoff, then room-code UI. Some NAT/firewall pairs may require TURN;
TURN/relay is not implemented. UI, Steam account auth/lobbies, matchmaking,
persistence, resume and rendezvous host migration remain separate work.

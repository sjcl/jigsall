# Rendezvous client v1

`game/src/network/gns/rendezvous/` implements the optional production routing
adapter for `sjcl/puzzella-rendezvous`. The adapter remains independent of Bevy runtime/UI, gameplay authority, core,
WireMessage, replication and Sync. The Internet runtime driver now owns it and
hands established P2P connections to the existing game runtime. The server protocol
and golden examples are canonical in that repository's `docs/PROTOCOL_V1.md`;
the mirrored schema is `protocol.rs`, with identical
`game/tests/fixtures/protocol_v1.jsonl` vectors.

## Features and use

`rendezvous` forwards `gns` and optional Tokio/tokio-tungstenite/rustls,
futures-util, URL and Base64 dependencies. Default still enables neither GNS nor
WebSocket dependencies. `gns` alone retains the existing Direct IP backend and
native P2P foundation without the WebSocket client. Root `rendezvous` forwards game and UI Internet features; `gns` alone exposes
Direct IP. The normal UI reads only start/cancel APIs and NetworkStatus.

```rust,ignore
use puzzella_game::network::gns::{IceConfig, rendezvous::{EndpointUrl, RendezvousAdapter, P2P_VIRTUAL_PORT}};
let url = EndpointUrl::production(configured_wss_url)?;
let (mut backend, mut adapter) = RendezvousAdapter::new(url, P2P_VIRTUAL_PORT, configured_ice)?;
// Caller polls adapter and backend each frame, starting create/join after Welcome.
// On HostReady: backend.connect_peer(host_peer_id, P2P_VIRTUAL_PORT)?;
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
route. The standalone `poll()` retains this explicit-owner contract. Production
runtime uses `poll_with_peer_connections` and `reclaim_unavailable_routes`:
unavailable bindings with neither an owned native handle (including Connecting)
nor queued inbound signals are released. Reconciliation runs between control
messages, before another bind can exhaust the finite 64-route cap, and after
native polling to reclaim ICE failures before Connected. The bounded route table
itself tracks unavailable peers; no additional unbounded peer history is kept.
HostReady retains its installed route through delivery of that poll's events,
even when control loss follows RoomJoined in the same batch, so the owner can
start connect_peer before reconciling native ownership.
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

## Internet runtime and ordinary UI

`NetworkSession` owns `RendezvousRuntimeDriver`; UI never polls the adapter or
accesses GnsP2p, signaling, or route origin. Direct IP still uses `start_host` /
`start_join`; Internet uses `start_rendezvous_host` / `start_rendezvous_join` with
separate, owned options. Both use shared private World preparation helpers for
puzzle/image validation, persistence generation, old-puzzle cleanup, input and
selection reset, and GameSetup transition. Hosting from UI waits for the existing
CPU generation and RenderReady barriers before either entrypoint.

Common `Runtime<T: Transport>` owns SecureTransport, bootstrap, sync, presentation,
commands, decode and game connection teardown. It has no listener token or
address establishment API. `DirectIpDriver<T: DirectIpTransport>` owns explicit
listener cleanup, including failed address lookup, Cancel, failure and Menu.
There is no DirectIpTransport implementation on GnsP2p and no shared address enum.

The small driver stages are ControlConnecting, CreatingRoom, JoiningRoom,
PeerConnecting and Running. Host constructs its one common runtime/bootstrap
from one validated puzzle/image and owned password, but does not poll it until
RoomCreated. Welcome sends CreateRoom; RoomCreated publishes the canonical code
and Hosting phase. The adapter stays alive to process AuthorizePeer, PeerJoined
and Signal for subsequent joiners. Both peers use `P2P_VIRTUAL_PORT` (v1: 0).

Join holds only backend and owned request before HostReady. Welcome sends
JoinRoom; the adapter processes RoomJoined and installs the host route before
emitting HostReady. The driver calls connect_peer, obtains ConnectionId, wraps
SecureTransport and only then constructs ClientBootstrap. GNS Connected starts
mandatory SPAKE2; Authenticated starts the existing image/baseline/catch-up sync;
only the normal Ready commit assigns gameplay identity. RoomCreated, HostReady,
Connected, Authenticated and Ready are separate states. A room code is a routing
identifier, never a password credential.

`NetworkStatus.connection_method` distinguishes Internet/DirectIp; `address` is
Direct IP only; `room_code` is Internet host only. `rendezvous_control` separately
reports Connecting/Available/Unavailable. WSS/room/ICE map to Connecting, SPAKE2 to
Authenticating, then the existing Syncing and Ready phases. Server UnknownRoom,
capacity, timeout, protocol and connection errors map to typed UI categories;
diagnostics are never parsed for presentation.

**Rendezvous control loss != gameplay disconnect.** Before RoomCreated/HostReady,
control loss fails establishment. An Internet Host failure before RoomCreated
tears down the network session and returns to Host settings while retaining the
prepared CPU store, image and render epoch. Retry uses that same generated or
loaded puzzle and asks for the password again; Cancel returns to Menu and performs
the normal puzzle cleanup. Join failures retain the normal Menu path.
After RoomCreated or connect_peer, RoomClosed,
Disconnected or PeerUnavailable never closes the existing GNS connection or
revokes its active route. Pending ICE remains governed by GNS's own connection
deadline. Existing Ready gameplay continues; host code is removed and control
status becomes Unavailable with a localized warning that new players cannot join.
Unavailable routes are released once native handles and pending inbound signals
are gone, including peers that never became Connected. An owned Connecting handle
or a received Signal awaiting native admission keeps the route. Departure of one
live connection also preserves a Pending sibling for the same peer. No resume is added.

During Hosting/ICE/gameplay, server Backpressure and RateLimited are nonfatal:
the control socket, Available status and host code remain usable. A later actual
Disconnected still marks control Unavailable, and protocol errors still shut down
the control plane. Before RoomCreated/HostReady, typed establishment failures are
unchanged.

Cancel/Leave/Menu stop the adapter worker asynchronously, explicitly close game
connections, then drop bootstrap/sync/secure channels, backend and pending owned
passwords. Direct IP additionally closes its listener through SecureTransport's
existing event drain. Workers do not access World/GPU and do not block the frame
thread on join. Frame work remains bounded by adapter/connection caps; there is
no added piece/component/selection scan or GPU readback.

Multiplayer offers Internet / Direct IP. Internet host reuses puzzle selection and
asks for the committed player name and password, with no address/port fields.
The resulting code has Copy buttons in host status/HUD and the pause overlay.
Internet join asks for Room Code and password, retaining code independently of the
Zeroizing password draft. Input normalizes ASCII upper case and validates with
the protocol RoomCode parser (10 Crockford characters, no I/L/O/U aliases).
Wrong password/Back retains the code and clears secrets. Direct IP retains bind,
hostname/IP/port, DNS resolution and same-puzzle HostStartRequest address retry.
All added labels/errors are in the English and Japanese Fluent catalogs.

## Deployment configuration

Under `rendezvous`, explicitly inject `RendezvousRuntimeConfig { endpoint, ice }`
into the application World. It contains only validated EndpointUrl and IceConfig;
no password/code is stored there. Without it the Internet option is disabled and
Direct IP remains usable. No third-party STUN or production URL is supplied.
The ordinary binary can populate the resource from operator environment:

- `PUZZELLA_RENDEZVOUS_WSS_URL`: production `wss://…/v1/ws`, validated by EndpointUrl.
- `PUZZELLA_ICE_STUN_SERVERS`: comma-separated caller-configured STUN addresses.
- `PUZZELLA_ICE_ALLOW_PUBLIC_CANDIDATES`: `true` or `false` (default false).

Missing/invalid WSS configuration disables Internet. Remote plaintext WS and
certificate bypass are unavailable. Local tests explicitly inject
`EndpointUrl::loopback_for_test`, private candidates and no STUN. To test the
actual runtime against the real loopback server, start it as above and run:

```powershell
$env:PUZZELLA_RENDEZVOUS_SMOKE_URL = 'ws://127.0.0.1:8080/v1/ws'
cargo test --locked -p puzzella-game --features rendezvous gns_localhost_real_rendezvous_runtime_ready_command_roundtrip -- --ignored --nocapture --test-threads=1
Remove-Item Env:PUZZELLA_RENDEZVOUS_SMOKE_URL
```

This ignored test uses two independent native GNS processes, the public runtime
Host/Join entrypoints, code creation, native ICE, mandatory SPAKE2, image decode,
baseline/catch-up, Ready, Grab/Release roundtrip and session teardown. Regular CI
uses local transport/control seams for the same driver state machine and scheduled
egui tests; it needs no external repository or Internet access.

Real Internet NAT trials still require production WSS configuration, caller STUN
configuration, public candidates, and separate machines behind different NATs.
Some NAT/firewall pairs may require TURN; TURN/relay is not implemented.

## Runtime/UI verification record (2026-10-05)

Windows x86_64, Rust 1.97.0, LLVM/libclang 18.1.8, shared Cargo/vcpkg paths.
Foundation base: puzzella `4ac3c3d` / server `8c6900c` (latest fetched
`codex/rendezvous-v1` at work start). Server protocol is unchanged; only the stale
TCP-only abuse-source paragraph was corrected to the existing trusted-proxy rules.

| Command / configuration | Result |
| --- | --- |
| `cargo fmt --all --check`, `git diff --check` | Passed |
| `cargo clippy --workspace --locked --all-targets -- -D warnings` | Passed, default |
| Same Clippy command with `--features rendezvous` | Passed, all targets |
| `cargo test --workspace --locked` | Passed: game 706, UI 68; workspace/doctests passed |
| Same test command with `--features gns -- --skip gns_localhost` | Passed: game 720, UI 69; workspace/doctests passed |
| Same test command with `--features rendezvous -- --skip gns_localhost` | Passed: game 740, UI 78; workspace/doctests passed |
| `cargo test --workspace --locked --features rendezvous gns_localhost -- --nocapture --test-threads=1` | Passed: game 17, UI 1; ignored cross-repository tests remain opt-in |
| `cargo test --workspace --locked --features rendezvous gns_localhost_real_rendezvous_runtime_ready_command_roundtrip -- --ignored --nocapture --test-threads=1` | Passed with real loopback server and two native process identities |
| `cargo build --workspace --locked` | Passed, default |
| Same build command with `--features gns` | Passed, Direct IP application linked |
| Same build command with `--features rendezvous` | Passed, Internet application linked |

Seven new regular runtime tests cover code publication, delayed bootstrap creation,
pre-establishment failure, cancellation/drop, ICE-time control loss, Ready-time
control loss with command roundtrip, typed errors and config-absent World preservation.
Nine new UI tests cover method switching/availability, Internet/Direct fields,
protocol-based code validation, scheduled valid/invalid submission, clipboard command,
HUD/pause code and warning, secret masking, and wrong-password code retention.
Existing Direct IP retry, DNS, runtime Ready/commands/cursors and Menu teardown
regressions passed. Windows CI now also runs the full rendezvous runtime/UI suite.

An initial MSVC LNK1181 was resolved by adding the already-built GNS `out/lib` to
the command's LIB search path, as documented in WINDOWS_BUILD.md. No Rust build
cache/vcpkg path was changed, and build lock waits were allowed to complete.
An initial UI test used the wrong localized button label; it was corrected, and
the nine Internet UI tests plus the full workspace suite passed afterward.
This record covers local/headless runtime and scheduled egui checks; it does not
claim real Internet NAT traversal, cross-OS behavior, GPU frame-rate or visible
window end-to-end validation.

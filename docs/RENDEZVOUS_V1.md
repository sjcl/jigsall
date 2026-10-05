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
use jigsall_game::network::gns::{IceConfig, rendezvous::{EndpointUrl, RendezvousAdapter, P2P_VIRTUAL_PORT}};
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
frames at 512/second. A full event queue holds one additional event and pauses
worker reads for up to 2 seconds while outgoing commands continue. This avoids
deadlock when both owner channels fill and allows a room-wide 64-member expiry
burst to survive between owner polls. A stalled
owner or frame-rate exhaustion reports Backpressure and closes only the control
plane; the terminal status uses one bounded slot. Owner poll and shutdown remain
nonblocking, and cancellation interrupts the worker's event-capacity wait.

`poll()` consumes at most 32 worker events and 32 GNS outbound signals per call.
It retains one unsent ACK/Reject command and one separate unsent Signal under
backpressure. Required ACK/Reject commands are retried before reading more
messages; then all Revoke commands precede Confirm commands, followed by deferred
and new Signals. An unavailable/revoking/released route discards its deferred
Signal so opaque bytes cannot cross a later PeerId rebinding.
Up to 64 routes also retain one lifecycle command each until the worker accepts it.
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
An unavailable/revoking retained PeerId, a full route table, or insufficient
native connection headroom instead sends AuthorizeReject(join_id). The host room
and old native binding continue. Identity contradictions (available duplicate
peer, MemberId/JoinId collision, or self identity) still fail closed, even when
capacity is exhausted. Native headroom is advisory; GNS retains its per-request
admission check for total, Connecting and not-Ready handles.

The adapter retains at most 64 rejected transactions separately from native
routes. The server releases each pending slot immediately, returns Capacity to
the joiner and emits PeerUnavailable to the host as completion. Server FIFO orders
an old member's departure before replacement authorization and the rejection's
completion before further reuse of that PeerId. This completion clears only the
rejected transaction, even if the old native route has already been reclaimed.

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
runtime uses `poll_with_admission` and `reclaim_unavailable_routes`:
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
cargo test --locked -p jigsall-game --features rendezvous rendezvous
cargo test --locked -p jigsall-game --features rendezvous gns_localhost -- --nocapture --test-threads=1
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
$env:JIGSALL_RENDEZVOUS_SMOKE_URL = 'ws://127.0.0.1:8080/v1/ws'
cargo test --locked -p jigsall-game --features rendezvous gns_localhost_real_rendezvous_native_ice_password_secure_lanes -- --ignored --nocapture --test-threads=1
Remove-Item Env:JIGSALL_RENDEZVOUS_SMOKE_URL
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
| Jigsall default workspace check and tests | Passed |
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

### Game-authenticated membership

The unreleased v1 schema is updated together with `puzzella-rendezvous`.
AuthorizeAck opens routing, with a fixed 30-second deadline for game authentication.
The host runtime sends ConfirmPeer(peer_id, member_id) as soon as its existing
SPAKE2 bootstrap reaches Authenticated (or Syncing in that same poll), before
Ready/image transfer/baseline/catch-up. No password, PAKE bytes or player identity
is sent to the server. Only the current room host may confirm the exact
server-issued MemberId. Established members have no game-auth deadline.

Silent Routed members expire even while answering Ping/Pong; signals cannot renew
the deadline. Expiry frees the member slot and sends PeerUnavailable to the host.
Definitive native/auth/bootstrap failure sends RevokePeer when the last native
handle for that peer is gone, including failures before Connected or within one
poll. A Pending sibling preserves the member. Native retirement notices and
unsent lifecycle commands are bounded; per-route commands survive worker channel
backpressure, with Revoke superseding an unsent Confirm. Revoking routes retain
their binding until PeerUnavailable and discard in-flight signals. Host
UnknownTarget/JoinTimeout responses are nonfatal races with server cleanup.

Pending (12-second ACK deadline), Routed (30-second game-auth deadline), and
Established members all count toward the server's room capacity. These deadlines
limit unauthenticated occupancy; room codes and anonymous memberships still have
no account/Sybil guarantee. A Host must poll promptly to send its notifications.

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
Direct IP remains usable. The ordinary binary embeds optional local deployment
defaults at build time and applies operator environment overrides at startup.
Copy root `internet-defaults.env.example` to `internet-defaults.env`, set the
three values below using plain `KEY=value` lines (no quotes, expansion or inline
comments), then build with `rendezvous`. The local file is Git ignored; its values
are embedded in the executable, so distributing it alongside the binary is
unnecessary. They are public connection settings, not secrets. Editing or removing
the file triggers a rebuild. Without the file, built-in defaults are an empty
WSS URL, an empty STUN list and `false` for public candidates.

Each runtime environment variable overrides only its corresponding embedded value:

- `JIGSALL_RENDEZVOUS_WSS_URL`: production `wss://…/v1/ws`, validated by EndpointUrl.
- `JIGSALL_ICE_STUN_SERVERS`: comma-separated caller-configured STUN addresses.
- `JIGSALL_ICE_ALLOW_PUBLIC_CANDIDATES`: `true` or `false`.


An empty WSS override disables Internet; an empty STUN override clears the list.
Missing/invalid WSS configuration disables Internet. Invalid/non-Unicode overrides
disable Internet rather than silently falling back. Remote plaintext WS and
certificate bypass are unavailable. Local tests explicitly inject
`EndpointUrl::loopback_for_test`, private candidates and no STUN. To test the
actual runtime against the real loopback server, start it as above and run:

```powershell
$env:JIGSALL_RENDEZVOUS_SMOKE_URL = 'ws://127.0.0.1:8080/v1/ws'
cargo test --locked -p jigsall-game --features rendezvous gns_localhost_real_rendezvous_runtime_ready_command_roundtrip -- --ignored --nocapture --test-threads=1
Remove-Item Env:JIGSALL_RENDEZVOUS_SMOKE_URL
```

This ignored test uses two independent native GNS processes, the public runtime
Host/Join entrypoints, code creation, native ICE, mandatory SPAKE2, image decode,
baseline/catch-up, Ready, Grab/Release roundtrip and session teardown. Regular CI
uses local transport/control seams for the same driver state machine and scheduled
egui tests; it needs no external repository or Internet access.

The real-server runtime test transfers a 1600x1600 BMP (about 10 MB), exceeding the
reliable queue, and verifies the received image hash and decoded dimensions.
Room Code P2P uses the same post-authentication native send-rate configuration as
Direct IP: both `SendRateMin` and `SendRateMax` are set to 4 MiB/s before sync.
Previously only Direct IP raised them, leaving Room Code at GNS's 256 KiB/s
defaults. Raising only `SendRateMax` does not grow the pinned bandwidth estimate.
Image chunks remain at most `32 KiB - 64` bytes, with multiple chunks per frame;
the shared 128 KiB/frame, 4 MiB/s generation budget and 240 KiB Bulk queue bound
still apply. The rendezvous server carries signaling, not image data. Throughput
depends on the network, frame rate and queue drainage.

On 2026-10-05, Windows x86_64 / Rust 1.97.0 release verification passed 310
network tests, 20 native localhost tests and both real-server smoke tests.
Workspace Clippy with `rendezvous` (all targets, warnings denied), formatting and
diff checks passed. The isolated large-image runtime smoke completed in 2.93 s;
this includes connection/authentication, image transfer/decode, baseline/catch-up,
Ready, Grab/Release and teardown, with two headless processes polling at 1 ms.
The server used loopback WS and private ICE candidates. This is a local test
result, not an Internet throughput or native-window frame-rate guarantee.

Real Internet NAT trials still require production WSS configuration, caller STUN
configuration, public candidates, and separate machines behind different NATs.
Some NAT/firewall pairs may require TURN; the current UDP fallback and its
credential lifetime are documented below.

## Runtime/UI verification record (2026-10-05)

Windows x86_64, Rust 1.97.0, LLVM/libclang 18.1.8, shared Cargo/vcpkg paths.
Foundation base: jigsall `4ac3c3d` / server `8c6900c` (latest fetched
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

## Retained-route admission verification (2026-10-05)

An unauthenticated client could reconnect with the PeerId of its unavailable
native route, causing AuthorizePeer → bind → ProtocolViolation → control_lost.
Retained-route capacity divergence reached the same fatal boundary. The invariant
is that local admission shortage rejects only the pending join and preserves the
room/native connection, while contradictory server-issued identities still fail
closed. The fix extends the existing v1 control messages with AuthorizeReject,
keeps bounded rejection completions separate from old routes, and prioritizes
lifecycle commands above both deferred and new signaling.

The regression `retained_peer_and_full_route_table_reject_without_protocol_failure`
failed against the original implementation and passes with this patch. Tests in
`game/src/network/gns/rendezvous/tests/admission.rs` cover retained/revoking peers,
route retirement before rejection completion, full local route/native capacity,
130 rejection/reuse cycles, identity contradictions, one-slot command priority,
obsolete signaling on rebind and a full inbound queue with outgoing control.
The runtime test checks independent native headroom propagation. Mirrored strict
parser/golden tests cover AuthorizeReject. Server state and WebSocket tests cover
immediate resource release, same-PeerId reuse, host authority and cancelled/expired
authorization races. Existing auth, control-loss and gameplay tests remain controls.

| Verification gate / command | Result |
| --- | --- |
| Both repositories: `git diff --check`, Cargo fmt check | Passed |
| App: `cargo clippy --workspace --locked --all-targets --features rendezvous -- -D warnings` | Passed |
| Server: `cargo clippy --locked --all-targets -- -D warnings` | Passed |
| App: `cargo test --locked -p jigsall-game --features rendezvous network::gns::rendezvous -- --nocapture` | Passed, 28 adapter/protocol tests |
| App: `cargo test --workspace --locked --features rendezvous -- --skip gns_localhost` | Passed, workspace/unit/doc tests |
| Server: `cargo test --locked --quiet` | Passed, 34 unit and 8 WebSocket tests |
| App: `cargo test --workspace --locked --features rendezvous gns_localhost -- --nocapture --test-threads=1` | Passed, 17 game and 1 UI test |
| App: `cargo test --workspace --locked --features rendezvous gns_localhost_real_rendezvous -- --ignored --nocapture --test-threads=1` | Passed, 2 real-server/two-process native tests |
| App: `cargo build --workspace --locked --features rendezvous`; server: `cargo build --locked` | Passed |

Independent read-only candidate review found no concrete remaining bypass or
regression. Real loopback tests preserve SPAKE2, encrypted Control/Transient/Bulk
after WS shutdown, and Room Code → Ready → Grab/Release. Both v1 schema copies and
golden fixtures match. Production NAT/WSS/Caddy, cross-OS, GPU and performance
trials were not rerun; existing opt-in GPU/benchmark/Caddy tests remain skipped.

## Deployed WSS verification (2026-10-05)

Windows x86_64, Rust test profile, client commit `021f957`, using
`wss://rendezvous.jigsall.sjcl.me/v1/ws` with normal certificate validation.
The deployed server's `/healthz` returned HTTP 200 through Caddy.

```powershell
$env:JIGSALL_RENDEZVOUS_SMOKE_URL = 'wss://rendezvous.jigsall.sjcl.me/v1/ws'
cargo test --locked -p jigsall-game --features rendezvous gns_localhost_real_rendezvous -- --ignored --nocapture --test-threads=1
Remove-Item Env:JIGSALL_RENDEZVOUS_SMOKE_URL
```

Both opt-in tests passed (2 passed, 0 failed):

- Native ICE and SPAKE2 authentication, encrypted Control/Transient/Bulk payloads,
  and another successful exchange after both WebSocket workers stopped.
- Room creation/join through deployed WSS, native GNS, SPAKE2,
  image/baseline/catch-up sync, Ready, replicated Grab/Release and teardown.

The command used the existing GNS `out/lib` directories in its MSVC `LIB` search
path without changing Cargo or vcpkg cache configuration. The restricted execution
environment initially blocked outbound connections; the same tests passed when
rerun with external network access. Both client processes ran on this Windows
host with private ICE candidates and no external STUN. This verifies deployed
TLS/WebSocket signaling and the native/game runtime flow, but not connectivity
between different NATs, cross-OS behavior or visible-window UI interaction.

## Cloudflare TURN fallback and connection credentials

Rendezvous v1 Welcome can include `turn: { expires_at_unix, servers }`, with 1–4
UDP addresses and short-lived username/password pairs. The adapter queues this
before its Welcome event. GNS installs it before outgoing connects or incoming
signal processing. Later `turn_credentials` events update only outgoing defaults
and listener inheritance for future connections. Existing Connecting/Ready ICE
sessions retain their original server, username and password; ConnectionId,
SecureTransport, SPAKE2, Sync/Ready and gameplay state are preserved.
`turn_unavailable`, expired/older updates and WSS loss do not replace existing
connection credentials. On `turn_unavailable`, the adapter checks the latest
default expiry before queuing Disable. The backend clears TURN server/user/password
strings and the Relay bit in listener/future outgoing defaults only. It retains
the initial endpoint set and expiry watermark: a fresh credential with that set
re-enables future TURN. Early/stale unavailable events cannot discard a newer,
still-valid default. Set/Disable share one latest-update mailbox slot, so expiry
supersedes a queued Set and recovery supersedes a queued Disable. Disable carries
the initial endpoint set so it remains fixed even if the initial Set expires
before the backend first polls. A failed native defaults update blocks new handles
until it succeeds, while existing handles continue to poll.
There is no rendezvous reconnect/resume implementation.

STUN configuration remains in `IceConfig`. Host/private and reflexive ICE
candidates retain higher priority than TURN relay. Only UDP TURN is supported;
TCP/TLS requires a separate native transport change. Provider secrets belong only
in puzzella-rendezvous, never the client or `internet-defaults.env`.

Rendezvous obtains credentials before Welcome. One three-second deadline covers
waiting for a shared issuance permit and the HTTP call. Welcome fixes TURN
availability for that WSS session before peer establishment. A missing/expired
initial value makes the entire control session direct-only; no later issuance
task or late TURN installation starts, including for future peers on that session.
A new control session may obtain TURN after provider recovery.
Sessions with initial credentials obtain new defaults at half TTL (default TTL
24 hours, update about every 12 hours). Failures retain still-valid defaults and
retry with bounded backoff. After expiry, `turn_unavailable` disables future TURN
until provider recovery yields a fresh credential. WSS loss ends this refresh task.
TURN does not grant room membership, player identity or game password authentication.

For example, peer1/peer2 start with A. After the host receives B, peer1/peer2 still
use A, while newly accepted peer3 and outgoing peer4 use B. This does not extend
peer1/peer2's credential lifetime. Configure the credential TTL for the expected
connection duration; the server permits up to 172800 seconds (48 hours). Validity
starts at credential issuance, so cached defaults have only their remaining TTL. A route
requiring TURN can fail when its original credential expires, even if the WSS
session continues to receive newer defaults. ICE restart / allocation migration
is not implemented. Cloudflare documents its
[credential lifetime and ICE restart guidance](https://developers.cloudflare.com/realtime/turn/faq/).

[RFC 8656 sections 5/6](https://www.rfc-editor.org/rfc/rfc8656.html#section-5) bind an
allocation to its original authentication information. An authenticated
non-Allocate request with a different username must receive 441 Wrong Credentials.
The native patch snapshots and locks the connection's TURN configuration at ICE
initialization, including values inherited from a listener. No active allocation
credential-update API exists: `Puzzella_UpdateTURN` and its Rust calls were removed.
Allocation Refresh, CreatePermission and automatic reallocation use that
connection's original credentials. 438 changes the nonce, with bounded challenge
retries; it does not change the username/password. Refresh errors/timeouts retain
the native reallocation path, and permissions are renewed before their lifetime.
Native and WebSocket credential Debug/trace output is suppressed/redacted. See
[native patch provenance](../vendor/game-networking-sockets-sys/PUZZELLA_PATCH.md).

Updates still require exactly the initial endpoint address set (order may change).
The server treats a changed set as a provider error, retaining the old defaults
while retrying. The adapter/backend reject it before mutating listener defaults.

Local tests use `game/tests/fixtures/turn_server.py` (Python standard library only)
and a private IPv4 interface. They never contact Cloudflare. The fixture verifies
TURN long-term authentication, retains the allocation's original username/key,
and returns 441 for a different valid credential on Refresh/CreatePermission.
A UDP protocol test checks Allocate(A), Refresh(A), Refresh/Permission(B) -> 441,
and continued Refresh/Permission(A). Native tests retain A through default
updates to B, an in-flight Refresh and 438 without destroying the allocation;
mixed peers verify A on existing connections and B on future incoming connections.
An adapter/native integration test drives Welcome(A), defaults(B), B expiry,
TurnUnavailable and recovery(C). Peer1 continues with A, peer2 connects directly
with empty TURN strings/Relay disabled and zero Allocate requests (including
unauthenticated attempts), and peer3 uses C through the relay. After C, peer1 and
peer2 keep their original configuration and all three exchange data. A server
WebSocket test separately verifies provider outage, expiry notification and
recovery without closing the room. These tests use accelerated metadata expiry;
the fixture's static keys remain usable for the existing A allocation.

```sh
cargo test --locked -p jigsall-game --features rendezvous gns_localhost_turn -- --nocapture --test-threads=1
cargo test --locked -p jigsall-game --features rendezvous -- --skip gns_localhost
```

The cross-repository runtime smoke harness starts only local fixtures. Build
`cargo build --locked --example turn_fixture_server` in puzzella-rendezvous and
`cargo test --locked -p jigsall-game --features rendezvous --lib --no-run` here.
Cargo prints the game test executable; pass that path and the server example to:

```sh
python game/tests/fixtures/run_turn_smoke.py /path/to/turn_fixture_server /path/to/jigsall_game-test-executable
```

The harness pushes B while existing allocations continue to Refresh with A,
reaches SPAKE2 -> image / baseline / catch-up -> Ready, and verifies gameplay on
the same connection. It also verifies encrypted lanes after control-worker
shutdown and both flows when the provider is unavailable. A recovering mock API
becomes healthy after two seconds; Welcome without TURN remains direct-only and
continues through Ready/gameplay after recovery, without allocations.
Forced-relay cases restrict fixture forwarding to allocation pairs and require
native relay flags plus actual relay traffic. The runtime fixture waits for image
decode/install before checking dimensions/hash and announcing Ready. Known fixture
credentials must be absent from captured output. Loopback WS is test-only; this
harness does not test public WSS/TLS or production Cloudflare. Fixture keys are
static; the mock provider's metadata TTL is accelerated to exercise default
updates. Credential expiry, 24/48-hour wall-clock behavior and ICE restart recovery
are not validated by these fixtures.

Verification on 2026-10-05, Windows x86_64: all 26 non-ignored GNS localhost tests
and six cross-repository smoke runs passed. This includes 441 rejection of changed
allocation credentials, unchanged A authentication after a B default update,
438 recovery with A, future incoming connections using B, and expiry/Disable/recovery
with A on peer1, no Allocate for direct peer2, and C on future relay peer3. Server
tests passed 42 unit and 14 WebSocket cases; server fmt, all-target Clippy and builds
passed.
The rendezvous workspace suite passed 936 tests/doctests; game fmt, all-target
Clippy and the app build passed. Both Rust protocol copies and JSONL fixtures
remain byte-identical.

# Network transport

Networking is opt-in under `game::network`. It does not install systems into the
single-player schedule or implement the Host/Join menu, authentication negotiation,
snapshot/image transfer, interpolation, prediction, or migration orchestration.
The existing authority, replication, cursor, topology and schema 3 semantics are
unchanged. `core` has no transport/native dependency.

```text
protocol / replication (existing gameplay semantics)
        ↓
wire (versioned Postcard binary messages)
        ↓
Transport (opaque Puzzella identities, byte messages, lifecycle events)
        ├ Direct-IP open-source GameNetworkingSockets (gns feature)
        └ future Steamworks ISteamNetworkingSockets
```

## Identity and establishment

`Transport` exposes `poll`, `send` and `close`. `ConnectionId` and `ListenerId` are
private-field u64 tokens, with constructors for backend implementors. They have no
native-handle/address accessor. `GnsDirectIp` issues monotonically increasing,
process-wide tokens, never converting a GNS handle to a token. Its private maps
own the native handles. Stale IDs cannot refer to replacement connections.

`DirectIpTransport` separately exposes `listen(SocketAddr)`, `connect(SocketAddr)`,
`listener_address(ListenerId)` and `close_listener`. A listen port of zero requests
an OS-assigned port. The bundled native API rejects zero, so the backend briefly
reserves an ephemeral UDP port, releases it and passes that port to GNS, with at
most 16 retries for a competing bind. `GnsDirectIp::new()` initializes the wrapper's per-process GNS
singleton. A listener explicitly configures and accepts incoming `Connecting`
callbacks. Successful establishment emits `Connected`; a problem before that
emits `ConnectionFailed`, and an established remote close/problem emits
`Disconnected`. Local close also queues a lifecycle event for the next poll.
Drop closes every owned connection and listener, including pending connections.

`SessionConnections` observes lifecycle events and starts each connection with
`player: None`. A trusted session action calls `assign_player`. It enforces one
connection per player and prevents reassignment of a live connection. The local
test explicitly assigns A/B and the host; **Direct-IP is not user authentication**.
Never derive `PlayerId` from an IP, connection token, native handle or SteamID.
Authentication is an external session responsibility; envelope identity claims
are still validated separately by the existing adapters.

## Classes and lanes

| Class | Messages | GNS lane | Send flags | Priority / weight |
| --- | --- | --- | --- | --- |
| Transient | Client DragUpdate command, host RemoteDragUpdate | 0 | UNRELIABLE + NO_NAGLE + NO_DELAY | 0 / 1 |
| Control | Grab/Release commands, GrabAccepted/ReleaseCommitted events | 1 | RELIABLE | 0 / 4 |
| Bulk | Bounded opaque dummy/future chunk bytes | 2 | RELIABLE | 1 / 1 |

The **pinned native header** specifies lower numeric priority as higher priority.
Transient and Control share priority with a 1:4 weight; Bulk has lower priority.
Each reliable lane has its own ordered stream. Bulk does not occupy Control's
ordered queue, so missing Bulk fragments cannot head-of-line block Control.
Bandwidth congestion still affects latency; this does not promise zero delay.
`NO_DELAY` permits a transient send to be discarded by GNS when it cannot send
promptly. A send failure is reported; it never falls back to reliable Control.
Final correctness comes from ReleaseCommitted's final delta and fingerprint.

GNS handles UDP reliability, fragmentation, reassembly and native service threads.
The application does not implement UDP reliability or an extra background thread.
`poll(&mut events)` runs callbacks, normalizes lifecycle transitions, then receives
up to 32 messages and 128 callbacks per socket per call. It visits sockets and
connections, never pieces. There are at most 64 connections and 8 listeners per
backend. These bounds also bound a single frame's receive work. Poll appends events
and returns any native receive error so the caller can handle it.

## Wire v1

One native message is one Puzzella frame, with no stream reassembly:

| Bytes | Field |
| --- | --- |
| 0..4 | ASCII `PZLA` |
| 4..6 | u16 wire version, little-endian, currently 1 |
| 6 | Kind: 1 ClientControl, 2 AuthorityEvent, 3 RemoteDragUpdate, 4 ClientDrag, 5 BulkChunk |
| 7 | Reserved zero byte |
| 8..12 | u32 payload length, little-endian |
| 12.. | Postcard 1.x binary serialization of the indicated protocol type; Bulk is raw bytes |

`WireMessage::ClientCommand` maps to kind 1 or 4 based on its command. This allows
the Transient limit to be checked before deserializing. After decoding, its actual
class must match the header and receiving lane. Both header and Postcard trailing
bytes are rejected. Unsupported versions, unknown kinds, reserved bits, truncated
frames, malformed enums/varints/masks and excess lengths return `WireError`.
No gameplay wire uses JSON.

The v1 Postcard field order and enum representation are part of the wire contract.
A breaking type/codec change requires a new `WIRE_VERSION`; adding handshake,
snapshot or image chunk kinds can be done at this boundary. A future backend uses
these exact bytes and requires no protocol or replication change.

| Bound | Payload bytes (12-byte header is additional) |
| --- | --- |
| Control | 262,144 |
| Transient | 128 |
| Bulk chunk | 32,768 |
| Maximum whole frame | 262,156 (includes header) |

Native receive checks the lane's frame limit **before copying** the GNS buffer.
Wire decode checks the complete frame, kind-specific declared limit and exact
length before deserializing. Existing core bounded visitors clamp sparse lists
and rejected refs to 32 and masks to 31,250 u32 words / 1,000,000 pieces; malformed
dimensions and nonzero padding bits are rejected. There is no allocation based
on an unchecked network length. A full 1M-bit mask fits even with Postcard's
worst-case word varints. Drag packets contain only scalar identity/context/tick/
delta fields and are bounded by 140 bytes including the header.

BulkChunk is only a size-bounded opaque carrier for testing/reserving a class.
It has no transfer identifiers, offsets, completion, compression or image logic.
A future 16 MB snapshot must have a bounded application chunk protocol; the
current frame limits deliberately do not permit a giant single message.

## Routing and frame integration

The topology remains a host-authoritative star. No peer-to-peer mesh is created.

```text
peer ClientRouter::send_command
 → wire encode / class → Transport::send(host_connection)
 → host poll → SessionConnections identity → wire decode
 → HostRouter::route → ProtocolDragContexts::apply_replicated
 → HostCommandOutcome → HostRouter::publish
 → authority event to all assigned remote peers (including sender)
   OR RemoteDragUpdate to other assigned peers (excluding sender)
 → peer poll → designated host connection + assigned host identity
 → ClientRouter::route → PeerReplicationState::apply_event / apply_drag_update
```

The authority has already applied its local mutation. Publication never applies an
event to the host; a host-player connection is excluded. Unassigned connections
cannot submit commands or receive gameplay broadcasts. Clients reject messages
from any connection other than their designated, assigned host. Bulk bytes are
returned to the caller without touching gameplay; commands/events in the wrong
direction are rejected. The backend never interprets protocol objects or accesses
`PieceDataStore`.

During each Bevy frame, keep the backend in caller-owned state, call `poll`, process
each lifecycle event with `SessionConnections::observe`, assign identities through
your trusted session workflow, and route each message with the appropriate router.
Routers borrow the existing session/store/context; they do not duplicate gameplay.
No systems are automatically scheduled and no piece scan occurs while idle.

Host apply and publication are separate: `route` returns the already applied
outcome even if the next send might fail. `publish` attempts every eligible peer
and returns per-connection failures. The caller must retain and retry failed
Control bytes in original cursor order, or disconnect/resynchronize that peer;
**do not re-apply a command to retry publication**. A successful send means native
queue acceptance, not an application ACK. Late join/baseline synchronization,
backpressure recovery and disconnect ownership cancellation remain explicit
session responsibilities. Before observing a disconnect, retain its assigned
player. Disconnect removes the mapping; if that player was dragging, coordinate
`cancel_player` on authority and replicas using the existing
APIs before reusing that player. No new cancellation/migration protocol is invented.

## Native build and validation

The optional dependency is `game-networking-sockets = 0.3.0` (Rust library name
`gns`), locked with `game-networking-sockets-sys = 0.3.0`. The API/source were checked
in the downloaded crate, including `configure_connection_lanes`, `set_lane`,
`SendFlags`, `get_listen_socket_address`, accept and callback ownership. Sources:
[wrapper](https://github.com/hussein-aitlahcen/gns-rs),
[crate](https://crates.io/crates/game-networking-sockets/0.3.0), and
[Valve build instructions](https://github.com/ValveSoftware/GameNetworkingSockets/blob/master/BUILDING.md).

Default builds require no GNS native dependencies. Root `gns` forwards to
`puzzella-game/gns`. `--all-features` enables GNS and therefore needs native tools.
The Rust wrapper builds the bundled open-source GNS sources; no Steamworks SDK,
`steamworks` crate or Steam client is needed.

Windows MSVC prerequisites:

- Rust 1.95+, Visual Studio 2022 C++ tools and Windows SDK (existing prerequisites).
- CMake, Git, LLVM/libclang for bindgen (`LIBCLANG_PATH` if not auto-discovered).
- Internet access during the initial native build: the sys build script clones/
  bootstraps vcpkg and installs its manifest's protobuf, OpenSSL and their Abseil/
  UTF-8 dependencies. Runtime crypto uses Windows BCrypt in this build.
- A short `GNS_VCPKG_BUILDTREES_ROOT` path (the wrapper rejects paths >100 chars).
  Use a writable path for that build; do not bypass the length check.

Example in a short checkout path:

```powershell
$env:LIBCLANG_PATH = 'C:\Program Files\LLVM\bin'
$env:GNS_VCPKG_BUILDTREES_ROOT = Join-Path (Get-Location) 'target/vcpkg-trees'
cargo check --locked --features gns
cargo test --locked --features gns
cargo build --locked --features gns
```

Linux requires a C/C++ compiler, CMake, Git, libclang, pkg-config, protobuf headers/
libraries and `protoc`, OpenSSL headers/libraries, and Abseil/UTF-8 libraries matching
the installed protobuf. The sys build links static dependencies on Linux, so
install the corresponding static archives and pkg-config metadata. For Debian/
Ubuntu, start with `build-essential cmake git libclang-dev pkg-config libprotobuf-dev
protobuf-compiler libssl-dev libabsl-dev`; newer protobuf may require utf8-range
packages as well. No Windows tools are required on Linux.

```sh
# Header/limits, roundtrips, identity and fake host/client routing
cargo test --locked -p puzzella-game network::tests
# Actual localhost UDP sockets: ephemeral listener + two clients
cargo test --locked -p puzzella-game --features gns gns_localhost -- --nocapture
# Full requested validation; ignored GPU benchmarks stay ignored
cargo fmt --check
cargo check --locked
cargo clippy --workspace --locked --all-targets --all-features -- -D warnings
cargo test --locked
cargo test --locked --all-features
cargo build --locked
cargo check --locked --features gns
cargo build --locked --features gns
```

The GNS test uses only 127.0.0.1, an ephemeral port, 15-second deadlines with polling
backoff, explicit closes and RAII cleanup on panic. It validates actual listener
acceptance, A's reliable Grab, both replicas' ACK, a Transient drag delivered only
to B, a small independent Bulk message, reliable Release with snap, equal final
piece/connectivity/snapshot/cursor state, and disconnect on both ends. No external
network is contacted by tests; initial dependency downloads are build setup only.

## Future Steamworks backend

Implement `game/src/network/steamworks` behind its own feature later. It should own
`ISteamNetworkingSockets`, translate `CreateListenSocketP2P` / `ConnectP2P` and
`SteamNetworkingIdentity` to private native handles and freshly issued Puzzella
tokens, implement the shared `Transport`, and use the same class/lane/limit policy.
Expose separate `listen_p2p` / `connect_peer` establishment APIs; do not extend
DirectIpTransport or introduce an address enum into protocol messages.

Steam session authentication should validate the Steam user and session membership
before assigning the independent `PlayerId` through `SessionConnections`.
The existing wire codec, host/client routers and authority/replication adapters
remain the integration points. Steam P2P/SDR selection belongs to that future
backend. `SteamLobbyBackend`, if added, separately chooses session members and
the lobby owner/host identity; it does not send gameplay messages. No lobby or
Steamworks placeholder dependency/module is added in this change.

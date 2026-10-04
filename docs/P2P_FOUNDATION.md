# GNS P2P foundation

The pinned sources are `game-networking-sockets = 0.3.0` and its re-exported
`game-networking-sockets-sys = 0.3.0`. The actual bundled headers checked are
`steamnetworkingcustomsignaling.h`, `isteamnetworkingsockets.h`,
`steamnetworkingsockets_flat.h`; implementation checks cover the flat callback
adapters, singleton initialization and native ICE selection. The
[upstream interface](https://github.com/ValveSoftware/GameNetworkingSockets/blob/master/include/steam/steamnetworkingcustomsignaling.h)
describes the signaling ownership model; bundled sources, rather than inferred
API names, determine this implementation.

## Ownership and unsafe boundaries

Application raw P2P code is in `game/src/network/gns/p2p/native.rs`, which denies
implicit unsafe operations inside unsafe functions. The vendored wrapper also
contains the small identity initialization extension described below. Safe types keep native
connection handles and pointers private. The boundaries are:

| Boundary | Ownership and lifetime guarantee |
| --- | --- |
| Process initialization / identity | The patched pinned wrapper's get/get_with_identity share one initialization lock and OnceLock. Exactly one native Init receives the random GenericBytes identity. Later identity requests validate the actual singleton and reject conflicts without invoking Init. Stack identity/output buffers remain valid through synchronous calls; GNS copies identity. No live reset/Kill occurs. The default and configured accessors are concurrency-tested with an Init counter. |
| SendSignal / Release | Each flat adapter owns a separate Box containing immutable PeerId and a thread-safe mailbox Arc. GNS assumes ownership at Connect, including failure, or when the receive callback returns the adapter. Release destroys that Box exactly once; it can happen after backend Drop. SendSignal borrows only during the callback, validates length, copies opaque bytes, and never invokes GNS or user code while holding the mailbox lock. Poisoned locks fail closed instead of panicking across FFI. |
| Receive context / accept | ReceivedP2PCustomSignal2 retains neither payload nor context. A stack context exclusively borrows the backend's admission gate and start bucket during the synchronous call. The callback validates envelope identity/virtual port before spending origin/global credit for a new request, then configures/accepts only within admitted capacity. Its return transfers a separate signaling adapter to GNS. A rejected request returns null; partial setup owns a RAII connection guard. No user callback enters this boundary. |
| Native listener / connection | Unique private owners close each handle once. Backend Drop clears connections before closing the listener; failed configuration drops its connection. Native state queries write into initialized, correctly sized buffers. Lane/config arrays and CString storage outlive calls, and GNS copies settings. |
| Native message send | AllocateMessage reserves exactly the checked payload length. A unique buffer receives the copied bytes and lane/flags. SendMessages with deleteFailedMessages=true consumes/releases the reference on either outcome; Rust never releases it again. |
| Native message receive | ReceiveMessagesOnConnection requests exactly one output reference. A Message guard owns it; lane/size/pointer are checked before borrowing payload, which cannot outlive the guard. Record/header/rate checks precede the application Vec copy. Every exit, including invalid data, releases that native reference. |

Pending failures and local/remote closes remove the application mapping and emit
one lifecycle event. Mailbox Drop cleanup clears queues and rejects new writes;
late native callbacks keep only their bounded mailbox allocation alive until
Release. GNS may retain a closed connection briefly to send termination signals;
the backend does not synchronously Kill the shared singleton to force Release.

## Local tests

Run on a configured native GNS host:

```powershell
cargo test --locked -p puzzella-game --features gns p2p -- --nocapture
# Select the excluded patched wrapper in the root's optional GNS graph.
cargo test --locked -p puzzella-game -p game-networking-sockets --features puzzella-game/gns --lib identity_initialization -- --nocapture
```

The integration test re-enters the same Rust test executable in two child
processes, each with a different GNS singleton/PeerId. The parent implements fake
rendezvous using InMemorySignaling and bounded mailboxes, relaying opaque blobs
over test-only stdin/stdout pipes. Pipe readers are fixture threads; no backend
worker/async runtime is added. Private native ICE candidates establish a real UDP
connection between processes on this host; it is not CreateSocketPair, a mock,
or GNS's same-process self-connection path. Native connection descriptions must
report `P2P ICE`, with an actual UDP remote port and neither loopback-buffer nor
relay flags on both endpoints.

Both peers use new_routed with explicit trusted fixture account/session bindings,
emit Connected exactly once, and run the existing password bootstrap to
Authenticated, retain unassigned gameplay mappings, and exchange Control,
Transient and Bulk through SecureTransport. The fixture checks exact plaintext
at the receiver and ciphertext/AEAD overhead immediately below SecureTransport,
then reliable egress, explicit close, remote disconnect, destroyed channels and
one-shot lifecycle events, with every valid signaling packet delivered twice.
It does not grant Ready or claim runtime integration.
Other tests cover pending timeout, cleanup/recreation and token non-reuse,
malformed repeated signals, flood isolation/fair service, account-level budgets
shared by many peer IDs, global-budget deferral, bounded rate history, unknown
route rejection, revocation, per-origin pending limits/cooldown, queue
count/byte/size limits and mailbox shutdown.
Tests have deadlines and RAII child cleanup, use no external STUN/TURN, and need
host networking interfaces capable of private ICE host candidates.

On this Windows MSVC host the existing multi-config build can generate GNS under
RelWithDebInfo while the sys build script searches Debug. When LNK1181 occurs,
add that build output's `out/lib` directory to the command's LIB environment as
described in [WINDOWS_BUILD.md](WINDOWS_BUILD.md). This does not change the shared
Cargo cache or vcpkg paths. The P2P change does not patch the native build script.

## Production connection point and limits

The optional [rendezvous v1 client](RENDEZVOUS_V1.md) uses `GnsP2p::new_routed`, retains SignalingEndpoint,
and calls authorize_peer only after checking the rendezvous authority's session
and account binding. RouteOrigin's constructor does not perform cryptographic
verification: this is an explicit trust boundary for the local adapter. Many
peer IDs belonging to one account/session share one key; these values must never
be copied from untrusted envelope claims. Unknown peers fail closed in routed
mode. revoke_peer purges queues and closes connections on the next frame;
connection snapshots retain their original abuse key. The adapter forwards
pop_outbound payloads verbatim and delivers bound peers' blobs through receive.
It validates each message's sender using the authenticated server route, rather
than trusting an attacker-controlled sender field. At most 8 peer IDs can bind
to one account/session route, preserving table capacity for other accounts.
It handles mailbox backpressure/reconnection and route admission outside GNS;
it must not promote players or bypass password bootstrap. IceConfig is the place
to extend establishment-time server options. Runtime host/join generalization,
room-code runtime/UI integration, TURN credentials/relay and Internet
NAT traversal validation remain separate work. The v1 WSS server authenticates
anonymous room membership only; its MemberId occupies RouteOrigin.account and
provides no Steam account authentication or complete Sybil resistance. Active
route bindings are preserved after control loss until the connection owner
explicitly releases them. The localhost real-server test is documented in
[RENDEZVOUS_V1.md](RENDEZVOUS_V1.md).

The root crates.io patch replaces only game-networking-sockets 0.3.0 with
`vendor/game-networking-sockets`; sys 0.3.0 and its native library remain pinned.
See that directory's PUZZELLA_PATCH.md for the preserved upstream source and
the initializer extension. Default dependency graphs still exclude native GNS.

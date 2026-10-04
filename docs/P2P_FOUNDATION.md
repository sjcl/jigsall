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

All new unsafe code is in `game/src/network/gns/p2p/native.rs`, which denies
implicit unsafe operations inside unsafe functions. Its safe types keep native
connection handles and pointers private. The boundaries are:

| Boundary | Ownership and lifetime guarantee |
| --- | --- |
| Process initialization / identity | A OnceLock runs before either Puzzella backend creates sockets. Raw Init supplies a random GenericBytes identity. The pinned native Init explicitly succeeds when already initialized; the high-level GnsGlobal adopts that singleton. GetIdentity verifies it was not initialized with a different identity by another user. No live reset/Kill occurs. Stack identity and output buffers remain valid through synchronous calls. |
| SendSignal / Release | Each flat adapter owns a separate Box containing immutable PeerId and a thread-safe mailbox Arc. GNS assumes ownership at Connect, including failure, or when the receive callback returns the adapter. Release destroys that Box exactly once; it can happen after backend Drop. SendSignal borrows only during the callback, validates length, copies opaque bytes, and never invokes GNS or user code while holding the mailbox lock. Poisoned locks fail closed instead of panicking across FFI. |
| Receive context / accept | ReceivedP2PCustomSignal2 retains neither payload nor context. A stack context is exclusively borrowed during the synchronous call. The callback validates envelope identity/virtual port and configures/accepts a new native connection only within admitted capacity. Its return transfers a separate signaling adapter to GNS. A rejected request returns null; partial setup owns a RAII connection guard. |
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

Both peers emit Connected exactly once, run the existing password bootstrap to
Authenticated, retain unassigned gameplay mappings, and exchange Control,
Transient and Bulk through SecureTransport. The fixture checks exact plaintext
at the receiver and ciphertext/AEAD overhead immediately below SecureTransport,
then reliable egress, explicit close, remote disconnect, destroyed channels and
one-shot lifecycle events, with every valid signaling packet delivered twice.
It does not grant Ready or claim runtime integration.
Other tests cover pending timeout, cleanup/recreation and token non-reuse,
malformed repeated signals, queue count/byte/size limits and mailbox shutdown.
Tests have deadlines and RAII child cleanup, use no external STUN/TURN, and need
host networking interfaces capable of private ICE host candidates.

On this Windows MSVC host the existing multi-config build can generate GNS under
RelWithDebInfo while the sys build script searches Debug. When LNK1181 occurs,
add that build output's `out/lib` directory to the command's LIB environment as
described in [WINDOWS_BUILD.md](WINDOWS_BUILD.md). This does not change the shared
Cargo cache or vcpkg paths. The P2P change does not patch the native build script.

## Production connection point and limits

A future signaling client retains `SignalingEndpoint`, forwards `pop_outbound`
payloads verbatim, and supplies authenticated routing envelopes with `receive`.
It handles mailbox backpressure/reconnection and route admission outside GNS;
it must not promote players or bypass password bootstrap. IceConfig is the place
to extend establishment-time server options. Runtime host/join generalization,
route discovery, rendezvous authentication, TURN credentials/relay and Internet
NAT traversal validation are not implemented or certified by localhost tests.

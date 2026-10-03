# Network transport

The multiplayer CPU layer provides `JoinBaseline` schema 1 for same-epoch join:
GameSnapshot plus up to 64 active drag overlays captured in stable PlayerId order.
Its transactional install can continue existing authority events without a new
Grab. The opt-in Syncing runtime connects image preparation, generation-bound
Bulk baseline transfer and Reliable catch-up. Final active-drag reconciliation
and Ready handoff remain pending. Transport version and golden frames now use
wire v7 with a dedicated SyncControl kind; generic Bulk framing, SecureTransport, GNS
and rate limiting retain their existing designs. Migration uses snapshot only
plus a new epoch; save/load use checkpoint only; both discard
drags. See [JOIN_IN_PROGRESS.md](JOIN_IN_PROGRESS.md) for the CPU contract.

Networking is opt-in under `game::network`. It does not install systems into the
single-player schedule or implement the Host/Join menu,
interpolation, prediction, or migration orchestration.
Commands use the core authority, replication, cursor and topology semantics with
wire v7 and snapshot schema 4. `core` has no transport/native dependency.

```text
bootstrap (mandatory session password, authenticated/syncing/ready gate)
        ├ authentication SessionControl (Consumed)
        ├ authenticated SyncControl / Bulk (Syncing coordinator/router)
        └ Ready protocol / replication (HostRouter / ClientRouter)
        ↓
wire (versioned Postcard binary messages)
        ↓
SecureTransport<T> (PAKE-derived application AEAD, sequences, replay protection)
        ↓
Transport (opaque Puzzella identities, byte messages, lifecycle events)
        ├ Direct-IP open-source GameNetworkingSockets (gns feature)
        └ future Steamworks ISteamNetworkingSockets
```

## Identity and establishment

`PlayerId` is a session player identity. `game::resources::LocalPlayerId` is the
Bevy resource identifying the player currently controlled by this process.
`LOCAL_PLAYER` / `PlayerId(0)` is only the offline default: with local ID 42,
player 0 is remote. Input, selection rollback, drag presentation, and authority
Grab/Release replay receive this identity explicitly. `HostRouter` and
`ClientRouter` carry a `local_player` field supplied by the runtime; the low-level
adapters do not infer it from the authority host. `RemoteDragUpdate` bookkeeping
continues to track all accepted drag contexts as before.

Entering Menu resets `LocalPlayerId` to its offline default. Snapshot installation
may replace `PieceDataStore` but leaves the independent identity resource intact.
It is excluded from checkpoints, snapshots, save files, puzzle definitions, and
wire messages. The password bootstrap now exposes the host-assigned identity via
`ClientBootstrap::assigned_player()` after secure channel establishment. A future runtime
join will set `LocalPlayerId` from that value and pass it to the routers;
runtime/input/UI integration is still outside this change. This identity is
independent of SteamID.

`Transport` exposes `poll`, `send` and `close`. `ConnectionId` and `ListenerId` are
private-field u64 tokens. Their constructors are crate-private: backends inside
`game` issue them, and external callers obtain them through transport APIs/events.
They have no native-handle/address accessor. `GnsDirectIp` issues monotonically
increasing, process-wide tokens, never converting a GNS handle to a token. Its private maps
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
Incoming connections are closed explicitly before removing their bookkeeping;
outgoing connections close through the owning client socket wrapper's Drop.
Neither path issues a second explicit native close. The wrapper uses its generic
native close code for outgoing sockets; local lifecycle events retain the requested
`DisconnectReason`.

`SessionConnections` tracks gameplay-ready peers and starts connections with
`player: None`. `network::bootstrap` maintains authentication state separately;
only explicit Ready promotion calls `assign_player`. One connection per player
and no reassignment of a live connection remain enforced. Never derive `PlayerId`
from an IP, connection token, native handle or SteamID. Envelope identity claims
are still validated separately by the existing adapters.

Future disconnect runtime wiring must preserve the assigned PlayerId **before**
`SessionConnections::observe(Disconnected)` removes the mapping. Capture the player,
call authority `cancel_replicated(player)` once, publish/retain a returned
DragCancelled envelope, then remove the connection mapping (or remove it earlier
only after retaining the player and envelope). `HostRouter::publish_authority_event`
sends to all currently assigned Ready remote peers as Reliable Control, without
source exclusion or host loopback. It returns per-connection send failures while
attempting the remaining peers; retry the original event, never reapply cancellation
to create a replacement. No actual Bevy/transport disconnect coordinator, PlayerLeft
is automatically scheduled. The opt-in Syncing coordinator retains catch-up events.
Migration continues to discard drags in a new
epoch without requiring cancellation events.

## Mandatory session password authentication

Direct-IP sessions require a memory-only UTF-8 password of 8..=128 encoded bytes.
`SessionPassword` owns a `Zeroizing<String>`, redacts Debug, and provides no
serialization, Display or persistent-settings path. Neither plaintext passwords
nor password hashes/scalars are sent on the wire or saved. Password provisioning
is out of band; Host/Join/password-entry UI is still outside this change.

GNS transport security, SPAKE2 password authentication, and PAKE-derived application
AEAD are separate layers. `HostBootstrap` and `ClientBootstrap` require
`SecureTransport<T>`, without IP addresses,
native handles or SteamIDs, so a future backend can use the same password layer.
This authenticates knowledge of a shared session password, not a named account.

```text
GNS Connected / TransportConnected
  host -> client: ServerHello(metadata, reserved PlayerId, nonce, SPAKE2 share) plaintext
  client -> host: ClientProof(SPAKE2 share, client confirmation) plaintext
  host verifies client confirmation
  host -> client: AuthAccepted(assigned PlayerId, server confirmation) plaintext
  host installs channel only after AuthAccepted is queued; enters Securing
  client verifies server confirmation
  client installs channel and sends encrypted SecureChannelReady (Control sequence 0)
  client becomes Authenticated only after successful send; discards password
  host decrypts and consumes SecureChannelReady, then becomes Authenticated
Authenticated -> explicit begin_sync() -> Syncing
  session/image identity -> image availability negotiation
  optional PuzzleImage offer/accept + Bulk -> hash verification -> ImageReady
  begin_join() -> generation-bound JoinBaseline offer/accept + Bulk
  baseline install ACK -> Reliable catch-up + ACK
  Finalizing -> future full active-drag reconciliation -> final barrier/ACK
  future SyncReadyPermit -> promote_ready() -> Ready -> assign_player()
```

PAKE success != gameplay Ready. Authentication alone leaves both endpoint gameplay
mappings unassigned; on the client the eventual mapping identifies the host, while
the independently assigned local PlayerId stays in bootstrap state. Metadata can
be authenticated without owning a snapshot: SessionDefinition (SessionId/ImageHash),
AuthorityCursor and host PlayerId. SecureChannelReady confirms channel possession
only. The Syncing protocol verifies image identity and the offered baseline/catch-up
cursor. It deliberately stops at Finalizing; Reliable catch-up alone cannot mint
the private-construction Ready permit. The future barrier must also reconcile the
authority's current full active-drag scalar set and verify the final ACK.

The isolated `network::auth` adapter uses pinned `pakery-spake2` / `pakery-crypto`
0.6.0, the RFC 9382 P256-SHA256-HKDF-HMAC suite (not the crate's Ristretto suite).
The actual crate source was checked for public-point validation, A/B role separation,
zeroizing state, additional data and constant-time mutual confirmation. Password
scalar derivation follows pakery's documented SHA-512 then 512-bit wide reduction
into P-256's scalar field; RFC 9382 leaves password preprocessing to applications.
It is not a memory-hard password KDF. Online guessing is bounded below; password
entropy and the absence of an independent implementation audit remain assumptions.
[The upstream library states that it has not been independently audited](https://github.com/djx-y-z/pakery#security).

The additional-data byte sequence is `puzzella-session-auth-v1`, u16 LE wire version,
u128 LE SessionId, 32 ImageHash bytes, u64 LE host PlayerId, u64 LE cursor epoch,
u64 LE cursor sequence, 32 OS-CSPRNG nonce bytes and u64 LE reserved PlayerId.
SPAKE2 also binds both ephemeral shares and the fixed host/client role identities.
ConnectionId is not included because tokens differ between endpoints. IDs are
reserved before ServerHello so the assigned identity is covered by confirmation;
they become publicly assigned in bootstrap only after verification. Reservations
are burned on failure/disconnect. Allocation skips the externally supplied host,
existing gameplay assignments and caller-supplied allocated IDs, and never reuses
an issued number during the host bootstrap's lifetime. Recreating a bootstrap for
an ongoing session must seed it with that session's already allocated IDs.

SessionControl uses kind 6 on reliable Control, with fixed-size point coordinates
and confirmation arrays. The 4,096-byte declared AND actual payload limits are
checked from the header before postcard decoding. Malformed/unexpected control,
duplicate handshakes, or any gameplay before Ready closes the connection and
removes gameplay mappings immediately, including later events in the same batch.
`BootstrapOutcome::Consumed` stays in authentication, `Syncing` goes to the dedicated
sync router, and only `Gameplay` may reach a Ready gameplay router. Header-only kind
validation blocks pre-Ready gameplay before deserializing its payload; Ready
gameplay is decoded once by the existing router. The existing
`player(connection) == None` router/broadcast guard remains in place. No regular
gameplay broadcasts go to an authenticated or syncing peer.

Host-global start and failed-attempt token buckets each allow 4 attempts/s with a
burst of 8; an empty failure bucket also blocks new starts. Pending authentication
is capped at 32 with a 10-second timeout, starting at Connected and never extended
by packets. Call `expire` once per polling frame on both endpoints, even with no
messages. Exceeding a limit immediately rejects the connection. This is separate
from the backend's per-connection inbound byte limiter, uses no sleep/cooldown,
and performs no crypto or piece scans for established peers. Authentication failures
expose only a generic failure/close; client `failure()` can report password
authentication failed. OS RNG failure aborts nonce generation; pakery's infallible
RNG interface uses its documented SysRng/UnwrapErr adapter (fail-stop on RNG error).

The verified PAKE key is consumed by the secure-channel key derivation described
below. GNS encryption remains enabled, but all post-authentication gameplay traffic
must also pass through PAKE-derived application AEAD. Routers retain their existing
wire, authority, replication, and LocalPlayerId behavior.

## Application secure channel

`network::secure::SecureTransport<T>` implements `Transport` and delegates
`DirectIpTransport` establishment when supported by T. It contains no GNS type.
Bootstrap's typed transport boundary requires this wrapper; its inner backend is
private, with no production mutable/consuming accessor and no public downgrade or
raw-key installation API. Uninstalled connections only send and receive plaintext
SessionControl frames on Control, as validated by the wire header/class/size gate.
Gameplay, Bulk, malformed headers and class/size violations close that connection
as ProtocolViolation; rejected sends never reach the backend. Bootstrap still owns
SessionControl body decoding and state validation before routing. Installed connections
interpret every incoming payload exclusively as an encrypted record. Plaintext
gameplay or readiness markers never get a fallback decoder.

Pakery 0.6.0's actual `Spake2Output::into_session_key()` API returns Ke after peer
confirmation verification. In this P-256 suite Ke is **16 bytes** (NH / 2), distinct
from the 32-byte confirmation MAC. `auth.rs` copies it into a library-independent
`AuthenticatedSecret` with a `Zeroizing<Vec<u8>>` key, then drops the original zeroizing
SharedSecret. Neither secret wrapper implements Debug/Display/Serialize/Clone.
Only a successful confirmation yields this value. Client assignment validation
also precedes key extraction. Pakery binds the public session context into its
confirmation-key HKDF, rather than directly into Ke=Hash(TT)[..NH/2]. The adapter
therefore carries that confirmation-verified context into the application HKDF
too, explicitly binding SessionId, ImageHash, host, cursor, handshake nonce and
reserved/assigned PlayerId. Existing context confirmation remains unchanged.

HKDF-SHA256 uses Ke as IKM, `puzzella-secure-channel-v1` as salt, and the following
six separate info labels, each followed by the u16 LE WIRE_VERSION and the
complete confirmation-verified session context byte sequence described above:

```text
puzzella-secure-channel-v1/client-to-host/control
puzzella-secure-channel-v1/client-to-host/transient
puzzella-secure-channel-v1/client-to-host/bulk
puzzella-secure-channel-v1/host-to-client/control
puzzella-secure-channel-v1/host-to-client/transient
puzzella-secure-channel-v1/host-to-client/bulk
```

Each result is a 32-byte ChaCha20Poly1305 key; HKDF does not increase Ke's entropy.
The PAKE secret is destroyed after this one-time derivation. Per-connection keys
use `Zeroizing<[[u8; 32]; 6]>`; temporary cipher key copies use the crate's zeroize
feature. Disconnect, connection failure, explicit close, replacement Connected,
authentication timeout and failed encrypted send destroy the channel. Native close
failure does not preserve keys or permit later plaintext sends. Closing a listener
drains its queued backend lifecycle events immediately, destroys affected
channels and blocks deferred records before the next ordinary poll. Unrelated
connections keep their channels. Dropping the wrapper destroys all keys.
Zeroization reduces owned-buffer lifetime; it is not a guarantee about compiler,
register/stack copies, OS memory, or every dependency's internal KDF temporary.

Standard RustCrypto `chacha20poly1305` 0.11.0 provides the RFC 8439 algorithm and
16-byte tag. `hkdf` 0.13.0 and `sha2` 0.11.0 are direct dependencies (the latter is
aliased for this layer, keeping existing SHA-256 callers on their current version).
Their Rust 1.85 minimum is compatible with the workspace's Rust 1.95. No cipher or
MAC primitive is implemented here. RustCrypto describes an audit of an earlier
ChaCha20Poly1305 implementation; that does not certify these versions, pakery,
this integration, or the complete application. See the
[RustCrypto security notes](https://docs.rs/chacha20poly1305/0.11.0/chacha20poly1305/#security-notes).

The complete existing wire frame (including PZLA header) is the AEAD plaintext.
One native message carries this fixed outer record, without serde or reassembly:

```text
u64 sequence LE | ciphertext of entire inner PZLA frame | 16-byte AEAD tag
```

Nonce is `00 00 00 00 || sequence_le_u64`. AAD is the ASCII
`puzzella-secure-record-v1`, u16 LE WIRE_VERSION, one class byte
(Control=0, Transient=1, Bulk=2), and u64 LE sequence. Opposite directions and classes
have independent keys, and each key starts its send sequence at zero. Counters
never roll back, including after an ambiguous backend send error: that error
closes the connection rather than allowing nonce reuse or a reliable gap. u64::MAX
is reserved as exhausted and never sent/accepted. Reconnect performs a fresh PAKE;
old session records cannot authenticate under the new keys.

Control and Bulk independently accept exactly their next expected sequence;
duplicates, stale records and gaps close only that connection. Transient permits
gaps, accepts only a sequence greater than the highest authenticated sequence,
and silently drops duplicate/reordered old records. Authentication failure does
not advance receive state. Reflection, class substitution, ciphertext/tag changes,
malformed records, pre-install secure packets and plaintext after installation
cause a generic ProtocolViolation close. No cryptographic failure details, keys,
passwords or session secrets are sent to the peer or logged.

Native size checks now allow `wire::frame_limit(class) + 24` **before copying**.
The existing inbound limiter charges the encrypted outer bytes before the copy
and AEAD verification. Inner Control/Transient/Bulk/SessionControl payload bounds
are unchanged. Open verifies outer bounds, decrypts in the owned bounded buffer,
and returns the original frame. Existing `wire::decode_for_class` still applies
the inner kind/class/declared/actual limits before gameplay.

Plaintext handshakes are per-connection poll-batch barriers: return at most one
such event, preserve the raw tail, let bootstrap install keys, then decrypt that
tail on the next poll. The wrapper finishes this bounded native batch before
requesting another, so deferred pre-auth data cannot grow without bound. Stable
secure connections decrypt the whole batch with no handshake/KDF/password work,
piece scans, per-piece cryptography or connection-map rebuild. Consume every
returned event through bootstrap before the next poll. Securing remains subject
to the existing pending cap and original 10-second authentication deadline.

For shared-password authentication, a MITM relaying SPAKE2 unchanged while
terminating GNS connections cannot read or modify the application channel without
the PAKE-derived keys. A relay can still drop/delay traffic. A participant who
knows the password can instead establish its own authenticated session and act
as either role; password possession does not make gameplay trustworthy. Host
authority remains responsible for malicious gameplay, identity claims and state
validation. Compromised endpoints, password disclosure, PAKE/library security and
fresh OS randomness remain security assumptions; this is not an independent audit.

## Classes and lanes

| Class | Messages | GNS lane | Send flags | Priority / weight |
| --- | --- | --- | --- | --- |
| Transient | Client DragUpdate command, host RemoteDragUpdate | 0 | UNRELIABLE + NO_NAGLE + NO_DELAY | 0 / 1 |
| Control | Gameplay commands/events, SessionControl handshake, dedicated SyncControl | 1 | RELIABLE | 0 / 4 |
| Bulk | Syncing PuzzleImage / JoinBaseline transfer framing | 2 | RELIABLE | 1 / 1 |

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
up to 128 callbacks per socket and drains messages in reusable 32-slot chunks.
Sockets take turns until their queues are empty or a shared 512-message budget
is reached. The next poll resumes with the next socket. Unknown/invalid messages
also consume this budget. A final partial chunk receives only the remaining budget,
so unread native messages stay queued for the next poll instead of being released.
It visits sockets and connections, never pieces. There are at most 64 connections
and 8 listeners per backend. These bounds also bound a single frame's receive work.
Poll appends events and returns any native receive error so the caller can handle it.

## Inbound rate limiting

`network::rate_limit::InboundRateLimiter` is a pure Rust helper with three independent
per-connection token buckets. The GNS backend stores it directly in `Connection`,
starting with full buckets when an incoming or outgoing connection is created and
discarding it with that connection. It is active before `SessionConnections` assigns
a player, including connected peers with `player: None`.

The immutable `DEFAULT_INBOUND_POLICY` uses the following byte-equivalent rates:

| Class | Steady rate | Burst capacity | Minimum charge | Over-limit action |
| --- | --- | --- | --- | --- |
| Transient | 128 KiB/s | 256 KiB | 512 B | Silent drop; connection stays open |
| Control | 4 MiB/s | 8 MiB | 16 KiB | Disconnect with `DisconnectReason::RateLimited` |
| Bulk | 8 MiB/s | 16 MiB | 32 KiB | Disconnect with `DisconnectReason::RateLimited` |

Every message costs `max(native_payload.len(), policy.minimum_charge)`, using its
class's `BucketPolicy`. The native payload includes the inner wire header and, after activation, the
24-byte secure record overhead.
Empty and tiny malformed packets therefore consume credit too. These are generous
initial policies: 120 maximum-size 164-byte secure Drag records per second are charged
61,440 bytes/s, comfortably below 128 KiB/s. Control's burst holds 31 maximum-size
262,180-byte secure records; Bulk's burst holds 511 maximum-size 32,804-byte secure records.
Each burst holds two
seconds of steady credit, allowing ordinary same-frame Grab/Release and handshake
bursts while bounding sustained abnormal traffic. Each class permits at most 512
tiny messages from a full bucket without refill, and 256 tiny messages/s at steady
rate. The larger reliable-class minimum charges prevent their generous byte bursts
from permitting hundreds of thousands of tiny messages that monopolize the shared
receive budget. This bounds per-message CPU work with the existing single bucket.

Refill uses elapsed `Instant` time and integer arithmetic: multiply elapsed
nanoseconds by the class's bytes/s rate, retain fractional byte credit in
billionths, and add whole bytes up to capacity. Saturating `u128` intermediates
make even extreme rates and very long idle durations safe; a full bucket discards
surplus credit. Zero elapsed time earns nothing. There is no fixed one-second
window, float accounting, rate history, allocation or background task. Each check
updates only its class's bucket in O(1). On x86_64 Windows, the added limiter state
is 104 bytes per connection (three 32-byte buckets and one 8-byte shared policy
reference); adding class-specific minimum charges leaves this state size unchanged.
The immutable 72-byte policy is shared rather than copied per connection.

In `GnsDirectIp::poll`, native receive first resolves the existing connection and
validates the lane and frame-size limit, then calls `rate_limit.check` **before**
`payload.to_vec()`. Only `Allow` creates a `TransportEvent::Message`; a rejected
payload has no application allocation/copy or routing event. Wire decode runs
later, so malformed wire bytes within the native size limit are charged too.
Unknown lanes and oversized native messages still disconnect as `InvalidMessage`.
No per-message spam log or metrics backend is added.

Transient is latest-wins presentation, so an over-limit frame can be dropped and
later frames accepted after refill; Release preserves final correctness. Reliable
Control and Bulk must never silently lose a frame and then continue with a protocol
sequence gap. Their first message that cannot fit the available credit closes the
connection; tolerance for ordinary bursts comes from capacity, without a strike
counter or grace-period drops. The local close-code mapping assigns `RateLimited`
code 1003 for incoming native closes. Outgoing sockets retain their existing
wrapper-owned close behavior and generic native code; local events still carry
`RateLimited`, and the peer observes `RemoteClosed`.

All dequeued messages, including drops, invalid messages and messages belonging to
already-removed connections, consume the existing shared 512-message receive
budget. The 32-slot chunks, partial-chunk retention and socket round-robin cursor
are unchanged. There is no additional connection map/lookup, piece scan, idle
gameplay work, protocol change or transfer-level backpressure.

Time is passed explicitly to `check(class, payload_len, now)` for deterministic
unit tests. `with_policy(&'static InboundRatePolicy, now)` permits small test policies;
production GNS uses the defaults. A future Steamworks backend can store the same
limiter in each connection and call it with the native `SteamNetworkingMessage_t`
length before copying or routing. The helper knows no GNS handles, addresses,
SteamIDs or networking identities and is available without the `gns` feature.

## Wire v7

Each inner Puzzella frame has this header; after activation it is inside one
secure record/native message, with no stream reassembly:

| Bytes | Field |
| --- | --- |
| 0..4 | ASCII `PZLA` |
| 4..6 | u16 wire version, little-endian, currently 7 |
| 6 | Kind: 1 ClientControl, 2 AuthorityEvent, 3 RemoteDragUpdate, 4 ClientDrag, 5 BulkTransfer, 6 SessionControl, 7 SyncControl |
| 7 | Reserved zero byte |
| 8..12 | u32 payload length, little-endian |
| 12.. | Postcard 1.x binary serialization of the indicated protocol type, including BulkTransferMessage |

`WireMessage::ClientCommand` maps to kind 1 or 4 based on its command. This allows
the Transient limit to be checked before deserializing. After decoding, its actual
class must match the header and receiving lane. Both header and Postcard trailing
bytes are rejected. Unsupported versions, unknown kinds, reserved bits, truncated
frames, malformed enums/varints/masks and excess lengths return `WireError`.
No gameplay wire uses JSON.

The v7 Postcard field order and enum representation are part of the wire contract.
A breaking type/codec change requires a new `WIRE_VERSION`; adding handshake,
snapshot or image chunk kinds can be done at this boundary. A future backend uses
these exact bytes and requires no protocol or replication change.
Version 5 appends DragCancelled as authority event variant index 4, after
DragRotationCommitted. Its only fields are player and grab_sequence. It consumes
one authority cursor, does not resend a target, commits no delta and performs no
snap. Existing event indices and payload field order are unchanged. It retains
version 4's mandatory PAKE-derived AEAD and encrypted SecureChannelReady,
Rotate / RotationCommitted, RotateDrag / DragRotationCommitted and the
RemoteDragUpdate basis sequence. Version 6 replaces kind 5's opaque bytes with
typed Start/Chunk/Finish/Abort. Gameplay payload layouts remain unchanged; only
their version header advances. Version 7 adds SyncControl kind 7 on Reliable Control,
including generation-bound baseline offers, catch-up events and ACKs. Bulk fields
and gameplay layouts remain unchanged. Only version 7 is decoded; pre-release versions
1 through 6 and future versions are rejected without a compatibility decoder.
WIRE_VERSION also binds PAKE context, HKDF application keys and secure record AAD
to v7. No cryptographic design change is made.
Fixed v7 golden frames cover Client Grab, Client Drag, Rotate, RotateDrag (with and
without prior ticks), GrabAccepted (including a rejected reference), ReleaseCommitted,
RotationCommitted, DragRotationCommitted, DragCancelled, RemoteDragUpdate,
AuthAccepted, SecureChannelReady, all four Bulk variants and SyncControl. Each checks encoding
against literal bytes and decodes those same bytes; field/variant order changes
cannot silently pass through an encoder/decoder roundtrip. Review the fixtures
alongside any wire version change.

RotateDrag and DragRotationCommitted use reliable Control. No target/membership is
resent: the accepted Grab context remains active. The reliable final_delta commits
the displayed transform, rotates each component, and rebases to zero delta without
snapping. through_tick sets the stale transient floor (None before any update).
The sender keeps ticks monotonic and tags subsequent ClientDrag moves with the
successful rotation control sequence in after_control_sequence. Forwarded
RemoteDragUpdate carries that basis_sequence separately from the original
grab_sequence. Hosts/peers reject old-basis updates and new-basis updates arriving
before the matching reliable event; later absolute updates or reliable final_delta
restore presentation. This keeps drag updates O(1) with scalar metadata only.

| Bound | Payload bytes (12-byte header is additional) |
| --- | --- |
| Control | 262,144 |
| SessionControl (Control lane) | 4,096 |
| Transient | 128 |
| Bulk transfer message | 32,768 |
| Maximum whole plaintext frame | 262,156 (includes header) |
| Maximum secure outer record | 262,180 (includes sequence/tag) |

Native receive checks the lane's outer record limit (plaintext frame limit +24)
**before copying** the GNS buffer. Decryption preserves the inner limits below.
Wire decode checks the complete frame, kind-specific declared limit and exact
length before deserializing. Existing core bounded visitors clamp sparse lists
and rejected refs to 32 and masks to 31,250 u32 words / 1,000,000 pieces; malformed
dimensions and nonzero padding bits are rejected. There is no allocation based
on an unchecked network length. A full 1M-bit mask fits even with Postcard's
worst-case word varints. Drag packets contain only scalar identity/context/tick/
delta fields and are bounded by 140 plaintext bytes including the header,
or 164 bytes as a secure record.

## Bounded Bulk transfer foundation

`network::bulk` is backend-independent and has no connection/native/ECS/store
dependency. A caller owns one `BulkTransferReceiver` and one `BulkTransferSender`
per secure connection/direction. Outer kind **5** always uses Reliable ordered
`MessageClass::Bulk`, with these Postcard variants in stable index/field order:

| Index | Variant | Fields in order |
| --- | --- | --- |
| 0 | Start | TransferId(u64), BulkTransferKind, total_size(u64), sha256([u8; 32]) |
| 1 | Chunk | TransferId, offset(u64), data(Vec<u8>) |
| 2 | Finish | TransferId |
| 3 | Abort | TransferId |

Kind indices are **0 JoinBaseline**, **1 PuzzleImage**. Production limits are:

| Bound | Limit |
| --- | --- |
| `MAX_BULK_WIRE_PAYLOAD` | 32,768 bytes including typed Postcard metadata |
| `MAX_BULK_DATA_BYTES` | 32,704 bytes (32 KiB minus a 64-byte margin) |
| JoinBaseline declared total | 64 MiB |
| PuzzleImage declared total | 512 MiB |
| Active transfers per receiver | 2 |
| Total declared in-flight per receiver | 576 MiB |

`BulkTransferLimits` allows small injected test policies. Start rejects zero,
kind-cap excess, active-count excess and checked aggregate overflow/excess **before
creating content storage**. Start does not preallocate total size: it stores only
metadata, an incremental SHA-256 and an empty `Vec<Vec<u8>>` with zero capacity.
Accepted Start IDs must increase strictly. Completion, Abort and `clear()` retain
the last accepted ID, preventing reuse without an unbounded tombstone set; a new
secure connection may construct a new receiver. Rejected Starts do not advance it.

Chunk's bounded serde visitor ignores untrusted length prefixes/size hints for
reservation, growing from `Vec::new()` only on actual decoded bytes. It rejects
more than 32,704 bytes; wire encode also rejects oversized direct Rust objects
before serialization. Tests confirm even maximum u64 ID/offset encodings fit the
64-byte margin. Receiver additionally rejects empty or oversized chunks, requires
`offset == received`, uses checked `offset + len`, and rejects ends past total.
Duplicate/overlap offsets and gaps/out-of-order offsets return OffsetMismatch.
Each non-final chunk is canonical full size; the final chunk must equal all
remaining bytes (1..=32,704). Thus metadata is bounded by
`ceil(total / MAX_BULK_DATA_BYTES)`, at most 16,417 chunks for a 512 MiB image.

Validated data Vecs are moved into chunk storage, without copying into a growing
contiguous buffer. SHA-256 updates incrementally only after validation. Finish
requires exact received total and verifies SHA-256 before yielding
`CompletedBulkTransfer`; hash mismatch drops content and reclaims the declared
budget. Invalid Start/Chunk and premature Finish leave prior state intact.
Unknown transfer IDs return a typed error. Abort and `abort_local(id)` remove an
active transfer/drop its chunks/reclaim its budget; `clear()` releases all active
storage and sets active count/declared bytes to zero.

The sender checks kind size, issues checked monotonic IDs, hashes an `Arc<[u8]>`,
then generates Start, canonical contiguous Chunks and Finish one message per
`next_message()` call. No whole-message-list allocation occurs. Counter exhaustion
is an error. Completed content exposes `chunks()` and consuming `into_chunks()`
for future storage streaming; explicit post-completion `into_bytes()` performs a
fallible `try_reserve_exact` before flattening. The caller owns completed content:
in-flight budgets bound receiver-active transfers, not caller-retained completions.
SHA-256 provides corruption detection, **not authorization or provenance**. The
Syncing layer compares the receiver's actual verified image digest to the
PAKE-authenticated `SessionDefinition.image_hash`. Cache bytes are hashed against
that same identity before claiming availability; trusting a Start hash is insufficient.

HostSyncCoordinator/ClientSyncRouter now own authorization and phase routing.
Host-to-client transfers require an exact Reliable Control offer and acceptance
before Start: TransferId/kind/size/hash, plus generation/cursor for JoinBaseline.
Restart retires the old association, sends Bulk Abort and Control Restart, and
preserves monotonic ID high-water marks. Cross-lane reordering cannot rebind old
completion to the new generation. The client immediately validates/installs a
baseline or hands verified image chunks to the caller, with no completion queue.
Client-to-host Bulk is rejected in every current phase. Ready routers and Ready
bootstrap reject current Sync Bulk entirely. Future Ready Bulk needs an explicit
protocol extension. Persistent cache, compression, file streaming and automatic
runtime scheduling remain pending. Bulk's 8 MiB/s, 16 MiB burst and 32 KiB minimum
charge policy and GNS physical record limits remain unchanged. Snapshot schema
**4** and JoinBaseline schema **1** remain unchanged.

## Routing and frame integration

The topology remains a host-authoritative star. No peer-to-peer mesh is created.

```text
peer ClientRouter::send_command
 → wire encode / class → SecureTransport::send → AEAD record → backend
 → host SecureTransport::poll → AEAD verification/decryption
 → bootstrap Ready gate → SessionConnections identity → wire decode
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
from any connection other than their designated, assigned host. Sync Bulk goes
only to the dedicated client sync router; commands/events in the wrong
direction are rejected. The backend never interprets protocol objects or accesses
`PieceDataStore`.

During each Bevy frame, keep SecureTransport<T> in caller-owned state, call its
`poll`, process every event through HostBootstrap/ClientBootstrap::process, and
route BootstrapOutcome::Syncing to HostSyncCoordinator/ClientSyncRouter and
BootstrapOutcome::Gameplay to HostRouter/ClientRouter. Call bootstrap
`expire` each frame too.
Routers borrow the existing session/store/context; they do not duplicate gameplay.
No systems are automatically scheduled and no piece scan occurs while idle.

Start the client sync router after authentication, passing optional cached bytes;
start the host sync coordinator for that connection to advertise the authenticated
identity and puzzle definition. Keep sync state only while a join exists. Schedule
bounded `pump` calls for joining peers and enforce timeouts using exposed
`SyncTiming` (overall start and transfer progress). On disconnect, remove sync state
and the catch-up entry. On scope/migration boundaries call `observe_host_state`;
store changes restart, while changed PAKE-bound session/epoch/host needs reauthentication.
Use `HostRouter::route_with_sync` for remote authority commands and explicit
`record_command_outcome` / `record_authority_event` for local commands/cancellation:
apply -> immediate catch-up record -> ordinary Ready publication. Retention failure
remains separate from the already-applied outcome; Ready publication still proceeds.
See [join runtime contract](JOIN_IN_PROGRESS.md#application-syncing-routing-and-generations).

Ready routers return `DroppedTransient(TransientDrop)` for decoded Transient
MissingDragContext, WrongDragContext, DuplicateUpdate and StaleUpdate. These include
updates before GrabAccepted/RotateDrag commit and after ReleaseCommitted/DragCancelled.
Host command-stream equivalents (ControlNotProcessed, StaleMoveContext and duplicate/
stale Move commands) are classified only on the Transient path. Reliable errors
are never run through that classifier. WrongSession, WrongEpoch, WrongHost,
InvalidDelta, malformed authenticated frames and Reliable cursor gaps remain errors;
ReplicationError::Diverged still requires resync. The core replication/replay
semantics are unchanged. Valid late updates are O(1) presentation drops.

Host apply and publication are separate: `route` returns the already applied
outcome even if the next send might fail. `publish` attempts every eligible peer
and returns per-connection failures. The caller must retain and retry failed
Control bytes in original cursor order, or disconnect/resynchronize that peer;
a secure send failure always closes that channel, so its recovery requires a
fresh connection/authentication/synchronization rather than same-key retries;
**do not re-apply a command to retry publication**. A successful send means native
queue acceptance, not an application ACK. Late join/baseline synchronization,
backpressure recovery and disconnect ownership cancellation remain explicit
session responsibilities. Before observing a disconnect, retain its assigned
player. Disconnect observation removes the mapping; use `cancel_replicated` and
`publish_authority_event` with the retained player identity as described above.
PlayerIds are not reused in the session. Migration's new-epoch drag discard remains
separate from this Reliable cancellation event.

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
- CMake, Git, LLVM/libclang 18.1.8 for the locked bindgen 0.70.1
  (`LIBCLANG_PATH` if not auto-discovered). LLVM 23.1.2 produces an incomplete
  callback struct with this bindgen; see the Windows setup's compatibility note.
- Internet access during the initial native build: the sys build script clones/
  bootstraps vcpkg and installs its manifest's protobuf, OpenSSL and their Abseil/
  UTF-8 dependencies. Runtime crypto uses Windows BCrypt in this build.
- A short `GNS_VCPKG_BUILDTREES_ROOT` path (the wrapper rejects paths >100 chars).
  Use a writable path for that build; do not bypass the length check.

For Windows, persist `LIBCLANG_PATH` and a short
`GNS_VCPKG_BUILDTREES_ROOT` in the host's Cargo user configuration. This host is
already configured, so ordinary PowerShell and Codex builds need no per-command
environment setup. See [Windows build setup](WINDOWS_BUILD.md#gnsを使うビルド)
for the configuration and setup on another host. The fixed buildtrees path and
existing shared Rust build cache are reused from both the primary checkout and
worktrees; wait for any Cargo build-directory lock to be released.

From an ordinary shell on the configured host:

```powershell
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
cargo test --locked -p puzzella-game --features gns
cargo build --locked --features gns
```

The GNS tests use only 127.0.0.1, ephemeral ports, 15-second deadlines with polling
backoff, explicit closes and RAII cleanup on panic. The routing test validates
actual listener acceptance, A's reliable Grab, both replicas' ACK, a Transient drag
delivered only to B, reliable Release with snap, equal final
piece/connectivity/snapshot/cursor state, and disconnect on both ends. That test
now performs mutual SPAKE2 confirmation plus encrypted SecureChannelReady for both
clients, checks ciphertext immediately below the wrapper on actual UDP sends, and verifies independent
PlayerIds and unassigned gameplay mappings, and explicitly promotes through
Syncing to Ready with test-only completion permits before Grab/Drag/Release. A
dedicated localhost join test exercises image and baseline transfer/catch-up through
the actual secure lanes and stops at Finalizing without gameplay registration.
A separate actual-socket wrong-password
test verifies rejection, close, no channel installation, no registration and no
gameplay mutation. An actual-socket ciphertext-tampered Grab test verifies
ProtocolViolation close, destroyed keys and unchanged host pieces/cursor.
Socket-free bootstrap tests cover byte-length password validation/redaction,
both confirmations, altered metadata/identity, nonce/session/image replay,
malformed points/control, pre-decode size limits, timeouts, pending capacity,
independent global start/failure buckets, pre-Ready routing/broadcast exclusion,
send failure cleanup, stale registrations and one-time Ready promotion. Secure
unit tests cover both directions/all classes, reflection and lane substitution,
ciphertext/tag/sequence tampering, reliable replay/gaps, Transient gaps and stale
drops, exhaustion, owned-key zeroization, fresh-PAKE key separation, downgrade
rejection, outer/inner limits, same-batch activation and channel lifecycle.
Bootstrap tests also cover Securing timeout/capacity and encrypted-ready send
failure. Receive
regressions preload reliable UDP messages and wait for native ACKs without polling
the receiver, then verify shared 512-message limits, rotation between listeners,
partial-chunk retention, outgoing-socket draining and server-initiated close.
Receive-drain fixtures use a larger Control burst so their 513-message backlog
tests receive behavior independently of the production tiny-message rate limit.
Rate-limit unit tests cover burst spending, zero elapsed time, fractional refill,
capacity, extreme idle/rate arithmetic, per-class minimum charge and tiny-message
burst/steady bounds, Transient recovery, reliable disconnects, independent
classes/connections, zero policies and default
120 Hz drags/maximum-frame bursts. Small-policy localhost tests cover Transient
drop without events or disconnect, dropped messages consuming the receive budget,
Control independence, fresh connection credit, Control/Bulk disconnect exactly once
on incoming and outgoing sockets, and oversized-message reason precedence. They
use native reliable delivery on the Transient lane only to preload deterministic
backlogs; production flags and the ordinary Grab/Drag/Release routing test remain
unchanged. Refill timing tests advance explicit time without sleeping.
No external network is contacted by tests; initial dependency downloads are build
setup only.

## Future Steamworks backend

Implement `game/src/network/steamworks` behind its own feature later. It should own
`ISteamNetworkingSockets`, translate `CreateListenSocketP2P` / `ConnectP2P` and
`SteamNetworkingIdentity` to private native handles and freshly issued Puzzella
tokens, implement the shared `Transport`, and use the same class/lane/limit policy.
Store `InboundRateLimiter` in each connection and apply the shared inbound policy
before native payload copies, as described above.
Expose separate `listen_p2p` / `connect_peer` establishment APIs; do not extend
DirectIpTransport or introduce an address enum into protocol messages.

Steam session authentication should validate the Steam user and session membership
alongside the reusable session password layer. Independent PlayerId assignment
still belongs to bootstrap, and SessionConnections registration waits for Ready.
Wrap the future Steamworks Transport in SecureTransport<T> and apply its outer
record limit before native copies. The existing wire codec, host/client routers
and authority/replication adapters remain the integration points. The same Syncing
protocol and generic Bulk substrate use this secured Transport; the future final
barrier coordinates Ready promotion on both ends. Steam P2P/SDR selection belongs to that future
backend. `SteamLobbyBackend`, if added, separately chooses session members and
the lobby owner/host identity; it does not send gameplay messages. No lobby or
Steamworks placeholder dependency/module is added in this change.

# Multiplayer command protocol

This is an opt-in, transport-independent CPU protocol boundary. It adds no systems
to local play and implements no packet send/receive, sockets, Steamworks, lobby,
host election, encryption, image transfer, or join-in-progress.

## Layers

```text
interaction / local representation (PieceCommand, PieceBitSet, DragTransform)
        ↓ explicit protocol boundary only
protocol target encoding (PieceTarget::from_selection)
        ↓
client command (ProtocolCommandEnvelope)
        ↓ authenticated sender + session / epoch / sequence validation
authority validation (current connectivity, ownership, placed, enabled)
        ↓
authoritative gameplay (existing grab / relative Z / release / snapping)
        ↓ mutation complete, semantic outcome + authority cursor
semantic authority event (ProtocolAuthorityEventEnvelope)
        ↓ authenticated host + session / epoch / cursor validation
peer replica (PeerReplicationState::apply_event)
        ↓ deterministic replay + Release result fingerprint verification
same authoritative puzzle state
```

`core/src/protocol.rs` owns wire types and pure target resolution.
`core/src/session.rs` supplies the existing shared sequence tracker and migration
state. `game/src/multiplayer/protocol.rs` provides `ProtocolDragContexts::apply`,
which validates and applies commands synchronously against `PieceDataStore`.
The backend must authenticate the sender and pass its identity separately from
the envelope's player claim. A mismatched claim is rejected before sequencing.

The generic `ClientCommandEnvelope<C = PieceCommand>` keeps existing adapter
callers compatible. New transports use `ProtocolCommandEnvelope`, the alias with
`C = ProtocolPieceCommand`; local scalar commands are not the network drag API.
Both adapters use the same tracker rather than independent sequence mechanisms.

## Target forms and stable identity

```rust
pub struct ComponentRef {
    pub member: PieceId,
    pub expected_size: u32,
}

pub enum PieceTarget {
    Component(ComponentRef),
    Components(Vec<ComponentRef>),
    Dense(DenseTarget),
}

pub struct DenseTarget {
    pub members: PieceBitSet,
    pub component_count: u32,
    pub topology_digest: u128,
}
```

`member` is the minimum member PieceId, independent of union history and snapshot
restore. A DSU root is never sent. `expected_size` detects intervening component
merges. Resolution checks the PieceId range, current minimum, and exact current
size before enumerating or changing the component. Zero/fake sizes, nonminimum
members, invalid IDs, and pre-merge references cannot silently expand their target.

Encoding policy is deliberately simple: one component uses `Component`, zero or
2–32 components use `Components`, and more than 32 use `Dense`. Sparse entries
are unique and sorted by minimum member. Encoding stops collecting sparse entries
at the 33rd component and shares the original COW mask for dense fallback, then
computes the fingerprint using a temporary canonical mask, without allocating a
list of all component references.
It does not depend on HashMap iteration or a DSU root. Empty selection is
`Components([])`. Selection dimensions must match current connectivity.

Sparse input lists and ACK rejection lists are capped at 32 during deserialization;
sparse targets are checked again for
direct Rust callers. The established bounded `PieceBitSet` decoder is unchanged.
This is a representation threshold, not a claim about optimal serialized size
for every small puzzle or serializer. Transport framing and message byte limits
remain the future backend's responsibility.

Dense records the component count and a target-specific topology fingerprint at
encoding time. SHA-256 input is the UTF-8 domain
`puzzella/component-topology/v1` followed by one zero byte, then the touched
components' `(minimum_member, component_size)` pairs in ascending minimum order,
each field as little-endian u32, then the component count as little-endian u32.
`topology_digest` is the first 16 hash bytes interpreted as a little-endian u128.
This fixes ordering, byte encoding and the algorithm independently of serde,
HashMap order, platform hashers, and DSU history. It is a stale-state fingerprint,
not a MAC or authorization mechanism; normal ownership validation still applies.

## Authority canonicalization and partial acceptance

For `Component` / `Components`, invalid or stale entries are rejected individually;
other valid components continue. Duplicate valid references apply only once.
The adapter then checks **every current member** for ownership, PLACED and ENABLED
before accepting a component. If any member is remote-held, placed, disabled, or
already held (including by the same player), that whole component is rejected.
Accepted components retain the existing global relative-Z order and MAX_Z
compaction behavior.

`Dense` must have exactly the authority's puzzle dimensions. Authority resolves the
touched components into a canonical mask and recomputes their count and digest.
Any mismatch rejects the **entire Dense target** as `StaleTopology` before any
gameplay mutation or context creation. This detects both merging two selected
components and adding an unselected component to a selected one, even when the
count is unchanged. A merge among unrelated components does not invalidate it.
On a match, partial components are expanded from authority connectivity and receive
the same full-member gameplay validation. No component-reference list is built.

The command result reports applied piece counts and a `GrabAccepted` ACK containing
the exact canonical accepted target. Invalid/stale sparse entries are listed as
rejections; gameplay-ineligible components are omitted from the accepted target.
A client can determine its accepted membership directly, including partial
acceptance of differently sized components. A single invalid dense shape, stale
fingerprint, or oversized sparse list rejects the target.
No client field can set placed, ownership, or connectivity.

## Reliable Grab and Release

```text
Control(42): Grab { target }
Move { after_control_sequence: 42, tick: 0 }: DragUpdate { delta }
Move { after_control_sequence: 42, tick: 3 }: DragUpdate { delta }
Control(43): Release { grab_sequence: 42, final_delta }
```

Control numbers start at zero for each player / authority epoch, must be contiguous,
and reject gaps, duplicates and old values. Session and epoch must match and the
authority session must be active. Counters do not wrap. Protocol commands must use
their designated stream. A sequence-valid control is consumed even if gameplay
validation rejects it; the sender continues with the next control number.

Grab creates at most one active context per player, using its control sequence as
the context ID. `ActiveDrag.target` is adaptive:

```rust
pub enum ActiveDragTarget {
    Sparse(Vec<ComponentRef>),
    Dense(DenseTarget),
}
```

This stores the **authority-accepted** result, never an unvalidated client target.
Up to 32 accepted components use sparse references; larger sets use a canonical
accepted bitset with its recomputed fingerprint. A million singleton selection
therefore retains 125,000 bytes of mask words plus fixed metadata, rather than an
8 MB reference list. The returned dense ACK shares those mask words with the
context via Arc. A large incoming target that accepts only a few components becomes
a sparse context/ACK. Ownership and base positions stay in the existing canonical
store. The existing operation-local ID list for relative-Z sorting and temporary
release roots remain; neither is retained in the active context. A second Grab
during an active drag is rejected, and an empty/all-rejected Grab creates no context.

Release sends the Grab sequence and a finite final absolute delta, with no target
resend. Sparse contexts resolve saved references again and reject stale entries
individually. Dense contexts recompute their accepted topology fingerprint; a
mismatch rejects the whole Release before mutation and retains the context for
explicit cancellation. Successful topology validation is followed by checking
all members' ownership, HELD, ENABLED and PLACED flags component by component.
Valid sibling components can still release when another component fails gameplay
validation. The existing release implementation commits final positions.
All accepted translations commit before snapping, retaining board priority,
fixed rigid-transform snap, same-rotation rounded closure and no chained translation. Existing whole-
component overflow handling also applies: nonfinite translated positions keep
the base positions while releasing holds.

A processed matching Release ends its context. Holds belonging to stale or
otherwise invalid components remain unchanged; they are not expanded or moved.
The backend can call `cancel_player` to clear that player's remaining holds without
translation or snap. Wrong-context or nonfinite Release is rejected and retains
the context so a later valid reliable control can release it. Duplicate Release
is rejected by the control tracker.

## Grab ACK and authority event boundary

`ProtocolCommandResult::Grabbed { applied, ack }` returns this serializable ACK:

```rust
pub struct GrabAccepted {
    pub player: PlayerId,
    pub grab_sequence: u64,
    pub accepted: PieceTarget,
    pub rejected: Vec<RejectedComponentRef>,
}
```

`accepted` contains exact canonical membership in the same adaptive wire forms,
with a fingerprint derived only from accepted components for Dense. It is also
returned for empty/all-rejected, topology-valid grabs as `Components([])`; only
nonempty acceptance creates an active context. `rejected` describes invalid/stale
sparse entries, not a large list of every gameplay-ineligible dense component.
Clients use `accepted` as the definitive membership, rather than reconstructing it
from piece counts or assuming every requested component was accepted.
`player` comes from the authenticated authority operation, so peers can distinguish
different players using the same Grab control sequence.

`ProtocolAuthorityEvent::GrabAccepted(ack)` and `ProtocolAuthorityEventEnvelope`
define the semantic host-to-peer boundary using the existing session, host and
authority cursor. `ProtocolDragContexts::apply_replicated` applies the authenticated
client command, then returns `HostCommandOutcome { result, authority_event,
drag_update }`. Successful Grab (including empty acceptance) and Release produce
reliable envelopes and advance the host cursor exactly once. Counter exhaustion
is checked before mutation. Rejected commands produce no publication object and
do not advance the authority cursor. Successful transient commands produce only
`RemoteDragUpdate`. Delivery/retention belongs to a future transport; gameplay
never invokes a send callback. Use this entrypoint consistently for a replicated
session; the existing `apply` remains an unpublishing authority adapter.

## Replication streams and cursor semantics

| Stream | Payload | Ordering |
| --- | --- | --- |
| Reliable client command | Grab, final Release | Per-player contiguous control sequence |
| Reliable authority event | GrabAccepted, ReleaseCommitted | Contiguous authority cursor |
| Best-effort drag presentation | RemoteDragUpdate | Independent per-player/per-grab latest tick |

`PeerReplicationState::apply_event` first validates active migration state,
session, epoch, claimed host and cursor via `AuthoritySession::validate_event`.
The backend-authenticated sender is also checked against the actual session host.
Only then is gameplay applied. Only after successful gameplay **and** result
verification does `record_applied_event` record the cursor. Failure never advances
it. There is no buffering or resend implementation.

At cursor 100, event 102 returns `Protocol(EventGap { expected: 101 })` without
mutation. Duplicate and older cursors both return `Protocol(StaleEvent)` before
gameplay, so no operation can apply twice. Epoch counters do not wrap; sequence
zero is the new epoch baseline. Presentation messages never change that cursor.

## Peer accepted Grab and presentation

`PeerReplicationState` retains a map from player to `RemoteDrag`:
`grab_sequence`, adaptive `ActiveDragTarget`, scalar absolute `delta`, and an
optional last transient tick. `remote_drag`/`remote_drags` expose these to a future
renderer. No base-position copies or per-piece transforms are retained.

The peer resolves **only** the event's accepted membership, without needing the
client request. It checks all accepted members for contradictions before any
mutation. A stale reference/topology or incompatible ownership/HELD/PLACED/ENABLED
state returns `Diverged`; it never silently accepts a subset or replaces an owner.
It then applies that complete accepted set using the shared Grab mutation, with
the same `(z_order, PieceId)` ordering and MAX_Z compaction as the host. Ownership,
HELD and Z therefore match. Existing component-wide local selection exclusion is
reused; local selection and local drag presentation are not wire fields.

Empty accepted membership consumes its reliable event but creates no drag.
Sparse contexts retain at most 32 references; Dense contexts retain a canonical
mask and its target-specific topology fingerprint. A million singletons remain
125,000 bytes of COW mask words, never a permanent million-reference vector.

`RemoteDragUpdate` contains session, authority epoch, player, grab sequence, basis sequence, tick
and finite absolute delta. The authenticated sender must be the host. Tick gaps
are allowed; larger ticks replace the delta, equal ticks return `DuplicateUpdate`
and older ticks return `StaleUpdate`. Wrong session/epoch/host, missing context,
wrong Grab sequence and nonfinite delta are rejected without consuming the tick.
Updates after Release or cancellation have no context and are rejected. Ticks can
reach u64::MAX but never wrap. Validation and application only access the context
and scalar delta: no piece enumeration, topology resolution, mask creation,
authoritative position writes or dirty-state changes occur. Presentation is
authoritative base position plus that delta.

## ReleaseCommitted and deterministic replay

```rust
pub struct ReleaseCommitted {
    pub player: PlayerId,
    pub grab_sequence: u64,
    pub final_delta: Vec2,
    pub result: ReleaseResultFingerprint,
}
```

The final event resends no target. The peer uses the matching saved accepted Grab
target and the event's final delta, independent of received presentation updates.
Both host and peer call `game/src/multiplayer/release.rs::release_drag`, which
preserves the existing target/ownership checks and calls `PieceDataStore::release_roots`.
All translations commit before connected snap; zero-rotation board priority, fixed rigid transform,
same-rotation/same-offset rounded closure, strict thresholds and no chained translation use the
existing resolver. There is no peer-specific snap implementation. A matching
successful Release removes the context.

Host and peer must supply the same validated immutable `PuzzleDefinition`, or
both supply `None` for unsnapped coordinate-only application. The existing
authority adapter's behavior for an invalid/mismatched definition remains `None`.

## Release result fingerprint and divergence detection

`ReleaseResultFingerprint(u128)` is the first 16 SHA-256 bytes interpreted as a
little-endian u128. It reuses the existing SHA-256 dependency with domain separation.
It is a divergence checksum, not a security MAC. Only final components containing
actually released roots are visited, including neighbors absorbed during snap.
Unaffected piece arrays are never scanned or hashed. Singleton members are hashed
directly, without member-list or member-mask allocation. Other components reuse
one member-order scratch buffer. Both final minima and component members use
sorted/deduplicated Vec storage up to `max(128, ceil(piece_count / 32))` input IDs;
larger inputs use temporary bitsets with ascending iteration instead of ID sorts.
The minima input bound is the released-root count; the member bound is the exact
component size. Each dense mask has at most 125,000 bytes of words for 1M pieces.
Sparse Vec capacity and dense masks are reused across components, then dropped at
the end of Release; no scratch is retained by the replica. Dense iteration scans
mask words, but hashes only affected IDs. This changes neither the v1 byte sequence
nor its digest/domain: ordering is still ascending stable minima and member IDs.

The v1 serializer-independent SHA-256 input is fixed in this exact order:

| Field | Encoding / ordering |
| --- | --- |
| Domain | UTF-8 `puzzella/release-result/v1`, then one zero byte |
| Final affected component count | u32 little endian |
| Each affected component | Ascending stable minimum PieceId; deduplicated |
| Component minimum, size | Two u32 little endian fields |
| Representative logical offset X, Y | Two f32 `to_bits()` values as u32 little endian |
| Representative placed | One byte: 0 or 1 |
| Each component member | Ascending PieceId, independent of DSU/list history |
| Member ID | u32 little endian |
| Member position X, Y | Two f32 `to_bits()` values as u32 little endian |
| Member Z | u32 little endian |
| Member authoritative flags | u32 little endian, masked to PLACED(1), HELD(8), ENABLED(16), canonical connected edges(32+64+128+256), rotation bits 9–10 |
| Member owner present | One byte: 0 or 1 |
| Member owner | u64 little endian; zero when absent (presence distinguishes PlayerId(0)) |
| Released count, newly placed count, total placed_count | Three u32 little endian fields |
| next_z_order | u32 little endian |

Representative offset is `position - rotate_quarter(definition.correct_position(minimum), rotation)` when
a valid definition is supplied, otherwise `position - Vec2::ZERO`. Float bytes
preserve signed zero and the exact IEEE-754 bit representation; they are not
rounded, normalized, formatted as text, or serialized through serde. Counts fit
u32 under the existing MAX_PIECES bound. Member IDs grouped by component encode
membership topology; member positions, Z, owner and edge-cache bits detect
discrepancies a representative offset/size alone would miss. Local selection,
preview and other local presentation fields are excluded.

The peer compares the computed fingerprint with the event **after** replay.
Mismatch returns `ReplicationError::Diverged`, leaves the cursor unchanged and
latches `needs_resync`. Reliable semantic inconsistencies (including missing or
wrong Release contexts) also latch resync. While latched, subsequent gameplay and
presentation stop; retrying the failed event cannot apply its delta twice. Replay
may already have mutated affected pieces when a checksum fails. It is not rolled
back or staged using a whole-puzzle/snapshot clone; a trusted snapshot must replace
the store before continuation. Protocol/authentication/gap/stale rejections do not
latch divergence and leave gameplay untouched.

## Snapshot restore, migration and future resync

Contexts are scoped to session ID, authority epoch and store generation. A restored
store immediately hides old context through these scope checks; the next explicit
apply clears retained old contexts. Frozen migration hides presentation and rejects
both streams. Completing the existing migration snapshot installation changes
store generation, host and epoch; old host events and old epoch updates are rejected,
and new epoch Grab/Release events restart replication.

`PeerReplicationState::install_snapshot` validates a trusted same-epoch baseline
at/after the current applied cursor before replacing store/session state and
clearing resync/context. Failed snapshot validation leaves them intact. Existing
direct snapshot installation also invalidates contexts through store generation.
Snapshot schema **4** adds rotation bits 9–10 while retaining the 16-byte piece representation.
It intentionally omits ownership and transient drag contexts: a backend must
coordinate host/peer hold cancellation at a resync baseline, or use the existing
new-epoch migration flow, before continuing an in-flight drag. A pending Release
without a post-baseline Grab context requests resync rather than guessing membership.

`cancel_player` removes that player's context and uses existing component-wide
hold cleanup without translation or snap. Disconnect/timeout decisions must be
coordinated by the backend on the host and all replicas; no timeout scheduler or
cancellation wire event is added here. Snapshot request messages, source
authentication, retention/catch-up policy and coordination are future transport
work.

## Best-effort DragUpdate and presentation

`ClientCommandSequence::Move` is also the transient drag stream, not only scalar
local Move. Gaps are allowed, duplicate / old ticks are rejected, and each accepted
update replaces the previous delta (latest wins). The active drag basis (initial
Grab or latest successful RotateDrag control) must match `after_control_sequence`. Future/unprocessed control
contexts, older contexts, wrong authenticated players, wrong epochs and updates
after Release are rejected. Ticks remain monotonic across rebases within one Grab;
the rebase through_tick is the minimum retained floor. Rejected RotateDrag controls
leave the previous move basis and tick tracker usable.

The delta is absolute from the most recent committed/rebased state, never an increment accumulated onto
previous updates. DragUpdate updates only the stored scalar delta. It does not
enumerate components, regenerate masks, serialize membership, move authoritative
base positions, snap, or dirty-upload piece positions. Future presentation can use
authoritative base state plus this transient delta, following local `DragTransform`.
Remote rendering, interpolation and prediction are not implemented here.

Release always uses its own final delta, never the last received DragUpdate.
Identical Release commands yield identical final state with no updates, lost
updates, or reordered updates. Gameplay correctness therefore does not depend on
best-effort delivery. Sequence validation and active-context validation are both
required; the sequence tracker alone does not create gameplay contexts.

## Context lifecycle and local performance

Contexts are scoped to session identity, authority epoch and store generation.
Snapshot restore or successful migration invalidates old contexts; frozen migration
rejects commands and hides active presentation. On disconnect or externally
cancelled ownership, use `ProtocolDragContexts::cancel_player` before reusing the
player. It clears the context and existing component-wide holds without snapping.
Do not mix local position/hold mutations into a protocol drag without cancelling
it. The normal local schedule does not use protocol contexts.

Normal gameplay prevents external snapping into held components and does not merge
the dragged components before Release, so accepted Dense topology stays fixed.
The stored fingerprint also detects unexpected topology mutation defensively.

Local `PieceCommand::RotateDrag` adds a discrete commit/rebase operation. GPU rectangle results, selected pieces,
dirty pieces, local drag membership and bulk operations continue using PieceBitSet.
Scalar Grab / Release reuse the existing mask-free paths. Point/Ctrl/rectangle
selection, component expansion and outlines, O(1) pointer drag, GPU picking, renderer,
dirty upload, completion UI and existing migration/snapshot behavior are retained.
Protocol encoding and canonicalization run only when explicitly called, never on
idle or local pointer frames.

Snapshot schema **4** uses 16 bytes per piece, including rotation bits 9–10 and
`SNAPSHOT_CONNECTED_RIGHT` and `SNAPSHOT_CONNECTED_DOWN`. Snapshot restoration
rebuilds connectivity, so minimum-member references remain meaningful even when
the restored DSU chooses another root. Command protocol and snapshot are separate.

## Serialization checks

Tests use the existing serde_json test dependency and round-trip Component,
Components(8), Components(32), fingerprinted Dense(1M), command envelopes and Grab
ACK/event envelopes. Broad payload
bounds and `Component size * 100 < Dense(1M) size` catch accidental dense dependence
without fixing serializer-specific byte layouts. DragUpdate and Release envelopes
also have small fixed bounds. These are correctness checks, not benchmarks or
a commitment to JSON as the future transport encoding. Correctness tests also
assert the SHA-256 fixed vector, root-history stability, unrelated-merge tolerance,
stale Dense atomic rejection and the million-singleton context's 125,000-byte mask
allocation with shared ACK storage. Superseded naked Dense masks cannot deserialize
into the new target type. Snapshot format is unaffected by this command change.

## Host/peer simulation coverage

Normal `cargo test` runs socket-free simulations with one host and two peers
restored from the same initial schema-4 snapshot. They compare exact position
bits, placed/held/enabled flags, Z, stable component membership/size, connected
edge cache, owners, placed_count and next_z_order. Coverage includes simple
Grab/Release, lost and reordered presentation, connected and board snap, board
priority, rounded/same-offset closure, multiple released roots, no chained
translation, partial/empty acceptance, Z ties/compaction, million-piece Dense
contexts, Dense Release/unrelated merges/stale topology, event gaps/duplicates,
authenticated sender checks, divergence and snapshot recovery, cancellation,
snapshot context invalidation and new-host/new-epoch migration. A different
host/restore DSU root history is also replayed through connected snapping.
The result fingerprint has an independently encoded fixed-byte vector and tests
that unaffected piece changes do not enter its digest. No benchmarks are added
or run; existing ignored benchmarks stay ignored.

## Future multiplayer work

The same command protocol can be carried by Steam Networking or direct IP. No
transport is selected or implemented. Future work includes:

- Steam Networking / direct IP transport, authentication and packet framing.
- Reliable resend / congestion policy and best-effort update delivery.
- Delivery and retention of semantic authority events and transient presentation;
  snapshot request/resync messages and coordinated cancellation. ReleaseCommitted
  already replays merge/placed outcomes without transmitting full puzzle state.
- Join-in-progress: latest snapshot, then authority events after its cursor to catch
  up to current state; event log retention and recovery policy are not implemented.
- Snapshot compression / chunking, separate from schema 4 and command targets.
- Image transfer using `SessionDefinition.image_hash`: look up local cache, fetch
  retained encoded original bytes on a miss, and verify SHA-256 before use.
- Remote interpolation and client prediction.
- Rollback netcode and event log storage.

No image transfer, snapshot/event-log backend, network encryption, dedicated
server, NAT traversal, matchmaking, or host election is added by this change.


## Reliable quarter-turn rotation

`ProtocolPieceCommand::Rotate { target: PieceTarget, quarter_turns: i8 }` is a
reliable control. Signed turns normalize modulo four. The authority resolves the
existing sparse/dense topology, rejects complete placed, disabled, held or
inconsistent bodies, and rotates each accepted component about its own current
world-space position AABB center. Active player drags use RotateDrag instead. Sparse stale
entries are rejected individually; a stale dense fingerprint rejects the command.

`RotationCommitted` contains the exact accepted `PieceTarget`, normalized turns,
player, and affected-state result fingerprint. Replicas preflight the whole event
before mutation, then use the same canonical reconstruction and compare the
fingerprint. Cursor/authentication checks and the divergence latch are shared with
Grab/Release. Normal rotation adds no transient presentation state.

The result fingerprint also hashes rotation bits and reconstructs representative
translations using the component rotation. Zero-rotation v1 byte vectors retain
their existing values. For rotation events the released/placed counts are zero;
member flags and positions encode the rotated result. See [ROTATION.md](ROTATION.md).

## Reliable rotation during one Grab

```text
Control(42): Grab { target }
Move { after_control_sequence: 42, tick: 10 }: DragUpdate { delta }
Control(43): RotateDrag { grab_sequence: 42, final_delta, through_tick: Some(11), quarter_turns }
Move { after_control_sequence: 43, tick: 12 }: DragUpdate { delta }
Control(44): RotateDrag { grab_sequence: 42, final_delta, through_tick: Some(12), quarter_turns }
Control(45): Release { grab_sequence: 42, final_delta }
```

No mask or PieceTarget is sent during rebases. The existing authority-accepted
ActiveDragTarget must resolve completely, and every member must be enabled,
unplaced, held by the same player with matching HELD/owner, and a consistent rigid
transform with uniform rotation. One invalid component rejects the whole rebase
before mutation. Rotation rebuilds final canonical coordinates directly from the
displayed position AABB, preserves holds/Z/selection/connectivity, and does not snap.
Pointer-only drag stays O(1), with no piece-state or membership uploads; only an
explicit rotation visits O(k) members and uploads their changed states.

through_tick is the maximum tick sent before Q/E, or None if no update has been
sent. It cannot regress behind the authority/replica's last accepted tick or a
previous floor. Equal floors permit multiple rotations without intervening moves.
Successful rebases zero delta and retain the same Grab context. Released positions
use the final rebased state plus Release.final_delta, followed by the existing snap.

DragRotationCommitted carries player, original grab_sequence, new basis_sequence
(the rotation's client control sequence), through_tick, final_delta, normalized
turns and result fingerprint. Replicas validate the whole accepted context before
mutation, replay the same rotation calculation, and compare a SHA-256 fingerprint
of affected canonical states and the retained player/Grab/basis/tick/zero-delta
context. A mismatch latches Diverged and requires the existing coordinated resync.

RemoteDragUpdate also carries basis_sequence. Old basis packets, tick values at or
below the floor, and future basis packets overtaking the reliable event are rejected
without modifying presentation. The latter need no resync: the next absolute
Transient or a reliable final_delta repairs presentation. Successful rebases alone
advance the sequence tracker's move basis; rejected reliable rebases consume their
control number while leaving states, holds and the previous basis intact.

Wire version 2 and fixed golden frames cover all rotation commands/events, signed
turns, optional floors, field order and enum indices. Pre-release wire v1 is rejected
without a legacy decoder; snapshot schema 4 and its 16-byte records are unchanged.

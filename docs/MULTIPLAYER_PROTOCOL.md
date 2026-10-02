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
fixed-offset snap, rounded closure and no chained translation. Existing whole-
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
authority cursor. A future backend authenticates the host and advances/wraps the
cursor when publishing an authoritative outcome. Delivery, event retention, peer
application and remote rendering are not implemented here.

## Best-effort DragUpdate and presentation

`ClientCommandSequence::Move` is also the transient drag stream, not only scalar
local Move. Gaps are allowed, duplicate / old ticks are rejected, and each accepted
update replaces the previous delta (latest wins). Both the latest consumed control
and the active Grab must match `after_control_sequence`. Future/unprocessed control
contexts, older contexts, wrong authenticated players, wrong epochs and updates
after Release are rejected. Ticks reset when a control is consumed.

The delta is absolute from Grab's base state, never an increment accumulated onto
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

Local `PieceCommand` remains unchanged. GPU rectangle results, selected pieces,
dirty pieces, local drag membership and bulk operations continue using PieceBitSet.
Scalar Grab / Release reuse the existing mask-free paths. Point/Ctrl/rectangle
selection, component expansion and outlines, O(1) pointer drag, GPU picking, renderer,
dirty upload, completion UI and existing migration/snapshot behavior are retained.
Protocol encoding and canonicalization run only when explicitly called, never on
idle or local pointer frames.

Snapshot schema **3** is unchanged: 16 bytes per piece, including
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

## Future multiplayer work

The same command protocol can be carried by Steam Networking or direct IP. No
transport is selected or implemented. Future work includes:

- Steam Networking / direct IP transport, authentication and packet framing.
- Reliable resend / congestion policy and best-effort update delivery.
- Publication of the defined GrabAccepted event and additional semantic authority
  events such as Release committed, components merged and pieces placed, wrapped
  in `AuthorityEventEnvelope` with an authority cursor. Ordinary operations should
  not replicate all million states.
- Join-in-progress: latest snapshot, then authority events after its cursor to catch
  up to current state; event log retention and recovery policy are not implemented.
- Snapshot compression / chunking, separate from schema 3 and command targets.
- Image transfer using `SessionDefinition.image_hash`: look up local cache, fetch
  retained encoded original bytes on a miss, and verify SHA-256 before use.
- Remote interpolation and client prediction.

No image transfer, snapshot/event-log backend, network encryption, dedicated
server, NAT traversal, matchmaking, or host election is added by this change.

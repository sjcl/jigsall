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
    Dense(PieceBitSet),
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
at the 33rd component and shares the original COW mask for dense fallback.
It does not depend on HashMap iteration or a DSU root. Empty selection is
`Components([])`. Selection dimensions must match current connectivity.

Sparse input lists are capped at 32 during deserialization and checked again for
direct Rust callers. The established bounded `PieceBitSet` decoder is unchanged.
This is a representation threshold, not a claim about optimal serialized size
for every small puzzle or serializer. Transport framing and message byte limits
remain the future backend's responsibility.

## Authority canonicalization and partial acceptance

For `Component` / `Components`, invalid or stale entries are rejected individually;
other valid components continue. Duplicate valid references apply only once.
The adapter then checks **every current member** for ownership, PLACED and ENABLED
before accepting a component. If any member is remote-held, placed, disabled, or
already held (including by the same player), that whole component is rejected.
Accepted components retain the existing global relative-Z order and MAX_Z
compaction behavior.

`Dense` must have exactly the authority's puzzle dimensions. Its bits name current
components, so a partial component is expanded from authority connectivity, never
from a client-provided member list. Dense masks intentionally have no historical
size assertion: use compact references when stale-membership detection at Grab is
required. Every expanded component still receives the same full-member validation.

The command result reports applied piece counts and explicit invalid/stale
reference rejections. Gameplay-ineligible components are omitted from the accepted
context. A single invalid dense shape or oversized sparse list rejects the target.
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
the context ID. The authority stores only **accepted** stable component references
and a transient delta (`ActiveDrag`); it does not retain a client target mask or
per-piece base-position copy. Storage is O(accepted components), potentially a large
reference list for dense singleton selections. Ownership and base positions stay
in the existing canonical store. A second Grab during an active drag is rejected,
and an empty/all-rejected Grab creates no active context.

Release sends the Grab sequence and a finite final absolute delta, with no target
resend. It resolves each saved reference again, rejects stale entries individually,
and rechecks all members' ownership, HELD, ENABLED and PLACED flags before calling
the existing release implementation. Valid sibling components can still release.
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
Components(8), Components(32), Dense(1M), and all command envelopes. Broad payload
bounds and `Component size * 100 < Dense(1M) size` catch accidental dense dependence
without fixing serializer-specific byte layouts. DragUpdate and Release envelopes
also have small fixed bounds. These are correctness checks, not benchmarks or
a commitment to JSON as the future transport encoding.

## Future multiplayer work

The same command protocol can be carried by Steam Networking or direct IP. No
transport is selected or implemented. Future work includes:

- Steam Networking / direct IP transport, authentication and packet framing.
- Reliable resend / congestion policy and best-effort update delivery.
- Host → peer semantic authority events such as Grab accepted, Release committed,
  components merged and pieces placed, wrapped in `AuthorityEventEnvelope` with an
  authority cursor. Ordinary operations should not replicate all million states.
- Join-in-progress: latest snapshot, then authority events after its cursor to catch
  up to current state; event log retention and recovery policy are not implemented.
- Snapshot compression / chunking, separate from schema 3 and command targets.
- Image transfer using `SessionDefinition.image_hash`: look up local cache, fetch
  retained encoded original bytes on a miss, and verify SHA-256 before use.
- Remote interpolation and client prediction.

No image transfer, snapshot/event-log backend, network encryption, dedicated
server, NAT traversal, matchmaking, or host election is added by this change.

# Join baseline CPU foundation

`JoinBaseline` schema **1** combines the existing `GameSnapshot` schema **4**
with at most **64** `BaselineDrag` records. This is a transport-independent CPU
API; it is not yet a wire message or a complete join protocol.

```rust
pub struct JoinBaseline {
    pub schema_version: u16,
    pub snapshot: GameSnapshot,
    pub active_drags: Vec<BaselineDrag>,
}
pub struct BaselineDrag {
    pub player: PlayerId,
    pub grab_sequence: u64,
    pub basis_sequence: u64,
    pub last_tick: Option<u64>,
    pub target: PieceTarget,
    pub delta: Vec2,
}
```

`ActiveDrag` remains authority-created, with no serde implementation. Received
records become internal `ActiveDrag` instances only after complete validation.
The drag vector decoder caps allocation independently of `size_hint`, stops at
64 entries, and rejects any extra entry. Direct Rust callers are checked too.
Existing sparse ref and dense bitset decoding limits also apply.

`GameSnapshot.pieces` is bounded during deserialization to `puzzella_core::MAX_PIECES`,
including when nested inside `JoinBaseline`. The decoder starts with zero capacity
and does not trust the sequence size hint for large upfront allocation. Semantic
validation separately requires exactly `definition.piece_count()` pieces.

## Capture at a command boundary

Call `JoinBaseline::capture(&session, &store, &contexts, &definition)` between
complete synchronous `ProtocolDragContexts::apply_replicated()` or
`cancel_replicated()` calls. Shared
borrows cover the session cursor, canonical store and active contexts together;
the caller must not capture these separately around intervening commands.
Transient updates do not advance the reliable cursor, but their latest basis,
tick and delta are captured in the same borrowed view. No drag is ended, committed,
cancelled or awaited, including the authority's own local-player drag.

The snapshot is captured by `GameSnapshot::capture()`. The read-only
crate-private `ProtocolDragContexts::active_drags()` exposes contexts only for
the current SessionId, AuthorityEpoch and store epoch. Capture sorts records by
ascending numeric PlayerId. Sparse refs retain their canonical ordering; dense
targets share the authority's COW words instead of expanding into member refs.

Capture checks finite deltas, basis >= grab sequence, unique players, nonempty
exact current targets, no rejected/stale refs, no placed members, disjoint targets,
and every member's HELD flag and matching owner. A rare full piece scan checks
HELD/owner occupancy/context coverage, rejecting orphan holds and contradictory
mirrors instead of silently producing an incomplete baseline. Out-of-store owner
occupancy is rejected too. Old-scope contexts cannot hide live holds.

## Transactional install and stream continuation

Use `PeerReplicationState::install_join_baseline(session, store, baseline, expected)`
with a **trusted** `SnapshotExpectation` from the session negotiation. Authenticate
the authority and delivery separately; neither the snapshot nor topology digest
proves provenance. The API requires an active session, the same session/image and
authority epoch, and an expected cursor >= the current cursor. The snapshot must
exactly match the expected cursor, definition and schema. It cannot roll back or
adopt a future epoch.

Before changing any state, snapshot expectation and all canonical records are
validated, building one `PieceConnectivity`. All drag targets then resolve against
that connectivity. Sparse resolution may not reject even one ref. Dense topology
must match, and the supplied mask must already name whole components; partial masks
are rejected rather than expanded silently. One operation-local `PieceBitSet`
detects overlap across all sparse/dense targets. Every target must be nonempty and
unplaced. Schema, count, duplicate, delta, sequence, stale/invalid target, overlap,
placed and authority capture mirror failures have distinct errors.

Prepared targets retain validated sparse refs or canonical COW dense masks. Once
all checks pass, `GameSnapshot::install_with_validated_connectivity()` reuses the
same DSU through `CheckpointView::install_with_validated_connectivity()`. There is
no second target resolution or connectivity reconstruction at commit. These
crate-private commit helpers have no remaining fallible validation. An invalid
fourth drag therefore leaves store, session, replica contexts/divergence, epoch,
owners, selections and local presentation unchanged.

`PieceDataStore::restore_baseline_holds()` changes only HELD and `held_by`.
Sparse targets walk the validated components; dense targets iterate their masks
and union occupied words. Owners use the existing dense Vec/occupied bitset and
one player-count HashMap update per drag. No GrabAccepted is synthesized, so
snapshot position, rotation, Z, next Z, connectivity and placed state stay intact.
The snapshot's new store epoch schedules the normal full GPU upload, including
restored HELD bits, without adding a duplicate dirty structure. Old extracted
uploads remain immutable. Local drag members/delta and selection remain empty.

Install restores grab sequence, basis sequence, last tick, target and delta into
`remote_drags`, resets divergence and binds scope to the current session/authority
epoch and new store epoch. The session cursor becomes the snapshot cursor.
Existing RemoteDragUpdate, ReleaseCommitted, DragRotationCommitted and
DragCancelled can follow immediately, without a replayed Grab. Last-tick stale/duplicate/gap checks and
rotation basis checks continue unchanged; Release/Rotate fingerprints use the
existing deterministic replay paths.

`JoinBaseline @ C` can contain A's active drag even when A disconnects immediately
after capture. Authority `cancel_replicated(A)` emits Reliable
`DragCancelled { player: A, grab_sequence } @ C+1`; installing the baseline then
applying that event clears the restored holds/context and reaches the same
canonical/ownership state and cursor as the host. No GrabAccepted or target resend
is needed. Cancellation commits no transient delta and performs no snap; canonical
position, rotation, Z and connectivity, including earlier RotateDrag commits, stay
intact. It removes local presentation membership and marks HELD changes dirty
without advancing store epoch. Other players' active contexts remain intact.

CPU tests feed the generated continuation directly, covering sparse/dense targets,
simultaneous drags and rotation/rebase before cancellation. Event retention for a
joining peer is not implemented. Future disconnect callers must retain the assigned
PlayerId before removing its SessionConnections mapping, cancel once, and publish
or retain the resulting envelope even if an individual send fails.

## Memory and scheduling

Capture/install are explicit rare operations; no ECS systems or idle/pointer
piece scans are added. At 1M pieces, overlap/coverage scratch is **125,000 bytes**,
shared by the whole operation. Existing dense resolution may temporarily use its
adaptive root scratch (up to roughly 125 KB of words plus a touched-word list);
a malformed partial dense target can also trigger a temporary 125 KB COW mask
before rejection. These temporaries are freed, never retained per drag.

The existing canonical snapshot holds 16 MB of records. Validation builds the
existing 8 MB DSU once; install adopts that DSU and constructs the usual 16 MB GPU
state array. Owner restoration uses the existing lazily allocated 8.125 MB owner
storage, plus at most 64 player counts. These are store state, not new baseline
scratch. Prepared/captured dense targets share words; sparse targets keep at most
32 refs per drag even for a million-member component. Each dense drag's retained
target has the existing 125 KB mask representation, with at most 64 such contexts;
there is no million-entry member-ref list or new permanent per-piece metadata.

## Separate lifecycle semantics

| Lifecycle | State used | Active drags |
| --- | --- | --- |
| Same-epoch join | GameSnapshot + JoinBaseline active_drags | Restored as remote authority contexts |
| Migration/recovery | Canonical snapshot + new authority epoch | Discarded |
| Snapshot-only resync | Existing install_snapshot API | Cleared; backend must coordinate cancellation/epoch |
| Save/load | Existing checkpoint/save format | Omitted on save and cleared on load |

GameSnapshot remains canonical persistent state: no HELD, owner, drag delta or
context fields. SNAPSHOT_SCHEMA_VERSION, PuzzleCheckpoint and save format are
unchanged. Wire v5 appends DragCancelled as authority event index 4; fixed golden
frames preserve earlier payload layouts and update their version header. Join
overlays are never applied automatically to migration or save/load.

Future work must connect authenticated bootstrap/Syncing/Ready, bounded bulk
transfer, image transfer, post-capture authority-event queues/backpressure and
catch-up drag refresh. None is implemented here. Remote drag rendering,
interpolation and GPU owner buffers remain future work. SecureTransport/GNS/rate
limiting retain their existing designs. Benchmarks are outside this change. CPU
tests feed post-baseline events directly in order and cover both sparse and dense
targets.

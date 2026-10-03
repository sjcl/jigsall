# Join baseline and bounded catch-up CPU foundation

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
simultaneous drags and rotation/rebase before cancellation. The catch-up coordinator
below retains post-capture events. Future disconnect callers must retain the assigned
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
transfer, image transfer and final catch-up drag refresh. These transport/runtime
steps are not implemented here. Remote drag rendering,
interpolation and GPU owner buffers remain future work. SecureTransport/GNS/rate
limiting retain their existing designs. Benchmarks are outside this change. CPU
tests feed post-baseline events directly in order and cover both sparse and dense
targets.

## Bounded JoinCatchUpCoordinator

`multiplayer::catch_up` is a CPU coordinator keyed by authenticated `PlayerId`.
It does not use ConnectionId, GNS handles or Steam identities, change HostRouter
parameters, install Bevy systems, call bootstrap `begin_sync`/`promote_ready`, or
add wire messages. JOIN_BASELINE_SCHEMA_VERSION **1**, SNAPSHOT_SCHEMA_VERSION
**4**, WIRE_VERSION **5** and golden frames are unchanged.

Call `begin_join(player, session, store, contexts, definition)` between complete
authority operations. This one API borrows the session cursor, canonical store and
drag contexts together and calls `JoinBaseline::capture`; it registers retention
at exactly that baseline cursor before returning `JoinCatchUpStart`. Capture is
the existing rare O(N) operation and never freezes gameplay or waits for a drag.
The coordinator keeps cursor/scalar context metadata, not a copy of the baseline
or its targets. The caller owns the returned snapshot during transfer.

An initial join starts at generation **0** in `BaselinePending`. Events and
post-capture transients are retained immediately, before installation is ACKed.
Duplicate joins, the host's own PlayerId and a 65th pending join reject. Failed
begin capture inserts no join. RestartRequired entries count toward the pending
cap until restarted or explicitly removed.

The future authority caller must use this synchronous ordering:

```text
apply_replicated / cancel_replicated (apply gameplay once)
→ record_command_outcome / record_authority_event (already-applied outcome)
→ publish the original outcome to ordinary peers
```

The helper records `authority_event` and/or `drag_update`; an empty outcome is a
no-op. Cancellation's `authority_event` uses `record_authority_event` directly.
Retention is independent of send success. Publication retries must not reapply,
regenerate or re-record the event. Duplicate and older record cursors explicitly
return `DuplicateEvent`/`StaleEvent` without mutation or double insertion.
Catch-up failures concern joining peers: the caller must still publish the original
already-applied event to ordinary peers and continue authority gameplay.

While joins exist, a recorded envelope must match the current SessionId, host,
authority epoch and **current session cursor**, and advance the coordinator's
observed cursor by exactly one. Generating multiple events and recording them
later as a batch is rejected. A forward gap or event/session cursor mismatch
discards every pending history with `MissedAuthorityEvent`; it never silently
retains only the later event. Transient recording, caught-up checks and new
baseline capture also compare the observed cursor with the current session cursor,
so an omitted reliable hook cannot hide behind a transient, an empty queue or a
second join. Duplicate/stale publication retries do not repair a missed hook.

Scope is SessionId, AuthorityEpoch, host PlayerId and `store.epoch`. A scope change
invalidates all joins with `ScopeChanged`; migration freeze invalidates them with
`AuthorityFrozen`, even before a new epoch is activated. Runtime callers should
call `observe_host_state` at migration/restore boundaries too. There is no automatic
runtime wiring: APIs without session/store arguments only know the last observed
scope. A fresh successful begin/restart capture rebinds global observation at the
current cursor; older invalidated joins remain RestartRequired.

## Catch-up memory budgets and restart

Each joining peer holds a bounded `VecDeque<Arc<RetainedAuthorityEvent>>` in
ascending contiguous cursor order. One event envelope is cloned into one internal
Arc and shared across every eligible peer's queue; dense COW mask words are also
shared with the original outcome and returned event batches. No per-peer deep
copy of a dense target, permanent per-piece catch-up metadata, serialization
scratch or target/piece scan is added during recording.

Production `CatchUpLimits::default()` allows **4,096 events** and **16 MiB logical
retained bytes per peer**. `MAX_PENDING_JOIN_SYNCS` is **64**. Tests can inject
smaller limits, including zero. Logical bytes use the envelope's `size_of` plus
dynamic accepted Components refs, Dense words and GrabAccepted rejected refs:
`len * size_of(element)`, with saturating arithmetic. ReleaseCommitted,
DragRotationCommitted and DragCancelled add no dynamic storage. This is a history
budget, not wire length or exact allocator overhead; Arc-sharing does not remove
the per-peer logical charge. Event count independently bounds queue slots.

Each peer also keeps only the latest post-capture `RemoteDragUpdate` per active
player in a numeric PlayerId-ordered BTreeMap, plus bounded scalar grab/basis/tick
guards initialized from baseline drags. The latest map starts **empty**: baseline
presentation is already included in `active_drags`. Both maps are capped at
`MAX_BASELINE_DRAGS` (**64**). Same-basis ticks must strictly increase; an unknown
or different grab/basis is rejected until the matching reliable event establishes
it. Tick guards survive reliable ACK pruning. GrabAccepted clears any old latest
value, DragRotationCommitted clears the old basis presentation and updates its
scalar guard, and ReleaseCommitted/DragCancelled remove the player's latest value
and guard. RotationCommitted leaves protocol drag presentation unchanged.

Overflow of event count, byte budget or active-player bounds sets only the affected
peer to `RestartRequired(reason)`, drops its queue **and backing allocation**,
clears transient/context maps and resets retained bytes to zero. Other peers
continue. Recording never captures a replacement snapshot, blocks gameplay or
automatically retries. Global cursor/scope failures invalidate all peers instead.
Per-peer overflow is visible through `status`; successful global recording may
return Ok even when one peer overflowed.

The caller explicitly calls `restart_join` at another command boundary. It must
be RestartRequired; a successful fresh baseline capture replaces its history,
updates its baseline/ACK/last-recorded cursors to the current cursor, and returns
to BaselinePending with generation incremented using checked arithmetic. A failed
capture or exhausted generation preserves the failed join's state/generation.
No counter wraps. Old-generation baseline completion, event reads, ACKs, latest
reads, caught-up checks and removal reject without changing the new generation.

When no joins exist, the record APIs and outcome helper return in O(1) before
examining/cloning events, calculating bytes or allocating queues/maps. Explicit
`remove_join(player, generation)` releases a retired join; it does not promote
Ready. The caller must retire outstanding old transfers before removing/reusing
an authenticated PlayerId's join lifetime. Restart provides stale-generation
protection within a lifetime; the coordinator does not retain unbounded tombstones
for removed players.

## Installation, ACK and future transport flow

`mark_baseline_installed(player, generation, cursor)` requires the current
generation, BaselinePending phase and exact baseline snapshot cursor. It sets
CatchingUp and acknowledges that baseline. Before installation, retained events
cannot be read/ACKed as a catch-up batch. `pending_events(player, generation, max)`
returns a retryable contiguous prefix from ACK + 1; reading/sending never pops it.
Returned semantic envelopes use existing cheap dense COW clones.

`acknowledge_through` cumulatively prunes events through an applied same-epoch
cursor and subtracts their logical bytes. Duplicate ACK is idempotent; an older
ACK, different epoch or cursor beyond the last retained reliable event rejects.
`is_reliable_caught_up` requires CatchingUp, matching active scope, an empty queue,
ACK == current session cursor and observed cursor == current session cursor.
It detects unrecorded authority gameplay and is **not** a Ready decision.
`latest_drag_updates` returns latest presentation in ascending PlayerId order;
the caller must deliver it after the corresponding reliable basis.

Planned transport lifecycle (not implemented by this change):

```text
Authenticated
→ Syncing
→ begin_join: baseline generation G
→ baseline/image transfer (host gameplay continues)
→ baseline installed ACK(G)
→ Reliable catch-up batches
→ authority cursor ACK
→ latest drag refresh
→ final sync ACK
→ Ready
```

Bulk chunking, byte reassembly, image transfer, SessionControl sync messages, final
refresh ACK and Ready handoff remain future work, as do disconnect coordinator
wiring, remote drag rendering and Steamworks integration.

CPU tests exercise baseline-pending gameplay through sparse/dense Grab, transient,
RotateDrag, cancellation and Release, then install/replay/ACK and compare canonical
pieces, connectivity, owners, contexts and cursor to the host. They also cover
latest-only ordering/rebase cleanup, staggered peers and shared event Arcs,
independent slow-peer overflow, byte/count/zero limits, restart recovery, failed
capture/exhausted counters, stale generations, missing reliable/transient hooks,
ACK validation, scope/freeze invalidation, capacity and the no-join fast path.

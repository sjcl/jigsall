# Join baseline, bounded catch-up and Ready handoff

`JoinBaseline` schema **1** combines the existing `GameSnapshot` schema **1**
with at most **64** `BaselineDrag` records. This is a transport-independent CPU
API. `network::syncing` now connects it to authenticated runtime routing and
generation-bound Bulk transfer, authoritative drag reconciliation and Ready handoff.

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

`GameSnapshot.pieces` is bounded during deserialization to `jigsall_core::MAX_PIECES`,
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
below retains post-capture events. The game runtime retains the assigned
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
unchanged. Wire v1 retains DragCancelled as authority event index 4; fixed golden
frames preserve earlier payload layouts and update their version header. Join
overlays are never applied automatically to migration or save/load.

The opt-in Syncing runtime below connects authenticated bootstrap, image preparation,
bounded Bulk, baseline installation, Reliable catch-up, final active-drag
reconciliation and Ready handoff. Remote drag rendering,
interpolation and GPU owner buffers remain future work. SecureTransport/GNS/rate
limiting retain their existing designs. Benchmarks are outside this change. CPU
tests feed post-baseline events directly in order and cover both sparse and dense
targets.

## Bounded JoinCatchUpCoordinator

`multiplayer::catch_up` is a CPU coordinator keyed by authenticated `PlayerId`.
It does not use ConnectionId, GNS handles or Steam identities, change HostRouter
parameters, install Bevy systems, call bootstrap `begin_sync`/`promote_ready`, or
add wire messages. JOIN_BASELINE_SCHEMA_VERSION **1** and SNAPSHOT_SCHEMA_VERSION
**1** remain unchanged. The application Syncing protocol uses WIRE_VERSION **2**
and fixed v2 frames; catch-up semantics and generic Bulk fields are unchanged.

Only after image preparation and verified `ImageReady`, call
`begin_join(player, session, store, contexts, definition)` between complete
authority operations. This one API borrows the session cursor, canonical store and
drag contexts together and calls `JoinBaseline::capture`; it registers retention
at exactly that baseline cursor before returning `JoinCatchUpStart`. Capture is
the existing rare O(N) operation and never freezes gameplay or waits for a drag.
The coordinator keeps cursor/scalar context metadata, not a copy of the baseline
or its targets. The caller owns the returned snapshot during transfer.

An initial join starts at generation **0** in `BaselinePending`. Events and
post-capture transients are retained immediately, before installation is ACKed.
Duplicate joins, the host's own PlayerId and a 13th pending join reject. Failed
begin capture inserts no join. RestartRequired entries count toward the pending
cap until restarted or explicitly removed.

The authority runtime caller must use this synchronous ordering:

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
retained bytes per peer**. `MAX_PENDING_JOIN_SYNCS` is **12**. Tests can inject
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
Before updating or removing a guard, DragRotationCommitted, ReleaseCommitted and
DragCancelled require the player's guard to exist with the same grab_sequence.
A missing or mismatched guard marks only that joining peer
`RestartRequired(InconsistentDragContext)` and discards its retained history and
transients. This detects damaged coordinator metadata even when the global cursor
is contiguous; the already-applied host event and other joining peers continue.

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

Runtime lifecycle (`network::syncing`, wire v2):

```text
Authenticated
→ explicit begin_sync()
→ Syncing: session/image identity, mandatory ClientProfile, then image availability
→ optional PuzzleImage offer/accept + Bulk (host gameplay continues)
→ SHA-256 == authenticated SessionDefinition.image_hash
→ ImageReady (Reliable Control)
→ AwaitingBaselineSlot: acquire one of two authority-wide baseline slots
→ JoinCatchUpCoordinator::begin_join: baseline generation G at cursor C
→ generation-bound JoinBaseline offer/accept + Bulk TransferId T
→ transactional baseline install, ACK(G, C, T)
→ generation-bound Reliable catch-up events and cursor ACKs
→ Finalizing: Reliable catch-up complete != Ready
→ Finalize(generation, cursor, revision, full authoritative active-drag scalar set)
→ client full-set reconciliation, FinalizeAck(generation, cursor, revision)
→ host rechecks scope, Reliable currency and current authoritative scalar set
→ host Ready registration, then ReadyCommit(generation, cursor, revision, roster snapshot, latest drag scalars)
→ client Ready registration, then Ready gameplay
```

Image transfer happens before any retention entry exists, so a 512 MiB image
cannot consume the coordinator's 4,096-event / 16 MiB catch-up window.
Initial baseline-slot waiting also happens before `begin_join()`, so it retains
no history and captures no snapshot. Gameplay continues while peers wait.
`ClientSyncRouter::start` verifies optional caller-owned cache bytes by SHA-256;
an incorrect cache is treated as missing. Transferred content uses the Bulk
receiver's actual verified digest, separately compared with PAKE-authenticated
`SessionDefinition.image_hash`. A Start hash is never image authorization.
Verified completed image chunks are handed immediately to the caller. Persistent
cache/PersistenceService integration remains future work.

## Application Syncing routing and generations

`SyncControlMessage` uses authenticated Reliable Control kind 7, separate from
handshake SessionControl and normal Ready AuthorityEvent/ClientCommand traffic.
`HostSyncCoordinator` owns the existing catch-up coordinator and at most 12 joining
connections. `ClientSyncRouter` owns one offered transfer and the existing bounded
Bulk receiver. No client-to-host Bulk receiver exists. ACKs/status use Control.
Ready HostRouter/ClientRouter reject all current Bulk and SyncControl messages.
Bootstrap returns `Consumed`, `Syncing`, or `Gameplay`; only the matching router
may receive each routed outcome. Gameplay application remains gated until Ready.
While Syncing, the client silently consumes/drops Transient gameplay without
applying or buffering it: a live broadcast can overtake ReadyCommit across lanes.
Reliable gameplay still rejects before Ready, and the host rejects all pre-Ready
gameplay. Authenticated states also reject all gameplay.
Authentication states accept authentication messages only. `Authenticated` waits
for explicit `begin_sync()` and rejects SyncControl/Bulk. Both `start` methods call
`begin_sync()` before handling sync traffic. Only `ConnectionState::Syncing` can
route SyncControl/authorized Bulk; Ready accepts gameplay only. Wrong-phase frames
are protocol violations, including SyncControl/Bulk after Ready.

`MAX_CONCURRENT_BASELINE_TRANSFERS` is **2** across the host coordinator, independently
of the 12 Syncing admissions. Image-ready peers enter the host-only
`AwaitingBaselineSlot` FIFO, which stores only connection IDs. The front waiter
acquires a slot before `begin_join()`/`restart_join()`, capture and serialization;
the baseline cursor is the authority's current cursor when the slot becomes free.
At most two baseline payloads are retained, rather than one per waiting peer.
Each retains the existing 64 MiB transfer cap (128 MiB serialized payload total;
capture/encoding scratch and transport buffers are additional bounded storage).
Slots stay occupied through installation ACK, including after Finish releases
host payload bytes. Restart invalidation and disconnect release slots; restart
attempts rejoin the FIFO behind existing waiters, with their old coordinator
entry still RestartRequired and retaining no history. Failed capture/offer work
keeps its reserved slot until runtime error handling removes the connection.
Call `pump` for waiting peers too: it admits the front waiter when capacity is free,
rechecking authenticated identity and active authority before capture. Disconnect
also removes queued IDs. Overall Syncing timeout includes queue waiting, while
peer-response deadlines do not apply to host-side slot waits. Their finite global
lifetime still applies; expiration is reported as HostCapacityTimeout without an
origin penalty. Transfer progress starts after TransferAccepted, not at the offer.

`SyncTransferBinding` contains TransferId, kind, size and hash. `BaselineOffer`
associates that binding with the coordinator's generation and baseline cursor.
The client records this exact offer before sending `TransferAccepted`; the host
waits for acceptance before generating Bulk Start. A Start with different
kind/size/hash or an unoffered ID is rejected without reserving transfer state.
Bulk framing still contains only its generic transfer fields, without generation,
authority cursor or PlayerId. IDs are monotonically issued for the secure connection.

`RestartRequired` immediately drops host outbound baseline state and pending ACK
associations. The host sends Bulk Abort only after receiving `TransferAccepted`
(including transfers already sent/finished), proving the client has seen the offer.
For an unaccepted offer it sends only Control Restart: no Bulk could have started,
and Abort itself must not overtake an unseen offer. Control Restart precedes the
next capture's offer. The client drops receiver content/declared budget and
the baseline association, retaining only generation/TransferId scalar high-water
marks. Old accepted IDs can never be offered again. Late old Start/Chunk/Finish/
Abort are obsolete drops, including when new-generation Control overtakes old
Bulk; old generation baseline/catch-up ACKs cannot advance the new join. There is
no unbounded tombstone set or completed-payload queue. Baselines are decoded,
validated and transactionally installed immediately after authorized completion.

`HostRouter::route_with_sync` performs apply then immediate retention and returns
the applied publication separately from any retention failure. The caller must
publish that original outcome to Ready peers even when a joining peer restarts.
Authority-local commands use `record_command_outcome`; disconnect cancellation
uses `record_authority_event`, both before normal publication. Call
`observe_host_state` at migration/restore boundaries and `disconnect` on teardown.
The existing scope/freeze/missed-hook invalidation remains authoritative. A
same-session store restore can reuse the verified image and restart the baseline;
changed PAKE-bound session/host/authority epoch requires fresh authentication.
The client also rejects and invalidates changed local session/epoch/host/store scope.

`pump` sends at most one Bulk message or Reliable catch-up event per call; one
outstanding catch-up event is ACKed before the next is sent. Only advancing ACKs
of sent cursors count as peer progress. Image transfer has its own two-slot FIFO,
`AwaitingImageSlot`, with no peer payload, chunk list or catch-up retention before
slot acquisition. Cached-image peers use baseline slots independently.
`VerifiedPuzzleImage` verifies immutable Arc content against the session once;
`begin_image` reuses that verified digest while generic `begin` hashes its payload.

`SyncTiming` distinguishes sync start, phase entry, last meaningful peer progress,
outstanding response and observed Bulk delivery. `SyncPolicy` defaults to 12-second
control responses, 30-second delivery stall, 30-second throughput grace and a
128 KiB/s minimum delivery rate. Offer acceptance, image-ready, baseline-installed,
catch-up ACK and finalization ACK all have separate response waits. Duplicate,
obsolete and invalid traffic cannot renew them; local send/pump cannot renew them.

Transfer lifetime is grace + ceil(size / minimum rate). Sync lifetime is 300 seconds
plus that size budget once per transfer kind; same-kind restart never renews it.
Thus a progressing large image may exceed 300 seconds, while a one-byte trickle
cannot keep its slot indefinitely. After Finish, Bulk timing continues until
native pending + unacknowledged bytes drain; only then does the short application
ACK deadline begin. The host's `expire` owns Syncing timers and removes peers,
slots, queues and retention even if no further packet arrives. Bootstrap owns
only authentication and its separate 5-second Authenticated handoff. Clients use
the same progress/lifetime policy and drop Bulk storage on failure.

Bulk generation preflights native reliable backlog before advancing the sender or
sealing a record. Host generation is bounded to 128 KiB/frame and 4 MiB/s including
reserved framing; runtime rotates joining peers each frame. See the complete
[resource and timeout policy](NETWORK_TRANSPORT.md#bounded-connection-and-join-lifecycle).

### Authoritative final reconciliation and barrier

**ReliableComplete != Ready.** Transient updates have no authority cursor, so
Reliable currency cannot prove drag presentation completeness. `latest_drag_updates()`
is presentation assistance, never final truth: a missed retention hook must not
carry stale presentation into Ready.

Finalizing preserves the retention entry. `pump` captures `ProtocolDragContexts`
directly at a complete command boundary into `FinalDragSet`: at most 64 scalar
records `(player, grab_sequence, basis_sequence, last_tick, delta)`, sorted by
numeric PlayerId. No target/membership is resent and no piece scan occurs. The
bounded wire visitor starts with an empty Vec, ignores size hints and rejects
entry 65. Direct encoding also enforces the cap. Noncanonical/duplicate players,
nonfinite deltas and basis before grab reject.

The client transactionally compares the entire set to `PeerReplicationState`:
same player set, grab sequence and basis sequence. Missing/extra contexts or
identity mismatch require disconnect and fresh join/resync; no target is inferred.
Only after all validation succeeds are last_tick/delta overwritten with authority
values, including rollback. This is a final synchronization operation, separate
from ordinary monotonic Transient application and its benign late-update drops.

`SyncFinalization` binds generation, AuthorityCursor and revision. Revision starts
at 1, increases with checked arithmetic for every candidate, survives baseline
restarts and never wraps or reuses a token. Client reconciliation sends FinalizeAck
but leaves the connection Syncing. On that exact ACK, the host rechecks generation,
session/host/authority epoch, active authority, store generation, Reliable currency
and cursor, then recaptures and compares the Reliable drag structure (player set,
grab sequence and basis sequence) to the candidate. The unchanged Reliable cursor
and scope preserve accepted targets and ownership. Tick/delta equality is not a
readiness condition: these Transient scalars may keep changing throughout the join.
Only a still-current candidate can construct the private `SyncReadyPermit`.

New Reliable events invalidate the candidate and return to CatchingUp; after
ReliableComplete a fresh finalization follows. A structural mismatch also retires
the candidate and requires a fresh revision. Transient-only differences instead
complete the same revision with the latest scalars in ReadyCommit, so continuous
dragging cannot starve finalization. Superseded revisions, old generations and
ACKs arriving during restart/fallback are harmless obsolete drops. Future or
inconsistent tokens reject. Scope changes/freeze/migration/missed Reliable hooks
invalidate candidates through the existing catch-up scope boundary.

Host ACK routing calls `promote_ready` before queuing Reliable Control ReadyCommit.
The SessionConnections mapping therefore exists before the client can receive
commit and submit gameplay on either lane. The client requires the matching
reconciled candidate and unchanged scope/cursor before its own `promote_ready`.
ReadyCommit includes the full bounded FinalDragSet captured at ACK validation.
The client verifies that its Reliable structure matches the candidate, validates
the whole set against current replica contexts, then applies its latest tick/delta
transactionally before promotion. It never reapplies the older candidate scalars.
Live Transient broadcasts can reach that client before Control ReadyCommit; the
client drops them while Syncing and accepts subsequent updates after commit.
ReadyCommit's FinalDragSet supplies the reconciled presentation state at that
commit boundary, even if every pre-Ready Transient was dropped. Updates generated
after that boundary use normal gameplay delivery after Ready.
Its mapping identifies the host; local PlayerId remains independently assigned.
The synchronous route borrow covers validation, registration and commit sending,
so authority commands cannot interleave that boundary. No host freeze is needed;
existing Ready peers keep playing during retries.

Sync route errors invalidate/remove join state, close the secure channel and remove
bootstrap/gameplay mappings. Host commit-send or registration failure rolls back
any host registration before returning; no commit is sent on failed registration.
Client ACK/commit-validation/promotion failure closes without leaving a participant.
As with other transport failure, the remote endpoint removes its mapping on the
disconnect event. Runtime must process disconnects before subsequent gameplay.

Successful host commit removes the entire sync peer, catch-up history, latest
Transient cache, transfer association, restart metadata and candidate. Baseline
slots were already released by install ACK. Client success releases candidate,
definition, baseline/generation and receiver state and reports `ClientSyncOutcome::Ready`;
the caller can drop the router then. Pump only joining connections. Test-only Ready
fixtures remain available for isolated gameplay tests; production has no permit
factory outside the validated barrier/commit path.

No Ready idle join work or piece scans are added. Capture/install remain rare
operations, dense masks retain COW representation, bounded history/Bulk budgets
remain unchanged, and image import keeps its 512 MiB policy. Host/Join UI, remote
drag rendering, presence UI, persistent image cache integration and Steamworks
remain future work.

CPU tests exercise baseline-pending gameplay through sparse/dense Grab, transient,
RotateDrag, cancellation and Release, then install/replay/ACK and compare canonical
pieces, connectivity, owners, contexts and cursor to the host. They also cover
latest-only ordering/rebase cleanup, staggered peers and shared event Arcs,
independent slow-peer overflow, byte/count/zero limits, restart recovery, failed
capture/exhausted counters, stale generations, missing reliable/transient hooks,
ACK validation, scope/freeze invalidation, capacity and the no-join fast path.
Secure routing tests additionally cover empty/multiple final sets, scalar rollback,
missing/extra/wrong/duplicate contexts, continuous unrecorded Transient updates
completing in one revision, latest-scalar Ready handoff, Reliable
fallback, stale revision/generation ACKs, scope changes and failed ACK/commit/
registration cleanup. The actual localhost GNS join test runs SPAKE2, missing-image
negotiation/verification, baseline, catch-up, reconciliation, final ACK and production
Ready promotion, then sends and replicates a normal Grab over encrypted Control.

## Bounded Bulk substrate (wire v2, framing retained from pre-release v6)

`network::bulk` provides `BulkTransferKind::{JoinBaseline, PuzzleImage}`,
monotonic `TransferId(u64)` and typed Start/Chunk/Finish/Abort under outer wire
kind **5**, always Reliable ordered Bulk. Start declares kind, total bytes and
SHA-256. Limits are **64 MiB JoinBaseline**, **512 MiB PuzzleImage**, **2 active
transfers** and **576 MiB aggregate declared in-flight bytes** per receiver.
Zero totals and policy/checked arithmetic overflow reject before content storage.

Start does not preallocate total size, including a legal 512 MiB declaration.
Storage begins as an empty `Vec<Vec<u8>>`; only validated received Chunk Vecs are
moved into it. `MAX_BULK_DATA_BYTES` is **32,704 bytes**, preserving the **32 KiB**
physical wire payload including typed Postcard fields. A bounded data visitor
ignores untrusted size hints for upfront reservation, and direct Rust wire encode
also checks the cap. Every non-final chunk is canonical full size; the final
chunk equals remaining bytes. This bounds image chunk metadata to 16,417 entries.

Each transfer requires strict contiguous `offset == received` with checked end
arithmetic. Duplicate/overlap/gap/out-of-order, empty, oversized and out-of-range
chunks reject before progress/hash changes. Two transfers can interleave while
each preserves continuity. Finish requires all declared bytes and verifies the
incremental SHA-256; a hash mismatch drops that transfer and releases its budget.
This hash detects corruption, not authorization; the application Syncing layer
compares the verified digest to `SessionDefinition.image_hash` separately.

Wire Abort and local `abort_local` release chunks/count/budget. `clear()` releases
all active state and resets count/budget to zero, retaining the Start-ID high-water
mark to reject reuse. A fresh secure connection can construct a new receiver.
The outbound sender hashes shared content and generates one Start/Chunk/Finish
message at a time with checked ID issuance. `CompletedBulkTransfer::chunks()` and
`into_chunks()` permit future streaming; explicit `into_bytes()` is the only
post-completion contiguous allocation and reports reservation failure as Result.

Syncing routers authorize/reassemble current host-to-client transfers; gameplay
routers reject them. Baseline capture/install/catch-up CPU semantics, image import's
local 512 MiB policy and persistence are unchanged. No persistent cache, file
streaming, compression or Steamworks is added. Bevy scheduling, worker image decode,
World installation and disconnect ownership now live in the
[game runtime](DIRECT_IP_RUNTIME.md).
See [NETWORK_TRANSPORT.md](NETWORK_TRANSPORT.md#bounded-bulk-transfer-foundation)
for the generic framing, receiver error semantics and fixed v2 golden contract.

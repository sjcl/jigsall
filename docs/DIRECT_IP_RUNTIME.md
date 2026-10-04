# Direct-IP game runtime

## Player profiles and presence

`PlayerDisplayName` in core is display metadata only: Unicode whitespace trim,
nonempty, at most 32 Unicode scalars / 128 UTF-8 bytes, no controls, line separators
or bidi formatting controls. Empty user input becomes None. Serde validates borrowed
Postcard strings before ownership allocation. Duplicate names are permitted.
`PlayerId` remains the session protocol/ownership identity; platform account IDs
remain separate. Names never enter authentication, routing, PAKE transcripts,
piece state, authority cursors, save filenames or snapshot identities.

World `PlayerRoster` is the only player list, bounded to host + MAX_CONNECTIONS
(65). The runtime moves that resource at frame boundaries without a second roster
mirror. Host starts at revision 0 with itself; each successful Ready join or Ready
leave checked-increments the revision. Syncing peers remain absent.

After encrypted Session, clients send mandatory ClientProfile (name only, including
None), then ImageAvailability on Reliable Control. Host accepts the profile once
and binds it to bootstrap's reserved PlayerId for the connection. Pending profiles
survive baseline restarts but are discarded on sync failure/disconnect.

Final ACK validation prepares a complete next roster before Ready registration.
Host promotes the connection, enqueues ReadyCommit(token, roster), then commits
its roster and discards sync/catch-up state. Immediate send failure rolls back
registration and leaves roster/revision unchanged. Existing Ready peers receive
PlayerJoined; the joiner gets its own entry in ReadyCommit only. Client validates
count, ascending unique IDs, authenticated host, assigned self and revision before
transactional install; later events require exactly revision + 1.

Presence uses wire kind 8, Reliable Control, FrameRoute::Gameplay, Host→Client only.
The same lane guarantees ReadyCommit(R) precedes PlayerJoined/PlayerLeft(R+1),
even when another peer leaves immediately after registration. Pre-Ready Transient
gameplay remains silently dropped by the client. Presence is never sent to Syncing
peers or included in saves, snapshots, checkpoints or JoinBaseline.

Runtime disconnect retains Ready identity and owns teardown once through `live`:
DragCancelled publication → roster remove → PlayerLeft publication. Without a drag
only the last two steps run. Syncing/duplicate disconnects produce no PlayerLeft
or revision change. Publication failures enter the same coordinator; their leaves
propagate to every remaining Ready endpoint in contiguous revision order.

General settings edits a raw draft and validates on Save name / Enter. The `player`
section of settings.json persists only display_name, using the existing settings
worker. Host/Join UI can clone `PlayerSettingsState.current.display_name` into the
options. Offline puzzle initialization uses the same preference (None in isolated
tests). HUD count/overlay read PlayerRoster, retaining localized default names and
score display. Menu/session teardown clears it. Settings edits apply next session.
Future Steam integration can validate persona names into PlayerDisplayName without
changing platform identity, PlayerId or roster APIs. In-session rename,
score replication, Steam and roster resync after divergence remain future work.

Injected runtime tests cover named host/A/B joins and A leave, duplicate names,
World resource lifecycle, cancellation-before-leave, duplicate/syncing disconnects,
cascading publication failure and immediate join/leave after queued ReadyCommit.

`GamePlugin` installs `NetworkRuntimePlugin`. A main-thread `NetworkSession` owns
one secured transport, bootstrap, sync coordinator/router, Ready connections,
authority session, host drag contexts or client replica, and local command sender.
It borrows the existing World `PieceDataStore`; there is no second store or
per-piece network Entity. The wire version is 9, snapshot schema 4, and join
baseline schema 1.

## Programmatic entrypoints

Under `gns`, `network::runtime::start_host(&mut World, HostOptions)` hosts the
current puzzle/store and returns the actual listener address (including an
ephemeral port). `start_join(&mut World, JoinOptions)` starts a connection and
advances through scheduled frames. `stop_session(&mut World)` closes the session
and requests Menu. `host_with_transport` / `join_with_transport` support an
injected `DirectIpTransport` without GNS, including headless World tests.
Joining requires `PuzzleImageLimits` and `ImageSettingsState`; headless callers
must supply explicit limits and settings before connecting.

Host/Join options also supply optional validated display names.
Host options supply address, immutable `SessionDefinition`, host `PlayerId`, and
`SessionPassword`. Join options supply endpoint, password, and optional encoded
cache bytes. Passwords are owned by bootstrap and never written to settings.
Hosting requires a valid existing puzzle with no outstanding offline holds.
An active session must be stopped before starting another one. Complete the
scheduled Menu transition before starting another session. A pending Menu cleanup
makes the World invalid; hosting also rejects unfinished offline generation.

UI reads `NetworkStatus`: role, phase, listener/endpoint, assigned local and host
identity, peer connection states, image readiness/source availability, and error.
`NetworkSession::authority()` and `replica()` expose read-only session/remote drag
state. UI need not access native sockets, bootstrap, or sync internals. Server
browser, lobby, NAT traversal, cursor rendering and migration remain future work.

## Menu entrypoints

The title first offers Single Player or Multiplayer. Single Player offers New
Game / Load Game; Multiplayer offers Host / Join. Host offers New Game / Load
Game. A new host switches between Puzzle settings and Multiplayer settings tabs; a
loaded host selects a save first, then configures multiplayer settings before
the load starts. Start & Host initializes/restores the canonical store and waits
for the existing RenderReady barrier before calling start_host. It never listens
while generating the puzzle. Missing encoded image bytes block hosting.

Host preparation sets PersistenceState.retain_image_for_host: image imports and
saves retain the shared encoded Arc, and load_for_host returns verified encoded
bytes from the persistence worker. Offline imports/saves/loads still release
them. Menu cleanup resets retention and drops the original image.

Join only asks for an IP literal with port and a session password. SocketAddr
input supports IPv4 and bracketed IPv6 (e.g. [2001:db8::1]:27015), not DNS names.
The connection screen replaces GameSetup's image/piece controls throughout
Connecting, Authenticating and Syncing. It shows a spinner with localized status,
not phase-count percentages or internal enum names. Failure/disconnect shows a
localized error category; the runtime keeps the diagnostic. Cancel/Back calls
stop_session so sockets, bootstrap, sync and Bulk ownership are discarded.

UI drafts are memory-only. Password buffers are zeroized when forms close or
Settings opens, and moved into SessionPassword once on submission. Display names
come from PlayerSettingsState.current; the forms do not own another name draft.
Submission is latched before issuing any start request to suppress repeated
clicks. A build without gns shows that multiplayer is unavailable and disables
start controls.

The pause menu separates the listen endpoint from invitation guidance. Wildcard
bind addresses are never invitation endpoints; loopback is labeled local-only.
Players need a reachable IP/port, with manual port forwarding where required.

## Frame order and canonical authority

PreUpdate polls `SecureTransport` once and processes events in order through
bootstrap. Only `BootstrapOutcome::Syncing` enters the sync router and only
`Gameplay` enters the host/client gameplay router. A live-connection set suppresses
messages after that connection's removal, including later messages in the same
batch. Backend Connecting expiry, bootstrap authentication/handoff expiry and
coordinator Syncing expiry are observed every frame. Only
joining connections receive bounded, rotating `HostSyncCoordinator::pump` work; Ready peers
have no baseline/catch-up pumping.
Bounded catch-up overflow schedules the existing restart/FIFO baseline path;
it does not leave a RestartRequired join waiting until timeout.

PostUpdate handles commands after existing egui, camera and piece input. With no
`NetworkSession`, the established `apply_piece_commands` and legacy snap
notification path operate directly; no sender, serialization or network poll
runs. Network mode gates those authority systems off. Host local controls use
`ProtocolDragContexts::apply_replicated`, immediate catch-up recording, and the
same Ready publication as remote controls. Clients only submit commands; canonical
gameplay changes come from authenticated Reliable authority events through
`ClientRouter` and `PeerReplicationState`. Transient remote updates affect only
replica presentation, never canonical positions.

## Local command bridge

The adapter encodes `Grab` / `GrabGroup` and unheld `Rotate` at Reliable control
boundaries using `ComponentRef`, `PieceTarget` and COW Dense masks. It retains a
bounded control queue (64) while waiting for the client's authority response.
`ReleaseGroup` maps to a context-based Release with exact sampled `final_delta`;
membership is not resent. Scalar legacy Grab/Move/Release producers are also
supported. Inputs continue to produce ordinary `ClientCommand` messages.

Control starts at zero and increments with checked arithmetic for the lifetime
of the session. Accepted Grab establishes its Move basis. Move tick starts at
zero and increases without wrapping; a committed RotateDrag changes the basis
and retains the through-tick high-water mark. Cancellation/rejection preserves
Control history. A new session constructs a new sender.

After controls, at most one DragUpdate is generated per frame from the local
presentation delta. Unchanged deltas and pending Reliable acknowledgements are
suppressed. This path reads scalar context only and does not scan membership,
rebuild masks or serialize Dense targets. Reliable Release is the final source
of correctness after lost Transient updates.

GrabAccepted replaces the current gesture's requested membership with the exact
authority-accepted membership. Dense membership stays a bitset. Empty acceptance
ends the preview and queued releases become no-ops. Every gesture has a token so
a delayed ACK cannot restore or rebase a later gesture selecting the same mask.
Rotation rebases only on committed DragRotationCommitted, using the pointer
sampled with that control. Movement made while waiting remains a residual delta;
queued releases/rotations sampled in the previous basis are adjusted once.
Rejected host controls retain the old pointer basis and consumed sequence history.
Remote Reliable route errors follow the existing close/resync contract.

### Remote Transient smoothing

Host and clients share the bounded remote presentation bridge. An accepted
Transient replaces only the latest target; the replica/authority context keeps
that exact network delta. CPU presentation uses `Time<Real>` in Last, after
network poll/commands and before upload preparation/extraction. Displayed delta
converges exponentially without prediction or extrapolation, over at most 64
slots. Packet ticks are sequence ordering, never a clock or sample interval.

Grab and reconciled Ready initialization set displayed and target to the current
context delta immediately. Reliable drag rotation rebases both to the new basis;
Release/Cancel discard the slot and all smoothing state immediately. Slot reuse,
scope/epoch changes and session teardown cannot retain old offsets. Local active
and pending-release feedback stay immediate. Tuning, exact-settle conditions and
the 512-byte active / zero-byte settled delta upload are documented in
[ARCHITECTURE.md](ARCHITECTURE.md#remote-drag-presentation).

### Local Release presentation while awaiting authority

Local Q/E feedback uses the separate sparse `PieceDataStore.local_rotation`
presentation. `CommandBridge` retains the exact predicted intent on its in-flight
control and the ordered queued controls, including gesture tokens. Reliable
reconciliation is **committed prefix + uncommitted predicted suffix**: ClientRouter
first applies canonical authority state; the bridge retires only the matching
control and replays the remaining suffix before upload/extraction. A partial ACK
never rolls accepted display pose back to an earlier local intent.

For RotateDrag, queued deltas use the existing ACK-only basis/pointer rebase.
Prediction composes a rotated base minus the translation consumed by speculative
controls with the current scalar local drag delta. Pointer movement stays immediate
without a membership scan. Release ends the pointer gesture but retains both the
rotation suffix and frozen pending translation; rotation ACKs rebase both together.
ReleaseCommitted hands off to the final canonical pose, allowing authority snap.

Partial Grab replaces optimistic membership with exact accepted members; partial
Rotate acceptance hands accepted components to canonical and corrects rejected
components. Z/ownership/nonrotation flags always come from the latest CPU state.
Failure/scope/session cleanup restores affected GPU ranges. Host controls apply
synchronously and offline controls still use immediate canonical authority.

| State | Position used for presentation | Canonical position |
| --- | --- | --- |
| Local active drag | Accepted local COW membership plus current pointer delta | Last Reliable authority commit |
| Local pending release | Same accepted membership plus frozen Release `final_delta` | Unchanged until `ReleaseCommitted` |
| Local uncommitted rotation | Sparse predicted position/rotation base composed with active/pending local translation | Last Reliable authority commit; never predicted |
| Remote transient drag | Existing piece-to-slot mapping plus displayed delta smoothed toward the latest accepted Transient target | Last Reliable authority commit |
| Canonical authority position | CPU `PieceDataStore.states` | Used for snap, placement, connectivity, progress and snapshots |

After sending Release, the client retains the final presentation, but canonical
position does not change until Reliable `ReleaseCommitted`. `CommandBridge`
retains one pending release delta/token and the GrabAccepted membership in its
local context. `PieceDataStore.drag` shares that accepted COW bitset for the
existing local renderer path, which keeps its priority over remote slots. The
original Grab request is never restored as pending membership, including partial
acceptance. A release queued before GrabAccepted waits for accepted membership;
empty acceptance removes the pending presentation and makes Release a no-op.

Pointer/camera movement cannot recompute a submitted Release delta. If Release
waits behind RotateDrag, its queued delta and pending presentation are adjusted
together at the rotation ACK; selecting the actual wire Release freezes the
adjusted `final_delta`. Existing through-tick, basis sequence and pointer-anchor
rebasing remain in use. No further DragUpdate is sent for a released context.

The pointer gesture ends at release. While its result is outstanding,
`PieceInteraction` suppresses new piece gestures and rotation picks; camera and
normal UI remain available. A queued control cannot overwrite the pending token,
delta or accepted mask. There is no speculative gesture queue. Pause/focus loss
does not send another Release or erase an already-pending offset.

For a local `ReleaseCommitted`, ClientRouter/PeerReplicationState first applies
the canonical release and verifies its result. Only then does the bridge remove
the pending local presentation, before Last/upload and renderer extraction. The
old delta is never added to the committed position. Without authority snap, the
handoff leaves the displayed position unchanged; authority snap remains visible
only when committed. Remote players' releases use the existing remote slot path.

Local DragCancelled, rejection, protocol/send failure, disconnect, session stop
and Menu cleanup remove pending presentation without predicting canonical
positions. New sessions, baseline/Ready installation and session/authority/store
scope invalidation reset it; a store reinstall within the same authority scope
preserves consumed Control sequence history. Late Transients cannot recreate the
released/cancelled context. Pending offsets are excluded from checkpoints, saves,
GameSnapshot and JoinBaseline canonical state. Host local Release applies
synchronously and leaves no pending offset, including semantic rejection.
Offline release continues to use immediate local authority without ACK state.

Pending idle frames share the same 125,000-byte bitset for 1M members and update
only scalar/Arc presentation state. They do not inspect canonical pieces, rebuild
membership or request canonical uploads. Wire version 9, snapshot schema 4,
JoinBaseline schema 1 and the 16-byte GpuPieceState are unchanged.

## Joining World and image lifecycle

Authenticated metadata constructs the client's authority session. Negotiated
definition is retained before the sync router releases it at Ready. The existing
baseline installer replaces the World store and replica, never LocalPlayerId.
Only ReadyCommit installs the assigned local `PlayerId` and authenticated host
identity into `LocalPlayerId` / `SessionHostId`; neither side assumes zero is local.

Verified image Bulk completion is moved to a bounded worker channel. Flattening
and the shared `decode_image_bytes` codec run on that worker, which never accesses
World or GPU resources. The join captures this client's GPU/settings cap on the
main thread. The worker preserves the common logical dimensions while resizing
the local texture before asset registration; cached and transferred originals
use the same policy. Host and client compare logical dimensions with the session
definition and retain the original encoded bytes/hash, independently of texture
resolution. Decoded images enter `Assets<Image>` on the main thread
and use the existing procedural renderer upload/RenderReady lifecycle. Protocol
Ready and image/GPU readiness remain distinct. The client enters InGame only
after Ready, baseline installation and decoded image availability. Initialization
continues at UploadingGpu without regenerating or replacing the canonical store.
The image dimensions must match the negotiated definition. Old image-selection
and persistence results are invalidated at join start. The previous puzzle's
preview/grid and save destination are cleared; the new puzzle gets a fresh
persistent game identity so a later save cannot overwrite the old checkpoint.

Host retained `OriginalPuzzleImage.encoded` bytes are hashed against the session
ImageHash before listening. A mismatching original identity is rejected. Missing
encoded bytes are explicitly `HostImageSource::Unavailable`; the runtime does not
read files synchronously. A verified-cache join can still proceed. Rehydrating
the host source from persistent storage is a separate task.

## Disconnect, failures and teardown

Before bootstrap/mapping removal, runtime retains the Ready player's identity.
Removing the live connection owns cleanup exactly once. An active replicated
drag generates an already-applied `DragCancelled`; runtime records it in catch-up
and publishes it to the remaining Ready peers. Duplicate closes do not advance
the authority cursor again. Syncing disconnects remove the join, transfer,
image/baseline waiters and slots, outbound Arc, finalization candidate and catch-up entry without a gameplay cancellation.

Publication attempts every Ready peer. Each failed secure send closes that
connection and enters the same disconnect coordinator. Publication uses the
already-applied outcome; it never reapplies the originating command. Catch-up
retention failure is reported separately and cannot suppress Ready publication.
Client host loss (including ConnectionFailed before Connected), protocol errors,
decode failure and sync timeout end the session safely, without host migration.
A failed session gates offline authority until Menu cleanup.

Stop/Menu closes listener and connections, destroys transport channels, passwords,
bootstrap, sync state, mappings, contexts/replica, worker receiver and sender.
Outstanding holds/preview are cleared without snapping. Intent messages are
cleared and normal Menu cleanup resets offline identity. Late worker replies have
no receiver in a later session.

## Regression coverage and validation

`game/src/network/runtime/tests.rs` uses scheduled Bevy Worlds and injected secure
links for offline authority, client authority-only roundtrip, nonzero identity,
lost Transient Release convergence, host commands during baseline catch-up,
partial Grab presentation, asynchronous rotation/release rebasing, disconnect
cancellation, Syncing cleanup, send failure isolation and session reset.
Bridge exhaustion tests cover checked Control/Move counters. The `gns` test
`gns_localhost_runtime_entrypoints_join_ready_and_command_roundtrip` starts real
localhost host/join through runtime APIs, transfers/decodes an actual PNG, promotes
Ready, and runs Grab/Release through scheduled commands. It is selected by the
existing serial GNS CI test filter.

`runtime/tests/pending_release_tests.rs` delays encrypted host-to-client Control
for several scheduled frames, with and without all Transients dropped. It covers
continuous Release handoff before upload, frozen pointer movement, suppressed
new gestures, partial acceptance, queued/rebased rotations, failure/teardown,
host rejection and offline release. `runtime/bridge_tests.rs` checks local-only
cancellation, scope/token invalidation and 1M-member pending frames: the same mask
allocation is retained, a test guard forbids canonical piece-state access, and
state/root upload revisions remain unchanged.

The repository's GitHub Actions workflow covers fmt, Clippy, tests/doctests and
builds on Windows/Linux with default and all features, plus serial localhost GNS
tests. Local validation follows repository and task-specific instructions.
No runtime FPS or cross-GPU/OS bit identity is claimed by these tests.
Remote drag GPU rendering uses the existing bounded slot presentation path; see
[ARCHITECTURE.md](ARCHITECTURE.md#remote-drag-presentation).

Remote smoothing adds four math regressions in `resources/remote_drag.rs` for
equal elapsed time at different frame divisions, burst/latest-target reversal,
no overshoot, exact settling/hitches and Reliable rebase/reset. Its 1M mapping
regression runs 9,999 accepted target/frame updates under the canonical-access
guard, preserving mapping allocation, membership Arc, mapping revision and
upload ranges. Runtime Worlds verify host/peer smoothing before Last upload,
unchanged canonical/replica state, real-time advance while virtual time is paused,
lagged rotation/release/cancellation and slot reuse. Final-reconciliation tests
also cover exact Ready initialization, duplicate/stale/old-basis/late drops and
scope reset. The GPU regression now checks intermediate display pixels and
readback deltas, active 512-byte and settled zero-byte uploads.

2026-10-04 Windows validation for remote smoothing: release workspace default
tests (716 including doctests), all-feature tests (733 including all localhost
GNS tests and doctests), all-feature Clippy with `-D warnings`, default workspace
check, fmt and the extended real-GPU remote presentation test passed. The native
all-feature test link emitted its existing library-export notice and non-fatal
Tracy `SymInitialize` diagnostic; profiler symbol resolution was not validated.

2026-10-04 initial Windows validation for pending local Release: workspace default tests
(691 including doctests), all-feature tests (692 including doctests, excluding
the 14 serial GNS localhost tests), all 14 GNS localhost tests, default/all-feature
Clippy with `-D warnings`, fmt and the 33 release-mode runtime tests passed.
The 1M pending regression ran in release mode without canonical piece access,
membership replacement or state/root upload changes. Existing real-GPU release
tests `gpu_remote_presentation_normal_far_culling_picking_and_scalar_uploads` and
`gpu_drag_transform_and_preview_without_readback` also passed.
The all-feature native link used the command-local GNS `out/lib` search path
described in [WINDOWS_BUILD.md](WINDOWS_BUILD.md), retaining shared Cargo/vcpkg
directories. All-feature startup emitted a non-fatal Tracy `SymInitialize`
diagnostic; this validation does not verify profiler symbol resolution.
After integration with the bounded join lifecycle, the default workspace suite
(711 including doctests), all-feature Clippy with `-D warnings` and fmt passed again.

## Join resource policy

`network/lifecycle.rs` centralizes explicit-clock policy. Connections: 64 total,
32 pending Ready, 16 Connecting; bootstrap authentication: 32 pending; joins: 12
concurrent, separate from Ready capacity. Canonical-IP pending quota is 4. Connecting
and authentication each expire after 10 s in their own owner. Authenticated has a
fresh bootstrap-owned 5 s handoff; runtime starts sync in the same event handler.

Host sync owns independent two-slot image and baseline FIFOs. Waiting peers retain
no serialized payload/chunks or new catch-up history. Session/availability, offer
acceptance, completion ACK, catch-up ACK and FinalizeAck wait at most 12 s; host slot
waits use the global bound. Bulk preflight checks native pending + unacknowledged
reliable bytes before generation. Bulk queue threshold is 240 KiB/connection,
all reliable egress 512 KiB; host generation is 128 KiB/frame and 4 MiB/s, with
rotating peer priority. The immutable session image digest is verified once and
reused; generic Bulk retains content hashing.

Transfer progress requires observed delivery, with 30 s idle/grace and a 128 KiB/s
policy floor. Size-derived transfer budgets are credited once per kind on top of
the 300 s sync start-clock limit; restarting never renews it. ACK waits start after
native drain, not Finish-enqueue. Duplicate/obsolete messages and host sends do not
renew deadlines. Join admission has global (12 burst, one/2 s) and origin (4 burst,
one/5 s) guards independent of authentication. Three abusive failures trigger a
30 s cooldown; histories have 256-entry capacity and 120 s TTL. Host-capacity expiry
is distinguished and does not penalize the peer.

Sync expiration releases channels/bootstrap/mappings, live/joining state, transfer
Arcs, both FIFOs/slots, catch-up retention and candidates while host gameplay
continues. Client failures invalidate receiver storage immediately. See the
[complete ownership/resource table](NETWORK_TRANSPORT.md#bounded-connection-and-join-lifecycle)
for release conditions and the meaning of progress.

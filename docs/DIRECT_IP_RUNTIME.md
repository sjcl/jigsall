# Direct-IP game runtime

`GamePlugin` installs `NetworkRuntimePlugin`. A main-thread `NetworkSession` owns
one secured transport, bootstrap, sync coordinator/router, Ready connections,
authority session, host drag contexts or client replica, and local command sender.
It borrows the existing World `PieceDataStore`; there is no second store or
per-piece network Entity. The wire version remains 8, snapshot schema 4, and join
baseline schema 1.

## Programmatic entrypoints

Under `gns`, `network::runtime::start_host(&mut World, HostOptions)` hosts the
current puzzle/store and returns the actual listener address (including an
ephemeral port). `start_join(&mut World, JoinOptions)` starts a connection and
advances through scheduled frames. `stop_session(&mut World)` closes the session
and requests Menu. `host_with_transport` / `join_with_transport` support an
injected `DirectIpTransport` without GNS, including headless World tests.

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
state. UI need not access native sockets, bootstrap, or sync internals. No Host/Join
screen, server browser, lobby, NAT traversal, cursor/name rendering or migration
is added.

## Frame order and canonical authority

PreUpdate polls `SecureTransport` once and processes events in order through
bootstrap. Only `BootstrapOutcome::Syncing` enters the sync router and only
`Gameplay` enters the host/client gameplay router. A live-connection set suppresses
messages after that connection's removal, including later messages in the same
batch. Authentication expiry and Syncing timeouts are observed every frame. Only
joining connections receive bounded `HostSyncCoordinator::pump` work; Ready peers
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

## Joining World and image lifecycle

Authenticated metadata constructs the client's authority session. Negotiated
definition is retained before the sync router releases it at Ready. The existing
baseline installer replaces the World store and replica, never LocalPlayerId.
Only ReadyCommit installs the assigned local `PlayerId` and authenticated host
identity into `LocalPlayerId` / `SessionHostId`; neither side assumes zero is local.

Verified image Bulk completion is moved to a bounded worker channel. Flattening
and the shared `decode_image_bytes` codec run on that worker, which never accesses
World or GPU resources. Decoded images enter `Assets<Image>` on the main thread
and use the existing procedural renderer upload/RenderReady lifecycle. Protocol
Ready and image/GPU readiness remain distinct. The client enters InGame only
after Ready, baseline installation and decoded image availability. Initialization
continues at UploadingGpu without regenerating or replacing the canonical store.
The image dimensions must match the negotiated definition. Old image-selection
and persistence results are invalidated at join start.

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
baseline waiter/slot and catch-up entry without a gameplay cancellation.

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

Normal fmt checks, Clippy, test/doctest and builds run in GitHub Actions under
AGENTS.md. No runtime FPS or cross-GPU/OS bit identity is claimed by these tests.
Remote drag GPU rendering remains a follow-up.

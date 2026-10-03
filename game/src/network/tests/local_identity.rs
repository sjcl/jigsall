use super::*;
use crate::resources::pieces::DragTransform;
use std::sync::Arc;

#[test]
fn routers_replay_own_42_and_remote_zero_grab_and_release_on_sparse_and_dense_paths() {
    let local = PlayerId(42);
    let remote = PlayerId(0);
    for dense in [false, true] {
        for player in [local, remote] {
            // Explicit host presentation identity must not be inferred from HOST.
            for host_local in [local, PlayerId(77)] {
                let definition = PuzzleDefinition {
                    generator_version: GENERATOR_VERSION,
                    seed: 42,
                    grid_size: UVec2::new(96, 1),
                    image_size: UVec2::new(1920, 20),
                    snap_distance: 5.0,
                };
                let mut host_store = PieceDataStore::default();
                host_store.initialize(
                    (0..96)
                        .map(|id| definition.correct_position(PieceId(id)) + Vec2::splat(1000.0))
                        .collect(),
                );
                host_store.connectivity.union(PieceId(0), PieceId(1));
                let cursor = AuthorityCursor::new(3, 0);
                let snapshot =
                    GameSnapshot::capture(&host_store, &definition, SESSION, cursor).unwrap();
                let mut peer_store = PieceDataStore::default();
                snapshot
                    .install(
                        &mut peer_store,
                        SnapshotExpectation {
                            session: SESSION.id,
                            image_hash: SESSION.image_hash,
                            cursor,
                            definition: &definition,
                        },
                    )
                    .unwrap();
                snapshot
                    .install(
                        &mut host_store,
                        SnapshotExpectation {
                            session: SESSION.id,
                            image_hash: SESSION.image_hash,
                            cursor,
                            definition: &definition,
                        },
                    )
                    .unwrap();
                let count = if dense { 8 } else { 2 };
                let mut members = PieceBitSet::new(96);
                members.extend((0..count).map(PieceId));
                for store in [&mut host_store, &mut peer_store] {
                    store.selected_pieces = members.clone();
                    store.drag = DragTransform {
                        members: members.words().clone(),
                        delta: Vec2::new(7.0, 9.0),
                    };
                }
                let target = if dense {
                    PieceTarget::Dense(
                        DenseTarget::from_selection(&host_store.connectivity, &members).unwrap(),
                    )
                } else {
                    PieceTarget::from_selection(&host_store.connectivity, &members).unwrap()
                };
                let mut host_session = AuthoritySession::new(SESSION, HOST, cursor);
                let mut peer_session = AuthoritySession::new(SESSION, HOST, cursor);
                let mut host_connections = SessionConnections::default();
                let mut peer_connections = SessionConnections::default();
                connected(&mut host_connections, HA, player);
                connected(&mut peer_connections, HA, HOST);
                let mut contexts = ProtocolDragContexts::default();
                let mut replica = PeerReplicationState::default();
                let mut host = HostRouter {
                    local_player: host_local,
                    connections: &host_connections,
                    contexts: &mut contexts,
                    session: &mut host_session,
                    store: &mut host_store,
                    definition: Some(&definition),
                };
                let grab = ProtocolCommandEnvelope {
                    session: SESSION.id,
                    authority_epoch: cursor.epoch,
                    player,
                    sequence: ClientCommandSequence::Control(0),
                    command: ProtocolPieceCommand::Grab { target },
                };
                let HostRouteOutcome::Applied(outcome) = host
                    .route(&message_event(HA, &WireMessage::ClientCommand(grab)))
                    .unwrap()
                else {
                    panic!()
                };
                let event = outcome.authority_event.unwrap();
                // Exercise the legacy no-snap replay independently of Grab validation.
                host.definition = None;
                assert!(
                    matches!(&event.event, ProtocolAuthorityEvent::GrabAccepted(ack) if ack.player == player)
                );
                assert_eq!(host.store.selected_pieces.is_empty(), player != host_local);
                assert_eq!(
                    host.store.drag.members.iter().all(|&word| word == 0),
                    player != host_local
                );
                let mut client = ClientRouter {
                    local_player: local,
                    host_connection: HA,
                    connections: &peer_connections,
                    replica: &mut replica,
                    session: &mut peer_session,
                    store: &mut peer_store,
                    definition: None,
                };
                let ClientRouteOutcome::Authority(applied) = client
                    .route(&message_event(HA, &WireMessage::AuthorityEvent(event)))
                    .unwrap()
                else {
                    panic!()
                };
                assert_eq!(applied.grabbed, count as usize);
                for id in members.iter() {
                    assert_eq!(client.store.held_by.get(&id), Some(&player));
                    assert_eq!(client.store.selected_pieces.contains(&id), player == local);
                }
                if player == local {
                    assert!(Arc::ptr_eq(&client.store.drag.members, members.words()));
                    assert_eq!(client.store.drag.delta, Vec2::new(7.0, 9.0));
                } else {
                    assert!(client.store.drag.members.iter().all(|&word| word == 0));
                }
                // Existing RemoteDragUpdate semantics retain every accepted context,
                // including our own identity, rather than filtering player 42 here.
                assert!(client
                    .replica
                    .remote_drag(client.session, client.store, player)
                    .is_some());
                // A remote release must not clear this independent local cache.
                client.store.drag.members = members.words().clone();
                let release = ProtocolCommandEnvelope {
                    session: SESSION.id,
                    authority_epoch: cursor.epoch,
                    player,
                    sequence: ClientCommandSequence::Control(1),
                    command: ProtocolPieceCommand::Release {
                        grab_sequence: 0,
                        final_delta: Vec2::new(3.0, 4.0),
                    },
                };
                let HostRouteOutcome::Applied(outcome) = host
                    .route(&message_event(HA, &WireMessage::ClientCommand(release)))
                    .unwrap()
                else {
                    panic!()
                };
                let event = outcome.authority_event.unwrap();
                assert!(
                    matches!(&event.event, ProtocolAuthorityEvent::ReleaseCommitted(commit) if commit.player == player)
                );
                let ClientRouteOutcome::Authority(applied) = client
                    .route(&message_event(HA, &WireMessage::AuthorityEvent(event)))
                    .unwrap()
                else {
                    panic!()
                };
                assert_eq!(applied.released, count as usize);
                assert!(client.store.held_by.is_empty());
                assert_eq!(
                    client.store.drag.members.iter().all(|&word| word == 0),
                    player == local
                );
                assert_eq!(client.store.states, host.store.states);
                assert_eq!(client.session.cursor(), host.session.cursor());
                assert!(client
                    .replica
                    .remote_drag(client.session, client.store, player)
                    .is_none());
            }
        }
    }
}

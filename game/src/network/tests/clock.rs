use super::*;

#[test]
fn ordered_clock_updates_require_authenticated_host_and_scope_without_advancing_cursors() {
    let mut store = PieceDataStore::default();
    store.initialize(vec![Vec2::ZERO]);
    let mut session = AuthoritySession::new(SESSION, HOST, AuthorityCursor::new(3, 0));
    let mut replica = PeerReplicationState::default();
    let update = GameClockUpdate {
        session: SESSION.id,
        authority_epoch: AuthorityEpoch(3),
        elapsed_millis: 12_345,
    };
    let cursor = session.cursor();
    assert_eq!(
        replica.apply_clock_update(&session, &mut store, A, &update),
        Err(crate::multiplayer::replication::ReplicationError::Protocol(
            ProtocolError::WrongHost
        ))
    );
    assert!(store.elapsed_play_time.is_zero());
    replica
        .apply_clock_update(&session, &mut store, HOST, &update)
        .unwrap();
    replica
        .apply_clock_update(
            &session,
            &mut store,
            HOST,
            &GameClockUpdate {
                elapsed_millis: 99_999,
                ..update.clone()
            },
        )
        .unwrap();
    assert_eq!(
        store.elapsed_play_time,
        std::time::Duration::from_millis(99_999)
    );
    assert_eq!(session.cursor(), cursor);
    let wrong_session = GameClockUpdate {
        session: SessionId(999),
        ..update.clone()
    };
    assert_eq!(
        replica.apply_clock_update(&session, &mut store, HOST, &wrong_session),
        Err(crate::multiplayer::replication::ReplicationError::Protocol(
            ProtocolError::WrongSession
        ))
    );
    let wrong_epoch = GameClockUpdate {
        authority_epoch: AuthorityEpoch(4),
        ..update.clone()
    };
    assert_eq!(
        replica.apply_clock_update(&session, &mut store, HOST, &wrong_epoch),
        Err(crate::multiplayer::replication::ReplicationError::Protocol(
            ProtocolError::WrongEpoch
        ))
    );
    let mut connections = SessionConnections::default();
    connected(&mut connections, HA, A);
    let mut contexts = ProtocolDragContexts::default();
    let mut host = HostRouter {
        local_player: HOST,
        connections: &connections,
        contexts: &mut contexts,
        session: &mut session,
        store: &mut store,
        definition: None,
    };
    assert_eq!(
        host.route(&message_event(HA, &WireMessage::GameClock(update)))
            .unwrap_err(),
        HostRouteError::WrongDirection
    );
    assert_eq!(
        host.store.elapsed_play_time,
        std::time::Duration::from_millis(99_999)
    );
}

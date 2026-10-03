use super::*;
use crate::{multiplayer::replication::ReplicationError, network::transient::*};

#[test]
fn ready_client_drops_lane_reordered_duplicate_and_stale_transients() {
    for cancel in [false, true] {
        let mut s = Scenario::new();
        connected(&mut s.host.connections, HA, A);
        connected(&mut s.peers[0].connections, HA, HOST);
        let HostRouteOutcome::Applied(grabbed) = s
            .host_router()
            .route(&message_event(HA, &WireMessage::ClientCommand(grab())))
            .unwrap()
        else {
            unreachable!()
        };
        let HostRouteOutcome::Applied(moved) = s
            .host_router()
            .route(&message_event(HA, &WireMessage::ClientCommand(update())))
            .unwrap()
        else {
            unreachable!()
        };
        let mut drag = moved.drag_update.unwrap();
        let route = |s: &mut Scenario, update: &RemoteDragUpdate| {
            s.client_router(0, HA)
                .route(&message_event(HA, &WireMessage::DragUpdate(update.clone())))
        };
        assert!(matches!(
            route(&mut s, &drag).unwrap(),
            ClientRouteOutcome::DroppedTransient(TransientDrop::MissingDragContext)
        ));
        s.client_router(0, HA)
            .route(&message_event(
                HA,
                &WireMessage::AuthorityEvent(grabbed.authority_event.unwrap()),
            ))
            .unwrap();
        drag.tick = 4;
        assert!(matches!(
            route(&mut s, &drag).unwrap(),
            ClientRouteOutcome::Drag(CommandSequenceStatus::Gap { .. })
        ));
        assert!(matches!(
            route(&mut s, &drag).unwrap(),
            ClientRouteOutcome::DroppedTransient(TransientDrop::DuplicateUpdate)
        ));
        drag.tick = 3;
        assert!(matches!(
            route(&mut s, &drag).unwrap(),
            ClientRouteOutcome::DroppedTransient(TransientDrop::StaleUpdate)
        ));
        let rotate = command(
            ClientCommandSequence::Control(1),
            ProtocolPieceCommand::RotateDrag {
                grab_sequence: 0,
                final_delta: Vec2::ZERO,
                through_tick: Some(4),
                quarter_turns: 1,
            },
        );
        let HostRouteOutcome::Applied(rotated) = s
            .host_router()
            .route(&message_event(HA, &WireMessage::ClientCommand(rotate)))
            .unwrap()
        else {
            unreachable!()
        };
        drag.basis_sequence = 1;
        drag.tick = 5;
        assert!(matches!(
            route(&mut s, &drag).unwrap(),
            ClientRouteOutcome::DroppedTransient(TransientDrop::WrongDragContext)
        ));
        s.client_router(0, HA)
            .route(&message_event(
                HA,
                &WireMessage::AuthorityEvent(rotated.authority_event.unwrap()),
            ))
            .unwrap();
        assert!(matches!(
            route(&mut s, &drag).unwrap(),
            ClientRouteOutcome::Drag(_)
        ));
        let event = if cancel {
            s.contexts
                .cancel_replicated(&mut s.host.session, &mut s.host.store, A)
                .unwrap()
                .unwrap()
                .authority_event
        } else {
            let mut released = release();
            released.sequence = ClientCommandSequence::Control(2);
            let HostRouteOutcome::Applied(released) = s
                .host_router()
                .route(&message_event(HA, &WireMessage::ClientCommand(released)))
                .unwrap()
            else {
                unreachable!()
            };
            released.authority_event.unwrap()
        };
        s.client_router(0, HA)
            .route(&message_event(HA, &WireMessage::AuthorityEvent(event)))
            .unwrap();
        let cursor = s.peers[0].session.cursor();
        assert!(matches!(
            route(&mut s, &drag).unwrap(),
            ClientRouteOutcome::DroppedTransient(TransientDrop::MissingDragContext)
        ));
        assert_eq!(s.peers[0].session.cursor(), cursor);
        assert!(!s.peers[0]
            .replica
            .needs_resync(&s.peers[0].session, &s.peers[0].store));
    }
}

#[test]
fn benign_classification_does_not_hide_invalid_identity_delta_or_reliable_divergence() {
    let mut s = Scenario::new();
    connected(&mut s.host.connections, HA, A);
    connected(&mut s.peers[0].connections, HA, HOST);
    let mut drag = RemoteDragUpdate {
        session: SESSION.id,
        authority_epoch: AuthorityEpoch(3),
        player: A,
        grab_sequence: 0,
        basis_sequence: 0,
        tick: 0,
        delta: Vec2::ZERO,
    };
    for (session, epoch, delta, error) in [
        (
            SessionId(999),
            AuthorityEpoch(3),
            Vec2::ZERO,
            ReplicationError::Protocol(ProtocolError::WrongSession),
        ),
        (
            SESSION.id,
            AuthorityEpoch(4),
            Vec2::ZERO,
            ReplicationError::Protocol(ProtocolError::WrongEpoch),
        ),
        (
            SESSION.id,
            AuthorityEpoch(3),
            Vec2::splat(f32::NAN),
            ReplicationError::InvalidDelta,
        ),
    ] {
        drag.session = session;
        drag.authority_epoch = epoch;
        drag.delta = delta;
        assert_eq!(
            s.client_router(0, HA)
                .route(&message_event(HA, &WireMessage::DragUpdate(drag.clone())))
                .unwrap_err(),
            ClientRouteError::Replication(error)
        );
    }
    for error in [
        ReplicationError::Diverged,
        ReplicationError::InvalidDelta,
        ReplicationError::Protocol(ProtocolError::WrongHost),
        ReplicationError::Protocol(ProtocolError::EventGap {
            expected: AuthoritySequence(2),
        }),
    ] {
        assert_eq!(replication_drop(&error), None);
    }
    // A Reliable event with missing context remains an error, even though the
    // same missing-context error on a Transient message is a benign drop.
    let cancel = ProtocolAuthorityEventEnvelope {
        session: SESSION.id,
        host: HOST,
        cursor: AuthorityCursor::new(3, 1),
        event: ProtocolAuthorityEvent::DragCancelled(DragCancelled {
            player: A,
            grab_sequence: 0,
        }),
    };
    assert_eq!(
        s.client_router(0, HA)
            .route(&message_event(HA, &WireMessage::AuthorityEvent(cancel)))
            .unwrap_err(),
        ClientRouteError::Replication(ReplicationError::MissingDragContext)
    );
    let mut invalid = update();
    invalid.command = ProtocolPieceCommand::DragUpdate {
        delta: Vec2::splat(f32::NAN),
    };
    assert_eq!(
        s.host_router()
            .route(&message_event(HA, &WireMessage::ClientCommand(invalid)))
            .unwrap_err(),
        HostRouteError::Command(ProtocolCommandError::InvalidDelta)
    );
    assert!(matches!(
        s.host_router()
            .route(&message_event(HA, &WireMessage::ClientCommand(update())))
            .unwrap(),
        HostRouteOutcome::DroppedTransient(_)
    ));
    assert!(s
        .host_router()
        .route(&message_event(HA, &WireMessage::ClientCommand(release())))
        .is_err());
}

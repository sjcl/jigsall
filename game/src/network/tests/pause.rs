use super::*;
use crate::multiplayer::protocol::ProtocolCommandResult;

#[test]
fn paused_authority_refuses_every_wire_operation_and_rejects_client_resume() {
    let definition = PuzzleDefinition {
        generator_version: GENERATOR_VERSION,
        seed: 42,
        grid_size: UVec2::new(2, 1),
        image_size: UVec2::new(200, 100),
        snap_distance: 5.0,
        rotation_enabled: true,
    };
    let mut store = PieceDataStore::default();
    store.initialize(vec![Vec2::splat(100.0), Vec2::splat(200.0)]);
    let mut session = AuthoritySession::new(SESSION, HOST, AuthorityCursor::new(3, 0));
    let mut connections = SessionConnections::default();
    connected(&mut connections, HA, A);
    let mut contexts = ProtocolDragContexts::default();
    let mut host = HostRouter {
        local_player: HOST,
        connections: &connections,
        contexts: &mut contexts,
        session: &mut session,
        store: &mut store,
        definition: Some(&definition),
    };
    host.route(&message_event(HA, &WireMessage::ClientCommand(grab())))
        .unwrap();
    host.contexts
        .set_paused(host.session, host.store, true)
        .unwrap();
    assert!(host.store.held_by.is_empty());
    let states = host.store.states.clone();
    let next_z = host.store.next_z_order;
    let target = PieceTarget::Component(ComponentRef {
        member: PieceId(0),
        expected_size: 1,
    });
    let commands = [
        update(),
        release(),
        command(
            ClientCommandSequence::Control(2),
            ProtocolPieceCommand::Grab {
                target: target.clone(),
            },
        ),
        command(
            ClientCommandSequence::Control(3),
            ProtocolPieceCommand::Rotate {
                target,
                quarter_turns: 1,
            },
        ),
        command(
            ClientCommandSequence::Control(4),
            ProtocolPieceCommand::RotateDrag {
                grab_sequence: 0,
                final_delta: Vec2::splat(-100.0),
                through_tick: None,
                quarter_turns: 1,
            },
        ),
    ];
    for envelope in commands {
        let sequence = envelope.sequence;
        let HostRouteOutcome::Rejected(outcome) = host
            .route(&message_event(HA, &WireMessage::ClientCommand(envelope)))
            .unwrap()
        else {
            panic!("paused command applied");
        };
        assert_eq!(outcome.result, ProtocolCommandResult::Paused);
        assert!(outcome.drag_update.is_none());
        match sequence {
            ClientCommandSequence::Control(sequence) => {
                assert!(matches!(outcome.authority_event.unwrap().event,
                ProtocolAuthorityEvent::CommandRejected(CommandRejected { player: A, sequence: rejected }) if rejected == sequence))
            }
            ClientCommandSequence::Move { .. } => assert!(outcome.authority_event.is_none()),
        }
        assert_eq!(host.store.states, states);
        assert_eq!(host.store.next_z_order, next_z);
        assert_eq!(host.store.placed_count, 0);
        assert!(host.store.held_by.is_empty());
        assert!(!host
            .store
            .connectivity
            .same_component(PieceId(0), PieceId(1)));
    }
    let forged_resume = ProtocolAuthorityEventEnvelope {
        session: SESSION.id,
        host: HOST,
        cursor: host.session.cursor(),
        event: ProtocolAuthorityEvent::PauseChanged(PauseChanged { paused: false }),
    };
    assert_eq!(
        host.route(&message_event(
            HA,
            &WireMessage::AuthorityEvent(forged_resume)
        ))
        .unwrap_err(),
        HostRouteError::WrongDirection
    );
    let mut spoof = grab();
    spoof.player = HOST;
    spoof.sequence = ClientCommandSequence::Control(5);
    assert_eq!(
        host.route(&message_event(HA, &WireMessage::ClientCommand(spoof)))
            .unwrap_err(),
        HostRouteError::Command(ProtocolCommandError::WrongPlayer)
    );
    assert!(host.session.gameplay_paused());
    host.contexts
        .set_paused(host.session, host.store, false)
        .unwrap();
    let mut resumed = grab();
    resumed.sequence = ClientCommandSequence::Control(5);
    assert!(matches!(
        host.route(&message_event(HA, &WireMessage::ClientCommand(resumed)))
            .unwrap(),
        HostRouteOutcome::Applied(_)
    ));
    assert_eq!(host.store.held_by.get(&PieceId(0)), Some(&A));
}

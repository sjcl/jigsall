use super::*;

fn envelope(
    peer: &Endpoint,
    sequence: ClientCommandSequence,
    command: ProtocolPieceCommand,
) -> ProtocolCommandEnvelope {
    let session = peer.runtime.runtime.session.as_ref().unwrap();
    ProtocolCommandEnvelope {
        session: session.session_id(),
        authority_epoch: session.cursor().epoch,
        player: peer.runtime.runtime.status.local_player.unwrap(),
        sequence,
        command,
    }
}
fn grab(peer: &Endpoint) -> ProtocolCommandEnvelope {
    envelope(
        peer,
        ClientCommandSequence::Control(0),
        ProtocolPieceCommand::Grab {
            target: PieceTarget::Component(ComponentRef {
                member: PieceId(0),
                expected_size: 1,
            }),
        },
    )
}
fn send_bytes(peer: &mut Endpoint, class: MessageClass, bytes: &[u8]) {
    let Role::Client(client) = &peer.runtime.runtime.role else {
        unreachable!()
    };
    let id = client.bootstrap.host_connection();
    peer.runtime
        .runtime
        .transport
        .send(id, class, bytes)
        .unwrap();
}
fn send_command(peer: &mut Endpoint, command: ProtocolCommandEnvelope) {
    let message = WireMessage::ClientCommand(command);
    send_bytes(peer, message.class(), &wire::encode(&message).unwrap());
}

#[test]
fn malformed_identity_numeric_and_reliable_gap_disconnect_only_the_offending_peer() {
    for case in 0..4 {
        let bus = Arc::new(Mutex::new(Bus::default()));
        let mut host = Endpoint::host(&bus);
        let mut a = Endpoint::client(&bus, 1);
        let mut b = Endpoint::client(&bus, 2);
        join(&mut host, &mut [&mut a, &mut b]);
        a.poll();
        b.poll();
        let player = a.runtime.runtime.status.local_player.unwrap();
        let id = host.connection(player);
        let command = grab(&a);
        send_command(&mut a, command);
        host.poll();
        b.poll();
        let before = host.runtime.runtime.session.as_ref().unwrap().cursor();
        match case {
            0 => send_bytes(&mut a, MessageClass::Control, &[0; 12]),
            1 => {
                let mut command = grab(&a);
                command.player = b.runtime.runtime.status.local_player.unwrap();
                command.sequence = ClientCommandSequence::Control(1);
                send_command(&mut a, command);
            }
            2 => {
                let command = envelope(
                    &a,
                    ClientCommandSequence::Move {
                        after_control_sequence: 0,
                        tick: 0,
                    },
                    ProtocolPieceCommand::DragUpdate {
                        delta: Vec2::splat(f32::NAN),
                    },
                );
                send_command(&mut a, command);
            }
            3 => {
                let command = envelope(
                    &a,
                    ClientCommandSequence::Control(2),
                    ProtocolPieceCommand::Release {
                        grab_sequence: 0,
                        final_delta: Vec2::ZERO,
                    },
                );
                send_command(&mut a, command);
            }
            _ => unreachable!(),
        }
        host.poll();
        b.poll();
        assert!(!host.runtime.runtime.live.contains(&id));
        assert_eq!(host.runtime.runtime.connections.player(id), None);
        assert!(!host.store.held_by.has_player(player));
        assert_eq!(host.roster(), b.roster());
        assert_eq!(host.store.states, b.store.states);
        assert_eq!(
            host.runtime
                .runtime
                .session
                .as_ref()
                .unwrap()
                .cursor()
                .sequence
                .0,
            before.sequence.0 + 1,
            "one cancellation after rejecting the invalid command"
        );
        assert_eq!(host.runtime.runtime.status.phase, RuntimePhase::Hosting);
        assert_eq!(host.runtime.runtime.status.failure, None);
        assert!(host.runtime.runtime.active);
        let revision = host.roster().revision;
        bus.lock()
            .unwrap()
            .inbox
            .entry(0)
            .or_default()
            .push_back(TransportEvent::Disconnected {
                connection: id,
                reason: DisconnectReason::RemoteClosed,
            });
        host.poll();
        assert_eq!(
            host.roster().revision,
            revision,
            "backend close cannot repeat cleanup"
        );
    }
}

#[test]
fn invalid_host_definition_uses_backend_failure_and_preserves_other_peer() {
    let bus = Arc::new(Mutex::new(Bus::default()));
    let mut host = Endpoint::host(&bus);
    let mut a = Endpoint::client(&bus, 1);
    let mut b = Endpoint::client(&bus, 2);
    join(&mut host, &mut [&mut a, &mut b]);
    a.poll();
    b.poll();
    let bid = host.connection(b.runtime.runtime.status.local_player.unwrap());
    host.runtime.runtime.definition.as_mut().unwrap().grid_size = UVec2::ZERO;
    let command = grab(&a);
    send_command(&mut a, command);
    host.poll();
    assert_eq!(
        host.runtime.runtime.status.failure,
        Some(NetworkFailureKind::Internal)
    );
    assert_eq!(host.runtime.runtime.status.phase, RuntimePhase::Hosting);
    assert!(host.runtime.runtime.live.contains(&bid));
    assert!(bus
        .lock()
        .unwrap()
        .inbox
        .get(&1)
        .unwrap()
        .iter()
        .any(|event| matches!(
            event,
            TransportEvent::Disconnected {
                reason: DisconnectReason::BackendFailure,
                ..
            }
        )));
}

#[test]
fn internal_cancellation_fault_still_removes_roster_and_drains_all_pending_disconnects() {
    let bus = Arc::new(Mutex::new(Bus::default()));
    let mut host = Endpoint::host(&bus);
    let mut a = Endpoint::client(&bus, 1);
    let mut b = Endpoint::client(&bus, 2);
    join(&mut host, &mut [&mut a, &mut b]);
    let player = a.runtime.runtime.status.local_player.unwrap();
    let aid = host.connection(player);
    let bid = host.connection(b.runtime.runtime.status.local_player.unwrap());
    // Orphan authority holds cannot generate a safe replicated cancellation.
    host.store.apply_command(
        player,
        &PieceCommand::Grab(PieceId(0)),
        host.runtime.runtime.definition.as_ref(),
        PlayerId(0),
    );
    let failure = host
        .runtime
        .runtime
        .disconnect_many(
            vec![
                (bid, DisconnectReason::RemoteClosed, None),
                (aid, DisconnectReason::RemoteClosed, None),
            ],
            &mut host.store,
        )
        .unwrap_err();
    assert_eq!(failure.kind, NetworkFailureKind::Internal);
    assert!(host.runtime.runtime.live.is_empty());
    assert_eq!(host.runtime.runtime.connections.peers().count(), 0);
    assert_eq!(host.roster().players.len(), 1);
    assert_eq!(host.roster().revision, 4);
    host.runtime
        .disconnect(
            aid,
            DisconnectReason::RemoteClosed,
            Some(player),
            &mut host.store,
        )
        .unwrap();
    assert_eq!(host.roster().revision, 4);
}

#[test]
fn missed_retention_restarts_join_but_publishes_the_applied_command_once_to_ready_peer() {
    let bus = Arc::new(Mutex::new(Bus::default()));
    let mut host = Endpoint::host(&bus);
    let mut a = Endpoint::client(&bus, 1);
    let mut b = Endpoint::client(&bus, 2);
    join(&mut host, &mut [&mut a, &mut b]);
    a.poll();
    b.poll();
    let player = a.runtime.runtime.status.local_player.unwrap();
    let aid = host.connection(player);
    let command = grab(&a);
    send_command(&mut a, command);
    host.poll();
    b.poll();
    let mut c = Endpoint::client(&bus, 3);
    let mut captured = false;
    for _ in 0..200 {
        host.poll();
        let Role::Host(state) = &host.runtime.runtime.role else {
            unreachable!()
        };
        if state.sync.resources().baseline_transfers != 0 {
            captured = true;
            break;
        }
        c.poll();
    }
    assert!(captured);
    // Inject a missed retention hook while still delivering that event to Ready.
    let runtime = &mut host.runtime.runtime;
    let Role::Host(state) = &mut runtime.role else {
        unreachable!()
    };
    let session = runtime.session.as_mut().unwrap();
    let cancel = state
        .contexts
        .cancel_replicated(session, &mut host.store, player)
        .unwrap()
        .unwrap();
    runtime.presentation.event(
        PlayerId(0),
        &cancel.authority_event.event,
        None,
        &mut host.store,
    );
    let failures = (HostRouter {
        local_player: PlayerId(0),
        connections: &runtime.connections,
        contexts: &mut state.contexts,
        session,
        store: &mut host.store,
        definition: runtime.definition.as_ref(),
    })
    .publish_authority_event(&mut runtime.transport, &cancel.authority_event)
    .unwrap();
    assert!(failures.is_empty());
    b.poll();
    let mut command = grab(&a);
    command.sequence = ClientCommandSequence::Control(1);
    send_command(&mut a, command);
    host.poll();
    b.poll();
    assert_eq!(
        host.runtime.runtime.status.failure,
        Some(NetworkFailureKind::Internal)
    );
    assert_eq!(host.runtime.runtime.status.phase, RuntimePhase::Hosting);
    assert_eq!(
        host.runtime
            .runtime
            .session
            .as_ref()
            .unwrap()
            .cursor()
            .sequence
            .0,
        3
    );
    assert_eq!(
        host.runtime.runtime.session.as_ref().unwrap().cursor(),
        b.runtime.runtime.session.as_ref().unwrap().cursor()
    );
    assert_eq!(host.store.states, b.store.states);
    assert!(host.runtime.runtime.live.contains(&aid));
    join(&mut host, &mut [&mut c]);
    assert_eq!(host.store.states, c.store.states);
    assert_eq!(
        host.runtime
            .runtime
            .session
            .as_ref()
            .unwrap()
            .cursor()
            .sequence
            .0,
        3,
        "retention restart never reapplies gameplay"
    );
}

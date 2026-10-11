use super::*;
use crate::network::{
    policy::{HostFailurePolicy, HostInternalRecovery},
    syncing::SyncError,
};

#[test]
fn unsafe_and_reliable_command_errors_disconnect_while_local_faults_are_internal() {
    use ProtocolCommandError::*;
    for error in [
        WrongPlayer,
        InvalidDelta,
        ActiveDragExists,
        NoActiveDrag,
        WrongDragContext,
        RotationDisabled,
        InvalidRebaseTick,
        Target(TargetError::InvalidMaskDimensions),
        Sequence(ProtocolError::DuplicateCommand),
        Sequence(ProtocolError::StaleCommand),
        Sequence(ProtocolError::ControlGap { expected: 2 }),
        Sequence(ProtocolError::WrongCommandStream),
        Sequence(ProtocolError::WrongSession),
        Sequence(ProtocolError::WrongEpoch),
    ] {
        assert_eq!(
            HostRouteError::Command(error).policy(),
            HostFailurePolicy::Disconnect(DisconnectReason::ProtocolViolation)
        );
    }
    for error in [
        InvalidDefinition,
        InconsistentDragTarget,
        Sequence(ProtocolError::CounterExhausted),
        Sequence(ProtocolError::Frozen),
    ] {
        assert_eq!(
            HostRouteError::Command(error).policy(),
            HostFailurePolicy::InternalFailure(HostInternalRecovery::StopSession)
        );
    }
    for error in [
        HostRouteError::Unauthenticated,
        HostRouteError::HostConnection,
        HostRouteError::WrongDirection,
        HostRouteError::Wire(WireError::WrongClass),
    ] {
        assert_eq!(
            error.policy(),
            HostFailurePolicy::Disconnect(DisconnectReason::ProtocolViolation)
        );
    }
    for error in [
        SyncError::BaselineEncoding,
        SyncError::Encoding(WireError::WrongClass),
        SyncError::Transport(TransportError::ProtocolViolation),
        SyncError::HostFinalDrag(crate::multiplayer::finalization::FinalDragError::ContextMismatch),
        SyncError::HostRoster(crate::players::RosterError::RevisionExhausted),
    ] {
        assert_eq!(
            error.host_policy(),
            HostFailurePolicy::InternalFailure(HostInternalRecovery::StopSession)
        );
    }
    for error in [
        SyncError::HostImageUnavailable,
        SyncError::HostWrongPhase,
        SyncError::FinalizationExhausted,
    ] {
        assert_eq!(
            error.host_policy(),
            HostFailurePolicy::InternalFailure(HostInternalRecovery::DisconnectPeer(
                DisconnectReason::BackendFailure
            ))
        );
    }
    for error in [
        SyncError::WrongIdentity,
        SyncError::WrongGeneration,
        SyncError::WrongFinalization,
        SyncError::Wire(WireError::WrongClass),
    ] {
        assert_eq!(
            error.host_policy(),
            HostFailurePolicy::Disconnect(DisconnectReason::ProtocolViolation)
        );
    }
}

#[test]
fn empty_rejection_is_retained_published_and_consumes_both_reliable_cursors() {
    let mut s = Scenario::new();
    connected(&mut s.host.connections, HA, A);
    connected(&mut s.peers[0].connections, HA, HOST);
    let mut sync = crate::network::syncing::HostSyncCoordinator::default();
    let before = s.host.session.cursor();
    for sequence in 0..2 {
        let command = command(
            ClientCommandSequence::Control(sequence),
            ProtocolPieceCommand::Grab {
                target: PieceTarget::Components(Vec::new()),
            },
        );
        let routed = s
            .host_router()
            .route_with_sync(
                &message_event(HA, &WireMessage::ClientCommand(command)),
                &mut sync,
            )
            .unwrap();
        routed.retention.unwrap();
        let HostRouteOutcome::Rejected(outcome) = routed.gameplay else {
            panic!("empty ACK refusal")
        };
        assert!(outcome.authority_event.is_some());
        let mut transport = FakeTransport::default();
        s.host_router()
            .publish(&mut transport, Some(HA), &outcome)
            .unwrap();
        assert_eq!(
            transport.sent.len(),
            1,
            "sender must receive its normal Reliable ACK"
        );
        s.client_router(0, HA).route(&transport.sent[0]).unwrap();
        assert_eq!(
            s.host.session.cursor().sequence.0,
            before.sequence.0 + sequence + 1
        );
        assert_eq!(s.host.session.cursor(), s.peers[0].session.cursor());
    }
    assert_eq!(s.host.store.states, s.peers[0].store.states);
    assert!(s
        .contexts
        .active_drag(&s.host.session, &s.host.store, A)
        .is_none());
}

#[test]
fn transient_drop_policy_never_applies_to_reliable_duplicate_or_missing_drag() {
    let mut s = Scenario::new();
    connected(&mut s.host.connections, HA, A);
    let route = |s: &mut Scenario, command| {
        s.host_router()
            .route(&message_event(HA, &WireMessage::ClientCommand(command)))
    };
    assert!(matches!(
        route(&mut s, update()).unwrap(),
        HostRouteOutcome::DroppedTransient(_)
    ));
    route(&mut s, grab()).unwrap();
    assert!(matches!(
        route(&mut s, update()).unwrap(),
        HostRouteOutcome::Applied(_)
    ));
    assert!(matches!(
        route(&mut s, update()).unwrap(),
        HostRouteOutcome::DroppedTransient(_)
    ));
    let cursor = s.host.session.cursor();
    let duplicate = route(&mut s, grab()).unwrap_err();
    assert_eq!(
        duplicate.policy(),
        HostFailurePolicy::Disconnect(DisconnectReason::ProtocolViolation)
    );
    assert_eq!(s.host.session.cursor(), cursor);
}

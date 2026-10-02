//! Fixed v4 frames: encoder/decoder agreement alone cannot detect schema drift.
use super::*;
use crate::network::session_control::{AuthAccepted, SessionControlMessage};

#[test]
fn wire_v4_secure_channel_ready_golden() {
    assert_v4_frame(
        WireMessage::SessionControl(SessionControlMessage::SecureChannelReady),
        "50 5a 4c 41 04 00 06 00 01 00 00 00 03",
    );
}

#[test]
fn wire_v4_auth_accepted_golden() {
    assert_v4_frame(
        WireMessage::SessionControl(SessionControlMessage::AuthAccepted(AuthAccepted {
            player: PlayerId(7),
            confirmation: [0; 32],
        })),
        "50 5a 4c 41 04 00 06 00 22 00 00 00
          02 07
          00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00
          00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00",
    );
}

fn assert_v4_frame(message: WireMessage, hex: &str) {
    assert_eq!(
        wire::WIRE_VERSION,
        4,
        "review these fixtures when versioning the wire schema"
    );
    // The literals below include the header, Postcard enum indices, field order,
    // multibyte unsigned varints and little-endian floats. Never regenerate them
    // from the current encoder merely to make a schema change pass.
    let expected: Vec<u8> = hex
        .split_ascii_whitespace()
        .map(|byte| u8::from_str_radix(byte, 16).unwrap())
        .collect();
    assert_eq!(wire::encode(&message).unwrap(), expected);
    assert_eq!(
        wire::decode_for_class(&expected, message.class()).unwrap(),
        message
    );
}

fn client(sequence: ClientCommandSequence, command: ProtocolPieceCommand) -> WireMessage {
    WireMessage::ClientCommand(ProtocolCommandEnvelope {
        session: SessionId(0x1234),
        authority_epoch: AuthorityEpoch(5),
        player: PlayerId(7),
        sequence,
        command,
    })
}

fn authority(sequence: u64, event: ProtocolAuthorityEvent) -> WireMessage {
    WireMessage::AuthorityEvent(ProtocolAuthorityEventEnvelope {
        session: SessionId(0x1234),
        host: PlayerId(9),
        cursor: AuthorityCursor::new(5, sequence),
        event,
    })
}

#[test]
fn wire_v4_client_grab_golden() {
    assert_v4_frame(
        client(
            ClientCommandSequence::Control(129),
            ProtocolPieceCommand::Grab {
                target: PieceTarget::Components(vec![
                    ComponentRef {
                        member: PieceId(300),
                        expected_size: 4,
                    },
                    ComponentRef {
                        member: PieceId(10),
                        expected_size: 2,
                    },
                ]),
            },
        ),
        "50 5a 4c 41 04 00 01 00 0f 00 00 00
         b4 24 05 07 00 81 01 00 01 02 ac 02 04 0a 02",
    );
}

#[test]
fn wire_v4_client_drag_golden() {
    assert_v4_frame(
        client(
            ClientCommandSequence::Move {
                after_control_sequence: 129,
                tick: 257,
            },
            ProtocolPieceCommand::DragUpdate {
                delta: Vec2::new(1.25, -2.5),
            },
        ),
        "50 5a 4c 41 04 00 04 00 12 00 00 00
         b4 24 05 07 01 81 01 81 02 01 00 00 a0 3f 00 00 20 c0",
    );
}

#[test]
fn wire_v4_client_rotate_golden() {
    assert_v4_frame(
        client(
            ClientCommandSequence::Control(129),
            ProtocolPieceCommand::Rotate {
                target: PieceTarget::Components(vec![
                    ComponentRef {
                        member: PieceId(300),
                        expected_size: 4,
                    },
                    ComponentRef {
                        member: PieceId(10),
                        expected_size: 2,
                    },
                ]),
                quarter_turns: -1,
            },
        ),
        // Rotate is command variant 3; Postcard encodes i8 -1 as ff.
        "50 5a 4c 41 04 00 01 00 10 00 00 00
         b4 24 05 07 00 81 01 03 01 02 ac 02 04 0a 02 ff",
    );
}

#[test]
fn wire_v4_grab_accepted_golden() {
    assert_v4_frame(
        authority(
            130,
            ProtocolAuthorityEvent::GrabAccepted(GrabAccepted {
                player: PlayerId(7),
                grab_sequence: 129,
                accepted: PieceTarget::Component(ComponentRef {
                    member: PieceId(300),
                    expected_size: 4,
                }),
                rejected: vec![RejectedComponentRef {
                    reference: ComponentRef {
                        member: PieceId(10),
                        expected_size: 2,
                    },
                    reason: TargetError::StaleComponent,
                }],
            }),
        ),
        "50 5a 4c 41 04 00 02 00 12 00 00 00
         b4 24 09 05 82 01 00 07 81 01 00 ac 02 04 01 0a 02 01",
    );
}

#[test]
fn wire_v4_release_committed_golden() {
    assert_v4_frame(
        authority(
            131,
            ProtocolAuthorityEvent::ReleaseCommitted(ReleaseCommitted {
                player: PlayerId(7),
                grab_sequence: 129,
                final_delta: Vec2::new(3.75, -4.5),
                result: ReleaseResultFingerprint(0x0123_4567_89ab_cdef_fedc_ba98_7654_3210),
            }),
        ),
        "50 5a 4c 41 04 00 02 00 24 00 00 00
         b4 24 09 05 83 01 01 07 81 01 00 00 70 40 00 00 90 c0
         90 e4 d0 b2 87 d3 ae ee fe df b7 de 9a f1 d9 a2 a3 02",
    );
}

#[test]
fn wire_v4_rotation_committed_golden() {
    assert_v4_frame(
        authority(
            132,
            ProtocolAuthorityEvent::RotationCommitted(RotationCommitted {
                player: PlayerId(7),
                accepted: PieceTarget::Component(ComponentRef {
                    member: PieceId(300),
                    expected_size: 4,
                }),
                quarter_turns: 3,
                result: ReleaseResultFingerprint(0x0123_4567_89ab_cdef_fedc_ba98_7654_3210),
            }),
        ),
        // RotationCommitted is event variant 2; normalized turns precede the fingerprint.
        "50 5a 4c 41 04 00 02 00 1f 00 00 00
         b4 24 09 05 84 01 02 07 00 ac 02 04 03
         90 e4 d0 b2 87 d3 ae ee fe df b7 de 9a f1 d9 a2 a3 02",
    );
}

#[test]
fn wire_v4_client_rotate_drag_golden() {
    assert_v4_frame(
        client(
            ClientCommandSequence::Control(130),
            ProtocolPieceCommand::RotateDrag {
                grab_sequence: 129,
                final_delta: Vec2::new(1.25, -2.5),
                through_tick: Some(257),
                quarter_turns: -1,
            },
        ),
        // Command variant 4, absolute floats, Some(tick), then signed i8 turns.
        "50 5a 4c 41 04 00 01 00 16 00 00 00
         b4 24 05 07 00 82 01 04 81 01 00 00 a0 3f 00 00 20 c0 01 81 02 ff",
    );
}

#[test]
fn wire_v4_client_rotate_drag_without_updates_golden() {
    assert_v4_frame(
        client(
            ClientCommandSequence::Control(130),
            ProtocolPieceCommand::RotateDrag {
                grab_sequence: 129,
                final_delta: Vec2::ZERO,
                through_tick: None,
                quarter_turns: 1,
            },
        ),
        "50 5a 4c 41 04 00 01 00 14 00 00 00
         b4 24 05 07 00 82 01 04 81 01 00 00 00 00 00 00 00 00 00 01",
    );
}

#[test]
fn wire_v4_drag_rotation_committed_golden() {
    assert_v4_frame(
        authority(
            133,
            ProtocolAuthorityEvent::DragRotationCommitted(DragRotationCommitted {
                player: PlayerId(7),
                grab_sequence: 129,
                basis_sequence: 130,
                through_tick: Some(257),
                final_delta: Vec2::new(1.25, -2.5),
                quarter_turns: 3,
                result: ReleaseResultFingerprint(0x0123_4567_89ab_cdef_fedc_ba98_7654_3210),
            }),
        ),
        "50 5a 4c 41 04 00 02 00 2a 00 00 00
         b4 24 09 05 85 01 03 07 81 01 82 01 01 81 02
         00 00 a0 3f 00 00 20 c0 03
         90 e4 d0 b2 87 d3 ae ee fe df b7 de 9a f1 d9 a2 a3 02",
    );
}

#[test]
fn wire_v4_remote_drag_update_golden() {
    assert_v4_frame(
        WireMessage::DragUpdate(RemoteDragUpdate {
            session: SessionId(0x1234),
            authority_epoch: AuthorityEpoch(5),
            player: PlayerId(7),
            grab_sequence: 129,
            basis_sequence: 129,
            tick: 258,
            delta: Vec2::new(-3.5, 4.25),
        }),
        "50 5a 4c 41 04 00 03 00 12 00 00 00
         b4 24 05 07 81 01 81 01 82 02 00 00 60 c0 00 00 88 40",
    );
}

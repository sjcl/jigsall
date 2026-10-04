//! Fixed v1 frames: encoder/decoder agreement alone cannot detect schema drift.
use super::*;
use crate::network::bulk::BulkTransferKind;
use crate::network::session_control::{AuthAccepted, SessionControlMessage};
use crate::network::{
    bulk::TransferId,
    sync_control::{SyncControlMessage as Control, SyncTransferBinding},
};

#[test]
fn wire_v1_finalization_and_ready_commit_golden() {
    use crate::{
        multiplayer::finalization::{FinalDragSet, FinalDragState},
        network::sync_control::SyncFinalization,
    };
    let token = SyncFinalization {
        generation: 129,
        cursor: AuthorityCursor::new(3, 2),
        revision: 129,
    };
    assert_v1_frame(
        WireMessage::SyncControl(Control::Finalize {
            token,
            drags: FinalDragSet { entries: vec![] },
        }),
        "50 5a 4c 41 01 00 07 00 08 00 00 00 0b 81 01 03 02 81 01 00",
    );
    assert_v1_frame(WireMessage::SyncControl(Control::Finalize { token, drags: FinalDragSet { entries: vec![FinalDragState {
        player: PlayerId(10), grab_sequence: 129, basis_sequence: 130, last_tick: Some(5), delta: Vec2::new(1.0, -2.0),
    }] } }),
        "50 5a 4c 41 01 00 07 00 17 00 00 00 0b 81 01 03 02 81 01 01 0a 81 01 82 01 01 05 00 00 80 3f 00 00 00 c0");
    assert_v1_frame(
        WireMessage::SyncControl(Control::FinalizeAck { token }),
        "50 5a 4c 41 01 00 07 00 07 00 00 00 0c 81 01 03 02 81 01",
    );
    assert_v1_frame(
        WireMessage::SyncControl(Control::ReadyCommit {
            token,
            roster: crate::players::RosterSnapshot {
                revision: 1,
                players: vec![
                    crate::players::RosterPlayer {
                        player: PlayerId(9),
                        display_name: None,
                    },
                    crate::players::RosterPlayer {
                        player: PlayerId(10),
                        display_name: None,
                    },
                ],
            },
        }),
        "50 5a 4c 41 01 00 07 00 0d 00 00 00 0d 81 01 03 02 81 01 01 02 09 00 0a 00",
    );
}

#[test]
fn wire_v1_sync_offer_and_ack_golden() {
    let binding = SyncTransferBinding {
        transfer_id: TransferId(7),
        kind: BulkTransferKind::JoinBaseline,
        total_size: 16,
        sha256: [0x42; 32],
    };
    assert_v1_frame(
        WireMessage::SyncControl(Control::BaselineOffer {
            generation: 129,
            cursor: AuthorityCursor::new(3, 2),
            transfer: binding,
        }),
        "50 5a 4c 41 01 00 07 00 28 00 00 00 05 81 01 03 02 07 00 10
         42 42 42 42 42 42 42 42 42 42 42 42 42 42 42 42
         42 42 42 42 42 42 42 42 42 42 42 42 42 42 42 42",
    );
    for (control, bytes) in [
        (
            Control::TransferAccepted {
                transfer_id: TransferId(129),
            },
            "50 5a 4c 41 01 00 07 00 03 00 00 00 03 81 01",
        ),
        (
            Control::BaselineInstalled {
                generation: 129,
                cursor: AuthorityCursor::new(3, 2),
                transfer_id: TransferId(7),
            },
            "50 5a 4c 41 01 00 07 00 06 00 00 00 06 81 01 03 02 07",
        ),
        (
            Control::CatchUpAck {
                generation: 129,
                cursor: AuthorityCursor::new(3, 2),
            },
            "50 5a 4c 41 01 00 07 00 05 00 00 00 08 81 01 03 02",
        ),
        (
            Control::ReliableComplete {
                generation: 129,
                cursor: AuthorityCursor::new(3, 2),
            },
            "50 5a 4c 41 01 00 07 00 05 00 00 00 09 81 01 03 02",
        ),
        (
            Control::Restart { generation: 129 },
            "50 5a 4c 41 01 00 07 00 03 00 00 00 0a 81 01",
        ),
    ] {
        assert_v1_frame(WireMessage::SyncControl(control), bytes);
    }
    assert_v1_frame(
        WireMessage::SyncControl(Control::ImageOffer(SyncTransferBinding {
            transfer_id: TransferId(129),
            kind: BulkTransferKind::PuzzleImage,
            total_size: 32704,
            ..binding
        })),
        "50 5a 4c 41 01 00 07 00 27 00 00 00 02 81 01 01 c0 ff 01
         42 42 42 42 42 42 42 42 42 42 42 42 42 42 42 42
         42 42 42 42 42 42 42 42 42 42 42 42 42 42 42 42",
    );
    assert_v1_frame(
        WireMessage::SyncControl(Control::ImageAvailability {
            image_hash: ImageHash([0x42; 32]),
            available: true,
        }),
        "50 5a 4c 41 01 00 07 00 22 00 00 00 01
         42 42 42 42 42 42 42 42 42 42 42 42 42 42 42 42
         42 42 42 42 42 42 42 42 42 42 42 42 42 42 42 42 01",
    );
    assert_v1_frame(
        WireMessage::SyncControl(Control::ImageReady {
            image_hash: ImageHash([0x42; 32]),
        }),
        "50 5a 4c 41 01 00 07 00 21 00 00 00 04
         42 42 42 42 42 42 42 42 42 42 42 42 42 42 42 42
         42 42 42 42 42 42 42 42 42 42 42 42 42 42 42 42",
    );
}

#[test]
fn wire_v1_sync_session_and_catch_up_event_golden() {
    assert_v1_frame(
        WireMessage::SyncControl(Control::Session {
            metadata: crate::network::session_control::SessionMetadata {
                definition: SessionDefinition {
                    id: SessionId(1),
                    image_hash: ImageHash([0; 32]),
                },
                cursor: AuthorityCursor::new(3, 0),
                host: PlayerId(9),
            },
            definition: PuzzleDefinition {
                generator_version: 1,
                seed: 42,
                grid_size: UVec2::new(2, 1),
                image_size: UVec2::new(40, 20),
                snap_distance: 5.0,
                rotation_enabled: true,
            },
        }),
        "50 5a 4c 41 01 00 07 00 30 00 00 00 00 01
         00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00
         00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00
         03 00 09 01 2a 02 01 28 14 00 00 a0 40 01",
    );
    assert_v1_frame(
        WireMessage::SyncControl(Control::CatchUpEvent {
            generation: 129,
            event: ProtocolAuthorityEventEnvelope {
                session: SessionId(123),
                host: PlayerId(9),
                cursor: AuthorityCursor::new(3, 2),
                event: ProtocolAuthorityEvent::DragCancelled(DragCancelled {
                    player: PlayerId(10),
                    grab_sequence: 129,
                }),
            },
        }),
        "50 5a 4c 41 01 00 07 00 0b 00 00 00 07 81 01 7b 09 03 02 04 0a 81 01",
    );
}

#[test]
fn wire_v1_bulk_start_golden() {
    assert_v1_frame(
        WireMessage::BulkTransfer(BulkTransferMessage::Start {
            transfer_id: TransferId(129),
            kind: BulkTransferKind::PuzzleImage,
            total_size: 32704,
            sha256: [0x42; 32],
        }),
        // Start index 00, ID 81 01, PuzzleImage index 01, size c0 ff 01,
        // then the fixed 32-byte hash: 39 payload bytes, kind 5.
        "50 5a 4c 41 01 00 05 00 27 00 00 00
         00 81 01 01 c0 ff 01
         42 42 42 42 42 42 42 42 42 42 42 42 42 42 42 42
         42 42 42 42 42 42 42 42 42 42 42 42 42 42 42 42",
    );
}

#[test]
fn wire_v1_bulk_chunk_golden() {
    assert_v1_frame(
        WireMessage::BulkTransfer(BulkTransferMessage::Chunk {
            transfer_id: TransferId(129),
            offset: 32704,
            data: vec![1, 2, 3],
        }),
        // Chunk index 01, ID, offset c0 ff 01, length 03, bytes 01 02 03.
        "50 5a 4c 41 01 00 05 00 0a 00 00 00
         01 81 01 c0 ff 01 03 01 02 03",
    );
}

#[test]
fn wire_v1_bulk_finish_golden() {
    assert_v1_frame(
        WireMessage::BulkTransfer(BulkTransferMessage::Finish {
            transfer_id: TransferId(129),
        }),
        "50 5a 4c 41 01 00 05 00 03 00 00 00 02 81 01",
    );
}

#[test]
fn wire_v1_bulk_abort_golden() {
    assert_v1_frame(
        WireMessage::BulkTransfer(BulkTransferMessage::Abort {
            transfer_id: TransferId(129),
        }),
        "50 5a 4c 41 01 00 05 00 03 00 00 00 03 81 01",
    );
}

#[test]
fn wire_v1_secure_channel_ready_golden() {
    assert_v1_frame(
        WireMessage::SessionControl(SessionControlMessage::SecureChannelReady),
        "50 5a 4c 41 01 00 06 00 01 00 00 00 03",
    );
}

#[test]
fn wire_v1_auth_accepted_golden() {
    assert_v1_frame(
        WireMessage::SessionControl(SessionControlMessage::AuthAccepted(AuthAccepted {
            player: PlayerId(7),
            confirmation: [0; 32],
        })),
        "50 5a 4c 41 01 00 06 00 22 00 00 00
          02 07
          00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00
          00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00",
    );
}

fn assert_v1_frame(message: WireMessage, hex: &str) {
    assert_eq!(
        wire::WIRE_VERSION,
        1,
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

#[test]
fn wire_v1_profile_and_presence_golden() {
    use crate::players::{PresenceMessage, RosterPlayer};
    use puzzella_core::PlayerDisplayName;
    assert_v1_frame(
        WireMessage::SyncControl(Control::ClientProfile { display_name: None }),
        "50 5a 4c 41 01 00 07 00 02 00 00 00 0e 00",
    );
    let name = Some(PlayerDisplayName::from_user_input("Alice").unwrap());
    assert_v1_frame(
        WireMessage::SyncControl(Control::ClientProfile {
            display_name: name.clone(),
        }),
        "50 5a 4c 41 01 00 07 00 08 00 00 00 0e 01 05 41 6c 69 63 65",
    );
    assert_v1_frame(
        WireMessage::Presence(PresenceMessage::PlayerJoined {
            revision: 129,
            player: RosterPlayer {
                player: PlayerId(10),
                display_name: name,
            },
        }),
        "50 5a 4c 41 01 00 08 00 0b 00 00 00 00 81 01 0a 01 05 41 6c 69 63 65",
    );
    assert_v1_frame(
        WireMessage::Presence(PresenceMessage::PlayerLeft {
            revision: 130,
            player: PlayerId(10),
        }),
        "50 5a 4c 41 01 00 08 00 04 00 00 00 01 82 01 0a",
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
fn wire_v1_client_grab_golden() {
    assert_v1_frame(
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
        "50 5a 4c 41 01 00 01 00 0f 00 00 00
         b4 24 05 07 00 81 01 00 01 02 ac 02 04 0a 02",
    );
}

#[test]
fn wire_v1_client_drag_golden() {
    assert_v1_frame(
        client(
            ClientCommandSequence::Move {
                after_control_sequence: 129,
                tick: 257,
            },
            ProtocolPieceCommand::DragUpdate {
                delta: Vec2::new(1.25, -2.5),
            },
        ),
        "50 5a 4c 41 01 00 04 00 12 00 00 00
         b4 24 05 07 01 81 01 81 02 01 00 00 a0 3f 00 00 20 c0",
    );
}

#[test]
fn wire_v1_client_rotate_golden() {
    assert_v1_frame(
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
        "50 5a 4c 41 01 00 01 00 10 00 00 00
         b4 24 05 07 00 81 01 03 01 02 ac 02 04 0a 02 ff",
    );
}

#[test]
fn wire_v1_grab_accepted_golden() {
    assert_v1_frame(
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
        "50 5a 4c 41 01 00 02 00 12 00 00 00
         b4 24 09 05 82 01 00 07 81 01 00 ac 02 04 01 0a 02 01",
    );
}

#[test]
fn wire_v1_release_committed_golden() {
    assert_v1_frame(
        authority(
            131,
            ProtocolAuthorityEvent::ReleaseCommitted(ReleaseCommitted {
                player: PlayerId(7),
                grab_sequence: 129,
                final_delta: Vec2::new(3.75, -4.5),
                result: ReleaseResultFingerprint(0x0123_4567_89ab_cdef_fedc_ba98_7654_3210),
            }),
        ),
        "50 5a 4c 41 01 00 02 00 24 00 00 00
         b4 24 09 05 83 01 01 07 81 01 00 00 70 40 00 00 90 c0
         90 e4 d0 b2 87 d3 ae ee fe df b7 de 9a f1 d9 a2 a3 02",
    );
}

#[test]
fn wire_v1_rotation_committed_golden() {
    assert_v1_frame(
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
        "50 5a 4c 41 01 00 02 00 1f 00 00 00
         b4 24 09 05 84 01 02 07 00 ac 02 04 03
         90 e4 d0 b2 87 d3 ae ee fe df b7 de 9a f1 d9 a2 a3 02",
    );
}

#[test]
fn wire_v1_client_rotate_drag_golden() {
    assert_v1_frame(
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
        "50 5a 4c 41 01 00 01 00 16 00 00 00
         b4 24 05 07 00 82 01 04 81 01 00 00 a0 3f 00 00 20 c0 01 81 02 ff",
    );
}

#[test]
fn wire_v1_client_rotate_drag_without_updates_golden() {
    assert_v1_frame(
        client(
            ClientCommandSequence::Control(130),
            ProtocolPieceCommand::RotateDrag {
                grab_sequence: 129,
                final_delta: Vec2::ZERO,
                through_tick: None,
                quarter_turns: 1,
            },
        ),
        "50 5a 4c 41 01 00 01 00 14 00 00 00
         b4 24 05 07 00 82 01 04 81 01 00 00 00 00 00 00 00 00 00 01",
    );
}

#[test]
fn wire_v1_drag_rotation_committed_golden() {
    assert_v1_frame(
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
        "50 5a 4c 41 01 00 02 00 2a 00 00 00
         b4 24 09 05 85 01 03 07 81 01 82 01 01 81 02
         00 00 a0 3f 00 00 20 c0 03
         90 e4 d0 b2 87 d3 ae ee fe df b7 de 9a f1 d9 a2 a3 02",
    );
}

#[test]
fn wire_v1_remote_drag_update_golden() {
    assert_v1_frame(
        WireMessage::DragUpdate(RemoteDragUpdate {
            session: SessionId(0x1234),
            authority_epoch: AuthorityEpoch(5),
            player: PlayerId(7),
            grab_sequence: 129,
            basis_sequence: 129,
            tick: 258,
            delta: Vec2::new(-3.5, 4.25),
        }),
        "50 5a 4c 41 01 00 03 00 12 00 00 00
         b4 24 05 07 81 01 81 01 82 02 00 00 60 c0 00 00 88 40",
    );
}

#[test]
fn wire_v1_drag_cancelled_golden() {
    assert_v1_frame(
        authority(
            134,
            ProtocolAuthorityEvent::DragCancelled(DragCancelled {
                player: PlayerId(7),
                grab_sequence: 129,
            }),
        ),
        // Control kind 2; payload: session b4 24, host 09, epoch 05,
        // cursor 86 01, appended event index 04, player 07, grab 81 01.
        // Ten payload bytes, no membership or transient delta.
        "50 5a 4c 41 01 00 02 00 0a 00 00 00
         b4 24 09 05 86 01 04 07 81 01",
    );
}

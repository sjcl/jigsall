use super::*;
use crate::network::{
    transport::MessageClass,
    wire::{self, WireMessage},
};
fn update(tick: u64, position: Option<Vec2>) -> CursorUpdate {
    CursorUpdate {
        session: SessionId(1),
        authority_epoch: AuthorityEpoch(2),
        tick,
        position,
    }
}
fn snapshot(sequence: u64, ids: &[u64]) -> CursorSnapshot {
    CursorSnapshot {
        session: SessionId(1),
        authority_epoch: AuthorityEpoch(2),
        sequence,
        entries: ids
            .iter()
            .map(|&id| CursorEntry {
                player: PlayerId(id),
                position: Vec2::splat(id as f32),
            })
            .collect(),
    }
}
fn state() -> CursorPresence {
    let mut state = CursorPresence::default();
    state.synchronize(SessionId(1), AuthorityEpoch(2));
    state
}
fn roster() -> PlayerRoster {
    let mut r = PlayerRoster::host_only(PlayerId(0), None);
    for id in 1..=3 {
        r.apply_presence(crate::players::PresenceMessage::PlayerJoined {
            revision: id,
            player: crate::players::RosterPlayer {
                player: PlayerId(id),
                display_name: None,
            },
        })
        .unwrap();
    }
    r
}
fn target(s: &CursorPresence, id: u64) -> Option<Vec2> {
    s.presentation
        .cursors()
        .find(|(p, _)| *p == PlayerId(id))
        .map(|(_, c)| c.target_world_position)
}

#[test]
fn cursor_wire_roundtrip_class_version_and_golden() {
    let msg = WireMessage::CursorUpdate(update(10, Some(Vec2::new(1.0, -2.0))));
    let expected = vec![
        0x50, 0x5a, 0x4c, 0x41, 11, 0, 9, 0, 12, 0, 0, 0, 1, 2, 10, 1, 0, 0, 0x80, 0x3f, 0, 0, 0,
        0xc0,
    ];
    assert_eq!(wire::encode(&msg).unwrap(), expected);
    let snap = WireMessage::CursorSnapshot(snapshot(20, &[1]));
    let golden = vec![
        0x50, 0x5a, 0x4c, 0x41, 11, 0, 10, 0, 13, 0, 0, 0, 1, 2, 20, 1, 1, 0, 0, 0x80, 0x3f, 0, 0,
        0x80, 0x3f,
    ];
    assert_eq!(wire::encode(&snap).unwrap(), golden);
    for msg in [msg, WireMessage::CursorUpdate(update(11, None)), snap] {
        assert_eq!(msg.class(), MessageClass::Transient);
        let bytes = wire::encode(&msg).unwrap();
        assert_eq!(
            wire::decode_for_class(&bytes, MessageClass::Transient).unwrap(),
            msg
        );
        assert_eq!(
            wire::decode_for_class(&bytes, MessageClass::Control),
            Err(wire::WireError::WrongClass)
        );
        assert_eq!(
            wire::decode_for_class(&bytes, MessageClass::Bulk),
            Err(wire::WireError::WrongClass)
        );
        for version in [9, 10] {
            let mut old = bytes.clone();
            old[4] = version;
            assert_eq!(
                wire::decode(&old),
                Err(wire::WireError::UnsupportedVersion(version as u16))
            );
        }
    }
}

#[test]
fn cursor_max_snapshot_size_and_authenticated_rate_budget() {
    use crate::network::{
        rate_limit::{InboundRateLimiter, RateDecision, DEFAULT_INBOUND_POLICY},
        secure::RECORD_OVERHEAD,
    };
    let max = CursorSnapshot {
        session: SessionId(u128::MAX),
        authority_epoch: AuthorityEpoch(u64::MAX),
        sequence: u64::MAX,
        entries: (0..MAX_ROSTER_PLAYERS)
            .map(|i| CursorEntry {
                player: PlayerId(u64::MAX - MAX_ROSTER_PLAYERS as u64 + i as u64),
                position: Vec2::splat(CURSOR_WORLD_BOUND),
            })
            .collect(),
    };
    let bytes = wire::encode(&WireMessage::CursorSnapshot(max.clone())).unwrap();
    assert_eq!(bytes.len() - wire::HEADER_SIZE, 1210);
    assert_eq!(
        wire::decode(&bytes).unwrap(),
        WireMessage::CursorSnapshot(max)
    );
    assert!(bytes.len() - wire::HEADER_SIZE <= wire::MAX_TRANSIENT_PAYLOAD);
    let rate = DEFAULT_INBOUND_POLICY.transient;
    assert_eq!(20 * rate.minimum_charge, 10_240);
    assert_eq!(20 * (bytes.len() + RECORD_OVERHEAD), 24_920);
    assert!(24_920 < rate.bytes_per_second as usize);
    let now = Instant::now();
    let mut limiter = InboundRateLimiter::new(now);
    for tick in 0..2000 {
        assert_eq!(
            limiter.check(
                MessageClass::Transient,
                bytes.len() + RECORD_OVERHEAD,
                now + CURSOR_INTERVAL * tick
            ),
            RateDecision::Allow
        );
    }
}

#[test]
fn cursor_decode_rejects_oversized_length_before_reading_elements() {
    struct Oversized;
    impl<'de> de::Deserializer<'de> for Oversized {
        type Error = de::value::Error;
        fn deserialize_any<V: de::Visitor<'de>>(self, visitor: V) -> Result<V::Value, Self::Error> {
            visitor.visit_seq(self)
        }
        serde::forward_to_deserialize_any! { bool i8 i16 i32 i64 u8 u16 u32 u64 f32 f64 char str string bytes byte_buf option unit unit_struct newtype_struct seq tuple tuple_struct map struct enum identifier ignored_any }
    }
    impl<'de> de::SeqAccess<'de> for Oversized {
        type Error = de::value::Error;
        fn next_element_seed<T: de::DeserializeSeed<'de>>(
            &mut self,
            _: T,
        ) -> Result<Option<T::Value>, Self::Error> {
            panic!("must reject count before allocating/reading entries")
        }
        fn size_hint(&self) -> Option<usize> {
            Some(usize::MAX)
        }
    }
    assert!(bounded_entries(Oversized).is_err());
    for entries in [
        snapshot(1, &[1, 1]),
        snapshot(1, &[2, 1]),
        snapshot(1, &(0..66).collect::<Vec<_>>()),
    ] {
        assert!(wire::encode(&WireMessage::CursorSnapshot(entries.clone())).is_err());
        let payload = postcard::to_allocvec(&entries).unwrap();
        let mut frame = b"PZLA".to_vec();
        frame.extend_from_slice(&11u16.to_le_bytes());
        frame.extend_from_slice(&[10, 0]);
        frame.extend_from_slice(&(payload.len() as u32).to_le_bytes());
        frame.extend(payload);
        assert!(wire::decode(&frame).is_err());
    }
}

#[test]
fn cursor_update_latest_tick_scope_invalid_position_and_expiry() {
    let now = Instant::now();
    let mut s = state();
    for (tick, pos, expected) in [
        (10, 10., 10.),
        (10, 100., 10.),
        (9, 9., 10.),
        (11, 11., 11.),
    ] {
        s.accept_update(PlayerId(1), update(tick, Some(Vec2::splat(pos))), now);
        assert_eq!(target(&s, 1), Some(Vec2::splat(expected)));
    }
    for wrong in [
        CursorUpdate {
            session: SessionId(0),
            ..update(12, None)
        },
        CursorUpdate {
            authority_epoch: AuthorityEpoch(0),
            ..update(12, None)
        },
    ] {
        s.accept_update(PlayerId(1), wrong, now);
        assert_eq!(target(&s, 1), Some(Vec2::splat(11.)));
    }
    s.expire(now + Duration::from_millis(399));
    assert!(target(&s, 1).is_some());
    s.expire(now + CURSOR_TIMEOUT);
    assert!(target(&s, 1).is_none());
    s.accept_update(
        PlayerId(1),
        update(11, Some(Vec2::ONE)),
        now + CURSOR_TIMEOUT,
    );
    assert!(target(&s, 1).is_none());
    for (i, pos) in [
        Vec2::splat(f32::NAN),
        Vec2::splat(f32::INFINITY),
        Vec2::splat(1e37),
        Vec2::splat(-1e37),
    ]
    .into_iter()
    .enumerate()
    {
        s.accept_update(PlayerId(1), update(12 + i as u64, Some(pos)), now);
        assert!(target(&s, 1).is_none());
    }
    s.accept_update(PlayerId(1), update(20, Some(Vec2::ONE)), now);
    assert!(target(&s, 1).is_some());
    s.accept_update(PlayerId(1), update(21, None), now);
    assert!(target(&s, 1).is_none());
}

#[test]
fn cursor_full_snapshot_order_presence_reorder_and_loss_self_heal() {
    let now = Instant::now();
    let mut s = state();
    let mut r = roster();
    s.accept_snapshot(snapshot(20, &[0, 1, 2, 3, 4]), PlayerId(0), &r, now);
    assert_eq!(s.presentation.cursors().count(), 3);
    assert!(target(&s, 0).is_none());
    assert!(target(&s, 4).is_none());
    s.accept_snapshot(snapshot(19, &[]), PlayerId(0), &r, now);
    assert_eq!(s.presentation.cursors().count(), 3);
    s.accept_snapshot(snapshot(21, &[1, 3]), PlayerId(0), &r, now);
    assert!(target(&s, 2).is_none());
    r.apply_presence(crate::players::PresenceMessage::PlayerJoined {
        revision: 4,
        player: crate::players::RosterPlayer {
            player: PlayerId(4),
            display_name: None,
        },
    })
    .unwrap();
    // Snapshots 22..24 were lost; do not wait for the gap.
    s.accept_snapshot(snapshot(25, &[1, 3, 4]), PlayerId(0), &r, now);
    assert!(target(&s, 4).is_some());
    r.remove_ready(PlayerId(3)).unwrap();
    s.remove(PlayerId(3));
    s.accept_snapshot(snapshot(26, &[1, 3, 4]), PlayerId(0), &r, now);
    assert!(target(&s, 3).is_none());
    for wrong in [
        CursorSnapshot {
            session: SessionId(0),
            ..snapshot(27, &[])
        },
        CursorSnapshot {
            authority_epoch: AuthorityEpoch(0),
            ..snapshot(27, &[])
        },
    ] {
        s.accept_snapshot(wrong, PlayerId(0), &r, now);
        assert!(target(&s, 1).is_some());
    }
    let mut invalid = snapshot(27, &[1, 4]);
    invalid.entries[0].position = Vec2::splat(1e37);
    s.accept_snapshot(invalid, PlayerId(0), &r, now);
    assert!(target(&s, 1).is_none());
    s.expire_snapshot(now + CURSOR_TIMEOUT);
    assert_eq!(s.presentation.cursors().count(), 0);
}

#[test]
fn cursor_360hz_is_20hz_stationary_heartbeat_immediate_hide_and_no_wrap() {
    let now = Instant::now();
    let mut s = state();
    let mut sent = 0;
    for frame in 0..360 {
        sent += usize::from(
            s.sample(
                Some(Vec2::ONE),
                now + Duration::from_secs_f64(frame as f64 / 360.),
            )
            .is_some(),
        );
    }
    assert_eq!(sent, 20);
    let mut s = state();
    let first = s.sample(Some(Vec2::ONE), now).unwrap();
    assert_eq!(
        s.sample(None, now + Duration::from_millis(1))
            .unwrap()
            .position,
        None
    );
    assert!(s.sample(None, now + Duration::from_millis(2)).is_none());
    assert!(s
        .sample(Some(Vec2::ONE), now + Duration::from_millis(2))
        .is_none());
    assert!(s.sample(None, now + Duration::from_millis(3)).is_none());
    assert!(s.sample(None, now + CURSOR_INTERVAL * 2).is_none());
    s.local_tick = Some(u64::MAX);
    assert!(s
        .sample(Some(Vec2::ONE), now + CURSOR_INTERVAL * 2)
        .is_none());
    s.snapshot_sequence = Some(u64::MAX);
    assert!(s.snapshot(PlayerId(0), &roster(), now).is_none());
    assert_eq!(first.tick, 0);
    s.synchronize(SessionId(9), AuthorityEpoch(3));
    assert!(s.latest.is_empty());
    assert!(s.received_sequence.is_none());
    assert_eq!(s.sample(Some(Vec2::ONE), now).unwrap().tick, 0);
    let mut s = state();
    let mut sent = 0;
    for frame in 0..360 {
        sent += usize::from(
            s.sample(
                (frame % 2 == 0).then_some(Vec2::ONE),
                now + Duration::from_secs_f64(frame as f64 / 360.),
            )
            .is_some(),
        );
    }
    assert!(
        sent <= 40,
        "visibility jitter must not create per-frame hides"
    );
}

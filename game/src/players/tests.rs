use super::*;
use jigsall_core::DisplayNameError;

fn entry(id: u64) -> RosterPlayer {
    RosterPlayer {
        player: PlayerId(id),
        display_name: Some(PlayerDisplayName::from_user_input("Alice").unwrap()),
    }
}
#[test]
fn names_unicode_trim_empty_limits_and_untrusted_decode() {
    for (input, expected) in [
        ("Alice", "Alice"),
        (" 日本語　", "日本語"),
        ("😀🧩", "😀🧩"),
        ("\u{2003}Alice\u{3000}", "Alice"),
    ] {
        let name = PlayerDisplayName::from_user_input(input).unwrap();
        assert_eq!(name.as_ref(), expected);
        assert_eq!(name.to_string(), expected);
        assert_eq!(
            postcard::from_bytes::<PlayerDisplayName>(&postcard::to_allocvec(&name).unwrap())
                .unwrap(),
            name
        );
        assert_eq!(
            serde_json::from_value::<PlayerDisplayName>(serde_json::json!(input)).unwrap(),
            name
        );
    }
    assert_eq!(PlayerDisplayName::optional_from_user_input("　 "), Ok(None));
    assert_eq!(
        PlayerDisplayName::from_user_input(""),
        Err(DisplayNameError::Empty)
    );
    for value in ["a".repeat(32), "😀".repeat(32)] {
        assert!(PlayerDisplayName::from_user_input(&value).is_ok());
    }
    assert_eq!("😀".repeat(32).len(), 128);
    assert_eq!(
        PlayerDisplayName::from_user_input(&"a".repeat(33)),
        Err(DisplayNameError::TooManyChars)
    );
    assert_eq!(
        PlayerDisplayName::from_user_input(&"😀".repeat(33)),
        Err(DisplayNameError::TooManyBytes)
    );
    for value in [
        "".to_owned(),
        "a".repeat(33),
        "a".repeat(129),
        " ".repeat(10000),
    ] {
        assert!(
            postcard::from_bytes::<PlayerDisplayName>(&postcard::to_allocvec(&value).unwrap())
                .is_err()
        );
    }
    for bytes in [vec![0xff; 10], vec![2, b'a'], vec![1, 0xff]] {
        assert!(postcard::from_bytes::<PlayerDisplayName>(&bytes).is_err());
    }
}
#[test]
fn names_forbid_controls_separators_and_bidi_before_trimming() {
    for c in [
        '\0', '\n', '\r', '\t', '\u{7f}', '\u{061c}', '\u{200e}', '\u{200f}', '\u{2028}',
        '\u{2029}', '\u{202a}', '\u{202b}', '\u{202c}', '\u{202d}', '\u{202e}', '\u{2066}',
        '\u{2067}', '\u{2068}', '\u{2069}',
    ] {
        for input in [format!("Alice{c}Bob"), format!("{c}Alice")] {
            assert_eq!(
                PlayerDisplayName::from_user_input(&input),
                Err(DisplayNameError::ForbiddenCharacter)
            );
            assert!(postcard::from_bytes::<PlayerDisplayName>(
                &postcard::to_allocvec(&input).unwrap()
            )
            .is_err());
        }
    }
}
#[test]
fn host_join_leave_are_canonical_and_allow_duplicate_names() {
    let mut roster = PlayerRoster::host_only(PlayerId(9), entry(9).display_name);
    assert_eq!((roster.revision(), roster.len()), (0, 1));
    roster = roster
        .prepare_join(entry(3))
        .unwrap()
        .prepare_join(entry(12))
        .unwrap();
    assert_eq!(
        roster.players().map(|p| p.id).collect::<Vec<_>>(),
        vec![PlayerId(3), PlayerId(9), PlayerId(12)]
    );
    assert!(roster
        .players()
        .all(|p| p.display_name.as_ref().unwrap().as_ref() == "Alice"));
    assert_eq!(roster.revision(), 2);
    let mut client = PlayerRoster::default();
    client
        .install_snapshot(roster.snapshot(), PlayerId(9), PlayerId(3))
        .unwrap();
    client
        .apply_presence(roster.remove_ready(PlayerId(12)).unwrap())
        .unwrap();
    assert_eq!(client, roster);
    assert_eq!(client.revision(), 3);
}
#[test]
fn snapshot_validation_is_transactional() {
    let mut roster = PlayerRoster::host_only(PlayerId(9), None);
    let original = roster.snapshot();
    for (players, revision, expected) in [
        (
            vec![entry(9), entry(9)],
            1,
            RosterError::NoncanonicalPlayers,
        ),
        (
            vec![entry(12), entry(9)],
            1,
            RosterError::NoncanonicalPlayers,
        ),
        (vec![entry(3), entry(12)], 1, RosterError::MissingHost),
        (vec![entry(9)], 1, RosterError::MissingLocalPlayer),
        (vec![entry(9), entry(12)], 0, RosterError::InvalidRevision),
        (
            (0..=MAX_ROSTER_PLAYERS as u64).map(entry).collect(),
            100,
            RosterError::TooManyPlayers,
        ),
    ] {
        assert_eq!(
            roster.install_snapshot(
                RosterSnapshot { revision, players },
                PlayerId(9),
                PlayerId(12)
            ),
            Err(expected)
        );
        assert_eq!(roster.snapshot(), original);
    }
}
#[test]
fn presence_revisions_are_contiguous_consistent_and_checked() {
    let mut roster = PlayerRoster::host_only(PlayerId(9), None)
        .prepare_join(entry(12))
        .unwrap();
    for event in [
        PresenceMessage::PlayerJoined {
            revision: 1,
            player: entry(13),
        },
        PresenceMessage::PlayerJoined {
            revision: 0,
            player: entry(13),
        },
        PresenceMessage::PlayerJoined {
            revision: 3,
            player: entry(13),
        },
        PresenceMessage::PlayerJoined {
            revision: 2,
            player: entry(12),
        },
        PresenceMessage::PlayerLeft {
            revision: 2,
            player: PlayerId(13),
        },
        PresenceMessage::PlayerLeft {
            revision: 2,
            player: PlayerId(9),
        },
    ] {
        let before = roster.snapshot();
        assert!(roster.apply_presence(event).is_err());
        assert_eq!(roster.snapshot(), before);
    }
    roster
        .apply_presence(PresenceMessage::PlayerLeft {
            revision: 2,
            player: PlayerId(12),
        })
        .unwrap();
    assert!(roster
        .apply_presence(PresenceMessage::PlayerLeft {
            revision: 2,
            player: PlayerId(12)
        })
        .is_err());
    roster.revision = u64::MAX;
    assert_eq!(
        roster.prepare_join(entry(12)),
        Err(RosterError::RevisionExhausted)
    );
    assert_eq!(
        roster.remove_ready(PlayerId(12)),
        Err(RosterError::RevisionExhausted)
    );
}
#[test]
fn roster_count_and_decode_are_bounded() {
    let mut roster = PlayerRoster::host_only(PlayerId(0), None);
    for id in 1..MAX_ROSTER_PLAYERS as u64 {
        roster = roster.prepare_join(entry(id)).unwrap();
    }
    assert_eq!(roster.len(), MAX_ROSTER_PLAYERS);
    assert_eq!(
        roster.prepare_join(entry(100)),
        Err(RosterError::TooManyPlayers)
    );
    assert_eq!(
        postcard::from_bytes::<RosterSnapshot>(&postcard::to_allocvec(&roster.snapshot()).unwrap())
            .unwrap(),
        roster.snapshot()
    );
    let mut forged = postcard::to_allocvec(&0u64).unwrap();
    forged.extend(postcard::to_allocvec(&u64::MAX).unwrap());
    assert!(postcard::from_bytes::<RosterSnapshot>(&forged).is_err());
    let oversized = RosterSnapshot {
        revision: 100,
        players: (0..=MAX_ROSTER_PLAYERS as u64).map(entry).collect(),
    };
    assert!(
        postcard::from_bytes::<RosterSnapshot>(&postcard::to_allocvec(&oversized).unwrap())
            .is_err()
    );
    assert!(
        serde_json::from_value::<RosterSnapshot>(serde_json::to_value(oversized).unwrap()).is_err()
    );
    roster.clear();
    assert!(roster.is_empty());
    assert_eq!(roster.revision(), 0);
}

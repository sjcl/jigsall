use super::*;
use bevy::math::{UVec2, Vec2};
use jigsall_core::{GENERATOR_VERSION, MAX_PIECES};
use serde::de::value::{SeqAccessDeserializer, UnitDeserializer};

fn snapshot() -> GameSnapshot {
    GameSnapshot {
        schema_version: SNAPSHOT_SCHEMA_VERSION,
        session: SessionId(55),
        image_hash: ImageHash([0x42; 32]),
        cursor: AuthorityCursor::new(3, 7),
        definition: PuzzleDefinition {
            generator_version: GENERATOR_VERSION,
            seed: 42,
            grid_size: UVec2::new(5, 2),
            image_size: UVec2::new(500, 200),
            snap_distance: 5.0,
            rotation_enabled: true,
        },
        next_z_order: 10,
        pieces: (0..10)
            .map(|index| SnapshotPieceState {
                position: Vec2::new(index as f32 * 3.0, -7.0),
                z_order: index,
                flags: 0,
            })
            .collect(),
    }
}

fn expected(snapshot: &GameSnapshot) -> SnapshotExpectation<'_> {
    SnapshotExpectation {
        session: snapshot.session,
        image_hash: snapshot.image_hash,
        cursor: snapshot.cursor,
        definition: &snapshot.definition,
    }
}

#[test]
fn snapshot_roundtrips_json_and_postcard_with_unchanged_wire_order() {
    let snapshot = snapshot();
    snapshot.validate(expected(&snapshot)).unwrap();
    let json = serde_json::to_vec(&snapshot).unwrap();
    assert_eq!(
        serde_json::from_slice::<GameSnapshot>(&json).unwrap(),
        snapshot
    );
    let wire = postcard::to_allocvec(&snapshot).unwrap();
    assert_eq!(
        postcard::from_bytes::<GameSnapshot>(&wire).unwrap(),
        snapshot
    );
    // Postcard structs encode the same sequence of fields as this pre-change order.
    let original_fields = (
        snapshot.schema_version,
        snapshot.session,
        snapshot.image_hash,
        snapshot.cursor,
        &snapshot.definition,
        snapshot.next_z_order,
        &snapshot.pieces,
    );
    assert_eq!(wire, postcard::to_allocvec(&original_fields).unwrap());
}

#[test]
fn snapshot_rotation_mode_is_required_and_must_match_the_session() {
    let mut snapshot = snapshot();
    let mut json = serde_json::to_value(&snapshot).unwrap();
    json["definition"]
        .as_object_mut()
        .unwrap()
        .remove("rotation_enabled");
    assert!(serde_json::from_value::<GameSnapshot>(json).is_err());
    let enabled = snapshot.clone();
    snapshot.definition.rotation_enabled = false;
    assert_eq!(
        snapshot.validate(expected(&enabled)),
        Err(SnapshotError::WrongDefinition)
    );
    snapshot.validate(expected(&snapshot)).unwrap();
    snapshot.pieces[0].flags = jigsall_core::with_rotation(0, 1);
    assert_eq!(
        snapshot.validate(expected(&snapshot)),
        Err(SnapshotError::RotationDisabled(jigsall_core::PieceId(0)))
    );
}

#[test]
fn decoded_piece_counts_still_require_exact_definition_match() {
    for count in [9, 10, 11] {
        let mut snapshot = snapshot();
        snapshot.pieces.resize(count, snapshot.pieces[0]);
        let json = serde_json::from_slice::<GameSnapshot>(&serde_json::to_vec(&snapshot).unwrap())
            .unwrap();
        let wire = postcard::from_bytes::<GameSnapshot>(&postcard::to_allocvec(&snapshot).unwrap())
            .unwrap();
        for decoded in [json, wire] {
            let result = decoded.validate(expected(&snapshot));
            if count == 10 {
                assert_eq!(result, Ok(()));
            } else {
                assert_eq!(
                    result,
                    Err(SnapshotError::WrongPieceCount {
                        expected: 10,
                        actual: count,
                    })
                );
            }
        }
    }
}

/// Generates real piece decodes without keeping a million-element input fixture.
struct RepeatedPieces<'a> {
    remaining: usize,
    hint: Option<usize>,
    wire_piece: &'a [u8],
    decoded: &'a mut usize,
    overflow_probes: &'a mut usize,
}
impl<'de> serde::de::SeqAccess<'de> for RepeatedPieces<'de> {
    type Error = serde::de::value::Error;

    fn next_element_seed<T: serde::de::DeserializeSeed<'de>>(
        &mut self,
        seed: T,
    ) -> Result<Option<T::Value>, Self::Error> {
        if self.remaining == 0 {
            return Ok(None);
        }
        self.remaining -= 1;
        if *self.decoded == MAX_PIECES {
            *self.overflow_probes += 1;
            // IgnoredAny accepts this marker; a full SnapshotPieceState does not.
            return seed.deserialize(UnitDeserializer::new()).map(Some);
        }
        *self.decoded += 1;
        seed.deserialize(&mut postcard::Deserializer::from_bytes(self.wire_piece))
            .map(Some)
            .map_err(serde::de::Error::custom)
    }

    fn size_hint(&self) -> Option<usize> {
        self.hint
    }
}

#[test]
fn huge_size_hint_does_not_reserve_or_reject_tiny_sequences() {
    let piece = snapshot().pieces[0];
    let wire_piece = postcard::to_allocvec(&piece).unwrap();
    for count in [0, 1, 3] {
        let mut decoded = 0;
        let mut overflow_probes = 0;
        let pieces = deserialize_snapshot_pieces(SeqAccessDeserializer::new(RepeatedPieces {
            remaining: count,
            hint: Some(usize::MAX),
            wire_piece: &wire_piece,
            decoded: &mut decoded,
            overflow_probes: &mut overflow_probes,
        }))
        .unwrap();
        assert_eq!(pieces, vec![piece; count]);
        assert_eq!(decoded, count);
        assert_eq!(overflow_probes, 0);
        if count == 0 {
            assert_eq!(pieces.capacity(), 0);
        } else {
            assert!(pieces.capacity() <= 16);
        }
    }
}

#[test]
fn actual_piece_count_is_bounded_independently_of_size_hint() {
    let wire_piece = postcard::to_allocvec(&snapshot().pieces[0]).unwrap();
    for hint in [None, Some(0), Some(usize::MAX)] {
        for count in [MAX_PIECES - 1, MAX_PIECES, MAX_PIECES + 1] {
            let mut decoded = 0;
            let mut overflow_probes = 0;
            let result = deserialize_snapshot_pieces(SeqAccessDeserializer::new(RepeatedPieces {
                remaining: count,
                hint,
                wire_piece: &wire_piece,
                decoded: &mut decoded,
                overflow_probes: &mut overflow_probes,
            }));
            assert_eq!(decoded, count.min(MAX_PIECES));
            if count <= MAX_PIECES {
                assert_eq!(result.unwrap().len(), count);
                assert_eq!(overflow_probes, 0);
            } else {
                assert_eq!(result.unwrap_err().to_string(), "Too many snapshot pieces");
                assert_eq!(overflow_probes, 1);
            }
        }
    }
}

#[test]
fn malformed_and_truncated_postcard_piece_lengths_are_errors() {
    let mut empty = snapshot();
    empty.pieces.clear();
    let mut header = postcard::to_allocvec(&empty).unwrap();
    assert_eq!(header.pop(), Some(0)); // Final field is the empty pieces sequence.
    let piece = postcard::to_allocvec(&snapshot().pieces[0]).unwrap();
    for count in [MAX_PIECES + 1, usize::MAX] {
        for payload in [&[][..], &piece[..3]] {
            let mut truncated = header.clone();
            truncated.extend(postcard::to_allocvec(&count).unwrap());
            truncated.extend_from_slice(payload);
            assert!(postcard::from_bytes::<GameSnapshot>(&truncated).is_err());
        }
    }
    let mut malformed = header;
    malformed.extend(std::iter::repeat_n(
        0xff,
        (usize::BITS as usize).div_ceil(7) + 1,
    ));
    assert!(postcard::from_bytes::<GameSnapshot>(&malformed).is_err());
}

#[test]
fn postcard_snapshot_piece_limit_also_applies_through_join_baseline() {
    use crate::multiplayer::{JoinBaseline, JOIN_BASELINE_SCHEMA_VERSION};

    let mut baseline = JoinBaseline {
        schema_version: JOIN_BASELINE_SCHEMA_VERSION,
        snapshot: snapshot(),
        active_drags: Vec::new(),
    };
    let wire = postcard::to_allocvec(&baseline).unwrap();
    assert_eq!(
        postcard::from_bytes::<JoinBaseline>(&wire).unwrap(),
        baseline
    );
    baseline.snapshot.definition.grid_size = UVec2::splat(1000);
    baseline.snapshot.next_z_order = MAX_PIECES as u32;
    baseline
        .snapshot
        .pieces
        .resize(MAX_PIECES, baseline.snapshot.pieces[0]);
    let wire = postcard::to_allocvec(&baseline).unwrap();
    assert_eq!(
        postcard::from_bytes::<JoinBaseline>(&wire).unwrap(),
        baseline
    );
    baseline.snapshot.pieces.push(baseline.snapshot.pieces[0]);
    let snapshot_wire = postcard::to_allocvec(&baseline.snapshot).unwrap();
    assert!(postcard::from_bytes::<GameSnapshot>(&snapshot_wire).is_err());
    let baseline_wire = postcard::to_allocvec(&baseline).unwrap();
    assert!(postcard::from_bytes::<JoinBaseline>(&baseline_wire).is_err());
}

use super::*;
use crate::{
    checkpoint::PuzzleCheckpoint,
    play_area::{component_center, PIVOT_VISITS},
};
use bevy::math::{UVec2, Vec2};
use puzzella_core::{
    protocol::PieceTarget,
    session::{AuthorityCursor, ImageHash, SessionDefinition},
    PieceBitSet, PieceId, GENERATOR_VERSION,
};

const PLAYER: PlayerId = PlayerId(10);
struct Fixture {
    definition: PuzzleDefinition,
    store: PieceDataStore,
    session: AuthoritySession,
    contexts: ProtocolDragContexts,
}
impl Fixture {
    fn new(count: usize) -> Self {
        let grid_size = if count == 1_000_000 {
            UVec2::splat(1000)
        } else {
            UVec2::new(count as u32, 1)
        };
        let definition = PuzzleDefinition {
            generator_version: GENERATOR_VERSION,
            seed: 42,
            grid_size,
            image_size: UVec2::splat(16_384),
            snap_distance: 5.0,
        };
        let mut store = PieceDataStore::default();
        store.initialize(
            (0..count)
                .map(|i| definition.correct_position(PieceId(i as u32)) + Vec2::splat(1000.0))
                .collect(),
        );
        Self {
            definition,
            store,
            session: AuthoritySession::new(
                SessionDefinition {
                    id: SessionId(123),
                    image_hash: ImageHash([0; 32]),
                },
                PLAYER,
                AuthorityCursor::new(3, 0),
            ),
            contexts: ProtocolDragContexts::default(),
        }
    }
    fn apply(
        &mut self,
        sequence: ClientCommandSequence,
        command: ProtocolPieceCommand,
    ) -> Result<HostCommandOutcome, ProtocolCommandError> {
        self.contexts.apply_replicated(
            &mut self.session,
            &mut self.store,
            PLAYER,
            &ProtocolCommandEnvelope {
                session: SessionId(123),
                authority_epoch: AuthorityEpoch(3),
                player: PLAYER,
                sequence,
                command,
            },
            Some(&self.definition),
            puzzella_core::LOCAL_PLAYER,
        )
    }
    fn grab_all(&mut self) {
        let mut members = PieceBitSet::new(self.store.len());
        members.fill();
        let target = PieceTarget::from_selection(&self.store.connectivity, &members).unwrap();
        self.apply(
            ClientCommandSequence::Control(0),
            ProtocolPieceCommand::Grab { target },
        )
        .unwrap();
    }
    fn update(
        &mut self,
        basis: u64,
        tick: u64,
        delta: Vec2,
    ) -> Result<HostCommandOutcome, ProtocolCommandError> {
        self.apply(
            ClientCommandSequence::Move {
                after_control_sequence: basis,
                tick,
            },
            ProtocolPieceCommand::DragUpdate { delta },
        )
    }
    fn checkpoint(&self) -> PuzzleCheckpoint {
        PuzzleCheckpoint::capture(&self.store, &self.definition, ImageHash([0; 32])).unwrap()
    }
}

#[test]
fn hostile_finite_and_nonfinite_deltas_never_publish_or_partially_mutate() {
    for delta in [
        Vec2::new(1e37, 0.0),
        Vec2::new(-1e37, 0.0),
        Vec2::new(0.0, 1e37),
        Vec2::new(0.0, -1e37),
        Vec2::splat(f32::MAX),
        Vec2::splat(f32::NAN),
        Vec2::splat(f32::INFINITY),
        Vec2::splat(f32::NEG_INFINITY),
    ] {
        for kind in 0..3 {
            let mut f = Fixture::new(3);
            f.store.connectivity.union(PieceId(0), PieceId(1));
            f.grab_all();
            f.update(0, 0, Vec2::ONE).unwrap();
            f.store.dirty_pieces.clear();
            let states = f.store.states.clone();
            let holds = f.store.held_by.clone();
            let connectivity = f.store.connectivity.clone();
            let drags = f.contexts.players.clone();
            let validation = f.contexts.validation.clone();
            let selection = f.store.selected_pieces.clone();
            let cursor = f.session.cursor();
            let result = match kind {
                0 => f.update(0, 1, delta),
                1 => f.apply(
                    ClientCommandSequence::Control(1),
                    ProtocolPieceCommand::Release {
                        grab_sequence: 0,
                        final_delta: delta,
                    },
                ),
                _ => f.apply(
                    ClientCommandSequence::Control(1),
                    ProtocolPieceCommand::RotateDrag {
                        grab_sequence: 0,
                        final_delta: delta,
                        through_tick: Some(0),
                        quarter_turns: 1,
                    },
                ),
            };
            assert!(
                matches!(result, Err(ProtocolCommandError::InvalidDelta)),
                "{kind}: {delta:?}: {result:?}"
            );
            assert_eq!(f.store.states, states);
            assert_eq!(f.store.held_by, holds);
            assert_eq!(f.store.connectivity, connectivity);
            assert_eq!(f.contexts.players, drags);
            assert_eq!(f.contexts.validation, validation);
            assert_eq!(f.store.selected_pieces, selection);
            assert!(f.store.dirty_pieces.is_empty());
            assert!(f.store.component_root_dirty.is_empty());
            assert_eq!(f.session.cursor(), cursor);
            // Rejected reliable controls remain consumed, as before this fix.
            let sequence = if kind == 0 { 1 } else { 2 };
            f.apply(
                ClientCommandSequence::Control(sequence),
                ProtocolPieceCommand::Release {
                    grab_sequence: 0,
                    final_delta: Vec2::ZERO,
                },
            )
            .unwrap();
            assert!(f.store.held_by.is_empty());
        }
    }
}

#[test]
fn area_edge_is_accepted_and_next_float_outside_is_rejected() {
    for sign in [-1.0, 1.0] {
        let mut f = Fixture::new(1);
        f.store.states[0].position = Vec2::ZERO;
        let edge = LogicalPlayArea::from_definition(&f.definition)
            .unwrap()
            .half_extents
            .x as f32;
        f.grab_all();
        let inside = Vec2::new(sign * edge, 0.0);
        assert!(f.update(0, 0, inside).unwrap().drag_update.is_some());
        assert!(matches!(
            f.update(0, 1, Vec2::new(sign * edge.next_up(), 0.0)),
            Err(ProtocolCommandError::InvalidDelta)
        ));
        f.apply(
            ClientCommandSequence::Control(1),
            ProtocolPieceCommand::Release {
                grab_sequence: 0,
                final_delta: inside,
            },
        )
        .unwrap();
        assert_eq!(f.store.states[0].position, inside);
        f.checkpoint();
    }
}

#[test]
fn displayed_f32_rounding_cannot_publish_a_pivot_outside_a_fractional_edge() {
    let mut f = Fixture::new(165);
    f.definition.grid_size = UVec2::new(11, 15);
    f.definition.image_size = UVec2::new(6276, 10697);
    let positions = puzzella_puzzle::placement::generate_placement_grid(
        11,
        15,
        6276.0 / 11.0,
        10697.0 / 15.0,
        6276.0,
        10697.0,
        f.definition.seed,
    );
    let position = *positions.iter().find(|p| p.x == -5990.7275).unwrap();
    f.store.initialize(positions);
    f.store.states[0].position = position;
    let target = PieceTarget::Component(
        puzzella_core::protocol::ComponentRef::from_member(&f.store.connectivity, PieceId(0))
            .unwrap(),
    );
    f.apply(
        ClientCommandSequence::Control(0),
        ProtocolPieceCommand::Grab { target },
    )
    .unwrap();
    let delta = Vec2::new(34_231.273, 0.0);
    let area = LogicalPlayArea::from_definition(&f.definition).unwrap();
    assert_eq!(
        (position.as_dvec2() + delta.as_dvec2()).x,
        area.half_extents.x
    );
    assert!(!area.contains((position + delta).as_dvec2()));
    assert!(matches!(
        f.update(0, 0, delta),
        Err(ProtocolCommandError::InvalidDelta)
    ));
    let inside = Vec2::new(delta.x.next_down(), 0.0);
    assert!(f.update(0, 1, inside).unwrap().drag_update.is_some());
    f.apply(
        ClientCommandSequence::Control(1),
        ProtocolPieceCommand::Release {
            grab_sequence: 0,
            final_delta: inside,
        },
    )
    .unwrap();
    f.checkpoint();
}

#[test]
fn edge_rotation_can_release_without_moving_the_pointer() {
    let mut f = Fixture::new(156);
    f.definition.grid_size = UVec2::new(13, 12);
    f.definition.image_size = UVec2::new(321, 353);
    let area = LogicalPlayArea::from_definition(&f.definition).unwrap();
    let correct = (0..4).map(|id| f.definition.correct_position(PieceId(id)));
    let pivot = component_center(correct).unwrap();
    let offset = Vec2::new((area.half_extents.x - pivot.x) as f32, 0.0);
    for id in 0..156 {
        f.store.states[id].position = f.definition.correct_position(PieceId(id as u32));
        if id < 4 {
            f.store.states[id].position += offset;
            if id != 0 {
                f.store.connectivity.union(PieceId(0), PieceId(id as u32));
            }
        }
    }
    let target = PieceTarget::Component(
        puzzella_core::protocol::ComponentRef::from_member(&f.store.connectivity, PieceId(0))
            .unwrap(),
    );
    f.store.rotate_target(&target, 1, &f.definition).unwrap();
    f.checkpoint();
    f.apply(
        ClientCommandSequence::Control(0),
        ProtocolPieceCommand::Grab { target },
    )
    .unwrap();
    f.apply(
        ClientCommandSequence::Control(1),
        ProtocolPieceCommand::Release {
            grab_sequence: 0,
            final_delta: Vec2::ZERO,
        },
    )
    .unwrap();
    assert!(f.store.held_by.is_empty());
    f.checkpoint();
}

#[test]
fn every_component_pivot_in_a_multi_grab_is_constrained() {
    let mut f = Fixture::new(2);
    let edge = LogicalPlayArea::from_definition(&f.definition)
        .unwrap()
        .half_extents
        .x as f32;
    f.store.states[0].position = Vec2::ZERO;
    f.store.states[1].position = Vec2::new(edge - 1.0, 0.0);
    f.grab_all();
    assert!(matches!(
        f.update(0, 0, Vec2::new(2.0, 0.0)),
        Err(ProtocolCommandError::InvalidDelta)
    ));
    assert!(f.update(0, 1, Vec2::new(1.0, 0.0)).is_ok());
}

#[test]
fn overhanging_wide_component_rotates_and_rebases_at_the_edge() {
    let mut f = Fixture::new(1000);
    let area = LogicalPlayArea::from_definition(&f.definition).unwrap();
    let pivot = Vec2::new(area.half_extents.x as f32 - 10.0, 0.0);
    for i in 0..1000 {
        f.store.states[i].position = f.definition.correct_position(PieceId(i as u32)) + pivot;
        if i != 0 {
            f.store.connectivity.union(PieceId(0), PieceId(i as u32));
        }
    }
    assert!(f.store.states[999].position.x > area.half_extents.x as f32);
    f.checkpoint();
    // Ordinary Rotate also keeps its legal pivot while geometry overhangs.
    let target = PieceTarget::Component(
        puzzella_core::protocol::ComponentRef::from_member(&f.store.connectivity, PieceId(0))
            .unwrap(),
    );
    let rotated = f.store.rotate_target(&target, 1, &f.definition).unwrap();
    assert_eq!(rotated.applied.rotated, 1000);
    f.grab_all();
    assert!(f.update(0, 0, Vec2::new(5.0, 0.0)).is_ok());
    f.apply(
        ClientCommandSequence::Control(1),
        ProtocolPieceCommand::RotateDrag {
            grab_sequence: 0,
            final_delta: Vec2::new(5.0, 0.0),
            through_tick: Some(0),
            quarter_turns: 1,
        },
    )
    .unwrap();
    let center = component_center(f.store.states.iter().map(|s| s.position)).unwrap();
    assert_eq!(center, (pivot + Vec2::new(5.0, 0.0)).as_dvec2());
    assert_eq!(f.contexts.players[&PLAYER].delta, Vec2::ZERO);
    assert!(matches!(
        f.update(1, 1, Vec2::new(6.0, 0.0)),
        Err(ProtocolCommandError::InvalidDelta)
    ));
    f.update(1, 2, Vec2::new(-30.0, 0.0)).unwrap();
    assert!(matches!(
        f.apply(
            ClientCommandSequence::Control(2),
            ProtocolPieceCommand::Release {
                grab_sequence: 0,
                final_delta: Vec2::new(6.0, 0.0)
            }
        ),
        Err(ProtocolCommandError::InvalidDelta)
    ));
    f.apply(
        ClientCommandSequence::Control(3),
        ProtocolPieceCommand::Release {
            grab_sequence: 0,
            final_delta: Vec2::new(-30.0, 0.0),
        },
    )
    .unwrap();
    f.checkpoint();
    assert!(f.store.held_by.is_empty());
}

#[test]
fn million_piece_dense_drag_validation_visits_no_pieces_or_components() {
    let mut f = Fixture::new(1_000_000);
    f.grab_all();
    assert!(matches!(
        f.contexts.players[&PLAYER].target,
        ActiveDragTarget::Dense(_)
    ));
    let before = f.store.states.clone();
    let members = if let ActiveDragTarget::Dense(dense) = &f.contexts.players[&PLAYER].target {
        dense.members.words().clone()
    } else {
        unreachable!()
    };
    PIVOT_VISITS.with(|visits| visits.set(0));
    for tick in 0..100 {
        f.update(0, tick, Vec2::splat(tick as f32)).unwrap();
    }
    let context = f.contexts.players[&PLAYER].clone();
    assert!(matches!(
        f.update(0, 100, Vec2::splat(1e37)),
        Err(ProtocolCommandError::InvalidDelta)
    ));
    assert_eq!(f.contexts.players[&PLAYER], context);
    assert_eq!(PIVOT_VISITS.with(|visits| visits.get()), 0);
    assert_eq!(f.store.states, before);
    let ActiveDragTarget::Dense(dense) = &f.contexts.players[&PLAYER].target else {
        unreachable!()
    };
    assert!(std::sync::Arc::ptr_eq(&members, dense.members.words()));
    assert!(std::mem::size_of::<DragValidation>() <= 64);
}

#[test]
fn checkpoint_rejects_far_component_pivots_transactionally() {
    let f = Fixture::new(2);
    let mut checkpoint = f.checkpoint();
    checkpoint.pieces[0].position = Vec2::splat(1e37);
    let mut restored = PieceDataStore::default();
    restored.initialize(vec![Vec2::ONE]);
    let before = restored.states.clone();
    assert!(matches!(
        checkpoint.install(&mut restored),
        Err(crate::checkpoint::CheckpointError::OutsidePlayArea(
            PieceId(0)
        ))
    ));
    assert_eq!(restored.states, before);
    assert_eq!(restored.len(), 1);
}

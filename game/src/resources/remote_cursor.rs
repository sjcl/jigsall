//! O(players) presentation only. No piece, selection, authority or GPU resources.
use crate::{
    network::cursor::{CURSOR_SETTLE_EPSILON, CURSOR_SETTLE_SECS, CURSOR_SMOOTHING_SECS},
    players::MAX_ROSTER_PLAYERS,
};
use bevy::prelude::*;
use jigsall_core::PlayerId;
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Debug, PartialEq)]
pub struct RemoteCursor {
    pub target_world_position: Vec2,
    pub displayed_world_position: Vec2,
    age: f64,
}
#[derive(Resource, Default, Debug, PartialEq)]
pub struct RemoteCursorPresentation {
    entries: BTreeMap<PlayerId, RemoteCursor>,
    revision: u64,
}
impl RemoteCursorPresentation {
    pub fn cursors(&self) -> impl ExactSizeIterator<Item = (PlayerId, &RemoteCursor)> {
        self.entries
            .iter()
            .map(|(&player, cursor)| (player, cursor))
    }
    pub(crate) fn clear(&mut self) {
        if !self.entries.is_empty() {
            self.entries.clear();
            self.revision += 1;
        }
    }
    pub(crate) fn remove(&mut self, player: PlayerId) {
        if self.entries.remove(&player).is_some() {
            self.revision += 1;
        }
    }
    pub(crate) fn set_target(&mut self, player: PlayerId, position: Option<Vec2>) {
        let Some(position) = position else {
            self.remove(player);
            return;
        };
        if let Some(cursor) = self.entries.get_mut(&player) {
            if cursor.target_world_position != position {
                cursor.target_world_position = position;
                cursor.age = 0.0;
                self.revision += 1;
            }
        } else if self.entries.len() < MAX_ROSTER_PLAYERS {
            self.entries.insert(
                player,
                RemoteCursor {
                    target_world_position: position,
                    displayed_world_position: position,
                    age: 0.0,
                },
            );
            self.revision += 1;
        }
    }
    pub(crate) fn replace(&mut self, entries: impl Iterator<Item = (PlayerId, Vec2)>) {
        let mut present = BTreeSet::new();
        for (player, position) in entries.take(MAX_ROSTER_PLAYERS) {
            present.insert(player);
            self.set_target(player, Some(position));
        }
        let previous_len = self.entries.len();
        self.entries.retain(|player, _| present.contains(player));
        if self.entries.len() != previous_len {
            self.revision += 1;
        }
    }
    pub(crate) fn advance(&mut self, dt: f64) -> bool {
        if dt.is_nan() || dt <= 0.0 {
            return false;
        }
        let alpha = -(-dt / CURSOR_SMOOTHING_SECS).exp_m1();
        let mut changed = false;
        for cursor in self.entries.values_mut() {
            let target = cursor.target_world_position;
            let previous = cursor.displayed_world_position;
            if previous == target {
                continue;
            }
            cursor.age += dt;
            let mut displayed = target;
            if cursor.age < CURSOR_SETTLE_SECS {
                for axis in 0..2 {
                    let from = f64::from(previous[axis]);
                    let to = f64::from(target[axis]);
                    displayed[axis] =
                        (from + (to - from) * alpha).clamp(from.min(to), from.max(to)) as f32;
                }
                if (0..2).all(|axis| {
                    (f64::from(displayed[axis]) - f64::from(target[axis])).abs()
                        <= CURSOR_SETTLE_EPSILON
                }) {
                    displayed = target;
                }
            }
            if displayed != previous {
                cursor.displayed_world_position = displayed;
                changed = true;
            }
        }
        if changed {
            self.revision += 1;
        }
        changed
    }
    pub(crate) fn revision(&self) -> u64 {
        self.revision
    }
}
pub(crate) fn smooth_remote_cursors(
    time: Res<Time<Real>>,
    mut presentation: ResMut<RemoteCursorPresentation>,
) {
    // Avoid Bevy change detection on settled frames.
    let state = presentation.bypass_change_detection();
    if state.advance(time.delta_secs_f64()) {
        presentation.set_changed();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn cursor_smoothing_matches_elapsed_time_no_overshoot_and_exact_settle() {
        let initial = Vec2::new(-30.0, 20.0);
        let target = Vec2::new(100.0, -70.0);
        let expected =
            initial + (target - initial) * (1.0 - (-0.1 / CURSOR_SMOOTHING_SECS).exp()) as f32;
        for hz in [30, 60, 120, 360] {
            let mut s = RemoteCursorPresentation::default();
            s.set_target(PlayerId(1), Some(initial));
            s.set_target(PlayerId(1), Some(target));
            for _ in 0..hz / 10 {
                s.advance(1.0 / hz as f64);
            }
            assert!(s.entries[&PlayerId(1)]
                .displayed_world_position
                .abs_diff_eq(expected, 0.0001));
            for next in [Vec2::new(-70.0, 90.0), Vec2::ZERO, Vec2::splat(1_000_000.)] {
                s.set_target(PlayerId(1), Some(next));
                for _ in 0..hz {
                    let prev = s.entries[&PlayerId(1)].displayed_world_position;
                    s.set_target(PlayerId(1), Some(next)); // identical heartbeats cannot restart age
                    s.advance(1.0 / hz as f64);
                    let pos = s.entries[&PlayerId(1)].displayed_world_position;
                    for axis in 0..2 {
                        assert!(
                            pos[axis] >= prev[axis].min(next[axis])
                                && pos[axis] <= prev[axis].max(next[axis])
                        );
                    }
                }
                assert_eq!(s.entries[&PlayerId(1)].displayed_world_position, next);
                let prev = s.entries.clone();
                assert!(!s.advance(1.0));
                assert_eq!(s.entries, prev);
            }
        }
    }
    #[test]
    fn cursor_smoothing_65_players_never_accesses_million_piece_state_or_dirty_upload() {
        use crate::resources::{PieceDataStore, PieceUpload};
        let mut store = PieceDataStore::default();
        store.initialize(vec![Vec2::ZERO; 1_000_000]);
        let states = store.states.clone();
        let mut s = RemoteCursorPresentation::default();
        for id in 0..MAX_ROSTER_PLAYERS {
            s.set_target(PlayerId(id as u64), Some(Vec2::ZERO));
        }
        s.set_target(PlayerId(66), Some(Vec2::ONE));
        assert_eq!(s.entries.len(), 65);
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .insert_resource(store)
            .init_resource::<PieceUpload>()
            .add_systems(Last, crate::resources::pieces::prepare_piece_upload);
        app.update();
        app.update();
        let revision = app.world().resource::<PieceUpload>().revision;
        crate::resources::pieces::without_piece_state_access(|| {
            for frame in 1..100 {
                for id in 0..65 {
                    s.set_target(PlayerId(id), Some(Vec2::splat(frame as f32)));
                }
                s.advance(1.0 / 120.0);
            }
        });
        app.update();
        assert_eq!(app.world().resource::<PieceUpload>().revision, revision);
        assert_eq!(app.world().resource::<PieceDataStore>().states, states);
        s.clear();
        assert_eq!(s.cursors().count(), 0);
    }
    #[test]
    fn cursor_settled_resource_does_not_mark_bevy_change_detection() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .init_resource::<RemoteCursorPresentation>()
            .add_systems(Update, smooth_remote_cursors);
        app.world_mut()
            .resource_mut::<RemoteCursorPresentation>()
            .set_target(PlayerId(1), Some(Vec2::ONE));
        app.update();
        let before = app
            .world()
            .get_resource_ref::<RemoteCursorPresentation>()
            .unwrap()
            .last_changed();
        app.update();
        assert_eq!(
            app.world()
                .get_resource_ref::<RemoteCursorPresentation>()
                .unwrap()
                .last_changed(),
            before
        );
    }
}

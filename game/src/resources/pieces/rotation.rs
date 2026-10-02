//! Operation-local rigid rotation plans. Nothing is retained on pointer frames.
use super::*;
use bevy::math::DVec2;
use puzzella_core::{
    add_quarter_turns, matches_transform,
    protocol::{PieceTarget, RejectedComponentRef, ResolvedPieceTarget},
};

pub(crate) struct RotationResult {
    pub applied: AppliedCommand,
    pub accepted: PieceTarget,
    pub rejected: Vec<RejectedComponentRef>,
    pub roots: Vec<PieceId>,
}

struct RotationPlan {
    minimum: PieceId,
    rotation: u32,
    translation: DVec2,
}

impl PieceDataStore {
    fn rotation_plan(
        &self,
        minimum: PieceId,
        quarter_turns: i8,
        definition: &PuzzleDefinition,
    ) -> Option<RotationPlan> {
        if !self.contains(minimum) {
            return None;
        }
        let geometry = definition.geometry();
        let representative = self.states[minimum.0 as usize];
        let old_rotation = decode_rotation(representative.flags);
        let old_translation = representative.position
            - rotate_quarter(geometry.correct_position(minimum), old_rotation);
        let rotation = add_quarter_turns(old_rotation, quarter_turns);
        let mut world_min = Vec2::splat(f32::INFINITY);
        let mut world_max = Vec2::splat(f32::NEG_INFINITY);
        let mut correct_min = world_min;
        let mut correct_max = world_max;
        for id in self.connectivity.iter_component(minimum) {
            let state = self.states[id.0 as usize];
            let correct = geometry.correct_position(id);
            let in_drag = self
                .drag
                .members
                .get(id.0 as usize / 32)
                .is_some_and(|word| word & (1 << (id.0 % 32)) != 0);
            if !self.is_selectable(id)
                || in_drag
                || decode_rotation(state.flags) != old_rotation
                || !matches_transform(state.position, correct, old_rotation, old_translation)
            {
                return None;
            }
            world_min = world_min.min(state.position);
            world_max = world_max.max(state.position);
            let rotated = rotate_quarter(correct, rotation);
            correct_min = correct_min.min(rotated);
            correct_max = correct_max.max(rotated);
        }
        // Compute the world AABB center and affine translation in f64. This avoids
        // cancellation/overflow when changing the canonical orientation.
        // Rebuild from canonical coordinates, never by rotating rounded positions.
        let pivot = (world_min.as_dvec2() + world_max.as_dvec2()) * 0.5;
        let translation = pivot - (correct_min.as_dvec2() + correct_max.as_dvec2()) * 0.5;
        self.connectivity
            .iter_component(minimum)
            .all(|id| {
                (rotate_quarter(geometry.correct_position(id), rotation).as_dvec2() + translation)
                    .as_vec2()
                    .is_finite()
            })
            .then_some(RotationPlan {
                minimum,
                rotation,
                translation,
            })
    }

    /// Replica preflight: reject the whole accepted event before any mutation.
    pub(crate) fn can_rotate_target(
        &self,
        target: &puzzella_core::protocol::PieceTarget,
        quarter_turns: i8,
        definition: &PuzzleDefinition,
    ) -> bool {
        if definition.validate().is_err() || definition.piece_count() != self.len() {
            return false;
        }
        let Ok(resolved) = target.resolve(&self.connectivity) else {
            return false;
        };
        if !resolved.rejected.is_empty() {
            return false;
        }
        match resolved.target {
            ResolvedPieceTarget::Sparse(refs) => refs.iter().all(|r| {
                self.rotation_plan(r.member, quarter_turns, definition)
                    .is_some()
            }),
            ResolvedPieceTarget::Dense(members) => members
                .iter()
                .filter(|&id| self.connectivity.minimum_member(id) == id)
                .all(|id| self.rotation_plan(id, quarter_turns, definition).is_some()),
        }
    }

    /// Revalidate topology, holds, enabled/placed state and the entire rigid body.
    /// Sparse stale entries follow Grab's partial acceptance; stale dense topology
    /// rejects before gameplay. Independent components each keep their own pivot.
    pub(crate) fn rotate_target(
        &mut self,
        target: &puzzella_core::protocol::PieceTarget,
        quarter_turns: i8,
        definition: &PuzzleDefinition,
    ) -> Result<RotationResult, puzzella_core::protocol::TargetError> {
        let resolved = target.resolve(&self.connectivity)?;
        let mut plans = Vec::new();
        let accepted = match resolved.target {
            ResolvedPieceTarget::Sparse(mut refs) => {
                refs.retain(|r| {
                    if let Some(plan) = self.rotation_plan(r.member, quarter_turns, definition) {
                        plans.push(plan);
                        true
                    } else {
                        false
                    }
                });
                if refs.len() == 1 {
                    PieceTarget::Component(refs[0])
                } else {
                    PieceTarget::Components(refs)
                }
            }
            ResolvedPieceTarget::Dense(mut members) => {
                // Iterate a shared frozen mask while rejecting whole components.
                let requested = members.clone();
                for id in requested
                    .iter()
                    .filter(|&id| self.connectivity.minimum_member(id) == id)
                {
                    if let Some(plan) = self.rotation_plan(id, quarter_turns, definition) {
                        plans.push(plan);
                    } else {
                        for member in self.connectivity.iter_component(id) {
                            members.remove(&member);
                        }
                    }
                }
                PieceTarget::from_selection(&self.connectivity, &members)?
            }
        };
        let mut applied = AppliedCommand::default();
        let roots = plans.iter().map(|p| p.minimum).collect();
        if add_quarter_turns(0, quarter_turns) != 0 {
            let geometry = definition.geometry();
            let states = &mut *self.states;
            for plan in plans {
                let singleton = self.connectivity.component_size(plan.minimum) == 1;
                for id in self.connectivity.iter_component(plan.minimum) {
                    let state = &mut states[id.0 as usize];
                    // A singleton rotates about its own position. Preserve that
                    // exact center, including tiny offsets and signed zero that
                    // would be lost even by f64 affine subtraction/addition.
                    if !singleton {
                        state.position =
                            (rotate_quarter(geometry.correct_position(id), plan.rotation)
                                .as_dvec2()
                                + plan.translation)
                                .as_vec2();
                    }
                    state.flags = with_rotation(state.flags, plan.rotation);
                    self.dirty_pieces.insert(id);
                    applied.rotated += 1;
                }
            }
        }
        Ok(RotationResult {
            applied,
            accepted,
            rejected: resolved.rejected,
            roots,
        })
    }
}

#[cfg(test)]
#[path = "rotation_tests.rs"]
mod tests;

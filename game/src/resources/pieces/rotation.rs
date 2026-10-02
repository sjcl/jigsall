//! Operation-local rigid rotation plans. Nothing is retained on pointer frames.
use super::*;
use bevy::math::DVec2;
use puzzella_core::{
    add_quarter_turns, matches_transform,
    protocol::{ActiveDragTarget, PieceTarget, RejectedComponentRef, ResolvedPieceTarget},
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
    singleton_center: Option<Vec2>,
}

impl PieceDataStore {
    fn rotation_plan(
        &self,
        minimum: PieceId,
        quarter_turns: i8,
        definition: &PuzzleDefinition,
        delta: Vec2,
        owner: Option<PlayerId>,
        members: Option<&PieceBitSet>,
    ) -> Option<RotationPlan> {
        if !self.contains(minimum) || !delta.is_finite() {
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
            let eligible = if let Some(player) = owner {
                state.flags & (PLACED | ENABLED | HELD) == (ENABLED | HELD)
                    && self.held_by.get(&id) == Some(&player)
                    && members.is_none_or(|mask| mask.contains(&id))
            } else {
                self.is_selectable(id) && !in_drag
            };
            let displayed = if delta == Vec2::ZERO {
                state.position
            } else {
                state.position + delta
            };
            if !eligible
                || !displayed.is_finite()
                || decode_rotation(state.flags) != old_rotation
                || !matches_transform(state.position, correct, old_rotation, old_translation)
            {
                return None;
            }
            world_min = world_min.min(displayed);
            world_max = world_max.max(displayed);
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
                singleton_center: (self.connectivity.component_size(minimum) == 1).then(|| {
                    if delta == Vec2::ZERO {
                        representative.position
                    } else {
                        representative.position + delta
                    }
                }),
            })
    }

    fn apply_rotation_plans(
        &mut self,
        plans: Vec<RotationPlan>,
        definition: &PuzzleDefinition,
    ) -> AppliedCommand {
        let geometry = definition.geometry();
        let mut applied = AppliedCommand::default();
        let states = &mut *self.states;
        for plan in plans {
            for id in self.connectivity.iter_component(plan.minimum) {
                let state = &mut states[id.0 as usize];
                let position = plan.singleton_center.unwrap_or_else(|| {
                    (rotate_quarter(geometry.correct_position(id), plan.rotation).as_dvec2()
                        + plan.translation)
                        .as_vec2()
                });
                let flags = with_rotation(state.flags, plan.rotation);
                if position.to_array().map(f32::to_bits)
                    != state.position.to_array().map(f32::to_bits)
                    || flags != state.flags
                {
                    state.position = position;
                    state.flags = flags;
                    self.dirty_pieces.insert(id);
                    self.exact_dirty_ranges = true;
                }
                applied.rotated += 1;
            }
        }
        applied
    }

    fn drag_rotation_plans(
        &self,
        roots: &[PieceId],
        player: PlayerId,
        delta: Vec2,
        quarter_turns: i8,
        definition: &PuzzleDefinition,
        members: Option<&PieceBitSet>,
    ) -> Option<Vec<RotationPlan>> {
        if roots.is_empty()
            || definition.validate().is_err()
            || definition.piece_count() != self.len()
        {
            return None;
        }
        roots
            .iter()
            .map(|&id| {
                self.rotation_plan(id, quarter_turns, definition, delta, Some(player), members)
            })
            .collect()
    }

    pub(crate) fn rotate_local_drag(
        &mut self,
        player: PlayerId,
        members: &PieceBitSet,
        delta: Vec2,
        quarter_turns: i8,
        definition: &PuzzleDefinition,
        local_player: PlayerId,
    ) -> Option<AppliedCommand> {
        if player != local_player
            || members.bit_len() != self.len()
            || (!Arc::ptr_eq(members.words(), &self.drag.members)
                && **members.words() != *self.drag.members)
        {
            return None;
        }
        if members
            .iter()
            .any(|id| !members.contains(&self.connectivity.minimum_member(id)))
        {
            return None;
        }
        let roots: Vec<_> = members
            .iter()
            .filter(|&id| self.connectivity.minimum_member(id) == id)
            .collect();
        let plans = self.drag_rotation_plans(
            &roots,
            player,
            delta,
            quarter_turns,
            definition,
            Some(members),
        )?;
        let mut applied = self.apply_rotation_plans(plans, definition);
        self.drag.delta = Vec2::ZERO;
        applied.drag_rebased = true;
        Some(applied)
    }

    /// Shared host/replica atomic rebase. Retains the exact accepted Grab target.
    pub(crate) fn rotate_drag_target(
        &mut self,
        player: PlayerId,
        target: &ActiveDragTarget,
        delta: Vec2,
        quarter_turns: i8,
        definition: &PuzzleDefinition,
    ) -> Option<(AppliedCommand, Vec<PieceId>)> {
        let (roots, members) = match target {
            ActiveDragTarget::Sparse(refs) => {
                let roots: Vec<_> = refs
                    .iter()
                    .map(|r| r.resolve(&self.connectivity).ok())
                    .collect::<Option<_>>()?;
                (roots, None)
            }
            ActiveDragTarget::Dense(dense) => {
                let members = dense.resolve(&self.connectivity).ok()?;
                let roots = members
                    .iter()
                    .filter(|&id| self.connectivity.minimum_member(id) == id)
                    .collect();
                (roots, Some(members))
            }
        };
        let plans = self.drag_rotation_plans(
            &roots,
            player,
            delta,
            quarter_turns,
            definition,
            members.as_ref(),
        )?;
        let mut applied = self.apply_rotation_plans(plans, definition);
        applied.drag_rebased = true;
        Some((applied, roots))
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
                self.rotation_plan(r.member, quarter_turns, definition, Vec2::ZERO, None, None)
                    .is_some()
            }),
            ResolvedPieceTarget::Dense(members) => members
                .iter()
                .filter(|&id| self.connectivity.minimum_member(id) == id)
                .all(|id| {
                    self.rotation_plan(id, quarter_turns, definition, Vec2::ZERO, None, None)
                        .is_some()
                }),
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
                    if let Some(plan) = self.rotation_plan(
                        r.member,
                        quarter_turns,
                        definition,
                        Vec2::ZERO,
                        None,
                        None,
                    ) {
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
                    if let Some(plan) =
                        self.rotation_plan(id, quarter_turns, definition, Vec2::ZERO, None, None)
                    {
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
            applied = self.apply_rotation_plans(plans, definition);
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

#[cfg(test)]
#[path = "drag_rotation_tests.rs"]
mod drag_tests;

//! Shared atomic host/replica cancellation. No release/snap or delta commit.
use super::*;
use jigsall_core::protocol::{ActiveDragTarget, MAX_COMPONENT_REFS};

impl PieceDataStore {
    /// Validate exact topology, flags and all of this player's ownership before
    /// clearing anything. Sparse storage stays bounded by component count; Dense
    /// uses an operation-local bitset, never a puzzle-sized member/reference Vec.
    pub(crate) fn cancel_drag_target(
        &mut self,
        player: PlayerId,
        target: &ActiveDragTarget,
    ) -> Option<AppliedCommand> {
        if self.connectivity.len() != self.len() {
            return None;
        }
        let valid_member = |id: PieceId| {
            self.states[id.0 as usize].flags & (HELD | PLACED | ENABLED) == (HELD | ENABLED)
                && self.held_by.get(&id) == Some(&player)
        };
        let count = match target {
            ActiveDragTarget::Sparse(refs) => {
                if refs.is_empty() || refs.len() > MAX_COMPONENT_REFS {
                    return None;
                }
                // Resolve every ref before mutation and reject duplicate components.
                let mut roots = refs
                    .iter()
                    .map(|reference| reference.resolve(&self.connectivity).ok())
                    .collect::<Option<Vec<_>>>()?;
                roots.sort_unstable();
                if roots.windows(2).any(|pair| pair[0] == pair[1]) {
                    return None;
                }
                let mut count = 0;
                for &root in &roots {
                    if !self.connectivity.iter_component(root).all(valid_member) {
                        return None;
                    }
                    count += self.connectivity.component_size(root);
                }
                if count != self.held_by.count_for(player) {
                    return None;
                }
                let states = &mut *self.states;
                for root in roots {
                    for id in self.connectivity.iter_component(root) {
                        states[id.0 as usize].flags &= !HELD;
                        self.held_by.occupied.remove(&id);
                        self.dirty_pieces.insert(id);
                        self.drag.remove(id);
                    }
                }
                count
            }
            ActiveDragTarget::Dense(dense) => {
                let members = dense.resolve(&self.connectivity).ok()?;
                let count = members.count();
                // A selection that expands is not an exact accepted drag target.
                if members != dense.members
                    || count == 0
                    || count != self.held_by.count_for(player)
                    || !members.iter().all(valid_member)
                {
                    return None;
                }
                let states = &mut *self.states;
                for id in members.iter() {
                    states[id.0 as usize].flags &= !HELD;
                }
                self.held_by.occupied.difference(&members);
                self.dirty_pieces.union(&members);
                self.drag.exclude(&members);
                count
            }
        };
        // The full ownership count matched the validated target. Stale dense
        // owner values remain unoccupied; update the player count just once.
        self.held_by.counts.remove(&player);
        Some(AppliedCommand {
            released: count,
            ..Default::default()
        })
    }
}

//! Operation-local component centers and constant-size drag validation.
use crate::resources::PieceDataStore;
use bevy::math::{DVec2, Vec2};
use puzzella_core::{protocol::ActiveDragTarget, PieceBitSet, PieceId};
use puzzella_puzzle::placement::LogicalPlayArea;

/// Also used by rotation and checkpoints. Sum in f64, including hostile f32s.
pub(crate) fn component_center(positions: impl IntoIterator<Item = Vec2>) -> Option<DVec2> {
    component_bounds(positions).map(|(min, max)| (min + max) * 0.5)
}

fn component_bounds(positions: impl IntoIterator<Item = Vec2>) -> Option<(DVec2, DVec2)> {
    let mut min = DVec2::splat(f64::INFINITY);
    let mut max = DVec2::splat(f64::NEG_INFINITY);
    for position in positions {
        #[cfg(test)]
        PIVOT_VISITS.with(|visits| visits.set(visits.get() + 1));
        if !position.is_finite() {
            return None;
        }
        min = min.min(position.as_dvec2());
        max = max.max(position.as_dvec2());
    }
    (min.is_finite() && max.is_finite()).then_some((min, max))
}

#[cfg(test)]
std::thread_local! { pub(crate) static PIVOT_VISITS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) }; }

/// Bounds of component PIVOTS, never bounds of the selected piece geometry.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct PivotEnvelope {
    pub min: DVec2,
    pub max: DVec2,
    // Intersected translation intervals for f32 presentation rounding. These
    // are scalar limits, not a constraint on the component's geometry extent.
    min_delta: Vec2,
    max_delta: Vec2,
}
impl PivotEnvelope {
    pub fn from_roots(
        store: &PieceDataStore,
        roots: impl IntoIterator<Item = PieceId>,
        area: LogicalPlayArea,
    ) -> Option<Self> {
        Self::from_roots_with_positions(store, roots, area, |id| {
            store.states[id.0 as usize].position
        })
    }

    pub(crate) fn from_roots_with_positions(
        store: &PieceDataStore,
        roots: impl IntoIterator<Item = PieceId>,
        area: LogicalPlayArea,
        position: impl Fn(PieceId) -> Vec2,
    ) -> Option<Self> {
        let mut envelope = Self {
            min: DVec2::splat(f64::INFINITY),
            max: DVec2::splat(f64::NEG_INFINITY),
            min_delta: Vec2::splat(f32::NEG_INFINITY),
            max_delta: Vec2::splat(f32::INFINITY),
        };
        for root in roots {
            let (min, max) =
                component_bounds(store.connectivity.iter_component(root).map(&position))?;
            let pivot = (min + max) * 0.5;
            if !area.contains(pivot) {
                return None;
            }
            envelope.min = envelope.min.min(pivot);
            envelope.max = envelope.max.max(pivot);
            for axis in 0..2 {
                let (low, high) =
                    presentation_limits(min[axis], max[axis], area.half_extents[axis]);
                envelope.min_delta[axis] = envelope.min_delta[axis].max(low);
                envelope.max_delta[axis] = envelope.max_delta[axis].min(high);
            }
        }
        (envelope.min.is_finite() && envelope.max.is_finite()).then_some(envelope)
    }
    pub fn from_members(
        store: &PieceDataStore,
        members: &PieceBitSet,
        area: LogicalPlayArea,
    ) -> Option<Self> {
        Self::from_roots(
            store,
            members
                .iter()
                .filter(|&id| store.connectivity.minimum_member(id) == id),
            area,
        )
    }
    pub fn from_target(
        store: &PieceDataStore,
        target: &ActiveDragTarget,
        area: LogicalPlayArea,
    ) -> Option<Self> {
        match target {
            ActiveDragTarget::Sparse(refs) => {
                Self::from_roots(store, refs.iter().map(|r| r.member), area)
            }
            ActiveDragTarget::Dense(dense) => Self::from_members(store, &dense.members, area),
        }
    }
    /// Pointer-frame preflight: no store, target, membership mask or allocation.
    pub fn accepts(self, area: LogicalPlayArea, delta: Vec2) -> bool {
        delta.is_finite()
            && delta.cmpge(self.min_delta).all()
            && delta.cmple(self.max_delta).all()
            && area.contains(self.min + delta.as_dvec2())
            && area.contains(self.max + delta.as_dvec2())
    }
    /// Local pointer policy. The authority independently rejects illegal inputs.
    pub fn clamp(self, area: LogicalPlayArea, delta: Vec2) -> Vec2 {
        if !delta.is_finite() {
            return Vec2::ZERO;
        }
        if delta == Vec2::ZERO && self.accepts(area, delta) {
            return delta;
        }
        // Leave coordinate ULPs for canonical f32 reconstruction at Release and
        // rotation. This local policy never relaxes the authority boundary.
        let reserve = area.half_extents * (8.0 * f64::from(f32::EPSILON));
        let low = (-area.half_extents + reserve - self.min).max(self.min_delta.as_dvec2());
        let high = (area.half_extents - reserve - self.max).min(self.max_delta.as_dvec2());
        let (low, high) = if low.cmple(high).all() {
            (low, high)
        } else {
            (self.min_delta.as_dvec2(), self.max_delta.as_dvec2())
        };
        if !low.cmple(high).all() {
            return Vec2::ZERO;
        }
        let value = delta.as_dvec2().clamp(low, high);
        // Round toward the interior so conversion to f32 cannot cross the edge.
        let inward = |value: f64, low: f64, high: f64| {
            let mut result = value as f32;
            if f64::from(result) < low {
                result = result.next_up();
            }
            if f64::from(result) > high {
                result = result.next_down();
            }
            result
        };
        Vec2::new(
            inward(value.x, low.x, high.x),
            inward(value.y, low.y, high.y),
        )
    }
}

/// f32 addition is monotonic: translating a component's two extrema gives its
/// displayed AABB center exactly. Find its legal scalar interval once at Grab.
/// Usually the inward-rounded ideal limit is already legal; binary search is
/// needed only when presentation rounding crosses the edge. No pointer frame
/// repeats this work, and no per-component metadata survives the intersection.
fn presentation_limits(min: f64, max: f64, half: f64) -> (f32, f32) {
    let pivot = (min + max) * 0.5;
    let center = |delta: f32| (f64::from(min as f32 + delta) + f64::from(max as f32 + delta)) * 0.5;
    let ordered = |value: f32| {
        let bits = value.to_bits();
        if value.is_sign_negative() {
            !bits
        } else {
            bits ^ (1 << 31)
        }
    };
    let from_ordered = |bits: u32| {
        f32::from_bits(if bits & (1 << 31) != 0 {
            bits ^ (1 << 31)
        } else {
            !bits
        })
    };
    let mut low = (-half - pivot) as f32;
    if f64::from(low) < -half - pivot {
        low = low.next_up();
    }
    if center(low) < -half {
        let (mut left, mut right) = (ordered(low), ordered(0.0));
        while left < right {
            let mid = left + (right - left) / 2;
            if center(from_ordered(mid)) >= -half {
                right = mid;
            } else {
                left = mid + 1;
            }
        }
        low = from_ordered(left);
    }
    let mut high = (half - pivot) as f32;
    if f64::from(high) > half - pivot {
        high = high.next_down();
    }
    if center(high) > half {
        let (mut left, mut right) = (ordered(0.0), ordered(high));
        while left < right {
            let mid = left + (right - left).div_ceil(2);
            if center(from_ordered(mid)) <= half {
                left = mid;
            } else {
                right = mid - 1;
            }
        }
        high = from_ordered(left);
    }
    (low, high)
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct DragValidation {
    pub area: LogicalPlayArea,
    pub pivots: PivotEnvelope,
}
impl DragValidation {
    pub fn accepts(self, delta: Vec2) -> bool {
        self.pivots.accepts(self.area, delta)
    }
}

impl PieceDataStore {
    /// Preserve a legal displayed pivot when f32 affine normalization at Release
    /// rounds outward. This is only a coordinate-precision correction after
    /// strict input validation; an illegal displayed delta is never adopted.
    pub(crate) fn connected_release_offset(
        &self,
        root: PieceId,
        delta: Vec2,
        geometry: &puzzella_core::PuzzleGeometry,
        area: Option<LogicalPlayArea>,
    ) -> Option<Vec2> {
        let representative = self.states[root.0 as usize];
        let rotation = puzzella_core::decode_rotation(representative.flags);
        let mut offset = representative.position
            - puzzella_core::rotate_quarter(geometry.correct_position(root), rotation)
            + delta;
        let Some(area) = area else {
            return Some(offset);
        };
        let displayed = component_center(
            self.connectivity
                .iter_component(root)
                .map(|id| self.states[id.0 as usize].position + delta),
        )?;
        if !area.contains(displayed) {
            return None;
        }
        let (min, max) = component_bounds(
            self.connectivity
                .iter_component(root)
                .map(|id| puzzella_core::rotate_quarter(geometry.correct_position(id), rotation)),
        )?;
        for _ in 0..=4 {
            let center =
                ((min.as_vec2() + offset).as_dvec2() + (max.as_vec2() + offset).as_dvec2()) * 0.5;
            if area.contains(center) {
                return Some(offset);
            }
            if !center.is_finite() {
                return None;
            }
            for axis in 0..2 {
                if center[axis] > area.half_extents[axis] {
                    offset[axis] = offset[axis].next_down();
                } else if center[axis] < -area.half_extents[axis] {
                    offset[axis] = offset[axis].next_up();
                }
            }
        }
        None
    }

    /// Release preflight uses the same f32 reconstruction as release_roots.
    /// Checking actual committed centers also covers rounding at the area edge.
    pub(crate) fn release_pivots_fit(
        &self,
        roots: impl IntoIterator<Item = PieceId>,
        delta: Vec2,
        definition: Option<&puzzella_core::PuzzleDefinition>,
        area: LogicalPlayArea,
    ) -> bool {
        let geometry = definition.map(puzzella_core::PuzzleDefinition::geometry);
        roots.into_iter().all(|root| {
            if let Some(g) = geometry
                .as_ref()
                .filter(|_| self.connectivity.component_size(root) > 1)
            {
                self.connected_release_offset(root, delta, g, Some(area))
                    .is_some()
            } else {
                component_center(
                    self.connectivity
                        .iter_component(root)
                        .map(|id| self.states[id.0 as usize].position + delta),
                )
                .is_some_and(|p| area.contains(p))
            }
        })
    }
    pub(crate) fn release_target_fits(
        &self,
        target: &ActiveDragTarget,
        delta: Vec2,
        definition: Option<&puzzella_core::PuzzleDefinition>,
        area: LogicalPlayArea,
    ) -> bool {
        match target {
            ActiveDragTarget::Sparse(refs) => self.release_pivots_fit(
                refs.iter()
                    .filter_map(|r| r.resolve(&self.connectivity).ok()),
                delta,
                definition,
                area,
            ),
            ActiveDragTarget::Dense(dense) => self.release_pivots_fit(
                dense
                    .members
                    .iter()
                    .filter(|&id| self.connectivity.minimum_member(id) == id),
                delta,
                definition,
                area,
            ),
        }
    }
}

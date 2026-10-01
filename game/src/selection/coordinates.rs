use super::api::{SelectionMode, SelectionRequest};
use bevy::prelude::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PixelRegion {
    pub min: UVec2,
    pub size: UVec2,
}

/// Floor/ceil covers every touched pixel. Clip before integer conversion, then
/// intersect the physical viewport. Degenerate rectangles have no fragments.
pub fn pixel_region(
    request: SelectionRequest,
    scale: f32,
    target: UVec2,
    viewport: URect,
) -> Option<PixelRegion> {
    if !scale.is_finite()
        || scale <= 0.0
        || target.min_element() == 0
        || !request.region.min.is_finite()
        || !request.region.max.is_finite()
    {
        return None;
    }
    let bounds_min = viewport.min.min(target);
    let bounds_max = viewport.max.min(target);
    if bounds_min.cmpge(bounds_max).any() {
        return None;
    }
    let a = request.region.min * scale;
    let b = request.region.max * scale;
    let (min, max) = if request.mode == SelectionMode::Point {
        let min = a.floor();
        (min, min + Vec2::ONE)
    } else {
        if a.x == b.x || a.y == b.y {
            return None;
        }
        (a.min(b).floor(), a.max(b).ceil())
    };
    let min = min.max(bounds_min.as_vec2());
    let max = max.min(bounds_max.as_vec2());
    if min.cmpge(max).any() {
        return None;
    }
    let min = min.as_uvec2();
    Some(PixelRegion {
        min,
        size: max.as_uvec2() - min,
    })
}

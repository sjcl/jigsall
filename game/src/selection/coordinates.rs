use super::api::{SelectionMode, SelectionRequest};
use bevy::prelude::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PixelRegion {
    pub min: UVec2,
    pub size: UVec2,
}

/// Expand one physical viewport pixel to the entire 1x1 picking target.
/// X/Y operate in homogeneous clip space; Z/W (including reverse-Z) stay intact.
pub(super) fn point_crop_projection(viewport: URect, pixel: UVec2) -> Mat4 {
    let size = viewport.size().as_vec2();
    let center = pixel.as_vec2() - viewport.min.as_vec2() + Vec2::splat(0.5);
    // Avoid computing NDC center by division and then multiplying it back: this
    // form keeps integer pixel boundaries exact even for large viewports.
    let offset = Vec2::new(size.x - 2.0 * center.x, 2.0 * center.y - size.y);
    Mat4::from_cols(
        Vec4::new(size.x, 0.0, 0.0, 0.0),
        Vec4::new(0.0, size.y, 0.0, 0.0),
        Vec4::Z,
        Vec4::new(offset.x, offset.y, 0.0, 1.0),
    )
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crop_maps_only_the_chosen_pixel_and_preserves_depth_and_w() {
        for viewport in [
            URect::new(0, 0, 3840, 2160),
            URect::new(137, 59, 3148, 1666),
            URect::new(4, 8, 5, 9),
        ] {
            for pixel in [
                viewport.min,
                viewport.max - UVec2::ONE,
                viewport.min + viewport.size() / 2,
            ] {
                let crop = point_crop_projection(viewport, pixel);
                for w in [0.5, 1.0, 2.0] {
                    for (offset, expected) in [
                        (Vec2::ZERO, Vec2::new(-1., 1.)),
                        (Vec2::splat(0.5), Vec2::ZERO),
                        (Vec2::ONE, Vec2::new(1., -1.)),
                        (Vec2::new(1.5, 0.5), Vec2::new(2., 0.)),
                    ] {
                        let relative = pixel.as_vec2() + offset - viewport.min.as_vec2();
                        let ndc = relative / viewport.size().as_vec2() * 2.0 - Vec2::ONE;
                        let clip = Vec4::new(ndc.x * w, -ndc.y * w, 0.42 * w, w);
                        let cropped = crop * clip;
                        assert!(
                            (cropped.truncate().truncate() / cropped.w - expected)
                                .abs()
                                .max_element()
                                < 0.001
                        );
                        assert_eq!(cropped.z, clip.z);
                        assert_eq!(cropped.w, clip.w);
                    }
                }
            }
        }
    }

    #[test]
    fn crop_uses_the_dpi_normalized_absolute_physical_pixel() {
        let viewport = URect::new(200, 100, 1200, 900);
        let request = SelectionRequest {
            request_id: 1,
            mode: SelectionMode::Point,
            region: Rect {
                min: Vec2::new(97.4, 79.0),
                max: Vec2::new(97.4, 79.0),
            },
        };
        let pixel = pixel_region(request, 2.5, UVec2::new(3840, 2160), viewport).unwrap();
        assert_eq!(pixel.min, UVec2::new(243, 197));
        assert_eq!(pixel.size, UVec2::ONE);
        let crop = point_crop_projection(viewport, pixel.min);
        assert_eq!(crop.w_axis, Vec4::new(913., -605., 0., 1.));
    }
}

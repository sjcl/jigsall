//! CPU/shader uniform layout and viewport/visual configuration.
use super::*;

// Physical pixels in the main camera viewport, also retained for cropped picks.
const FAR_ZOOM_THRESHOLD_PX: f32 = 1.5;
const FAR_SPLAT_MIN_PX: f32 = 1.0;

#[derive(Clone, ShaderType, Default)]
pub struct PuzzleUniform {
    pub clip_from_world: Mat4,
    pub seed: UVec2,
    pub grid: UVec2,
    pub image_size: Vec2,
    pub size: Vec2,
    pub view_min: Vec2,
    pub view_max: Vec2,
    pub count: u32,
    pub capacity: u32,
    pub opaque: u32,
    pub reserved: u32,
    pub selection_min: Vec2,
    pub selection_max: Vec2,
    pub selection_enabled: UVec4,
    pub drag_delta: Vec2,
    pub drag_active: u32,
    pub preview_active: u32,
    pub viewport_size: Vec2,
    pub viewport_origin: Vec2,
    pub piece_size_px: Vec2,
    pub pixel_world_size: Vec2,
    // Applied after geometry generation in the main viewport's pixel grid.
    // Identity for main/rectangle rendering; the crop for far-zoom point picks.
    pub render_clip_scale: Vec2,
    pub render_clip_offset: Vec2,
    pub far_zoom: u32,
    pub splat_min_px: f32,
    pub splat_padding: UVec2,
    pub rotation_time: f32,
    pub rotation_active: u32,
    pub drag_elevation_time: f32,
    pub drag_elevation_active: u32,
    pub pseudo_3d_direction: Vec2,
    pub shadow_base_offset_px: f32,
    pub shadow_lift_offset_px: f32,
    pub shadow_opacity: f32,
    pub shadow_enabled: u32,
    pub shadow_padding: UVec2,
    pub visual_cull_extent: Vec2,
    pub bevel_width_px: f32,
    pub bevel_enabled: u32,
    pub side_color: Vec4,
    pub side_thickness_px: f32,
    pub side_enabled: u32,
    pub bevel_highlight_strength: f32,
    pub bevel_shadow_strength: f32,
}
impl PuzzleUniform {
    pub(super) fn configure_visuals(&mut self, visuals: ResolvedPieceVisuals) {
        let visuals = visuals.for_frame(self.piece_size_px, self.far_zoom != 0);
        self.shadow_enabled = u32::from(visuals.shadow_enabled);
        self.pseudo_3d_direction = PSEUDO_3D_DIRECTION;
        self.shadow_base_offset_px = visuals.shadow_base_offset_px;
        self.shadow_lift_offset_px = visuals.shadow_lift_offset_px;
        self.shadow_opacity = visuals.shadow_opacity;
        self.side_enabled = u32::from(visuals.side_enabled);
        self.side_color = visuals.side_color.extend(visuals.side_opacity);
        self.side_thickness_px = visuals.side_thickness_px;
        self.bevel_enabled = u32::from(visuals.bevel_enabled);
        self.bevel_width_px = visuals.bevel_width_px;
        self.bevel_highlight_strength = visuals.bevel_highlight_strength;
        self.bevel_shadow_strength = visuals.bevel_shadow_strength;
        self.visual_cull_extent = Vec2::ZERO;
        let shadow_offset = if self.shadow_enabled != 0 {
            visuals.shadow_base_offset_px + visuals.shadow_lift_offset_px
        } else {
            0.0
        };
        let side_offset = if self.side_enabled != 0 {
            visuals.side_thickness_px
        } else {
            0.0
        };
        let maximum_offset = shadow_offset.max(side_offset);
        if maximum_offset > 0.0 {
            let ndc =
                PSEUDO_3D_DIRECTION * maximum_offset / self.viewport_size * Vec2::new(2.0, -2.0);
            self.visual_cull_extent = self
                .clip_from_world
                .inverse()
                .transform_vector3(ndc.extend(0.0))
                .truncate()
                .abs();
        }
    }
    pub(super) fn configure_screen_space(&mut self, viewport: URect) {
        self.viewport_size = viewport.size().as_vec2();
        self.viewport_origin = viewport.min.as_vec2();
        self.pixel_world_size = (self.view_max - self.view_min) / self.viewport_size;
        self.piece_size_px = self.size / self.pixel_world_size;
        self.far_zoom = u32::from(self.piece_size_px.min_element() < FAR_ZOOM_THRESHOLD_PX);
        self.splat_min_px = FAR_SPLAT_MIN_PX;
        self.render_clip_scale = Vec2::ONE;
        self.render_clip_offset = Vec2::ZERO;
    }
}
#[derive(Clone, ShaderType)]
pub(super) struct SortUniform {
    pub(super) shift: u32,
    pub(super) count: u32,
    pub(super) groups: u32,
    pub(super) pad: u32,
}

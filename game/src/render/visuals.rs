//! Local presentation settings; never part of gameplay, snapshots or saves.
use bevy::prelude::*;

/// Internal quality switch, ready for a future graphics settings UI.
#[derive(Resource, Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum PieceVisualQuality {
    Low,
    Medium,
    #[default]
    High,
}

/// Screen direction of thickness and shadow (X right, Y down).
pub const PSEUDO_3D_DIRECTION: Vec2 = Vec2::new(
    std::f32::consts::FRAC_1_SQRT_2,
    std::f32::consts::FRAC_1_SQRT_2,
);

#[derive(Clone, Copy, Debug)]
pub struct ResolvedPieceVisuals {
    pub shadow_enabled: bool,
    pub shadow_min_piece_px: f32,
    pub shadow_base_offset_px: f32,
    pub shadow_lift_offset_px: f32,
    pub shadow_opacity: f32,
    pub side_enabled: bool,
    pub side_min_piece_px: f32,
    pub side_thickness_px: f32,
    /// Linear RGB, independent of the source image's color.
    pub side_color: Vec3,
    pub side_opacity: f32,
    pub bevel_enabled: bool,
    pub bevel_min_piece_px: f32,
    pub bevel_width_px: f32,
    pub bevel_highlight_strength: f32,
    pub bevel_shadow_strength: f32,
}

impl PieceVisualQuality {
    /// Keep presets and the temporary default together in this module.
    pub fn resolve(self) -> ResolvedPieceVisuals {
        let (enabled, minimum, base, lift, opacity, side_minimum, thickness, color) = match self {
            Self::Low => (false, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0),
            Self::Medium => (true, 14.0, 2.5, 3.0, 0.20, 22.0, 1.0, 0.10),
            Self::High => (true, 10.0, 3.0, 4.5, 0.25, 14.0, 1.5, 0.06),
        };
        let (bevel_minimum, bevel_width, highlight, bevel_shadow) = match self {
            Self::Low => (0.0, 0.0, 0.0, 0.0),
            Self::Medium => (28.0, 1.0, 0.06, 0.09),
            Self::High => (18.0, 1.5, 0.10, 0.14),
        };
        ResolvedPieceVisuals {
            shadow_enabled: enabled,
            shadow_min_piece_px: minimum,
            shadow_base_offset_px: base,
            shadow_lift_offset_px: lift,
            shadow_opacity: opacity,
            side_enabled: enabled,
            side_min_piece_px: side_minimum,
            side_thickness_px: thickness,
            side_color: Vec3::splat(color),
            side_opacity: 1.0,
            bevel_enabled: enabled,
            bevel_min_piece_px: bevel_minimum,
            bevel_width_px: bevel_width,
            bevel_highlight_strength: highlight,
            bevel_shadow_strength: bevel_shadow,
        }
    }
}

impl ResolvedPieceVisuals {
    pub fn shadow_for_frame(self, piece_size_px: Vec2) -> bool {
        self.shadow_enabled && piece_size_px.min_element() >= self.shadow_min_piece_px
    }

    pub fn shadow_offset_px(self, elevation: f32) -> Vec2 {
        PSEUDO_3D_DIRECTION * (self.shadow_base_offset_px + elevation * self.shadow_lift_offset_px)
    }

    pub fn side_for_frame(self, piece_size_px: Vec2) -> bool {
        self.side_enabled && piece_size_px.min_element() >= self.side_min_piece_px
    }

    pub fn side_offset_px(self) -> Vec2 {
        PSEUDO_3D_DIRECTION * self.side_thickness_px
    }

    pub fn bevel_for_frame(self, piece_size_px: Vec2, far_zoom: bool) -> bool {
        self.bevel_enabled && !far_zoom && piece_size_px.min_element() >= self.bevel_min_piece_px
    }
}

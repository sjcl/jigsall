//! Local graphics preferences, separate from canonical gameplay and puzzle saves.
use bevy::prelude::*;
use serde::{Deserialize, Serialize};

/// Local graphics quality, persisted in the display settings section.
#[derive(Resource, Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
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

/// A quality preset's size rule, resolved once from the projected short edge.
#[derive(Clone, Copy, Debug)]
pub struct ProjectedDimension {
    pub scale: f32,
    pub min_px: f32,
    pub max_px: f32,
}
impl ProjectedDimension {
    pub const fn new(scale: f32, min_px: f32, max_px: f32) -> Self {
        Self {
            scale,
            min_px,
            max_px,
        }
    }

    pub const fn fixed(px: f32) -> Self {
        Self::new(0.0, px, px)
    }

    pub fn resolve(self, piece_px: f32) -> f32 {
        (piece_px * self.scale).clamp(self.min_px, self.max_px)
    }
}

/// Quality-resolved preset rules; these are not frame/uniform pixel values.
#[derive(Clone, Copy, Debug)]
pub struct ResolvedPieceVisuals {
    pub shadow_enabled: bool,
    pub shadow_min_piece_px: f32,
    pub shadow_base_offset: ProjectedDimension,
    pub shadow_lift_offset: ProjectedDimension,
    pub shadow_opacity: f32,
    pub side_enabled: bool,
    pub side_min_piece_px: f32,
    pub side_thickness: ProjectedDimension,
    /// Linear RGB, independent of the source image's color.
    pub side_color: Vec3,
    pub side_opacity: f32,
    pub bevel_enabled: bool,
    pub bevel_min_piece_px: f32,
    pub bevel_width: ProjectedDimension,
    pub bevel_highlight_strength: f32,
    pub bevel_shadow_strength: f32,
}

/// Camera/LOD-resolved dimensions for this frame; copied into PuzzleUniform.
#[derive(Clone, Copy, Debug, Default)]
pub struct FramePieceVisuals {
    pub shadow_enabled: bool,
    pub shadow_base_offset_px: f32,
    pub shadow_lift_offset_px: f32,
    pub shadow_opacity: f32,
    pub side_enabled: bool,
    pub side_thickness_px: f32,
    pub side_color: Vec3,
    pub side_opacity: f32,
    pub bevel_enabled: bool,
    pub bevel_width_px: f32,
    pub bevel_highlight_strength: f32,
    pub bevel_shadow_strength: f32,
}

impl PieceVisualQuality {
    /// Keep quality presets together in this module.
    pub fn resolve(self) -> ResolvedPieceVisuals {
        let (enabled, minimum, base, lift, opacity, side_minimum, thickness, color) = match self {
            Self::Low => (
                false,
                0.0,
                ProjectedDimension::fixed(0.0),
                ProjectedDimension::fixed(0.0),
                0.0,
                0.0,
                ProjectedDimension::fixed(0.0),
                0.0,
            ),
            Self::Medium => (
                true,
                14.0,
                ProjectedDimension::new(0.030, 1.5, 4.5),
                ProjectedDimension::new(0.040, 2.0, 6.0),
                0.20,
                22.0,
                ProjectedDimension::new(0.016, 0.75, 2.5),
                0.10,
            ),
            Self::High => (
                true,
                10.0,
                ProjectedDimension::new(0.040, 2.0, 6.0),
                ProjectedDimension::new(0.060, 3.0, 9.0),
                0.25,
                14.0,
                ProjectedDimension::new(0.025, 1.0, 4.0),
                0.06,
            ),
        };
        let (bevel_minimum, bevel_width, highlight, bevel_shadow) = match self {
            Self::Low => (0.0, ProjectedDimension::fixed(0.0), 0.0, 0.0),
            Self::Medium => (28.0, ProjectedDimension::new(0.0125, 0.75, 2.0), 0.06, 0.09),
            Self::High => (18.0, ProjectedDimension::new(0.020, 1.0, 3.0), 0.10, 0.14),
        };
        ResolvedPieceVisuals {
            shadow_enabled: enabled,
            shadow_min_piece_px: minimum,
            shadow_base_offset: base,
            shadow_lift_offset: lift,
            shadow_opacity: opacity,
            side_enabled: enabled,
            side_min_piece_px: side_minimum,
            side_thickness: thickness,
            side_color: Vec3::splat(color),
            side_opacity: 1.0,
            bevel_enabled: enabled,
            bevel_min_piece_px: bevel_minimum,
            bevel_width,
            bevel_highlight_strength: highlight,
            bevel_shadow_strength: bevel_shadow,
        }
    }
}

impl ResolvedPieceVisuals {
    pub fn for_frame(self, piece_size_px: Vec2, far_zoom: bool) -> FramePieceVisuals {
        let mut frame = FramePieceVisuals::default();
        let piece_px = piece_size_px.min_element();
        // Disabled/overview frames do not resolve any of the optional dimensions.
        if self.shadow_for_frame(piece_size_px) {
            frame.shadow_enabled = true;
            frame.shadow_base_offset_px = self.shadow_base_offset.resolve(piece_px);
            frame.shadow_lift_offset_px = self.shadow_lift_offset.resolve(piece_px);
            frame.shadow_opacity = self.shadow_opacity;
        }
        if self.side_for_frame(piece_size_px) {
            frame.side_enabled = true;
            frame.side_thickness_px = self.side_thickness.resolve(piece_px);
            frame.side_color = self.side_color;
            frame.side_opacity = self.side_opacity;
        }
        if self.bevel_for_frame(piece_size_px, far_zoom) {
            frame.bevel_enabled = true;
            frame.bevel_width_px = self.bevel_width.resolve(piece_px);
            frame.bevel_highlight_strength = self.bevel_highlight_strength;
            frame.bevel_shadow_strength = self.bevel_shadow_strength;
        }
        frame
    }

    pub fn shadow_for_frame(self, piece_size_px: Vec2) -> bool {
        self.shadow_enabled && piece_size_px.min_element() >= self.shadow_min_piece_px
    }

    pub fn side_for_frame(self, piece_size_px: Vec2) -> bool {
        self.side_enabled && piece_size_px.min_element() >= self.side_min_piece_px
    }

    pub fn bevel_for_frame(self, piece_size_px: Vec2, far_zoom: bool) -> bool {
        self.bevel_enabled && !far_zoom && piece_size_px.min_element() >= self.bevel_min_piece_px
    }
}

impl FramePieceVisuals {
    pub fn shadow_offset_px(self, elevation: f32) -> Vec2 {
        PSEUDO_3D_DIRECTION * (self.shadow_base_offset_px + elevation * self.shadow_lift_offset_px)
    }

    pub fn side_offset_px(self) -> Vec2 {
        PSEUDO_3D_DIRECTION * self.side_thickness_px
    }
}

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

/// Screen direction of the shadow (X right, Y down), shared by future lighting.
pub const SHADOW_DIRECTION: Vec2 = Vec2::new(
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
}

impl PieceVisualQuality {
    /// Keep presets and the temporary default together in this module.
    pub fn resolve(self) -> ResolvedPieceVisuals {
        let (enabled, minimum, base, lift, opacity) = match self {
            Self::Low => (false, 0.0, 0.0, 0.0, 0.0),
            Self::Medium => (true, 14.0, 1.0, 3.0, 0.20),
            Self::High => (true, 10.0, 1.5, 4.5, 0.25),
        };
        ResolvedPieceVisuals {
            shadow_enabled: enabled,
            shadow_min_piece_px: minimum,
            shadow_base_offset_px: base,
            shadow_lift_offset_px: lift,
            shadow_opacity: opacity,
        }
    }
}

impl ResolvedPieceVisuals {
    pub fn shadow_for_frame(self, piece_size_px: Vec2) -> bool {
        self.shadow_enabled && piece_size_px.min_element() >= self.shadow_min_piece_px
    }

    pub fn shadow_offset_px(self, elevation: f32) -> Vec2 {
        SHADOW_DIRECTION * (self.shadow_base_offset_px + elevation * self.shadow_lift_offset_px)
    }
}

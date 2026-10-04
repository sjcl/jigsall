//! Ordered embedded fallbacks shared by the UI and catalog coverage tests.
use bevy_egui::egui::{FontData, FontDefinitions, FontFamily};
use std::sync::Arc;

pub(crate) struct EmbeddedFallbackFont {
    pub name: &'static str,
    pub bytes: &'static [u8],
}

/// Add fonts here, with their license and provenance in ui/fonts.
/// Each fallback is appended after egui's default fonts in both UI families.
pub(crate) static EMBEDDED_FALLBACK_FONTS: &[EmbeddedFallbackFont] = &[EmbeddedFallbackFont {
    name: "jigsall_japanese",
    bytes: include_bytes!("../fonts/MPLUS1p-Regular.ttf"),
}];

pub(crate) fn definitions() -> FontDefinitions {
    let mut fonts = FontDefinitions::default();
    for fallback in EMBEDDED_FALLBACK_FONTS {
        fonts.font_data.insert(
            fallback.name.into(),
            Arc::new(FontData::from_static(fallback.bytes)),
        );
        for family in [FontFamily::Proportional, FontFamily::Monospace] {
            fonts
                .families
                .entry(family)
                .or_default()
                .push(fallback.name.into());
        }
    }
    fonts
}

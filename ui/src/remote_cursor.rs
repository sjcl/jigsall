//! Low-frequency name resolution and coverage rasterization. No cursor projection.
use crate::{fonts::EMBEDDED_FALLBACK_FONTS, localization::Localization};
use ab_glyph::{point, Font, FontRef, Glyph, ScaleFont};
use bevy::{asset::RenderAssetUsages, image::ImageSampler, prelude::*, render::render_resource::*};
use jigsall_core::PlayerId;
use jigsall_game::{
    players::{PlayerRoster, MAX_ROSTER_PLAYERS},
    render::remote_cursor::{
        CursorLabel, RemoteCursorLabelAtlas, RemoteCursorLabels, MAX_LABEL_ATLAS_BYTES,
        MAX_LABEL_ATLAS_DIMENSION,
    },
    resources::LocalPlayerId,
};
use std::{collections::BTreeMap, sync::Arc};

const FONT_SIZE: f32 = 12.0;
const MAX_RASTER_SCALE: f32 = 4.0;

#[derive(Clone, PartialEq)]
struct AtlasKey {
    session: Option<jigsall_core::session::SessionId>,
    roster_revision: u64,
    names: Vec<(PlayerId, String)>,
    fallback: String,
    locale: crate::localization::Locale,
    scale: f32,
}

#[derive(Resource, Default)]
pub(crate) struct LabelAtlasCache {
    key: Option<AtlasKey>,
    revision: u64,
    font: Option<FontRef<'static>>,
}

pub(crate) fn reset_label_atlas(
    mut cache: ResMut<LabelAtlasCache>,
    mut labels: ResMut<RemoteCursorLabels>,
) {
    cache.key = None;
    labels.members = Arc::default();
    labels.atlas = None;
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn prepare_label_atlas(
    roster: Res<PlayerRoster>,
    local: Res<LocalPlayerId>,
    i18n: Res<Localization>,
    windows: Query<&Window, With<bevy::window::PrimaryWindow>>,
    mut cache: ResMut<LabelAtlasCache>,
    mut labels: ResMut<RemoteCursorLabels>,
    mut images: ResMut<Assets<Image>>,
    session: Option<NonSend<jigsall_game::network::runtime::NetworkSession>>,
) {
    let scale = windows.single().map_or(1.0, |w| w.scale_factor());
    let scale = if scale.is_finite() && scale > 0.0 {
        scale
    } else {
        1.0
    };
    if labels.scale_factor != scale {
        labels.scale_factor = scale;
    }
    let session = session
        .as_ref()
        .and_then(|s| s.authority())
        .map(|s| s.session_definition().id);
    if cache
        .key
        .as_ref()
        .is_some_and(|key| key.session == session && key.scale == scale)
        && !roster.is_changed()
        && !local.is_changed()
        && !i18n.is_changed()
    {
        return;
    }
    let fallback = i18n.text("game-default-player");
    let names = roster
        .players()
        .filter(|p| p.id != local.0)
        .take(MAX_ROSTER_PLAYERS)
        .map(|p| {
            (
                p.id,
                p.display_name
                    .as_ref()
                    .map_or_else(|| fallback.clone(), |n| n.as_ref().to_owned()),
            )
        })
        .collect();
    let key = AtlasKey {
        session,
        roster_revision: roster.revision(),
        names,
        fallback,
        locale: i18n.locale(),
        scale,
    };
    if cache.key.as_ref() == Some(&key) {
        return;
    }
    if cache.font.is_none() {
        // Single embedded byte source, also used by the egui fallback definitions.
        cache.font = FontRef::try_from_slice(EMBEDDED_FALLBACK_FONTS[0].bytes).ok();
    }
    cache.revision += 1;
    labels.members = key.names.iter().map(|(id, _)| *id).collect();
    labels.atlas = cache
        .font
        .as_ref()
        .and_then(|font| rasterize(font, &key.names, scale))
        .map(|(image, metadata)| {
            Arc::new(RemoteCursorLabelAtlas {
                revision: cache.revision,
                image: images.add(image),
                labels: metadata,
            })
        });
    cache.key = Some(key);
}

struct TextLayout {
    glyphs: Vec<Glyph>,
    min: Vec2,
    size: UVec2,
}

fn layout(font: &FontRef<'_>, name: &str, scale: f32) -> TextLayout {
    let scaled = font.as_scaled(FONT_SIZE * scale);
    let mut x = 0.0;
    let mut previous = None;
    let mut glyphs = Vec::with_capacity(32);
    let mut min = Vec2::ZERO;
    let mut max = Vec2::new(0.0, scaled.height());
    for c in name.chars().take(32) {
        let mut id = scaled.glyph_id(c);
        if id.0 == 0 {
            id = scaled.glyph_id('�');
        }
        if id.0 == 0 {
            id = scaled.glyph_id('?');
        }
        if let Some(prev) = previous {
            x += scaled.kern(prev, id);
        }
        let glyph = id.with_scale_and_position(scaled.scale(), point(x, scaled.ascent()));
        if let Some(outline) = font.outline_glyph(glyph.clone()) {
            let b = outline.px_bounds();
            min = min.min(Vec2::new(b.min.x, b.min.y));
            max = max.max(Vec2::new(b.max.x, b.max.y));
        }
        x += scaled.h_advance(id);
        previous = Some(id);
        glyphs.push(glyph);
    }
    max.x = max.x.max(x);
    min = min.floor();
    TextLayout {
        glyphs,
        min,
        size: (max.ceil() - min).max(Vec2::ONE).as_uvec2(),
    }
}

/// Stable PlayerId shelf packing; one transparent texel guards each text rectangle.
/// No per-label bitmap or texture. Atlas allocation is at most 16 MiB.
fn rasterize(
    font: &FontRef<'_>,
    names: &[(PlayerId, String)],
    display_scale: f32,
) -> Option<(Image, BTreeMap<PlayerId, CursorLabel>)> {
    if names.is_empty() {
        return None;
    }
    let raster_scale = display_scale.clamp(0.5, MAX_RASTER_SCALE);
    let mut ordered: Vec<_> = names.iter().take(MAX_ROSTER_PLAYERS).collect();
    ordered.sort_by_key(|(id, _)| *id);
    let texts: Vec<_> = ordered
        .iter()
        .map(|(_, name)| layout(font, name, raster_scale))
        .collect();
    let width = texts
        .iter()
        .map(|t| t.size.x + 2)
        .max()?
        .max(512)
        .checked_next_power_of_two()?;
    if width > MAX_LABEL_ATLAS_DIMENSION {
        return None;
    }
    let mut rects = Vec::with_capacity(texts.len());
    let (mut x, mut y, mut row_height) = (0, 0, 0);
    for text in &texts {
        let size = text.size + UVec2::splat(2);
        if x + size.x > width {
            x = 0;
            y += row_height;
            row_height = 0;
        }
        if y + size.y > MAX_LABEL_ATLAS_DIMENSION {
            return None;
        }
        rects.push(UVec2::new(x + 1, y + 1));
        x += size.x;
        row_height = row_height.max(size.y);
    }
    let height = (y + row_height).checked_next_power_of_two()?;
    if height > MAX_LABEL_ATLAS_DIMENSION {
        return None;
    }
    let bytes = (width as usize).checked_mul(height as usize)?;
    if bytes > MAX_LABEL_ATLAS_BYTES {
        return None;
    }
    let mut bitmap = vec![0u8; bytes];
    let mut labels = BTreeMap::new();
    let atlas_size = Vec2::new(width as f32, height as f32);
    for (((player, _), text), origin) in ordered.into_iter().zip(texts).zip(rects) {
        for mut glyph in text.glyphs {
            glyph.position.x -= text.min.x;
            glyph.position.y -= text.min.y;
            if let Some(outline) = font.outline_glyph(glyph) {
                let bounds = outline.px_bounds();
                outline.draw(|gx, gy, alpha| {
                    let px = origin.x as i32 + bounds.min.x as i32 + gx as i32;
                    let py = origin.y as i32 + bounds.min.y as i32 + gy as i32;
                    if px >= origin.x as i32
                        && py >= origin.y as i32
                        && px < (origin.x + text.size.x) as i32
                        && py < (origin.y + text.size.y) as i32
                    {
                        let index = py as usize * width as usize + px as usize;
                        let coverage = (alpha * 255.0).round() as u8;
                        bitmap[index] = 255
                            - (((255 - bitmap[index]) as u16 * (255 - coverage) as u16) / 255)
                                as u8;
                    }
                });
            }
        }
        let uv_min = origin.as_vec2() / atlas_size;
        let uv_max = (origin + text.size).as_vec2() / atlas_size;
        labels.insert(
            *player,
            CursorLabel {
                uv: Vec4::new(uv_min.x, uv_min.y, uv_max.x, uv_max.y),
                logical_size: text.size.as_vec2() / raster_scale,
            },
        );
    }
    let mut image = Image::new(
        Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
        TextureDimension::D2,
        bitmap,
        TextureFormat::R8Unorm,
        RenderAssetUsages::RENDER_WORLD,
    );
    image.sampler = ImageSampler::linear();
    Some((image, labels))
}

#[cfg(test)]
mod tests;

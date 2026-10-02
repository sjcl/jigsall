use crate::{localization::Localization, theme};
use bevy::prelude::Resource;
use bevy_egui::egui;
use puzzella_core::session::ImageHash;
use puzzella_game::persistence::runtime::{PersistenceError, PersistenceService, ThumbnailReply};
use std::collections::HashMap;

const CACHE_CAPACITY: usize = 64;
const SLOT_SIZE: egui::Vec2 = egui::vec2(112.0, 84.0);

enum Thumbnail {
    Ready(egui::TextureHandle),
    Failed(PersistenceError),
}

#[cfg(test)]
mod tests;
struct CachedThumbnail {
    thumbnail: Thumbnail,
    last_seen: u64,
}
struct PendingThumbnail {
    hash: ImageHash,
    generation: u64,
    epoch: u64,
}

#[derive(Resource, Default)]
pub(crate) struct SaveThumbnails {
    cache: HashMap<ImageHash, CachedThumbnail>,
    in_flight: Option<PendingThumbnail>,
    generation: u64,
    epoch: u64,
    frame: u64,
    active: bool,
}

impl SaveThumbnails {
    pub fn invalidate(&mut self) {
        self.cache.clear();
        self.epoch = self.epoch.wrapping_add(1);
        // Keep tracking the worker until it replies, so reopening/refreshing
        // cannot queue an unbounded number of obsolete image decodes.
    }

    pub fn begin_frame(
        &mut self,
        ctx: &egui::Context,
        service: &PersistenceService,
        generation: u64,
        active: bool,
    ) {
        if generation != self.generation || (self.active && !active) {
            self.invalidate();
        }
        self.generation = generation;
        self.active = active;
        self.frame = self.frame.wrapping_add(1);
        while let Some(reply) = service.try_recv_thumbnail() {
            self.complete(ctx, reply);
        }
    }

    fn complete(&mut self, ctx: &egui::Context, reply: ThumbnailReply) {
        let Some(pending) = self.in_flight.as_ref() else {
            return;
        };
        if pending.hash != reply.hash || pending.generation != reply.generation {
            return;
        }
        let pending = self.in_flight.take().unwrap();
        if !self.active || pending.epoch != self.epoch || reply.generation != self.generation {
            return;
        }
        let thumbnail = match reply.result {
            Ok(image) => Thumbnail::Ready(ctx.load_texture(
                format!("save_thumbnail_{:?}", reply.hash),
                egui::ColorImage::from_rgba_unmultiplied(image.size, &image.rgba),
                egui::TextureOptions::LINEAR,
            )),
            Err(error) => Thumbnail::Failed(PersistenceError::Save(error)),
        };
        self.insert(reply.hash, thumbnail);
        ctx.request_repaint();
    }

    fn insert(&mut self, hash: ImageHash, thumbnail: Thumbnail) {
        if self.cache.len() >= CACHE_CAPACITY && !self.cache.contains_key(&hash) {
            if let Some(oldest) = self
                .cache
                .iter()
                .min_by_key(|(_, entry)| entry.last_seen)
                .map(|(hash, _)| *hash)
            {
                self.cache.remove(&oldest);
            }
        }
        self.cache.insert(
            hash,
            CachedThumbnail {
                thumbnail,
                last_seen: self.frame,
            },
        );
    }

    pub fn paint(
        &mut self,
        ui: &mut egui::Ui,
        hash: ImageHash,
        visible: &mut Vec<ImageHash>,
        i18n: &Localization,
    ) {
        let size = egui::vec2(SLOT_SIZE.x.min(ui.available_width()), SLOT_SIZE.y);
        let (rect, response) = ui.allocate_exact_size(size, egui::Sense::hover());
        if !ui.is_rect_visible(rect) {
            return;
        }
        if !visible.contains(&hash) {
            visible.push(hash);
        }
        ui.painter().rect_filled(rect, 6, theme::BACKGROUND);
        let label = match self.cache.get_mut(&hash) {
            Some(cached) => {
                cached.last_seen = self.frame;
                match &cached.thumbnail {
                    Thumbnail::Ready(texture) => {
                        let image_size = texture.size_vec2();
                        let scale = (rect.width() / image_size.x).min(rect.height() / image_size.y);
                        let image_rect =
                            egui::Rect::from_center_size(rect.center(), image_size * scale);
                        ui.painter().image(
                            texture.id(),
                            image_rect,
                            egui::Rect::from_min_max(egui::Pos2::ZERO, egui::pos2(1.0, 1.0)),
                            egui::Color32::WHITE,
                        );
                        return;
                    }
                    Thumbnail::Failed(error) => {
                        response.on_hover_text(i18n.persistence_error(error));
                        i18n.text("save-image-unavailable")
                    }
                }
            }
            None => {
                egui::Spinner::new().color(theme::ACCENT).paint_at(
                    ui,
                    egui::Rect::from_center_size(
                        rect.center() - egui::vec2(0.0, 10.0),
                        egui::Vec2::splat(24.0),
                    ),
                );
                i18n.text("save-loading-image")
            }
        };
        ui.painter().text(
            rect.center() + egui::vec2(0.0, 20.0),
            egui::Align2::CENTER_CENTER,
            label,
            egui::FontId::proportional(11.0),
            theme::MUTED,
        );
    }

    fn next_visible(&self, visible: &[ImageHash], busy: bool) -> Option<ImageHash> {
        if !self.active || busy || self.in_flight.is_some() {
            return None;
        }
        visible
            .iter()
            .copied()
            .find(|hash| !self.cache.contains_key(hash))
    }

    pub fn request_visible(
        &mut self,
        service: &PersistenceService,
        visible: &[ImageHash],
        busy: bool,
    ) {
        let Some(hash) = self.next_visible(visible, busy) else {
            return;
        };
        match service.request_thumbnail(self.generation, hash) {
            Ok(()) => {
                self.in_flight = Some(PendingThumbnail {
                    hash,
                    generation: self.generation,
                    epoch: self.epoch,
                });
            }
            Err(error) => self.insert(hash, Thumbnail::Failed(error)),
        }
    }
}

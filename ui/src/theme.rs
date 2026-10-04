use bevy_egui::egui::{self, Color32, Pos2, Rect, Stroke, Vec2};

pub const BACKGROUND: Color32 = Color32::from_rgb(14, 22, 31);
pub const PANEL: Color32 = Color32::from_rgb(23, 34, 44);
pub const SURFACE: Color32 = Color32::from_rgb(30, 44, 55);
pub const BORDER: Color32 = Color32::from_rgb(52, 69, 80);
pub const TEXT: Color32 = Color32::from_rgb(233, 237, 232);
pub const MUTED: Color32 = Color32::from_rgb(154, 171, 178);
pub const ACCENT: Color32 = Color32::from_rgb(177, 201, 176);
pub const DANGER: Color32 = Color32::from_rgb(224, 153, 144);

/// Shared palette and embedded artwork: menus do not depend on the launch directory.
pub fn prepare(ctx: &egui::Context) {
    let id = egui::Id::new("puzzella_menu_theme");
    if ctx.data(|data| data.get_temp::<bool>(id).unwrap_or(false)) {
        return;
    }
    ctx.set_fonts(crate::fonts::definitions());
    let mut style = (*ctx.style_of(egui::Theme::Dark)).clone();
    style.visuals = egui::Visuals::dark();
    style.visuals.interact_cursor = Some(egui::CursorIcon::PointingHand);
    style.visuals.override_text_color = Some(TEXT);
    style.visuals.panel_fill = PANEL;
    style.visuals.window_fill = PANEL;
    style.visuals.extreme_bg_color = BACKGROUND;
    style.visuals.faint_bg_color = SURFACE;
    style.visuals.selection.bg_fill = Color32::from_rgb(62, 84, 76);
    style.visuals.selection.stroke = Stroke::new(1.0, ACCENT);
    for widget in [
        &mut style.visuals.widgets.noninteractive,
        &mut style.visuals.widgets.inactive,
        &mut style.visuals.widgets.active,
        &mut style.visuals.widgets.hovered,
        &mut style.visuals.widgets.open,
    ] {
        widget.corner_radius = 8.into();
        widget.fg_stroke = Stroke::new(1.0, TEXT);
        widget.bg_stroke = Stroke::new(1.0, BORDER);
    }
    style.visuals.widgets.inactive.bg_fill = SURFACE;
    style.visuals.widgets.hovered.bg_fill = Color32::from_rgb(43, 61, 69);
    style.visuals.widgets.hovered.bg_stroke = Stroke::new(1.0, ACCENT);
    style.visuals.widgets.active.bg_fill = Color32::from_rgb(53, 74, 76);
    style.spacing.item_spacing = egui::vec2(10.0, 10.0);
    style.spacing.button_padding = egui::vec2(14.0, 10.0);
    style.spacing.interact_size.y = 34.0;
    style
        .text_styles
        .insert(egui::TextStyle::Body, egui::FontId::proportional(14.0));
    style
        .text_styles
        .insert(egui::TextStyle::Button, egui::FontId::proportional(14.0));
    ctx.set_theme(egui::Theme::Dark);
    ctx.set_style_of(egui::Theme::Dark, style);
    ctx.data_mut(|data| data.insert_temp(id, true));
}

pub fn logo(ctx: &egui::Context) -> egui::TextureHandle {
    let id = egui::Id::new("puzzella_logo_texture");
    if let Some(texture) = ctx.data(|data| data.get_temp::<egui::TextureHandle>(id)) {
        return texture;
    }
    let image = image::load_from_memory(include_bytes!("../../assets/menu-icon.png"))
        .expect("embedded menu icon is a valid PNG")
        .to_rgba8();
    let pixels = egui::ColorImage::from_rgba_unmultiplied(
        [image.width() as usize, image.height() as usize],
        image.as_raw(),
    );
    let texture = ctx.load_texture("puzzella_logo", pixels, egui::TextureOptions::LINEAR);
    ctx.data_mut(|data| data.insert_temp(id, texture.clone()));
    texture
}

pub fn background(ctx: &egui::Context) {
    prepare(ctx);
    let rect = ctx.content_rect();
    let painter = ctx.layer_painter(egui::LayerId::new(
        egui::Order::Background,
        "menu_backdrop".into(),
    ));
    let mut mesh = egui::Mesh::default();
    for (pos, color) in [
        (rect.left_top(), Color32::from_rgb(23, 38, 48)),
        (rect.right_top(), BACKGROUND),
        (rect.right_bottom(), Color32::from_rgb(21, 35, 42)),
        (rect.left_bottom(), BACKGROUND),
    ] {
        mesh.colored_vertex(pos, color);
    }
    mesh.add_triangle(0, 1, 2);
    mesh.add_triangle(0, 2, 3);
    painter.add(egui::Shape::mesh(mesh));
    // Large, quiet puzzle contours give the screen a tabletop character.
    let size = (rect.width() * 0.11).clamp(80.0, 160.0);
    for row in 0..4 {
        for col in 0..3 {
            let origin = egui::pos2(
                rect.right() - size * 2.9 + col as f32 * size * 1.04,
                rect.center().y - size * 1.7 + row as f32 * size * 1.04,
            );
            piece_outline(
                &painter,
                Rect::from_min_size(origin, Vec2::splat(size)),
                Stroke::new(1.0, Color32::from_rgb(32, 49, 57)),
            );
        }
    }
    painter.line_segment(
        [
            egui::pos2(rect.left() + 32.0, rect.bottom() - 42.0),
            egui::pos2(rect.right() - 32.0, rect.bottom() - 42.0),
        ],
        Stroke::new(1.0, Color32::from_rgb(37, 52, 60)),
    );
}

pub fn frame() -> egui::Frame {
    egui::Frame::new()
        .fill(PANEL)
        .stroke(Stroke::new(1.0, BORDER))
        .corner_radius(16)
        .inner_margin(24)
}

pub fn section(ui: &mut egui::Ui, title: impl Into<String>) {
    ui.label(egui::RichText::new(title).size(17.0).strong());
    ui.add_space(4.0);
}

pub fn heading(ui: &mut egui::Ui, title: impl Into<String>) {
    ui.label(
        egui::RichText::new(title)
            .size(if ui.ctx().content_rect().height() < 600.0 {
                26.0
            } else {
                30.0
            })
            .strong(),
    );
    ui.add_space(8.0);
}

pub fn hint(ui: &mut egui::Ui, text: impl Into<String>) {
    ui.label(egui::RichText::new(text).size(12.0).color(MUTED));
}

pub fn button(
    ui: &mut egui::Ui,
    label: impl Into<String>,
    width: f32,
    primary: bool,
) -> egui::Response {
    button_with_color(
        ui,
        label,
        width,
        primary,
        if primary { BACKGROUND } else { TEXT },
    )
}

pub fn danger_button(ui: &mut egui::Ui, label: impl Into<String>, width: f32) -> egui::Response {
    button_with_color(ui, label, width, false, DANGER)
}

fn button_with_color(
    ui: &mut egui::Ui,
    label: impl Into<String>,
    width: f32,
    primary: bool,
    text_color: Color32,
) -> egui::Response {
    let size = egui::vec2(
        width.max(0.0),
        if ui.ctx().content_rect().height() < 600.0 {
            38.0
        } else {
            44.0
        },
    );
    ui.add_sized(
        size,
        egui::Button::new(egui::RichText::new(label).size(15.0).color(text_color))
            // Preserve the requested size during an Area's initial sizing pass.
            .min_size(size)
            .fill(if primary { ACCENT } else { SURFACE })
            .stroke(Stroke::new(1.0, if primary { ACCENT } else { BORDER }))
            .corner_radius(8),
    )
}

pub fn card() -> egui::Frame {
    egui::Frame::new()
        .fill(SURFACE)
        .stroke(Stroke::new(1.0, BORDER))
        .corner_radius(10)
        .inner_margin(16)
}

pub fn piece_outline(painter: &egui::Painter, rect: Rect, stroke: Stroke) {
    let mut points = vec![];
    let pos = |x: f32, y: f32| rect.min + egui::vec2(x * rect.width(), y * rect.height());
    // Trace one edge and rotate it around the center for a consistent puzzle silhouette.
    for edge in 0..4 {
        let rotate = |p: Pos2| {
            let d = p - rect.center();
            rect.center()
                + match edge {
                    0 => d,
                    1 => egui::vec2(-d.y, d.x),
                    2 => -d,
                    _ => egui::vec2(d.y, -d.x),
                }
        };
        points.push(rotate(pos(0.0, 0.0)));
        points.push(rotate(pos(0.36, 0.0)));
        let p0 = pos(0.36, 0.0);
        let p1 = pos(0.23, -0.25);
        let p2 = pos(0.77, -0.25);
        let p3 = pos(0.64, 0.0);
        for step in 1..=16 {
            let t = step as f32 / 16.0;
            let s = 1.0 - t;
            let p = p0.to_vec2() * s.powi(3)
                + p1.to_vec2() * (3.0 * s * s * t)
                + p2.to_vec2() * (3.0 * s * t * t)
                + p3.to_vec2() * t.powi(3);
            points.push(rotate(p.to_pos2()));
        }
        points.push(rotate(pos(1.0, 0.0)));
    }
    painter.add(egui::Shape::closed_line(points, stroke));
}

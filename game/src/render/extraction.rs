//! Main-world camera and immutable upload extraction.
use super::*;

#[derive(Resource, Default)]
pub(super) struct ExtractedPuzzle {
    pub(super) upload: PieceUpload,
    pub(super) remote: RemoteDragUpload,
    pub(super) rotation: RotationVisualUpload,
    pub(super) drag_elevation: DragElevationUpload,
    pub(super) image: Option<AssetId<Image>>,
    pub(super) config: PuzzleUniform,
    pub(super) camera: Option<Entity>,
    pub(super) request: Option<SelectionRequest>,
    pub(super) region: Option<PixelRegion>,
    pub(super) target: UVec2,
    pub(super) viewport: URect,
}
#[allow(clippy::type_complexity)] // Keep the main-world camera's ECS access explicit.
pub(super) fn extract_puzzle(
    mut out: ResMut<ExtractedPuzzle>,
    upload: Extract<Option<Res<PieceUpload>>>,
    presentations: Extract<(
        Res<RemoteDragUpload>,
        Res<RotationVisualUpload>,
        Res<PieceVisualQuality>,
        Res<DragElevationUpload>,
    )>,
    image: Extract<Option<Res<PuzzleImage>>>,
    overlay: Extract<Option<Res<SelectionOverlay>>>,
    selection: Extract<Res<PuzzleSelection>>,
    cameras: Extract<Query<(&Camera, &GlobalTransform, &RenderEntity), With<MainCamera>>>,
) {
    out.remote = (*presentations.0).clone();
    out.rotation = (*presentations.1).clone();
    out.drag_elevation = (*presentations.3).clone();
    out.camera = None;
    out.image = None;
    out.request = selection.latest;
    out.region = None;
    // A joining baseline may arrive before its image worker completes. The
    // initial state/root snapshots live for one frame, so extract them even
    // without a texture; buffer preparation does not require drawing yet.
    out.upload = upload.as_deref().cloned().unwrap_or_default();
    let Some(image) = image.as_ref() else {
        return;
    };
    let Some(def) = &out.upload.definition else {
        return;
    };
    let (grid, seed, image_size, count) = (
        def.grid_size,
        def.seed,
        def.image_size.as_vec2(),
        def.piece_count() as u32,
    );
    let Ok((camera, transform, entity)) = cameras.single() else {
        return;
    };
    if !camera.is_active {
        return;
    }
    let (Some(target), Some(viewport), Some(scale)) = (
        camera.physical_target_size(),
        camera.physical_viewport_rect(),
        camera.target_scaling_factor(),
    ) else {
        return;
    };
    if viewport.size().min_element() == 0 {
        return;
    }
    let clip = camera.clip_from_view() * transform.to_matrix().inverse();
    let inverse = clip.inverse();
    let a = inverse
        .project_point3(Vec3::new(-1.0, -1.0, 0.0))
        .truncate();
    let b = inverse.project_point3(Vec3::new(1.0, 1.0, 0.0)).truncate();
    out.config = PuzzleUniform {
        clip_from_world: clip,
        seed: UVec2::new(seed as u32, (seed >> 32) as u32),
        grid,
        image_size,
        size: image_size / grid.as_vec2(),
        view_min: a.min(b),
        view_max: a.max(b),
        count,
        capacity: count,
        opaque: u32::from(image.opaque),
        reserved: 0,
        drag_delta: out.upload.drag.delta,
        drag_active: u32::from(!out.upload.drag.members.is_empty()),
        preview_active: u32::from(selection.preview_active),
        rotation_time: out.rotation.time,
        rotation_active: u32::from(out.rotation.active && out.rotation.epoch == out.upload.epoch),
        drag_elevation_time: out.drag_elevation.time,
        drag_elevation_active: u32::from(
            out.drag_elevation.active && out.drag_elevation.epoch == out.upload.epoch,
        ),
        ..default()
    };
    out.config.configure_screen_space(viewport);
    out.config.configure_visuals(presentations.2.resolve());
    if let Some(rect) = overlay.as_ref().and_then(|o| o.0) {
        out.config.selection_min = rect.min;
        out.config.selection_max = rect.max;
        out.config.selection_enabled = UVec4::X;
    }
    out.camera = Some(entity.id());
    out.image = Some(image.handle.id());
    out.target = target;
    out.viewport = viewport;
    out.region = out
        .request
        .and_then(|r| pixel_region(r, scale, target, viewport));
}

use super::*;
use crate::resources::pieces::{DragTransform, CONNECTED_EDGES};
use puzzella_core::{PieceBitSet, PieceCommand, LOCAL_PLAYER};

// Independent CPU reference using the unchanged procedural edge functions.
pub(super) fn edge_distances(local: Vec2, size: Vec2, profiles: [[u32; 2]; 4]) -> [f32; 4] {
    let h = size * 0.5;
    let short = size.min_element();
    [
        edge_distance(
            Vec2::new(local.x + h.x, local.y - h.y),
            profiles[0],
            size.x,
            short,
        ),
        edge_distance(
            Vec2::new(h.y - local.y, local.x - h.x),
            profiles[1],
            size.y,
            short,
        ),
        -edge_distance(
            Vec2::new(local.x + h.x, local.y + h.y),
            profiles[2],
            size.x,
            short,
        ),
        -edge_distance(
            Vec2::new(h.y - local.y, local.x + h.x),
            profiles[3],
            size.y,
            short,
        ),
    ]
}

#[test]
#[ignore = "requires a real GPU"]
fn gpu_connected_selection_outlines_preserve_coverage_picking_and_uploads() {
    let (mut app, camera, target) = gpu_app(256);
    let def = definition(UVec2::splat(3), 192, 42);
    let offset = Vec2::splat(10_000.0);
    app.world_mut().insert_resource(def.clone());
    app.world_mut()
        .get_mut::<Transform>(camera)
        .unwrap()
        .translation = offset.extend(0.0);
    let size = Vec2::splat(64.0);
    let width = size.x * 0.16 * 0.5;
    for ids in [
        vec![0],
        vec![0, 1],
        vec![0, 3],
        vec![0, 1, 3, 4],
        vec![0, 1, 3],
        vec![0, 1, 2, 3, 5, 6, 7, 8],
        (0..9).collect(),
    ] {
        let mut members = PieceBitSet::new(9);
        members.extend(ids.iter().copied().map(PieceId));
        {
            let mut store = app.world_mut().resource_mut::<PieceDataStore>();
            store.initialize(
                (0..9)
                    .map(|id| def.correct_position(PieceId(id)) + offset)
                    .collect(),
            );
            for id in 0..9 {
                if !members.contains(&PieceId(id)) {
                    store.states[id as usize].flags = 0;
                }
            }
            store.snap_unheld_component(PieceId(ids[0]), &def);
        }
        wait_ready(&mut app);
        // Unhighlighted silhouette is the coverage reference.
        let normal = rendered_pixels(&mut app, target.clone());
        app.world_mut()
            .resource_mut::<PieceDataStore>()
            .selected_pieces = members.clone();
        let highlighted = rendered_pixels(&mut app, target.clone());
        let mut seam_pixels = 0;
        let mut outer_pixels = 0;
        for y in 0..256 {
            for x in 0..256 {
                let pixel = (y * 256 + x) * 4;
                assert_eq!(
                    highlighted[pixel] > 0,
                    normal[pixel] > 0,
                    "coverage {ids:?} at {x},{y}"
                );
                let world = Vec2::new(x as f32 - 127.5, 127.5 - y as f32);
                // Highest ID wins reverse Z; use the CPU shape, not the render cache.
                let drawn = ids.iter().rev().copied().find(|&id| {
                    let local = world - def.correct_position(PieceId(id));
                    let profiles =
                        piece_profiles(def.seed, def.grid_size, UVec2::new(id % 3, id / 3));
                    piece_signed_distance(local, size, profiles) <= 0.0
                });
                let Some(id) = drawn else { continue };
                let local = world - def.correct_position(PieceId(id));
                let profiles = piece_profiles(def.seed, def.grid_size, UVec2::new(id % 3, id / 3));
                let edges = edge_distances(local, size, profiles);
                let neighbors = def.neighbors(PieceId(id));
                let external = [neighbors[2], neighbors[1], neighbors[3], neighbors[0]];
                let boundary = edges
                    .into_iter()
                    .zip(external)
                    .filter_map(|(distance, neighbor)| {
                        (!neighbor.is_some_and(|n| members.contains(&n))).then_some(distance)
                    })
                    .fold(-1e20_f32, f32::max);
                let full = edges.into_iter().fold(-1e20_f32, f32::max);
                if boundary.abs() > width + 3.0 {
                    assert_eq!(
                        &highlighted[pixel..pixel + 4],
                        &[255, 255, 255, 255],
                        "internal edge {ids:?} at {x},{y}, full={full}, boundary={boundary}"
                    );
                    if full.abs() < 1.5 {
                        seam_pixels += 1;
                    }
                } else if boundary.abs() < width - 2.0 {
                    assert!(
                        highlighted[pixel + 2] < 40,
                        "outer outline {ids:?} at {x},{y}"
                    );
                    outer_pixels += 1;
                }
            }
        }
        assert!(outer_pixels > 100, "outer outline survives {ids:?}");
        if ids.len() > 1 {
            assert!(seam_pixels > 20, "shared contours exercised {ids:?}");
        }
        let rect = Rect::new(0.0, 0.0, 256.0, 256.0);
        let connected_pick = pick(&mut app, rect, SelectionMode::Rectangle);
        assert_eq!(
            connected_pick,
            ids.iter().copied().map(PieceId).collect::<Vec<_>>()
        );
        let center = def.correct_position(PieceId(ids[0]));
        let point = Vec2::new(center.x + 128.0, 128.0 - center.y);
        let point = Rect::from_corners(point, point + Vec2::ONE);
        let connected_point = pick(&mut app, point, SelectionMode::Point);
        assert_eq!(connected_point, vec![PieceId(ids[0])]);
        // Flags only change the highlight evaluation, even for sparse/concave shapes.
        let cached_flags: Vec<_> = ids
            .iter()
            .map(|&id| app.world().resource::<PieceDataStore>().states[id as usize].flags)
            .collect();
        {
            let mut store = app.world_mut().resource_mut::<PieceDataStore>();
            for &id in &ids {
                store.states[id as usize].flags &= !CONNECTED_EDGES;
                store.dirty_pieces.insert(PieceId(id));
            }
        }
        assert_eq!(
            pick(&mut app, rect, SelectionMode::Rectangle),
            connected_pick
        );
        assert_eq!(pick(&mut app, point, SelectionMode::Point), connected_point);
        {
            let mut store = app.world_mut().resource_mut::<PieceDataStore>();
            for (&id, flags) in ids.iter().zip(cached_flags) {
                store.states[id as usize].flags = flags;
                store.dirty_pieces.insert(PieceId(id));
            }
        }
    }
    // Freeze a real connected drag after Grab; camera/pointer frames upload no states.
    {
        let mut store = app.world_mut().resource_mut::<PieceDataStore>();
        let members = store.selected_pieces.clone();
        store.apply_command(
            LOCAL_PLAYER,
            &PieceCommand::GrabGroup {
                members: members.clone(),
            },
            Some(&def),
            puzzella_core::LOCAL_PLAYER,
        );
        store.drag = DragTransform {
            members: members.words().clone(),
            delta: Vec2::ZERO,
        };
    }
    update_gpu(&mut app);
    for step in 0..4 {
        app.world_mut().resource_mut::<PieceDataStore>().drag.delta = Vec2::splat(step as f32);
        app.world_mut()
            .get_mut::<Transform>(camera)
            .unwrap()
            .translation
            .x += 1.0;
        app.world_mut().get_mut::<Transform>(camera).unwrap().scale =
            Vec3::new(1.0 + step as f32 * 0.01, 1.0, 1.0);
        update_gpu(&mut app);
        let gpu = app.sub_app(RenderApp).world().resource::<GpuRenderer>();
        assert_eq!(gpu.upload_bytes, 0);
        assert_eq!(gpu.drag_upload_bytes, 0);
        assert_eq!(gpu.selection_upload_bytes, 0);
    }
}

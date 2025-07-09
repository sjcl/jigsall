use bevy::prelude::*;
use crate::components::*;

/// Frustum cullingシステム - 画面外のピースを非表示にする
pub fn frustum_culling_system(
    mut piece_query: Query<(&Transform, &PuzzlePiece, &mut Visibility), With<PickablePiece>>,
    camera_query: Query<(&Camera, &GlobalTransform), With<MainCamera>>,
) {
    let Ok((camera, camera_transform)) = camera_query.single() else {
        return;
    };
    
    // カメラの視錐台を取得
    let Some(viewport_size) = camera.logical_viewport_size() else {
        return;
    };
    
    // ビューポートの境界を計算（マージンを追加して急な消失を防ぐ）
    let margin = 200.0; // ピースサイズより大きめのマージン
    let half_width = viewport_size.x / 2.0 + margin;
    let half_height = viewport_size.y / 2.0 + margin;
    
    // カメラのスケールを考慮
    let camera_scale = camera_transform.compute_transform().scale.x;
    let scaled_half_width = half_width * camera_scale;
    let scaled_half_height = half_height * camera_scale;
    
    let camera_pos = camera_transform.translation().truncate();
    
    let mut visible_count = 0;
    let mut culled_count = 0;
    
    for (transform, piece, mut visibility) in piece_query.iter_mut() {
        let piece_pos = transform.translation.truncate();
        
        // ピースの境界を考慮した判定
        let piece_bounds_half = Vec2::new(
            piece.bounds.width() / 2.0,
            piece.bounds.height() / 2.0,
        );
        
        // ピースが視錐台内にあるか判定
        let in_frustum = 
            piece_pos.x + piece_bounds_half.x >= camera_pos.x - scaled_half_width &&
            piece_pos.x - piece_bounds_half.x <= camera_pos.x + scaled_half_width &&
            piece_pos.y + piece_bounds_half.y >= camera_pos.y - scaled_half_height &&
            piece_pos.y - piece_bounds_half.y <= camera_pos.y + scaled_half_height;
        
        if in_frustum {
            *visibility = Visibility::Inherited;
            visible_count += 1;
        } else {
            *visibility = Visibility::Hidden;
            culled_count += 1;
        }
    }
    
    // デバッグ出力（頻度を制限）
    static mut FRAME_COUNT: usize = 0;
    unsafe {
        FRAME_COUNT += 1;
        if FRAME_COUNT % 600 == 0 {  // 10秒ごとに出力（60FPSの場合）
            println!("🎯 Frustum Culling: {} visible, {} culled", visible_count, culled_count);
        }
    }
}


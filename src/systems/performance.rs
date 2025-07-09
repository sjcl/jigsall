use bevy::prelude::*;
use crate::components::*;
use crate::resources::*;

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

/// フレーム開始時のパフォーマンス計測システム
pub fn performance_frame_start(
    mut perf_monitor: ResMut<PerformanceMonitor>,
) {
    perf_monitor.start_frame();
}

/// フレーム終了時のパフォーマンス計測システム
pub fn performance_frame_end(
    mut perf_monitor: ResMut<PerformanceMonitor>,
    time: Res<Time>,
) {
    // Bevyの実際のフレーム時間を使用
    let delta_time = time.delta();
    
    if perf_monitor.enabled {
        perf_monitor.frame_times.push(delta_time);
        
        // 古いフレームデータを削除
        if perf_monitor.frame_times.len() > perf_monitor.max_stored_frames {
            perf_monitor.frame_times.remove(0);
        }
        
        perf_monitor.frame_count += 1;
        
        // デバッグ: 異常に高速なフレームを検出
        if perf_monitor.debug_level == PerformanceDebugLevel::High {
            if delta_time.as_nanos() < 100_000 { // 0.1ms未満
                println!("⚠️  WARNING: Extremely fast frame detected: {:.3}ms", 
                    delta_time.as_secs_f32() * 1000.0);
            }
        }
    }
}

/// パフォーマンス計測のトグルシステム（F12キー）
pub fn performance_toggle_system(
    mut perf_monitor: ResMut<PerformanceMonitor>,
    keyboard_input: Res<ButtonInput<KeyCode>>,
) {
    if keyboard_input.just_pressed(KeyCode::F12) {
        perf_monitor.toggle_debug_level();
    }
}

/// パフォーマンスレポート生成システム
pub fn performance_report_system(
    mut perf_monitor: ResMut<PerformanceMonitor>,
    piece_query: Query<Entity, With<PuzzlePiece>>,
    cache: Res<PieceSelectionCache>,
    time: Res<Time>,
) {
    if !perf_monitor.should_report() {
        return;
    }

    let piece_count = piece_query.iter().count();
    
    // Bevyの正確なフレーム時間を使用
    let delta_time = time.delta();
    let frame_time_ms = delta_time.as_secs_f32() * 1000.0;
    let fps = if frame_time_ms > 0.0 {
        1000.0 / frame_time_ms
    } else {
        0.0
    };
    
    // フォールバック: 内部計測も比較のため取得
    let internal_fps = perf_monitor.get_fps();
    let internal_frame_time_ms = perf_monitor.get_frame_time_ms();
    
    // サニティチェック: 異常な値を検出
    let fps_clamped = if fps > 1000.0 || fps.is_nan() || fps.is_infinite() {
        fps.min(1000.0).max(0.0)
    } else {
        fps
    };
    
    let frame_time_clamped = if frame_time_ms > 1000.0 || frame_time_ms.is_nan() || frame_time_ms.is_infinite() {
        frame_time_ms.min(1000.0).max(0.0)
    } else {
        frame_time_ms
    };
    
    match perf_monitor.debug_level {
        PerformanceDebugLevel::Off => return,
        
        PerformanceDebugLevel::Low => {
            println!("📊 PERFORMANCE - FPS: {:.1}, Frame Time: {:.2}ms, Pieces: {}", 
                fps_clamped, frame_time_clamped, piece_count);
        }
        
        PerformanceDebugLevel::Medium => {
            println!("📊 PERFORMANCE REPORT");
            println!("  FPS: {:.1}, Frame Time: {:.2}ms, Pieces: {}", 
                fps_clamped, frame_time_clamped, piece_count);
            
            // システム別の時間を表示（上位5つ）
            let mut system_times: Vec<_> = perf_monitor.system_timings.iter().collect();
            system_times.sort_by(|a, b| b.1.last_duration.cmp(&a.1.last_duration));
            
            println!("  Top Systems by Last Duration:");
            for (i, (name, timing)) in system_times.iter().take(5).enumerate() {
                println!("    {}. {}: {:.2}ms (avg: {:.2}ms)", 
                    i + 1, name, 
                    timing.last_duration.as_secs_f32() * 1000.0,
                    timing.average_duration().as_secs_f32() * 1000.0);
            }
        }
        
        PerformanceDebugLevel::High => {
            println!("📊 DETAILED PERFORMANCE REPORT");
            println!("  FPS: {:.1}, Frame Time: {:.2}ms, Pieces: {}, Cache Size: {}", 
                fps_clamped, frame_time_clamped, piece_count, cache.all_pieces.len());
            println!("  Internal Timing: FPS={:.1}, Frame Time={:.2}ms (may be inaccurate)", 
                internal_fps, internal_frame_time_ms);
            
            // 全システムの詳細情報
            let mut system_times: Vec<_> = perf_monitor.system_timings.iter().collect();
            system_times.sort_by(|a, b| b.1.average_duration().cmp(&a.1.average_duration()));
            
            println!("  System Performance Details:");
            for (name, timing) in system_times.iter() {
                println!("    {}: Last={:.2}ms, Avg={:.2}ms, Min={:.2}ms, Max={:.2}ms, Calls={}",
                    name,
                    timing.last_duration.as_secs_f32() * 1000.0,
                    timing.average_duration().as_secs_f32() * 1000.0,
                    timing.min_duration.as_secs_f32() * 1000.0,
                    timing.max_duration.as_secs_f32() * 1000.0,
                    timing.call_count);
            }
            
            // フレーム時間の分析
            if !perf_monitor.frame_times.is_empty() {
                let min_frame = perf_monitor.frame_times.iter().min().unwrap();
                let max_frame = perf_monitor.frame_times.iter().max().unwrap();
                let total_nanos: u128 = perf_monitor.frame_times.iter().map(|d| d.as_nanos()).sum();
                let avg_nanos = total_nanos / perf_monitor.frame_times.len() as u128;
                
                println!("  Frame Time Analysis: Min={:.2}ms, Max={:.2}ms, Avg={:.2}ms, Frames={}", 
                    min_frame.as_secs_f32() * 1000.0,
                    max_frame.as_secs_f32() * 1000.0,
                    avg_nanos as f32 / 1_000_000.0,
                    perf_monitor.frame_times.len());
                
                // 異常に高速な値を検出
                if avg_nanos < 1_000_000 { // 1ms未満
                    println!("  ⚠️  WARNING: Frame times are unusually fast ({:.2}ms avg), check frame timing implementation", 
                        avg_nanos as f32 / 1_000_000.0);
                }
            }
        }
    }
    
    perf_monitor.reset_report_timer();
}

/// パフォーマンス計測用マクロ
#[macro_export]
macro_rules! time_system {
    ($perf_monitor:expr, $system_name:expr, $block:block) => {
        let start_time = $perf_monitor.start_system_timing($system_name);
        let result = $block;
        $perf_monitor.end_system_timing($system_name, start_time);
        result
    };
}

/// パフォーマンス計測用のスコープマクロ
#[macro_export]
macro_rules! time_scope {
    ($perf_monitor:expr, $scope_name:expr, $block:block) => {
        {
            let start_time = if $perf_monitor.enabled {
                Some(instant::Instant::now())
            } else {
                None
            };
            
            let result = $block;
            
            if let Some(start) = start_time {
                let duration = start.elapsed();
                let timing = $perf_monitor.system_timings
                    .entry($scope_name.to_string())
                    .or_insert_with(|| SystemTiming::new($scope_name.to_string()));
                timing.record_timing(duration);
            }
            
            result
        }
    };
}


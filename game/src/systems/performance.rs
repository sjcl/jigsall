use crate::resources::*;
use bevy::prelude::*;

/// フレーム開始時のパフォーマンス計測システム
pub fn performance_frame_start(mut perf_monitor: ResMut<PerformanceMonitor>) {
    perf_monitor.start_frame();
}

/// フレーム終了時のパフォーマンス計測システム
pub fn performance_frame_end(mut perf_monitor: ResMut<PerformanceMonitor>, time: Res<Time>) {
    // Bevyの実際のフレーム時間を使用
    let delta_time = time.delta();

    perf_monitor.frame_times.push(delta_time);

    // 古いフレームデータを削除
    if perf_monitor.frame_times.len() > perf_monitor.max_stored_frames {
        perf_monitor.frame_times.remove(0);
    }

    perf_monitor.frame_count += 1;

    // デバッグ: 異常に高速なフレームを検出
    if perf_monitor.debug_level == PerformanceDebugLevel::High && delta_time.as_nanos() < 100_000 {
        // 0.1ms未満
        println!(
            "⚠️  WARNING: Extremely fast frame detected: {:.3}ms",
            delta_time.as_secs_f32() * 1000.0
        );
    }
}

/// F12キーが押されたかチェックするRun Condition
pub fn f12_just_pressed(keyboard_input: Res<ButtonInput<KeyCode>>) -> bool {
    keyboard_input.just_pressed(KeyCode::F12)
}

/// パフォーマンス計測のトグルシステム（F12キー）
pub fn toggle_performance_debug(mut perf_monitor: ResMut<PerformanceMonitor>) {
    perf_monitor.toggle_debug_level();
}

/// パフォーマンス計測が有効かチェックするRun Condition
pub fn performance_monitoring_enabled(perf_monitor: Res<PerformanceMonitor>) -> bool {
    perf_monitor.debug_level != PerformanceDebugLevel::Off
}

/// パフォーマンス報告すべきかチェックするRun Condition
pub fn should_report_performance(perf_monitor: Res<PerformanceMonitor>) -> bool {
    perf_monitor.should_report()
}

/// パフォーマンスレポート生成システム
pub fn performance_report_system(
    mut perf_monitor: ResMut<PerformanceMonitor>,
    store: Res<PieceDataStore>,
    time: Res<Time>,
) {
    let piece_count = store.len();
    let collision_count = 0;

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
        fps.clamp(0.0, 1000.0)
    } else {
        fps
    };

    let frame_time_clamped =
        if frame_time_ms > 1000.0 || frame_time_ms.is_nan() || frame_time_ms.is_infinite() {
            frame_time_ms.clamp(0.0, 1000.0)
        } else {
            frame_time_ms
        };

    match perf_monitor.debug_level {
        PerformanceDebugLevel::Off => return,

        PerformanceDebugLevel::Low => {
            println!(
                "📊 PERFORMANCE - FPS: {:.1}, Frame Time: {:.2}ms, Pieces: {}",
                fps_clamped, frame_time_clamped, piece_count
            );
        }

        PerformanceDebugLevel::Medium => {
            println!("📊 PERFORMANCE REPORT");
            println!(
                "  FPS: {:.1}, Frame Time: {:.2}ms, Pieces: {}",
                fps_clamped, frame_time_clamped, piece_count
            );

            // システム別の時間を表示（上位5つ）
            let mut system_times: Vec<_> = perf_monitor.system_timings.iter().collect();
            system_times.sort_by_key(|entry| std::cmp::Reverse(entry.1.last_duration));

            println!("  Top Systems by Last Duration:");
            for (i, (name, timing)) in system_times.iter().take(5).enumerate() {
                println!(
                    "    {}. {}: {:.2}ms (avg: {:.2}ms)",
                    i + 1,
                    name,
                    timing.last_duration.as_secs_f32() * 1000.0,
                    timing.average_duration().as_secs_f32() * 1000.0
                );
            }
        }

        PerformanceDebugLevel::High => {
            println!("📊 DETAILED PERFORMANCE REPORT");
            println!(
                "  FPS: {:.1}, Frame Time: {:.2}ms, Pieces: {}, Cache Size: {}",
                fps_clamped, frame_time_clamped, piece_count, collision_count
            );
            println!(
                "  Internal Timing: FPS={:.1}, Frame Time={:.2}ms (may be inaccurate)",
                internal_fps, internal_frame_time_ms
            );

            // 全システムの詳細情報
            let mut system_times: Vec<_> = perf_monitor.system_timings.iter().collect();
            system_times.sort_by_key(|entry| std::cmp::Reverse(entry.1.average_duration()));

            println!("  System Performance Details:");
            for (name, timing) in system_times.iter() {
                println!(
                    "    {}: Last={:.2}ms, Avg={:.2}ms, Min={:.2}ms, Max={:.2}ms, Calls={}",
                    name,
                    timing.last_duration.as_secs_f32() * 1000.0,
                    timing.average_duration().as_secs_f32() * 1000.0,
                    timing.min_duration.as_secs_f32() * 1000.0,
                    timing.max_duration.as_secs_f32() * 1000.0,
                    timing.call_count
                );
            }

            // フレーム時間の分析
            if !perf_monitor.frame_times.is_empty() {
                let min_frame = perf_monitor.frame_times.iter().min().unwrap();
                let max_frame = perf_monitor.frame_times.iter().max().unwrap();
                let total_nanos: u128 = perf_monitor.frame_times.iter().map(|d| d.as_nanos()).sum();
                let avg_nanos = total_nanos / perf_monitor.frame_times.len() as u128;

                println!(
                    "  Frame Time Analysis: Min={:.2}ms, Max={:.2}ms, Avg={:.2}ms, Frames={}",
                    min_frame.as_secs_f32() * 1000.0,
                    max_frame.as_secs_f32() * 1000.0,
                    avg_nanos as f32 / 1_000_000.0,
                    perf_monitor.frame_times.len()
                );

                // 異常に高速な値を検出
                if avg_nanos < 1_000_000 {
                    // 1ms未満
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
    ($perf_monitor:expr, $scope_name:expr, $block:block) => {{
        let start_time = if $perf_monitor.enabled {
            Some(instant::Instant::now())
        } else {
            None
        };

        let result = $block;

        if let Some(start) = start_time {
            let duration = start.elapsed();
            let timing = $perf_monitor
                .system_timings
                .entry($scope_name.to_string())
                .or_insert_with(SystemTiming::new);
            timing.record_timing(duration);
        }

        result
    }};
}

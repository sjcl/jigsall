use crate::keybindings::{KeyAction, KeyBindingsState, KeyPresses};
use crate::resources::*;
use bevy::prelude::*;

/// ゲーム時間の停止・倍率・deltaの上限に影響されない実フレーム時間を収集する。
pub fn sample_performance_frame(
    mut perf_monitor: ResMut<PerformanceMonitor>,
    time: Res<Time<Real>>,
) {
    let delta_time = time.delta();
    // The first update has no preceding frame to measure.
    if delta_time.is_zero() {
        return;
    }
    perf_monitor.frame_times.push_back(delta_time);
    if perf_monitor.frame_times.len() > perf_monitor.max_stored_frames {
        perf_monitor.frame_times.pop_front();
    }
    perf_monitor.frame_count += 1;
}

pub fn performance_key_just_pressed(
    keyboard_input: Res<ButtonInput<KeyCode>>,
    bindings: Res<KeyBindingsState>,
    presses: Option<Res<KeyPresses>>,
    windows: Query<&Window, With<bevy::window::PrimaryWindow>>,
    egui_input: Option<Res<bevy_egui::input::EguiWantsInput>>,
) -> bool {
    windows.iter().all(|window| window.focused)
        && egui_input.is_none_or(|input| !input.wants_any_keyboard_input())
        && bindings.just_pressed(KeyAction::Performance, &keyboard_input, presses.as_deref())
}

/// FPSのみ → 詳細表示 → 非表示。
pub fn toggle_performance_debug(mut perf_monitor: ResMut<PerformanceMonitor>) {
    perf_monitor.toggle_debug_level();
}

pub fn performance_monitoring_enabled(perf_monitor: Res<PerformanceMonitor>) -> bool {
    perf_monitor.debug_level != PerformanceDebugLevel::Off
}

pub fn reset_performance_samples(mut perf_monitor: ResMut<PerformanceMonitor>) {
    perf_monitor.reset_samples();
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
        let start_time = $perf_monitor.start_system_timing($scope_name);
        let result = $block;
        $perf_monitor.end_system_timing($scope_name, start_time);
        result
    }};
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn f3_cycles_once_per_press_and_f12_does_nothing() {
        let mut app = App::new();
        app.init_resource::<ButtonInput<KeyCode>>()
            .insert_resource(KeyBindingsState::load(None))
            .init_resource::<PerformanceMonitor>()
            .add_systems(
                Update,
                toggle_performance_debug.run_if(performance_key_just_pressed),
            );

        app.world_mut()
            .resource_mut::<ButtonInput<KeyCode>>()
            .press(KeyCode::F12);
        app.update();
        assert_eq!(
            app.world().resource::<PerformanceMonitor>().debug_level,
            PerformanceDebugLevel::Off
        );
        for expected in [
            PerformanceDebugLevel::Fps,
            PerformanceDebugLevel::Verbose,
            PerformanceDebugLevel::Off,
            PerformanceDebugLevel::Fps,
        ] {
            let mut keys = app.world_mut().resource_mut::<ButtonInput<KeyCode>>();
            keys.release(KeyCode::F3);
            keys.clear();
            keys.press(KeyCode::F3);
            app.update();
            assert_eq!(
                app.world().resource::<PerformanceMonitor>().debug_level,
                expected
            );
            app.world_mut()
                .resource_mut::<ButtonInput<KeyCode>>()
                .clear();
            app.update();
            assert_eq!(
                app.world().resource::<PerformanceMonitor>().debug_level,
                expected,
                "holding F3 must not cycle again"
            );
        }
    }

    #[test]
    fn samples_real_time_even_when_game_time_is_paused_and_bounds_history() {
        let mut app = App::new();
        app.init_resource::<Time<Real>>()
            .init_resource::<Time>()
            .insert_resource(PerformanceMonitor {
                debug_level: PerformanceDebugLevel::Fps,
                max_stored_frames: 2,
                ..default()
            })
            .add_systems(Update, sample_performance_frame);
        app.update();
        assert!(app
            .world()
            .resource::<PerformanceMonitor>()
            .frame_times
            .is_empty());

        let mut real = app.world_mut().resource_mut::<Time<Real>>();
        real.update_with_duration(Duration::ZERO);
        for millis in [10, 20, 30] {
            app.world_mut()
                .resource_mut::<Time<Real>>()
                .update_with_duration(Duration::from_millis(millis));
            app.update();
        }
        let perf = app.world().resource::<PerformanceMonitor>();
        assert_eq!(app.world().resource::<Time>().delta(), Duration::ZERO);
        assert_eq!(perf.frame_count, 3);
        assert_eq!(perf.frame_times.len(), 2);
        assert_eq!(perf.get_frame_time_ms(), 25.0);
        assert_eq!(perf.get_fps(), 40.0);
    }

    #[test]
    fn system_timings_only_run_in_verbose_and_reenable_clears_old_samples() {
        let mut perf = PerformanceMonitor::default();
        assert!(perf.start_system_timing("test").is_none());
        perf.toggle_debug_level();
        assert!(perf.start_system_timing("test").is_none());
        perf.frame_times.push_back(Duration::from_millis(20));
        perf.frame_count = 1;
        perf.toggle_debug_level();
        let start = perf.start_system_timing("test");
        assert!(start.is_some());
        perf.end_system_timing("test", start);
        assert_eq!(perf.system_timings["test"].call_count, 1);
        perf.toggle_debug_level();
        assert!(perf.start_system_timing("test").is_none());
        perf.toggle_debug_level();
        assert!(perf.frame_times.is_empty());
        assert!(perf.system_timings.is_empty());
        assert_eq!(perf.frame_count, 0);
    }
}

use bevy::prelude::*;
use instant::Instant;
use std::collections::{HashMap, VecDeque};

/// F3で切り替えるパフォーマンスオーバーレイの表示モード。
#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub enum PerformanceDebugLevel {
    #[default]
    Off,
    Fps,
    Verbose,
}

/// 個別システムの計測データ
#[derive(Debug, Clone)]
pub struct SystemTiming {
    pub last_duration: std::time::Duration,
    pub total_duration: std::time::Duration,
    pub call_count: u64,
    pub min_duration: std::time::Duration,
    pub max_duration: std::time::Duration,
}

impl Default for SystemTiming {
    fn default() -> Self {
        Self::new()
    }
}

impl SystemTiming {
    pub fn new() -> Self {
        Self {
            last_duration: std::time::Duration::ZERO,
            total_duration: std::time::Duration::ZERO,
            call_count: 0,
            min_duration: std::time::Duration::MAX,
            max_duration: std::time::Duration::ZERO,
        }
    }

    pub fn record_timing(&mut self, duration: std::time::Duration) {
        self.last_duration = duration;
        self.total_duration += duration;
        self.call_count += 1;
        self.min_duration = self.min_duration.min(duration);
        self.max_duration = self.max_duration.max(duration);
    }

    pub fn average_duration(&self) -> std::time::Duration {
        if self.call_count > 0 {
            self.total_duration / self.call_count as u32
        } else {
            std::time::Duration::ZERO
        }
    }
}

/// パフォーマンス監視リソース
#[derive(Resource)]
pub struct PerformanceMonitor {
    pub debug_level: PerformanceDebugLevel,
    pub frame_times: VecDeque<std::time::Duration>,
    pub system_timings: HashMap<String, SystemTiming>,
    pub frame_count: u64,
    pub max_stored_frames: usize,
}

impl Default for PerformanceMonitor {
    fn default() -> Self {
        Self {
            debug_level: PerformanceDebugLevel::Off,
            frame_times: VecDeque::new(),
            system_timings: HashMap::new(),
            frame_count: 0,
            max_stored_frames: 120,
        }
    }
}

impl PerformanceMonitor {
    pub fn start_system_timing(&mut self, _system_name: &str) -> Option<Instant> {
        if self.debug_level == PerformanceDebugLevel::Verbose {
            Some(Instant::now())
        } else {
            None
        }
    }

    pub fn end_system_timing(&mut self, system_name: &str, start_time: Option<Instant>) {
        if self.debug_level == PerformanceDebugLevel::Verbose {
            if let Some(start) = start_time {
                let duration = start.elapsed();
                let timing = self
                    .system_timings
                    .entry(system_name.to_string())
                    .or_default();
                timing.record_timing(duration);
            }
        }
    }

    pub fn reset_samples(&mut self) {
        self.frame_times.clear();
        self.system_timings.clear();
        self.frame_count = 0;
    }

    pub fn get_fps(&self) -> f32 {
        if self.frame_times.is_empty() {
            return 0.0;
        }

        let total_time: std::time::Duration = self.frame_times.iter().sum();
        let avg_frame_time_nanos = total_time.as_nanos() / self.frame_times.len() as u128;

        if avg_frame_time_nanos > 0 {
            1_000_000_000.0 / avg_frame_time_nanos as f32
        } else {
            0.0
        }
    }

    pub fn get_frame_time_ms(&self) -> f32 {
        if self.frame_times.is_empty() {
            return 0.0;
        }

        let total_time: std::time::Duration = self.frame_times.iter().sum();
        let avg_frame_time_nanos = total_time.as_nanos() / self.frame_times.len() as u128;
        avg_frame_time_nanos as f32 / 1_000_000.0
    }

    pub fn toggle_debug_level(&mut self) {
        self.debug_level = match self.debug_level {
            PerformanceDebugLevel::Off => PerformanceDebugLevel::Fps,
            PerformanceDebugLevel::Fps => PerformanceDebugLevel::Verbose,
            PerformanceDebugLevel::Verbose => PerformanceDebugLevel::Off,
        };
        if self.debug_level == PerformanceDebugLevel::Fps {
            self.reset_samples();
        }
    }
}

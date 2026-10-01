use bevy::prelude::*;
use instant::Instant;
use std::collections::HashMap;

/// パフォーマンス計測のデバッグレベル
#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub enum PerformanceDebugLevel {
    #[default]
    Off,
    Low,    // 基本的な統計のみ
    Medium, // 個別システムの時間
    High,   // 詳細な内部計測
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
    pub enabled: bool,
    pub frame_start: Option<Instant>,
    pub frame_times: Vec<std::time::Duration>,
    pub system_timings: HashMap<String, SystemTiming>,
    pub last_report_time: Instant,
    pub report_interval: std::time::Duration,
    pub frame_count: u64,
    pub max_stored_frames: usize,
}

impl Default for PerformanceMonitor {
    fn default() -> Self {
        Self {
            debug_level: PerformanceDebugLevel::Off,
            enabled: false,
            frame_start: None,
            frame_times: Vec::new(),
            system_timings: HashMap::new(),
            last_report_time: Instant::now(),
            report_interval: std::time::Duration::from_secs(5), // 5秒間隔でレポート
            frame_count: 0,
            max_stored_frames: 300, // 5秒分のフレーム（60FPS想定）
        }
    }
}

impl PerformanceMonitor {
    pub fn start_frame(&mut self) {
        if self.enabled {
            self.frame_start = Some(Instant::now());
        }
    }

    pub fn start_system_timing(&mut self, _system_name: &str) -> Option<Instant> {
        if self.enabled {
            Some(Instant::now())
        } else {
            None
        }
    }

    pub fn end_system_timing(&mut self, system_name: &str, start_time: Option<Instant>) {
        if self.enabled {
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

    pub fn should_report(&self) -> bool {
        self.enabled && self.last_report_time.elapsed() >= self.report_interval
    }

    pub fn reset_report_timer(&mut self) {
        self.last_report_time = Instant::now();
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
            PerformanceDebugLevel::Off => PerformanceDebugLevel::Low,
            PerformanceDebugLevel::Low => PerformanceDebugLevel::Medium,
            PerformanceDebugLevel::Medium => PerformanceDebugLevel::High,
            PerformanceDebugLevel::High => PerformanceDebugLevel::Off,
        };
        self.enabled = self.debug_level != PerformanceDebugLevel::Off;

        if self.enabled {
            println!("🔍 Performance monitoring enabled: {:?}", self.debug_level);
        } else {
            println!("🔍 Performance monitoring disabled");
        }
    }
}

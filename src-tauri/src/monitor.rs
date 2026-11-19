//! 灵动岛仪表盘：系统监控采样（CPU / 内存 / 每核占用）。
//!
//! 从 lib.rs 按主题拆出（纯移动，不改行为）。

use std::sync::Mutex;
use sysinfo::System;

/// CPU 使用率是两次采样之间的差值，必须复用同一个 System；
/// 每次调用都新建实例只能拿到首次采样的垃圾值（常见 0% 或 100%）。
pub struct DashState(Mutex<System>);

impl DashState {
    pub fn new() -> Self {
        let mut system = System::new();
        system.refresh_cpu_usage();
        Self(Mutex::new(system))
    }

    /// 返回 (CPU 占用百分比, 总内存 MB, 可用内存 MB)。
    pub fn sample(&self) -> (f32, u64, u64) {
        let (cpu, _, total_mb, avail_mb) = self.snapshot();
        (cpu, total_mb, avail_mb)
    }

    /// 返回 (CPU 总占用, 每核占用, 总内存 MB, 可用内存 MB)。
    pub fn snapshot(&self) -> (f32, Vec<f32>, u64, u64) {
        let mut sys = self.0.lock().unwrap();
        sys.refresh_cpu_usage();
        sys.refresh_memory();
        let per_core = sys.cpus().iter().map(|cpu| cpu.cpu_usage()).collect();
        (
            sys.global_cpu_usage(),
            per_core,
            sys.total_memory() / 1024,
            sys.available_memory() / 1024,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::DashState;

    #[test]
    fn dash_state_samples_plausible_live_values() {
        let dash = DashState::new();
        std::thread::sleep(std::time::Duration::from_millis(300));
        let (cpu, total_mb, avail_mb) = dash.sample();
        assert!(
            cpu.is_finite() && (0.0..=100.0).contains(&cpu),
            "cpu={cpu}"
        );
        assert!(total_mb > 0, "total_mb={total_mb}");
        assert!(avail_mb <= total_mb, "avail={avail_mb} total={total_mb}");
    }

    #[test]
    fn monitor_snapshot_exposes_per_core_usage() {
        let dash = DashState::new();
        std::thread::sleep(std::time::Duration::from_millis(300));
        let (cpu, cores, total_mb, _) = dash.snapshot();
        assert!(
            cpu.is_finite() && (0.0..=100.0).contains(&cpu),
            "cpu={cpu}"
        );
        assert!(!cores.is_empty(), "没有读到 CPU 核心");
        assert!(
            cores.iter().all(|usage| usage.is_finite() && (0.0..=100.0).contains(usage)),
            "核心占用越界：{cores:?}"
        );
        assert!(total_mb > 0, "total_mb={total_mb}");
    }
}

//! 异步转换任务：状态快照、取消令牌与任务注册表。
//!
//! 状态变化统一通过 `bus::emit("convert-state", ...)` 回流前端；
//! 注册表只保存活跃任务，查询/取消按 id 定位。

use orbit_core::{ConvertJobState, ConvertJobStatus};
use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock, RwLock};
use tauri::AppHandle;

use crate::conversion::CancelToken;

pub(crate) struct ConvertJob {
    pub(crate) cancel: CancelToken,
    status: RwLock<ConvertJobStatus>,
}

impl ConvertJob {
    pub(crate) fn new(job_id: String, input_path: String, format: String) -> Self {
        let from = std::path::Path::new(&input_path)
            .extension()
            .and_then(|extension| extension.to_str())
            .unwrap_or("")
            .to_ascii_lowercase();
        Self {
            cancel: CancelToken::default(),
            status: RwLock::new(ConvertJobStatus {
                job_id,
                state: ConvertJobState::Queued,
                progress: 0.0,
                from,
                to: format.to_ascii_lowercase(),
                output_path: None,
                error: None,
            }),
        }
    }

    pub(crate) fn update(&self, mutate: impl FnOnce(&mut ConvertJobStatus)) -> ConvertJobStatus {
        let mut guard = self.status.write().unwrap_or_else(|error| error.into_inner());
        mutate(&mut guard);
        guard.clone()
    }

    pub(crate) fn snapshot(&self) -> ConvertJobStatus {
        self.status.read().unwrap_or_else(|error| error.into_inner()).clone()
    }

    /// 状态或进度变化后通知前端；事件未登记时只记日志，不影响转换本身。
    pub(crate) fn notify(&self, app: &AppHandle) {
        let status = self.snapshot();
        if let Err(error) = crate::bus::emit(app, "convert-state", &status) {
            log::warn!("转换状态事件发送失败：{error}");
        }
    }
}

pub(crate) struct JobRegistry {
    jobs: Mutex<HashMap<String, Arc<ConvertJob>>>,
}

impl JobRegistry {
    pub(crate) fn global() -> &'static Self {
        static REGISTRY: OnceLock<JobRegistry> = OnceLock::new();
        REGISTRY.get_or_init(|| Self { jobs: Mutex::new(HashMap::new()) })
    }

    pub(crate) fn add(&self, job: Arc<ConvertJob>) {
        let id = job.snapshot().job_id;
        if let Ok(mut jobs) = self.jobs.lock() {
            jobs.insert(id, job);
        }
    }

    pub(crate) fn get(&self, id: &str) -> Option<Arc<ConvertJob>> {
        self.jobs.lock().ok()?.get(id).cloned()
    }

    pub(crate) fn status(&self, id: &str) -> Option<ConvertJobStatus> {
        Some(self.get(id)?.snapshot())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_job_is_queued_with_normalized_formats() {
        let job = ConvertJob::new("job-1".into(), "C:\\tmp\\A.MP4".into(), "MP3".into());
        let status = job.snapshot();
        assert_eq!(status.state, ConvertJobState::Queued);
        assert_eq!(status.from, "mp4");
        assert_eq!(status.to, "mp3");
        assert_eq!(status.progress, 0.0);
    }

    #[test]
    fn cancel_token_flips_state() {
        let job = ConvertJob::new("job-2".into(), "a.mp4".into(), "mp3".into());
        assert!(!job.cancel.is_cancelled());
        job.cancel.cancel();
        assert!(job.cancel.is_cancelled());
    }
}

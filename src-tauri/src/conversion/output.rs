//! 输出路径分配：预占 + 归属标记 + 失败回滚。
//!
//! 借鉴 Filee 的 FinalAllocator / TempAllocator：
//! - 路径由分配器统一发放，带锁预占，避免并发任务拿到同名；
//! - 每个路径记录「是否本来就不存在」，失败/取消只删除自己独占的文件；
//! - 用户已有的同名文件永不覆盖，而是追加 ` (2)` / ` (3)`；
//! - 中间文件放任务私有工作目录，任务结束删除。

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

/// 归一化路径键：Windows 大小写不敏感，分隔符统一，避免同名不同写法绕过预占。
fn key(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/").to_lowercase()
}

static RESERVATIONS: OnceLock<Mutex<HashSet<String>>> = OnceLock::new();
fn reservations() -> &'static Mutex<HashSet<String>> {
    RESERVATIONS.get_or_init(|| Mutex::new(HashSet::new()))
}

#[derive(Debug)]
struct Allocation {
    path: PathBuf,
    /// true = 分配前该路径不存在，失败时可以安全删除。
    own: bool,
    /// true = 工作目录内的中间文件，失败时必须删除。
    intermediate: bool,
}

/// 一次转换任务的路径分配器。
pub(crate) struct JobAllocator {
    allocations: Vec<Allocation>,
    work_dir: PathBuf,
    made_dirs: Vec<PathBuf>,
}

impl JobAllocator {
    pub(crate) fn new(work_dir: PathBuf) -> Self {
        Self { allocations: Vec::new(), work_dir, made_dirs: Vec::new() }
    }

    /// 分配最终输出路径。已存在或被预占时自动追加序号，绝不覆盖。
    pub(crate) fn final_output(&mut self, requested: &Path, format: &str) -> Result<PathBuf, String> {
        let extension = format.trim_start_matches('.').to_ascii_lowercase();
        let mut guard = reservations().lock().map_err(|error| error.to_string())?;

        let mut candidate = with_extension(requested, &extension, None);
        let mut index = 1;
        while candidate.exists() || guard.contains(&key(&candidate)) {
            index += 1;
            candidate = with_extension(requested, &extension, Some(index));
        }

        let own = !candidate.exists();
        guard.insert(key(&candidate));
        drop(guard);

        self.ensure_parent(&candidate)?;
        self.allocations.push(Allocation { path: candidate.clone(), own, intermediate: false });
        Ok(candidate)
    }

    /// 分配任务私有工作目录内的中间文件路径。
    pub(crate) fn temp_output(&mut self, format: &str) -> Result<PathBuf, String> {
        if !self.work_dir.exists() {
            std::fs::create_dir_all(&self.work_dir).map_err(|error| error.to_string())?;
            self.made_dirs.push(self.work_dir.clone());
        }
        let extension = format.trim_start_matches('.').to_ascii_lowercase();
        let path = self.work_dir.join(format!("step-{}.{}", self.allocations.len(), extension));
        self.allocations.push(Allocation { path: path.clone(), own: true, intermediate: true });
        Ok(path)
    }

    /// 转换成功：释放预占，保留输出文件与工作目录（工作目录已经空了）。
    pub(crate) fn finish(&mut self) {
        self.release();
        self.remove_empty_dirs();
    }

    /// 转换失败/取消：删除所有本次独占的文件，释放预占，回收自建空目录。
    pub(crate) fn discard(&mut self) {
        for allocation in &self.allocations {
            if allocation.own || allocation.intermediate {
                let _ = std::fs::remove_file(&allocation.path);
            }
        }
        self.release();
        self.remove_empty_dirs();
    }

    fn release(&mut self) {
        if let Ok(mut guard) = reservations().lock() {
            for allocation in &self.allocations {
                guard.remove(&key(&allocation.path));
            }
        }
        self.allocations.clear();
    }

    fn remove_empty_dirs(&mut self) {
        for dir in &self.made_dirs {
            if std::fs::read_dir(dir).map(|mut entries| entries.next().is_none()).unwrap_or(false) {
                let _ = std::fs::remove_dir(dir);
            }
        }
        self.made_dirs.clear();
    }

    fn ensure_parent(&mut self, path: &Path) -> Result<(), String> {
        let Some(parent) = path.parent() else { return Ok(()) };
        if parent.as_os_str().is_empty() || parent.exists() {
            return Ok(());
        }
        std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
        self.made_dirs.push(parent.to_path_buf());
        Ok(())
    }
}

/// 生成 `name.ext` / `name (2).ext`，保留已有扩展名时替换而不是叠加。
fn with_extension(requested: &Path, extension: &str, index: Option<u32>) -> PathBuf {
    let stem = requested
        .file_stem()
        .map(|stem| stem.to_string_lossy().into_owned())
        .unwrap_or_else(|| "output".to_string());
    let name = match index {
        Some(index) => format!("{stem} ({index}).{extension}"),
        None => format!("{stem}.{extension}"),
    };
    match requested.parent() {
        Some(parent) if !parent.as_os_str().is_empty() => parent.join(name),
        _ => PathBuf::from(name),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join("orbit-allocator-tests").join(name);
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn final_output_replaces_extension() {
        let dir = temp_dir("replace-extension");
        let mut allocator = JobAllocator::new(dir.join("work"));
        let out = allocator.final_output(&dir.join("song.wav"), "mp3").unwrap();
        assert_eq!(out.file_name().unwrap(), "song.mp3");
        allocator.finish();
    }

    #[test]
    fn final_output_never_overwrites_existing_file() {
        let dir = temp_dir("no-overwrite");
        let existing = dir.join("song.mp3");
        std::fs::write(&existing, b"user data").unwrap();
        let mut allocator = JobAllocator::new(dir.join("work"));
        let out = allocator.final_output(&existing, "mp3").unwrap();
        assert_eq!(out.file_name().unwrap(), "song (2).mp3");
        assert_eq!(std::fs::read(&existing).unwrap(), b"user data");
        allocator.discard();
        assert!(existing.exists(), "失败回滚不能删除用户原有文件");
        assert!(!out.exists(), "失败回滚应删除本次新建的文件");
    }

    #[test]
    fn temp_output_is_inside_work_dir_and_removed_on_discard() {
        let dir = temp_dir("temp-output");
        let work = dir.join("work");
        let mut allocator = JobAllocator::new(work.clone());
        let temp = allocator.temp_output("wav").unwrap();
        assert!(temp.starts_with(&work));
        std::fs::write(&temp, b"partial").unwrap();
        allocator.discard();
        assert!(!temp.exists());
        assert!(!work.exists(), "自建工作目录应被回收");
    }

    #[test]
    fn reservation_blocks_a_second_allocator() {
        let dir = temp_dir("reservation");
        let mut first = JobAllocator::new(dir.join("work"));
        let a = first.final_output(&dir.join("clip.mp3"), "mp3").unwrap();
        let mut second = JobAllocator::new(dir.join("work2"));
        let b = second.final_output(&dir.join("clip.mp3"), "mp3").unwrap();
        assert_ne!(a, b);
        assert_eq!(b.file_name().unwrap(), "clip (2).mp3");
        first.discard();
        second.discard();
    }
}

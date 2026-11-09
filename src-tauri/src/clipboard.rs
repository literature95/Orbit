//! 剪贴板历史：OS 级事件监听 + SQLite 持久化。
//!
//! 采集链路参考 EcoPaste（Apache-2.0）的设计：
//! - 事件驱动替代定时轮询（Windows 为 `WM_CLIPBOARDUPDATE`）；
//! - content hash 去重 + use_count 累加，而非只与最新一条比较；
//! - 写回抑制（WritebackGuard）避免「写回 → 监听到自己 → 再次入库」回环；
//! - 高置信密钥/令牌命中即跳过入库；
//! - 读取失败做有界重试（Windows 上其他监听者会短暂占用剪贴板）。
//!
//! 与 EcoPaste 的差异：本模块用 rusqlite 同步连接（Mutex 保护）而非 sqlx 异步池，
//! 搜索用 LIKE 而非 FTS5；不采集富文本，也不记录来源应用。
//! 图片以 PNG 落盘、数据库只存路径，预览时按需生成缩略图，避免历史库膨胀。

use base64::Engine;
use clipboard_rs::{
    Clipboard, ClipboardContext, ClipboardHandler, ClipboardWatcher, ClipboardWatcherContext,
    ContentFormat, RustImageData,
};
use clipboard_rs::common::RustImage;
use regex::Regex;
use rusqlite::{Connection, OptionalExtension, params};
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};
use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tauri::{AppHandle, Emitter, Manager, State};

/// 前端监听此事件做增量刷新 / 计数更新。
pub const CLIP_CHANGED: &str = "clip://changed";
/// 窗口由快捷键/圆盘唤起时通知前端聚焦搜索框。
pub const CLIP_WINDOW_SHOWN: &str = "clip://shown";
/// 全局快捷键：Alt+V 切换剪贴板窗口。
pub const CLIP_SHORTCUT: &str = "Alt+V";

const DEFAULT_LIMIT: usize = 200;
const READ_RETRY_MS: [u64; 3] = [15, 35, 75];

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64
}

fn hash_of(value: &str) -> String {
    format!("{:x}", Sha256::digest(value.as_bytes()))
}

fn preview_of(text: &str) -> String {
    let single = text.replace(['\n', '\r', '\t'], " ");
    let trimmed = single.trim();
    let chars: Vec<char> = trimmed.chars().collect();
    if chars.len() > 90 {
        format!("{}…", chars[..90].iter().collect::<String>())
    } else {
        trimmed.to_string()
    }
}

// ============ 敏感信息检测（保守规则，命中即整条丢弃） ============

/// 只拦截带固定前缀、标准结构或明确私钥边界的内容，避免误伤普通长文本。
pub fn contains_secret(text: &str) -> bool {
    static RULES: OnceLock<Vec<Regex>> = OnceLock::new();
    let rules = RULES.get_or_init(|| {
        [
            r"-----BEGIN (?:[A-Z0-9 ]+ )?PRIVATE KEY-----",
            r"(?i)\b(?:gh[pousr]_[A-Za-z0-9_]{36,}|github_pat_[A-Za-z0-9_]{40,}|sk-(?:proj-)?[A-Za-z0-9_-]{32,}|xox[baprs]-[A-Za-z0-9-]{20,})\b",
            r"\b(?:AKIA|ASIA)[A-Z0-9]{16}\b",
            r"\beyJ[A-Za-z0-9_-]{8,}\.[A-Za-z0-9_-]{8,}\.[A-Za-z0-9_-]{8,}\b",
            r#"(?ix)\b(?:api[_-]?key|secret[_-]?key|client[_-]?secret|access[_-]?token|refresh[_-]?token|auth[_-]?token|bearer)\b['"]?\s*[:=]\s*['"]?[A-Za-z0-9][A-Za-z0-9._~+/=-]{23,}['"]?"#,
        ]
        .iter()
        .map(|pattern| Regex::new(pattern).expect("剪贴板敏感信息正则必须有效"))
        .collect()
    });
    rules.iter().any(|rule| rule.is_match(text))
}

// ============ 数据结构与存储 ============

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ClipItem {
    pub id: i64,
    /// text | files | image
    pub kind: String,
    /// 文本内容 / 文件列表 JSON 数组 / 图片 PNG 文件的绝对路径
    pub content: String,
    pub preview: String,
    pub hash: String,
    pub pinned: bool,
    pub favorite: bool,
    pub use_count: i64,
    pub created_at: i64,
    pub updated_at: i64,
}

#[derive(Clone)]
pub struct ClipboardState {
    db: Arc<Mutex<Connection>>,
    /// 图片文件目录（app_data_dir/clipboard_images）
    image_dir: Arc<PathBuf>,
    paused: Arc<AtomicBool>,
    /// 自身写回登记：(hash, 过期时间 ms)
    writeback: Arc<Mutex<Vec<(String, i64)>>>,
    /// 图片写回后 Windows 会重新编码位图，hash 不再匹配，用短时间窗抑制后续变更
    skip_until_ms: Arc<AtomicI64>,
    /// 窗口最近一次显示时间，避免「刚显示就失焦」被误判为用户离开
    shown_at_ms: Arc<AtomicI64>,
}

fn row_to_item(row: &rusqlite::Row<'_>) -> rusqlite::Result<ClipItem> {
    Ok(ClipItem {
        id: row.get(0)?,
        kind: row.get(1)?,
        content: row.get(2)?,
        preview: row.get(3)?,
        hash: row.get(4)?,
        pinned: row.get::<_, i64>(5)? != 0,
        favorite: row.get::<_, i64>(6)? != 0,
        use_count: row.get(7)?,
        created_at: row.get(8)?,
        updated_at: row.get(9)?,
    })
}

const SELECT_COLUMNS: &str =
    "id, kind, content, preview, hash, pinned, favorite, use_count, created_at, updated_at";

impl ClipboardState {
    pub fn open(path: &Path, image_dir: &Path) -> Result<Self, String> {
        std::fs::create_dir_all(image_dir).map_err(|error| error.to_string())?;
        let connection = Connection::open(path).map_err(|error| error.to_string())?;
        connection
            .execute_batch(
                "PRAGMA journal_mode = WAL;
                 PRAGMA synchronous = NORMAL;
                 CREATE TABLE IF NOT EXISTS clip_items (
                     id INTEGER PRIMARY KEY AUTOINCREMENT,
                     kind TEXT NOT NULL,
                     content TEXT NOT NULL,
                     preview TEXT NOT NULL,
                     hash TEXT NOT NULL UNIQUE,
                     pinned INTEGER NOT NULL DEFAULT 0,
                     favorite INTEGER NOT NULL DEFAULT 0,
                     use_count INTEGER NOT NULL DEFAULT 1,
                     created_at INTEGER NOT NULL,
                     updated_at INTEGER NOT NULL
                 );
                 CREATE INDEX IF NOT EXISTS idx_clip_items_updated ON clip_items(updated_at DESC);",
            )
            .map_err(|error| error.to_string())?;
        Ok(Self {
            db: Arc::new(Mutex::new(connection)),
            image_dir: Arc::new(image_dir.to_path_buf()),
            paused: Arc::new(AtomicBool::new(false)),
            writeback: Arc::new(Mutex::new(Vec::new())),
            skip_until_ms: Arc::new(AtomicI64::new(0)),
            shown_at_ms: Arc::new(AtomicI64::new(0)),
        })
    }

    // ============ 窗口状态（由 lib.rs 的快捷键 / 失焦逻辑调用） ============

    pub fn mark_shown(&self) {
        self.shown_at_ms.store(now_ms(), Ordering::Relaxed);
    }

    /// 显示后 400ms 内的失焦不隐藏，规避 Windows 前台激活的时序抖动。
    pub fn can_auto_hide(&self) -> bool {
        now_ms() - self.shown_at_ms.load(Ordering::Relaxed) > 400
    }

    // ============ 图片文件 ============

    fn store_image(&self, hash: &str, bytes: &[u8]) -> Result<PathBuf, String> {
        let path = self.image_dir.join(format!("{hash}.png"));
        if !path.exists() {
            std::fs::write(&path, bytes).map_err(|error| error.to_string())?;
        }
        Ok(path)
    }

    /// 仅删除本目录内的文件，避免误删任意路径。
    fn remove_image_file(&self, content: &str) {
        let path = Path::new(content);
        if path.starts_with(&*self.image_dir) {
            let _ = std::fs::remove_file(path);
        }
    }

    fn skip_following_changes(&self) {
        self.skip_until_ms
            .store(now_ms() + 800, Ordering::Relaxed);
    }

    fn in_skip_window(&self) -> bool {
        now_ms() < self.skip_until_ms.load(Ordering::Relaxed)
    }

    pub fn is_paused(&self) -> bool {
        self.paused.load(Ordering::Relaxed)
    }

    pub fn set_paused(&self, value: bool) {
        self.paused.store(value, Ordering::Relaxed);
    }

    /// 写回前登记 hash；监听到同内容时只消费一次标记后跳过。
    pub fn suppress(&self, hash: &str) {
        let mut guard = self.writeback.lock().unwrap_or_else(|e| e.into_inner());
        let now = now_ms();
        guard.retain(|(_, expires)| *expires > now);
        guard.push((hash.to_string(), now + 5_000));
    }

    /// 供旧的 `clip_write` 命令登记：写回纯文本后不应再被采集。
    pub fn suppress_text_write(&self, text: &str) {
        self.suppress(&hash_of(&format!("text:{text}")));
    }

    fn take_writeback(&self, hash: &str) -> bool {
        let mut guard = self.writeback.lock().unwrap_or_else(|e| e.into_inner());
        let now = now_ms();
        guard.retain(|(_, expires)| *expires > now);
        if let Some(index) = guard.iter().position(|(h, _)| h == hash) {
            guard.remove(index);
            return true;
        }
        false
    }

    /// 同内容命中去重：复用 id、累加 use_count、刷新时间；否则插入新行并裁剪旧数据。
    pub fn upsert(
        &self,
        kind: &str,
        content: &str,
        preview: &str,
        hash: &str,
    ) -> Result<(i64, bool), String> {
        let db = self.db.lock().unwrap_or_else(|e| e.into_inner());
        let ts = now_ms();
        let existing: Option<i64> = db
            .query_row(
                "SELECT id FROM clip_items WHERE hash = ?1",
                params![hash],
                |row| row.get(0),
            )
            .optional()
            .map_err(|error| error.to_string())?;

        if let Some(id) = existing {
            db.execute(
                "UPDATE clip_items SET use_count = use_count + 1, updated_at = ?1, preview = ?2 WHERE id = ?3",
                params![ts, preview, id],
            )
            .map_err(|error| error.to_string())?;
            return Ok((id, true));
        }

        db.execute(
            "INSERT INTO clip_items (kind, content, preview, hash, use_count, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, 1, ?5, ?5)",
            params![kind, content, preview, hash, ts],
        )
        .map_err(|error| error.to_string())?;
        let id = db.last_insert_rowid();
        // 只裁剪未置顶 / 未收藏的条目；先取出将被淘汰的图片路径以便同步清理文件
        let overflow = format!(
            "SELECT content FROM clip_items WHERE kind = 'image' AND pinned = 0 AND favorite = 0
             AND id IN (SELECT id FROM clip_items WHERE pinned = 0 AND favorite = 0
                        ORDER BY updated_at DESC LIMIT -1 OFFSET {DEFAULT_LIMIT})"
        );
        let mut statement = db.prepare(&overflow).map_err(|error| error.to_string())?;
        let stale_images = statement
            .query_map([], |row| row.get::<_, String>(0))
            .map_err(|error| error.to_string())?
            .filter_map(Result::ok)
            .collect::<Vec<_>>();
        drop(statement);
        db.execute(
            "DELETE FROM clip_items WHERE pinned = 0 AND favorite = 0 AND id IN (
                 SELECT id FROM clip_items WHERE pinned = 0 AND favorite = 0
                 ORDER BY updated_at DESC LIMIT -1 OFFSET ?1
             )",
            params![DEFAULT_LIMIT as i64],
        )
        .map_err(|error| error.to_string())?;
        for content in stale_images {
            self.remove_image_file(&content);
        }
        Ok((id, false))
    }

    pub fn count(&self) -> usize {
        let db = self.db.lock().unwrap_or_else(|e| e.into_inner());
        db.query_row("SELECT COUNT(*) FROM clip_items", [], |row| row.get::<_, i64>(0))
            .unwrap_or(0) as usize
    }

    pub fn get(&self, id: i64) -> Result<Option<ClipItem>, String> {
        let db = self.db.lock().unwrap_or_else(|e| e.into_inner());
        db.query_row(
            &format!("SELECT {SELECT_COLUMNS} FROM clip_items WHERE id = ?1"),
            params![id],
            row_to_item,
        )
        .optional()
        .map_err(|error| error.to_string())
    }

    pub fn list(&self, query: Option<String>, filter: &str, limit: usize) -> Result<Vec<ClipItem>, String> {
        let db = self.db.lock().unwrap_or_else(|e| e.into_inner());
        let sql = format!(
            "SELECT {SELECT_COLUMNS} FROM clip_items
             WHERE (?1 IS NULL OR content LIKE '%' || ?1 || '%')
               AND (?2 = 'all'
                    OR (?2 = 'favorite' AND favorite = 1)
                    OR (?2 = 'pinned' AND pinned = 1)
                    OR (?2 = 'text' AND kind = 'text')
                    OR (?2 = 'files' AND kind = 'files')
                    OR (?2 = 'image' AND kind = 'image'))
             ORDER BY pinned DESC, updated_at DESC
             LIMIT ?3"
        );
        let mut statement = db.prepare(&sql).map_err(|error| error.to_string())?;
        let rows = statement
            .query_map(params![query, filter, limit.min(500) as i64], row_to_item)
            .map_err(|error| error.to_string())?;
        rows.collect::<rusqlite::Result<Vec<_>>>()
            .map_err(|error| error.to_string())
    }

    pub fn set_flag(&self, id: i64, field: &str, value: bool) -> Result<(), String> {
        // field 只来自命令层的白名单，不接受外部字符串
        let column = match field {
            "pinned" => "pinned",
            "favorite" => "favorite",
            _ => return Err("非法的标志位".to_string()),
        };
        let db = self.db.lock().unwrap_or_else(|e| e.into_inner());
        db.execute(
            &format!("UPDATE clip_items SET {column} = ?1 WHERE id = ?2"),
            params![if value { 1 } else { 0 }, id],
        )
        .map_err(|error| error.to_string())?;
        Ok(())
    }

    pub fn delete(&self, id: i64) -> Result<(), String> {
        let db = self.db.lock().unwrap_or_else(|e| e.into_inner());
        let image_content: Option<String> = db
            .query_row(
                "SELECT content FROM clip_items WHERE id = ?1 AND kind = 'image'",
                params![id],
                |row| row.get(0),
            )
            .optional()
            .map_err(|error| error.to_string())?;
        db.execute("DELETE FROM clip_items WHERE id = ?1", params![id])
            .map_err(|error| error.to_string())?;
        drop(db);
        if let Some(content) = image_content {
            self.remove_image_file(&content);
        }
        Ok(())
    }

    pub fn clear(&self) -> Result<(), String> {
        let db = self.db.lock().unwrap_or_else(|e| e.into_inner());
        let mut statement = db
            .prepare("SELECT content FROM clip_items WHERE kind = 'image' AND pinned = 0 AND favorite = 0")
            .map_err(|error| error.to_string())?;
        let image_files = statement
            .query_map([], |row| row.get::<_, String>(0))
            .map_err(|error| error.to_string())?
            .filter_map(Result::ok)
            .collect::<Vec<_>>();
        drop(statement);
        // 保留置顶与收藏
        db.execute("DELETE FROM clip_items WHERE pinned = 0 AND favorite = 0", [])
            .map_err(|error| error.to_string())?;
        drop(db);
        for content in image_files {
            self.remove_image_file(&content);
        }
        Ok(())
    }

    pub fn bump_use(&self, id: i64) -> Result<(), String> {
        let db = self.db.lock().unwrap_or_else(|e| e.into_inner());
        db.execute(
            "UPDATE clip_items SET use_count = use_count + 1, updated_at = ?1 WHERE id = ?2",
            params![now_ms(), id],
        )
        .map_err(|error| error.to_string())?;
        Ok(())
    }
}

// ============ 采集：读取与事件监听 ============

struct Payload {
    kind: String,
    /// text / files 直接作为入库内容；image 时此字段为空，入库前替换为 PNG 路径
    content: String,
    preview: String,
    hash: String,
    /// 仅 image 携带：待落盘的 PNG 字节
    image_bytes: Option<Vec<u8>>,
}

impl Payload {
    fn simple(kind: &str, content: String, preview: String, hash: String) -> Self {
        Self { kind: kind.to_string(), content, preview, hash, image_bytes: None }
    }
}

/// 优先级：文件 > 图片 > 纯文本。剪贴板同一时刻常同时提供多种格式
/// （例如复制网页图片会同时带图片位图与文本 URL），按信息量最高的格式收录。
fn try_read_payload() -> Result<Option<Payload>, String> {
    let context = ClipboardContext::new().map_err(|error| error.to_string())?;

    if context.has(ContentFormat::Files) {
        let files = context.get_files().map_err(|error| error.to_string())?;
        if files.is_empty() {
            return Ok(None);
        }
        let content = serde_json::to_string(&files).map_err(|error| error.to_string())?;
        let names: Vec<String> = files
            .iter()
            .take(3)
            .map(|path| {
                Path::new(path)
                    .file_name()
                    .map(|name| name.to_string_lossy().into_owned())
                    .unwrap_or_else(|| path.clone())
            })
            .collect();
        let mut preview = format!("📁 {}", names.join("、"));
        if files.len() > 3 {
            preview.push_str(&format!(" 等 {} 项", files.len()));
        }
        let hash = hash_of(&format!("files:{content}"));
        return Ok(Some(Payload::simple("files", content, preview, hash)));
    }

    if context.has(ContentFormat::Image) {
        let image = context.get_image().map_err(|error| error.to_string())?;
        let (width, height) = image.get_size();
        if width == 0 || height == 0 {
            return Ok(None);
        }
        let png = image.to_png().map_err(|error| error.to_string())?;
        let bytes = png.get_bytes().to_vec();
        if bytes.is_empty() {
            return Ok(None);
        }
        let hash = format!("image:{:x}", Sha256::digest(&bytes));
        let preview = format!("🖼 图片 {width}×{height}");
        return Ok(Some(Payload {
            kind: "image".to_string(),
            content: String::new(),
            preview,
            hash,
            image_bytes: Some(bytes),
        }));
    }

    let text = context.get_text().map_err(|error| error.to_string())?;
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return Ok(None);
    }
    let preview = preview_of(trimmed);
    let hash = hash_of(&format!("text:{text}"));
    Ok(Some(Payload::simple("text", text, preview, hash)))
}

fn read_with_retry() -> Option<Payload> {
    let mut last_error = String::new();
    for attempt in 0..=READ_RETRY_MS.len() {
        match try_read_payload() {
            Ok(payload) => return payload,
            Err(error) => {
                last_error = error;
                if attempt < READ_RETRY_MS.len() {
                    std::thread::sleep(Duration::from_millis(READ_RETRY_MS[attempt]));
                }
            }
        }
    }
    log::warn!("clipboard watcher: 读取剪贴板失败：{last_error}");
    None
}

struct WatcherHandler {
    app: AppHandle,
    state: ClipboardState,
}

impl ClipboardHandler for WatcherHandler {
    fn on_clipboard_change(&mut self) {
        if self.state.is_paused() || self.state.in_skip_window() {
            return;
        }
        let Some(payload) = read_with_retry() else {
            return;
        };
        // 自身写回：跳过入库，避免回环
        if self.state.take_writeback(&payload.hash) {
            return;
        }
        // 高置信密钥/令牌不落盘
        if payload.kind == "text" && contains_secret(&payload.content) {
            log::info!("clipboard watcher: 命中敏感内容，跳过入库");
            return;
        }
        let content = if let Some(bytes) = payload.image_bytes.as_deref() {
            match self.state.store_image(&payload.hash, bytes) {
                Ok(path) => path.to_string_lossy().into_owned(),
                Err(error) => {
                    log::warn!("clipboard watcher: 图片落盘失败：{error}");
                    return;
                }
            }
        } else {
            payload.content
        };
        let (id, deduplicated) = match self.state.upsert(&payload.kind, &content, &payload.preview, &payload.hash) {
            Ok(result) => result,
            Err(error) => {
                log::warn!("clipboard watcher: 入库失败：{error}");
                return;
            }
        };
        let count = self.state.count();
        let _ = self.app.emit(
            CLIP_CHANGED,
            serde_json::json!({
                "id": id, "kind": payload.kind, "deduplicated": deduplicated, "count": count
            }),
        );
    }
}

/// 必须在**窗口创建之前**调用（plugin.initialize）。
///
/// Tauri 2 的 `setup()` 会先按 config 创建所有窗口，再执行用户 setup 回调；
/// 而窗口创建时前端 `onMount` 的 IPC 已经进入 WebView2 回调链。若到这里才
/// `app.manage()`，启动期的 `get_dash_info` / `clip_list` 会命中
/// `state() called before manage()` 并在 FFI 回调里 panic（0xc0000409）。
pub fn init_state(app: &AppHandle) -> Result<(), String> {
    let dir = app.path().app_data_dir().map_err(|error| error.to_string())?;
    std::fs::create_dir_all(&dir).map_err(|error| error.to_string())?;
    let state = ClipboardState::open(
        &dir.join("clipboard.db"),
        &dir.join("clipboard_images"),
    )?;
    app.manage(state);
    Ok(())
}

/// 在 setup 中调用：状态已注册，这里只启动独立监听线程。
pub fn start_watcher(app: &AppHandle) -> Result<(), String> {
    let state = app
        .try_state::<ClipboardState>()
        .ok_or_else(|| "剪贴板状态尚未初始化".to_string())?
        .inner()
        .clone();

    let handler_app = app.clone();
    let handler_state = state;
    std::thread::Builder::new()
        .name("clipboard-watcher".to_string())
        .spawn(move || {
            // 平台句柄留在本线程内构造，不跨线程移动
            let mut watcher = match ClipboardWatcherContext::new() {
                Ok(watcher) => watcher,
                Err(error) => {
                    log::error!("clipboard watcher: 创建失败：{error}");
                    return;
                }
            };
            watcher.add_handler(WatcherHandler {
                app: handler_app,
                state: handler_state,
            });
            log::info!("clipboard watcher started");
            watcher.start_watch();
        })
        .map_err(|error| format!("剪贴板监听线程启动失败：{error}"))?;
    Ok(())
}

// ============ 命令 ============

#[tauri::command]
pub fn clip_list(
    state: State<'_, ClipboardState>,
    query: Option<String>,
    filter: Option<String>,
    limit: Option<usize>,
) -> Result<Vec<ClipItem>, String> {
    let normalized = query
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty());
    state.list(
        normalized,
        filter.as_deref().unwrap_or("all"),
        limit.unwrap_or(DEFAULT_LIMIT),
    )
}

#[tauri::command]
pub fn clip_count(state: State<'_, ClipboardState>) -> usize {
    state.count()
}

#[tauri::command]
pub fn clip_write_back(state: State<'_, ClipboardState>, id: i64) -> Result<(), String> {
    let item = state.get(id)?.ok_or_else(|| "该条目已被删除".to_string())?;
    state.suppress(&item.hash);
    let context = ClipboardContext::new().map_err(|error| error.to_string())?;
    match item.kind.as_str() {
        "files" => {
            let files: Vec<String> =
                serde_json::from_str(&item.content).map_err(|error| error.to_string())?;
            context.set_files(files).map_err(|error| error.to_string())?;
        }
        "image" => {
            let image = RustImageData::from_path(&item.content).map_err(|error| error.to_string())?;
            context.set_image(image).map_err(|error| error.to_string())?;
            // Windows 重新编码后字节与原始 PNG 不同，hash 抑制失效，改用时间窗抑制
            state.skip_following_changes();
        }
        _ => context
            .set_text(item.content.clone())
            .map_err(|error| error.to_string())?,
    }
    state.bump_use(id)?;
    Ok(())
}

/// 按需生成图片缩略图，返回可直接用于 `<img src>` 的 data URL。
#[tauri::command]
pub fn clip_image_preview(state: State<'_, ClipboardState>, id: i64) -> Result<String, String> {
    let item = state.get(id)?.ok_or_else(|| "该条目已被删除".to_string())?;
    if item.kind != "image" {
        return Err("该条目不是图片".to_string());
    }
    let path = Path::new(&item.content);
    if !path.starts_with(&*state.image_dir) {
        return Err("图片路径不在历史目录内".to_string());
    }
    let image = RustImageData::from_path(&item.content).map_err(|error| error.to_string())?;
    let thumbnail = image.thumbnail(560, 360).map_err(|error| error.to_string())?;
    let png = thumbnail.to_png().map_err(|error| error.to_string())?;
    let encoded = base64::engine::general_purpose::STANDARD.encode(png.get_bytes());
    Ok(format!("data:image/png;base64,{encoded}"))
}

#[tauri::command]
pub fn clip_set_pin(state: State<'_, ClipboardState>, id: i64, value: bool) -> Result<(), String> {
    state.set_flag(id, "pinned", value)
}

#[tauri::command]
pub fn clip_set_favorite(state: State<'_, ClipboardState>, id: i64, value: bool) -> Result<(), String> {
    state.set_flag(id, "favorite", value)
}

#[tauri::command]
pub fn clip_delete(state: State<'_, ClipboardState>, id: i64) -> Result<(), String> {
    state.delete(id)
}

#[tauri::command]
pub fn clip_clear(state: State<'_, ClipboardState>) -> Result<(), String> {
    state.clear()
}

#[tauri::command]
pub fn clip_set_monitor(state: State<'_, ClipboardState>, enabled: bool) -> bool {
    state.set_paused(!enabled);
    enabled
}

#[tauri::command]
pub fn clip_get_monitor(state: State<'_, ClipboardState>) -> bool {
    !state.is_paused()
}

fn show_clipboard_window(app: &AppHandle) -> Result<(), String> {
    let window = app
        .get_webview_window("clipboard")
        .ok_or_else(|| "找不到剪贴板窗口".to_string())?;
    app.state::<ClipboardState>().mark_shown();
    window.show().map_err(|error| error.to_string())?;
    window.set_focus().map_err(|error| error.to_string())?;
    let _ = app.emit(CLIP_WINDOW_SHOWN, ());
    Ok(())
}

fn hide_clipboard_window(app: &AppHandle) -> Result<(), String> {
    let window = app
        .get_webview_window("clipboard")
        .ok_or_else(|| "找不到剪贴板窗口".to_string())?;
    window.hide().map_err(|error| error.to_string())
}

/// 供全局快捷键调用：可见则隐藏，不可见则显示并聚焦。
pub fn toggle_clipboard_window(app: &AppHandle) {
    let Some(window) = app.get_webview_window("clipboard") else {
        log::warn!("找不到剪贴板窗口");
        return;
    };
    let result = if window.is_visible().unwrap_or(false) {
        hide_clipboard_window(app)
    } else {
        show_clipboard_window(app)
    };
    if let Err(error) = result {
        log::warn!("切换剪贴板窗口失败：{error}");
    }
}

#[tauri::command]
pub fn clip_open_window(app: AppHandle) -> Result<(), String> {
    show_clipboard_window(&app)
}

#[tauri::command]
pub fn clip_close_window(app: AppHandle) -> Result<(), String> {
    hide_clipboard_window(&app)
}

#[tauri::command]
pub fn clip_shortcut() -> String {
    CLIP_SHORTCUT.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_state() -> ClipboardState {
        ClipboardState::open(
            Path::new(":memory:"),
            &std::env::temp_dir().join("orbit-clipboard-test"),
        )
        .unwrap()
    }

    #[test]
    fn secret_rules_are_conservative() {
        assert!(contains_secret(
            "-----BEGIN OPENSSH PRIVATE KEY-----\nabc\n-----END OPENSSH PRIVATE KEY-----"
        ));
        assert!(contains_secret("AKIAIOSFODNN7EXAMPLE"));
        assert!(contains_secret(r#"{"api_key": "dummy_secret_value_abcdefghijklmnopqr"}"#));
        assert!(!contains_secret("这是一段普通的剪贴板文本，包含 token 这个词。"));
        assert!(!contains_secret("验证码 123456"));
    }

    #[test]
    fn upsert_deduplicates_and_bumps_use_count() {
        let state = test_state();
        let hash = hash_of("text:hello");
        let (first, dedup1) = state.upsert("text", "hello", "hello", &hash).unwrap();
        assert!(!dedup1);
        let (second, dedup2) = state.upsert("text", "hello", "hello", &hash).unwrap();
        assert!(dedup2);
        assert_eq!(first, second);
        let item = state.get(first).unwrap().unwrap();
        assert_eq!(item.use_count, 2);
        assert_eq!(state.count(), 1);
    }

    #[test]
    fn writeback_suppression_consumes_once() {
        let state = test_state();
        state.suppress("abc");
        assert!(state.take_writeback("abc"));
        assert!(!state.take_writeback("abc"));
    }

    #[test]
    fn image_writeback_opens_skip_window() {
        let state = test_state();
        assert!(!state.in_skip_window());
        state.skip_following_changes();
        assert!(state.in_skip_window());
    }

    #[test]
    fn image_cleanup_stays_inside_history_dir() {
        let state = test_state();
        let path = state
            .store_image("unit-test-image", b"not-a-real-png")
            .unwrap();
        assert!(path.exists());
        state.remove_image_file(&path.to_string_lossy());
        assert!(!path.exists());

        // 目录外的路径不允许被清理
        let outside = std::env::temp_dir().join("orbit-outside-test.png");
        std::fs::write(&outside, b"x").unwrap();
        state.remove_image_file(&outside.to_string_lossy());
        assert!(outside.exists());
        let _ = std::fs::remove_file(&outside);
    }
}

//! 惰性窗口创建。
//!
//! `tauri.conf.json` 里 `clipboard` / `selection` 窗口带 `create: false`：启动时只创建
//! `main`，这两个窗口在首次使用时按同一份窗口配置在主线程上创建。窗口配置仍然保留在
//! `app.windows` 里，所以 `layout_config` 的尺寸单一事实源不受影响。
//!
//! 收益：启动期的 WebView2 渲染进程从 3 个降为 1 个（见 `docs/PERF.md` 的 P-02）。
//! 约束：创建必须走主线程（tauri `run_on_main_thread`），调用方不得在主线程内调用
//! `ensure`，否则会自锁（当前调用点都在命令总线的阻塞线程池或插件回调线程）。

use std::sync::mpsc;
use std::time::Duration;

use tauri::{AppHandle, Manager, WebviewWindow, WebviewWindowBuilder};

pub(crate) const CLIPBOARD_LABEL: &str = "clipboard";
pub(crate) const SELECTION_LABEL: &str = "selection";

/// 取窗口；不存在时按 `tauri.conf.json` 的配置创建。
pub(crate) fn ensure(app: &AppHandle, label: &str) -> Result<WebviewWindow, String> {
    if let Some(window) = app.get_webview_window(label) {
        return Ok(window);
    }
    let config = app
        .config()
        .app
        .windows
        .iter()
        .find(|window| window.label == label)
        .cloned()
        .ok_or_else(|| format!("tauri.conf.json 缺少 {label} 窗口配置"))?;

    let (tx, rx) = mpsc::channel();
    let handle = app.clone();
    app.run_on_main_thread(move || {
        let created = WebviewWindowBuilder::from_config(&handle, &config)
            .map_err(|error| error.to_string())
            .and_then(|builder| builder.build().map_err(|error| error.to_string()))
            .map(|_| ());
        let _ = tx.send(created);
    })
    .map_err(|error| format!("调度 {label} 窗口创建失败：{error}"))?;
    rx.recv_timeout(Duration::from_secs(5))
        .map_err(|error| format!("创建 {label} 窗口超时：{error}"))??;
    app.get_webview_window(label)
        .ok_or_else(|| format!("创建 {label} 窗口后仍取不到窗口句柄"))
}

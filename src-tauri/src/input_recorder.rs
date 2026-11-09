//! 键鼠录制：Windows 低级钩子（WH_KEYBOARD_LL / WH_MOUSE_LL）。
//! 不用 rdev::listen——它阻塞且无停止 API，每次录制会泄漏一个线程；
//! 原生钩子可通过 PostThreadMessage(WM_QUIT) + UnhookWindowsHookEx 干净停止。

use serde::Serialize;

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct KbEvent {
    pub timestamp: u64,
    pub kind: String,
    pub key: Option<String>,
    pub x: Option<i32>,
    pub y: Option<i32>,
    pub button: Option<String>,
    pub direction: Option<String>,
}

pub fn save_recording(
    dir: &std::path::Path,
    events: &[KbEvent],
    duration_ms: u64,
) -> Result<String, String> {
    let conn = rusqlite::Connection::open(dir.join("recordings.db"))
        .map_err(|e| format!("录制数据库打开失败：{e}"))?;
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS recordings (
            id          TEXT PRIMARY KEY,
            name        TEXT NOT NULL,
            created_at  INTEGER NOT NULL,
            duration_ms INTEGER NOT NULL,
            platform    TEXT NOT NULL,
            events      TEXT NOT NULL,
            metadata    TEXT
        );
        PRAGMA user_version = 1;",
    )
    .map_err(|e| format!("录制表初始化失败：{e}"))?;

    let created_at = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0);
    let id = format!("rec_{created_at}");
    let json = serde_json::to_string(events).map_err(|e| e.to_string())?;
    conn.execute(
        "INSERT INTO recordings (id, name, created_at, duration_ms, platform, events, metadata)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, NULL)",
        rusqlite::params![
            id,
            format!("键鼠录制 {id}"),
            created_at,
            duration_ms as i64,
            std::env::consts::OS,
            json,
        ],
    )
    .map_err(|e| format!("录制保存失败：{e}"))?;
    Ok(id)
}

#[cfg(target_os = "windows")]
pub use win::{RecordingSession, start, stop};

#[cfg(not(target_os = "windows"))]
pub struct RecordingSession;

#[cfg(not(target_os = "windows"))]
pub fn start() -> Result<RecordingSession, String> {
    Err("键鼠录制当前仅支持 Windows。".to_string())
}

#[cfg(not(target_os = "windows"))]
pub fn stop(_session: RecordingSession) -> Result<(Vec<KbEvent>, u64), String> {
    Err("键鼠录制当前仅支持 Windows。".to_string())
}

#[cfg(target_os = "windows")]
mod win {
    use super::KbEvent;
    use std::sync::{Arc, Mutex, mpsc};
    use std::time::{Duration, Instant};
    use windows::Win32::Foundation::{HINSTANCE, LPARAM, LRESULT, POINT, WPARAM};
    use windows::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows::Win32::System::Threading::GetCurrentThreadId;
    use windows::Win32::UI::WindowsAndMessaging::{
        CallNextHookEx, GetMessageW, KBDLLHOOKSTRUCT, MSLLHOOKSTRUCT, MSG,
        PostThreadMessageW, SetWindowsHookExW, UnhookWindowsHookEx,
        WH_KEYBOARD_LL, WH_MOUSE_LL, WM_KEYDOWN, WM_KEYUP, WM_LBUTTONDOWN, WM_LBUTTONUP,
        WM_MBUTTONDOWN, WM_MBUTTONUP, WM_MOUSEMOVE, WM_MOUSEWHEEL, WM_QUIT, WM_RBUTTONDOWN,
        WM_RBUTTONUP, WM_SYSKEYDOWN, WM_SYSKEYUP, WM_XBUTTONDOWN, WM_XBUTTONUP,
    };

    struct Shared {
        events: Arc<Mutex<Vec<KbEvent>>>,
        start: Instant,
    }

    // 钩子回调是 extern "system" fn，无法捕获上下文，只能走全局表。
    static BUFFER: Mutex<Option<Shared>> = Mutex::new(None);

    pub struct RecordingSession {
        thread_id: u32,
        handle: std::thread::JoinHandle<()>,
        events: Arc<Mutex<Vec<KbEvent>>>,
        start: Instant,
    }

    fn push(
        kind: &str,
        key: Option<String>,
        point: Option<POINT>,
        button: Option<&str>,
        direction: Option<&str>,
    ) {
        // 回调内绝不 panic（穿越 FFI 边界会直接终止进程），全部用 if-let。
        if let Ok(guard) = BUFFER.lock() {
            if let Some(shared) = guard.as_ref() {
                if let Ok(mut events) = shared.events.lock() {
                    events.push(KbEvent {
                        timestamp: shared.start.elapsed().as_millis() as u64,
                        kind: kind.to_string(),
                        key,
                        x: point.map(|p| p.x),
                        y: point.map(|p| p.y),
                        button: button.map(str::to_string),
                        direction: direction.map(str::to_string),
                    });
                }
            }
        }
    }

    fn vk_name(vk: u32) -> String {
        match vk {
            0x08 => "Backspace".into(),
            0x09 => "Tab".into(),
            0x0D => "Enter".into(),
            0x10 => "Shift".into(),
            0x11 => "Control".into(),
            0x12 => "Alt".into(),
            0x1B => "Escape".into(),
            0x20 => "Space".into(),
            0x25 => "ArrowLeft".into(),
            0x26 => "ArrowUp".into(),
            0x27 => "ArrowRight".into(),
            0x28 => "ArrowDown".into(),
            0x2E => "Delete".into(),
            0x30..=0x39 | 0x41..=0x5A => char::from_u32(vk)
                .map(|c| c.to_string())
                .unwrap_or_else(|| format!("vk{vk:#04x}")),
            0x70..=0x7B => format!("F{}", vk - 0x6F),
            0xA0 => "ShiftLeft".into(),
            0xA1 => "ShiftRight".into(),
            0xA2 => "ControlLeft".into(),
            0xA3 => "ControlRight".into(),
            0xA4 => "AltLeft".into(),
            0xA5 => "AltRight".into(),
            _ => format!("vk{vk:#04x}"),
        }
    }

    unsafe extern "system" fn keyboard_hook(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
        if code >= 0 {
            let info = unsafe { &*(lparam.0 as *const KBDLLHOOKSTRUCT) };
            let kind = match wparam.0 as u32 {
                WM_KEYDOWN | WM_SYSKEYDOWN => Some("key_press"),
                WM_KEYUP | WM_SYSKEYUP => Some("key_release"),
                _ => None,
            };
            if let Some(kind) = kind {
                push(kind, Some(vk_name(info.vkCode)), None, None, None);
            }
        }
        unsafe { CallNextHookEx(None, code, wparam, lparam) }
    }

    unsafe extern "system" fn mouse_hook(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
        if code >= 0 {
            let info = unsafe { &*(lparam.0 as *const MSLLHOOKSTRUCT) };
            match wparam.0 as u32 {
                WM_MOUSEMOVE => push("mouse_move", None, Some(info.pt), None, None),
                WM_LBUTTONDOWN => push("mouse_button", None, Some(info.pt), Some("Left"), Some("press")),
                WM_LBUTTONUP => push("mouse_button", None, Some(info.pt), Some("Left"), Some("release")),
                WM_RBUTTONDOWN => push("mouse_button", None, Some(info.pt), Some("Right"), Some("press")),
                WM_RBUTTONUP => push("mouse_button", None, Some(info.pt), Some("Right"), Some("release")),
                WM_MBUTTONDOWN => push("mouse_button", None, Some(info.pt), Some("Middle"), Some("press")),
                WM_MBUTTONUP => push("mouse_button", None, Some(info.pt), Some("Middle"), Some("release")),
                WM_XBUTTONDOWN | WM_XBUTTONUP => {
                    let which = (info.mouseData >> 16) & 0xFFFF;
                    let button = if which == 1 { "X1" } else { "X2" };
                    let direction = if wparam.0 as u32 == WM_XBUTTONDOWN { "press" } else { "release" };
                    push("mouse_button", None, Some(info.pt), Some(button), Some(direction));
                }
                WM_MOUSEWHEEL => {
                    // 高字为带符号滚动增量，正值向上。
                    let delta = (info.mouseData >> 16) as i16;
                    let direction = if delta > 0 { "up" } else { "down" };
                    push("mouse_wheel", None, Some(info.pt), None, Some(direction));
                }
                _ => {}
            }
        }
        unsafe { CallNextHookEx(None, code, wparam, lparam) }
    }

    pub fn start() -> Result<RecordingSession, String> {
        let events = Arc::new(Mutex::new(Vec::<KbEvent>::new()));
        let start = Instant::now();
        let (tx, rx) = mpsc::channel::<Result<u32, String>>();
        let ev = events.clone();

        let handle = std::thread::Builder::new()
            .name("orbit-input-recorder".into())
            .spawn(move || {
                *BUFFER.lock().unwrap() = Some(Shared { events: ev, start });
                unsafe {
                    let module = match GetModuleHandleW(None) {
                        Ok(m) => m,
                        Err(e) => {
                            let _ = tx.send(Err(format!("GetModuleHandle 失败：{e}")));
                            *BUFFER.lock().unwrap() = None;
                            return;
                        }
                    };
                    let hinstance = HINSTANCE(module.0);
                    let kb = SetWindowsHookExW(WH_KEYBOARD_LL, Some(keyboard_hook), Some(hinstance), 0);
                    let mouse = SetWindowsHookExW(WH_MOUSE_LL, Some(mouse_hook), Some(hinstance), 0);
                    match (kb, mouse) {
                        (Ok(kb), Ok(mouse)) => {
                            let _ = tx.send(Ok(GetCurrentThreadId()));
                            // 低级钩子要求安装线程跑消息循环；WM_QUIT 使其退出。
                            let mut msg = MSG::default();
                            while GetMessageW(&mut msg, None, 0, 0).0 > 0 {}
                            let _ = UnhookWindowsHookEx(kb);
                            let _ = UnhookWindowsHookEx(mouse);
                        }
                        (kb, mouse) => {
                            if let Ok(h) = kb {
                                let _ = UnhookWindowsHookEx(h);
                            }
                            if let Ok(h) = mouse {
                                let _ = UnhookWindowsHookEx(h);
                            }
                            let _ = tx.send(Err("安装键鼠钩子失败（可能缺少权限或被安全软件拦截）。".into()));
                        }
                    }
                }
                *BUFFER.lock().unwrap() = None;
            })
            .map_err(|e| format!("录制线程启动失败：{e}"))?;

        let thread_id = rx
            .recv_timeout(Duration::from_secs(5))
            .map_err(|_| "录制线程未在 5 秒内就绪。".to_string())?
            .map_err(|e| e)?;

        Ok(RecordingSession { thread_id, handle, events, start })
    }

    pub fn stop(session: RecordingSession) -> Result<(Vec<KbEvent>, u64), String> {
        unsafe { PostThreadMessageW(session.thread_id, WM_QUIT, WPARAM(0), LPARAM(0)) }
            .map_err(|e| format!("无法通知录制线程退出：{e}"))?;
        session
            .handle
            .join()
            .map_err(|_| "录制线程异常终止。".to_string())?;
        let duration_ms = session.start.elapsed().as_millis() as u64;
        let events = session
            .events
            .lock()
            .map_err(|e| e.to_string())?
            .clone();
        Ok((events, duration_ms))
    }
}

#[cfg(all(test, target_os = "windows"))]
mod tests {
    use enigo::{Direction, Enigo, Key, Keyboard, Settings};

    #[test]
    fn records_synthetic_input_and_stops_cleanly() {
        let session = super::start().expect("录制启动失败");
        let mut enigo = Enigo::new(&Settings::default()).expect("enigo 初始化失败");
        enigo.key(Key::Unicode('a'), Direction::Press).unwrap();
        enigo.key(Key::Unicode('a'), Direction::Release).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(250));

        let (events, duration_ms) = super::stop(session).expect("录制停止失败");
        assert!(duration_ms > 0);
        assert!(
            events.iter().any(|e| e.kind == "key_press" && e.key.as_deref() == Some("A")),
            "未捕获到合成按键，事件数：{}",
            events.len()
        );

        // 停止后必须能再次启动（验证钩子已卸载、无残留状态）。
        let second = super::start().expect("二次启动失败");
        let (_, _) = super::stop(second).expect("二次停止失败");
    }
}
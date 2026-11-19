//! 键鼠录制：Windows 低级钩子（WH_KEYBOARD_LL / WH_MOUSE_LL）。
//! 不用 rdev::listen——它阻塞且无停止 API，每次录制会泄漏一个线程；
//! 原生钩子可通过 PostThreadMessage(WM_QUIT) + UnhookWindowsHookEx 干净停止。
//!
//! 回放走 enigo（仅 Windows 编译），按录制时间戳排程，返回实际耗时与漂移，
//! 供 P-09（回放漂移 < 20ms / 10s）的测量使用。

/// 事件结构由 orbit-core 契约统一定义，总线返回与前端绑定共用一份。
pub use orbit_core::KbEvent;
use orbit_core::{KbReplayResult, RecordingItem};

const RECORDINGS_DB: &str = "recordings.db";

fn open_recordings(dir: &std::path::Path) -> Result<rusqlite::Connection, String> {
    let conn = rusqlite::Connection::open(dir.join(RECORDINGS_DB))
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
        );",
    )
    .map_err(|e| format!("录制表初始化失败：{e}"))?;
    Ok(conn)
}

pub fn save_recording(
    dir: &std::path::Path,
    events: &[KbEvent],
    duration_ms: u64,
) -> Result<String, String> {
    let conn = open_recordings(dir)?;
    conn.execute_batch("PRAGMA user_version = 1;")
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

/// 列出已保存的录制（按创建时间倒序），不加载事件体。
pub fn list_recordings(dir: &std::path::Path) -> Result<Vec<RecordingItem>, String> {
    let conn = open_recordings(dir)?;
    let mut stmt = conn
        .prepare(
            "SELECT id, name, created_at, duration_ms, platform,
                    (SELECT COUNT(*) FROM json_each(recordings.events))
             FROM recordings ORDER BY created_at DESC",
        )
        .map_err(|e| format!("录制查询准备失败：{e}"))?;
    let rows = stmt
        .query_map([], |row| {
            Ok(RecordingItem {
                id: row.get(0)?,
                name: row.get(1)?,
                created_at: row.get(2)?,
                duration_ms: row.get::<_, i64>(3)? as u64,
                platform: row.get(4)?,
                event_count: row.get::<_, i64>(5)? as u32,
            })
        })
        .map_err(|e| format!("录制查询失败：{e}"))?;
    rows.collect::<Result<Vec<_>, _>>()
        .map_err(|e| format!("录制行解析失败：{e}"))
}

/// 读取一条录制的事件体。
pub fn load_recording(
    dir: &std::path::Path,
    recording_id: &str,
) -> Result<(Vec<KbEvent>, u64), String> {
    let conn = open_recordings(dir)?;
    let (json, duration_ms): (String, i64) = conn
        .query_row(
            "SELECT events, duration_ms FROM recordings WHERE id = ?1",
            rusqlite::params![recording_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .map_err(|error| match error {
            rusqlite::Error::QueryReturnedNoRows => format!("找不到录制：{recording_id}"),
            other => format!("读取录制失败：{other}"),
        })?;
    let events: Vec<KbEvent> =
        serde_json::from_str(&json).map_err(|e| format!("录制事件解析失败：{e}"))?;
    Ok((events, duration_ms as u64))
}

/// 回放录制：按事件时间戳排程，返回事件数与耗时/漂移。
pub fn replay(events: &[KbEvent], recording_id: &str, speed: f64) -> Result<KbReplayResult, String> {
    if events.is_empty() {
        return Err("录制为空，无法回放。".to_string());
    }
    if !(0.1..=10.0).contains(&speed) {
        return Err(format!("回放倍速 {speed} 超出范围（0.1 ~ 10）。"));
    }
    let (elapsed_ms, drift_ms) = replay_events(events, speed)?;
    Ok(KbReplayResult {
        recording_id: recording_id.to_string(),
        event_count: events.len() as u32,
        elapsed_ms,
        drift_ms,
    })
}

/// 敏感窗口标题关键词：命中时跳过该窗口下的按键采集。
/// 覆盖中英文常见的密码 / 凭据 / 支付场景，大小写不敏感。
/// 这是纯函数，便于单测；实际标题由平台层低频读取，钩子回调只读其结论。
const SENSITIVE_TITLE_KEYWORDS: &[&str] = &[
    "密码", "口令", "凭据", "密钥", "银行卡", "网银", "验证码", "password", "credential",
    "passphrase", "private key", "pin code", "bank login",
];

pub fn is_sensitive_window_title(title: &str) -> bool {
    let lowered = title.to_lowercase();
    SENSITIVE_TITLE_KEYWORDS
        .iter()
        .any(|keyword| lowered.contains(keyword))
}

/// 鼠标按钮：与回放侧 `mouse_button` 事件保持同一组取值。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MouseButton {
    Left,
    Right,
    Middle,
    X1,
    X2,
}

/// 鼠标控制动作：由 `plan_mouse` 从契约参数归类而来，交给平台层执行。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MouseCommand {
    Move { x: i32, y: i32 },
    Click { button: MouseButton, count: u32 },
    Scroll { amount: i32 },
}

/// 纯校验：把 `InputMouseParams` 归类为可执行的 `MouseCommand`。
/// 不触碰系统输入，可在任意平台单测；错误统一用中文描述，避免静默丢字段。
pub fn plan_mouse(params: &orbit_core::InputMouseParams) -> Result<MouseCommand, String> {
    use orbit_core::MouseAction;
    match params.action {
        MouseAction::Move => {
            let x = params.x.ok_or_else(|| "移动鼠标缺少 x 坐标".to_string())?;
            let y = params.y.ok_or_else(|| "移动鼠标缺少 y 坐标".to_string())?;
            Ok(MouseCommand::Move { x, y })
        }
        MouseAction::Click => {
            let name = params
                .button
                .as_deref()
                .ok_or_else(|| "点击缺少 button 字段".to_string())?;
            let button = match name {
                "Left" => MouseButton::Left,
                "Right" => MouseButton::Right,
                "Middle" => MouseButton::Middle,
                "X1" => MouseButton::X1,
                "X2" => MouseButton::X2,
                other => return Err(format!("不支持的鼠标按钮：{other}")),
            };
            let count = params.count.unwrap_or(1);
            if !(1..=10).contains(&count) {
                return Err(format!("连击次数 {count} 超出范围（1 ~ 10）"));
            }
            Ok(MouseCommand::Click { button, count })
        }
        MouseAction::Scroll => {
            let amount = params
                .amount
                .ok_or_else(|| "滚轮缺少 amount 字段".to_string())?;
            if amount == 0 {
                return Err("滚轮步数不能为 0".to_string());
            }
            if !(-10..=10).contains(&amount) {
                return Err(format!("滚轮步数 {amount} 超出范围（-10 ~ 10）"));
            }
            Ok(MouseCommand::Scroll { amount })
        }
    }
}

/// 执行归类的鼠标动作（Windows 走 enigo 合成；其他平台返回不支持）。
pub fn apply_mouse(command: MouseCommand) -> Result<(), String> {
    apply_mouse_platform(command)
}

#[cfg(target_os = "windows")]
fn apply_mouse_platform(command: MouseCommand) -> Result<(), String> {
    use enigo::{Axis, Button, Coordinate, Direction, Enigo, Mouse, Settings};
    let mut enigo = Enigo::new(&Settings::default())
        .map_err(|e| format!("初始化输入合成失败（可能缺少权限）：{e}"))?;
    match command {
        MouseCommand::Move { x, y } => enigo
            .move_mouse(x, y, Coordinate::Abs)
            .map_err(|e| format!("移动鼠标失败：{e}")),
        MouseCommand::Click { button, count } => {
            let button = match button {
                MouseButton::Left => Button::Left,
                MouseButton::Right => Button::Right,
                MouseButton::Middle => Button::Middle,
                MouseButton::X1 => Button::Back,
                MouseButton::X2 => Button::Forward,
            };
            for _ in 0..count {
                enigo
                    .button(button, Direction::Click)
                    .map_err(|e| format!("合成鼠标点击失败：{e}"))?;
            }
            Ok(())
        }
        MouseCommand::Scroll { amount } => enigo
            .scroll(amount, Axis::Vertical)
            .map_err(|e| format!("合成滚轮失败：{e}")),
    }
}

#[cfg(not(target_os = "windows"))]
fn apply_mouse_platform(_command: MouseCommand) -> Result<(), String> {
    Err("鼠标控制当前仅支持 Windows。".to_string())
}

#[cfg(target_os = "windows")]
fn replay_events(events: &[KbEvent], speed: f64) -> Result<(u64, i64), String> {
    use enigo::{Axis, Button, Coordinate, Direction, Enigo, Keyboard, Mouse, Settings};
    use std::time::{Duration, Instant};

    let mut enigo = Enigo::new(&Settings::default())
        .map_err(|e| format!("初始化输入合成失败（可能缺少权限）：{e}"))?;
    let started = Instant::now();
    let mut scheduled = 0u64;

    for event in events {
        let target = (event.timestamp as f64 / speed) as u64;
        if target > scheduled {
            std::thread::sleep(Duration::from_millis(target - scheduled));
            scheduled = target;
        }
        match event.kind.as_str() {
            "key_press" | "key_release" => {
                let name = event
                    .key
                    .as_deref()
                    .ok_or_else(|| "按键事件缺少 key 字段。".to_string())?;
                let key = parse_key(name)?;
                let direction = if event.kind == "key_press" {
                    Direction::Press
                } else {
                    Direction::Release
                };
                enigo
                    .key(key, direction)
                    .map_err(|e| format!("合成按键 {name} 失败：{e}"))?;
            }
            "mouse_move" => {
                let (x, y) = (event.x.unwrap_or(0), event.y.unwrap_or(0));
                enigo
                    .move_mouse(x, y, Coordinate::Abs)
                    .map_err(|e| format!("移动鼠标失败：{e}"))?;
            }
            "mouse_button" => {
                let button = match event.button.as_deref() {
                    Some("Left") => Button::Left,
                    Some("Right") => Button::Right,
                    Some("Middle") => Button::Middle,
                    Some("X1") => Button::Back,
                    Some("X2") => Button::Forward,
                    Some(other) => return Err(format!("不支持的鼠标按钮：{other}")),
                    None => return Err("鼠标按钮事件缺少 button 字段。".to_string()),
                };
                let direction = match event.direction.as_deref() {
                    Some("press") => Direction::Press,
                    Some("release") => Direction::Release,
                    _ => return Err("鼠标按钮事件缺少 direction 字段。".to_string()),
                };
                enigo
                    .button(button, direction)
                    .map_err(|e| format!("合成鼠标按钮失败：{e}"))?;
            }
            "mouse_wheel" => {
                let length = match event.direction.as_deref() {
                    Some("up") => 1,
                    Some("down") => -1,
                    _ => return Err("滚轮事件缺少方向。".to_string()),
                };
                enigo
                    .scroll(length, Axis::Vertical)
                    .map_err(|e| format!("合成滚轮失败：{e}"))?;
            }
            other => return Err(format!("不支持的事件类型：{other}")),
        }
    }

    let elapsed_ms = started.elapsed().as_millis() as u64;
    let expected_ms = (events.last().map(|event| event.timestamp).unwrap_or(0) as f64 / speed) as i64;
    Ok((elapsed_ms, elapsed_ms as i64 - expected_ms))
}

#[cfg(not(target_os = "windows"))]
fn replay_events(_events: &[KbEvent], _speed: f64) -> Result<(u64, i64), String> {
    Err("键鼠回放当前仅支持 Windows。".to_string())
}

/// 录制侧键名 → enigo Key。未覆盖的 `vkXX` 明确报错，不静默跳过。
#[cfg(target_os = "windows")]
fn parse_key(name: &str) -> Result<enigo::Key, String> {
    use enigo::Key;
    let named = match name {
        "Backspace" => Some(Key::Backspace),
        "Tab" => Some(Key::Tab),
        "Enter" => Some(Key::Return),
        "Escape" => Some(Key::Escape),
        "Space" => Some(Key::Space),
        "ArrowLeft" => Some(Key::LeftArrow),
        "ArrowUp" => Some(Key::UpArrow),
        "ArrowRight" => Some(Key::RightArrow),
        "ArrowDown" => Some(Key::DownArrow),
        "Delete" => Some(Key::Delete),
        "ShiftLeft" | "Shift" => Some(Key::Shift),
        "ShiftRight" => Some(Key::RShift),
        "ControlLeft" | "Control" => Some(Key::Control),
        "ControlRight" => Some(Key::RControl),
        "AltLeft" | "Alt" => Some(Key::Alt),
        "AltRight" => Some(Key::Alt),
        "F1" => Some(Key::F1),
        "F2" => Some(Key::F2),
        "F3" => Some(Key::F3),
        "F4" => Some(Key::F4),
        "F5" => Some(Key::F5),
        "F6" => Some(Key::F6),
        "F7" => Some(Key::F7),
        "F8" => Some(Key::F8),
        "F9" => Some(Key::F9),
        "F10" => Some(Key::F10),
        "F11" => Some(Key::F11),
        "F12" => Some(Key::F12),
        _ => None,
    };
    if let Some(key) = named {
        return Ok(key);
    }
    let mut chars = name.chars();
    match (chars.next(), chars.next()) {
        (Some(ch), None) => Ok(Key::Unicode(ch)),
        _ => Err(format!("无法映射录制键位：{name}")),
    }
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
pub fn stop(_session: RecordingSession) -> Result<(Vec<KbEvent>, u64, u32), String> {
    Err("键鼠录制当前仅支持 Windows。".to_string())
}

#[cfg(target_os = "windows")]
mod win {
    use super::{KbEvent, is_sensitive_window_title};
    use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
    use std::sync::{Arc, Mutex, mpsc};
    use std::time::{Duration, Instant};
    use windows::Win32::Foundation::{HINSTANCE, LPARAM, LRESULT, POINT, WPARAM};
    use windows::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows::Win32::System::Threading::GetCurrentThreadId;
    use windows::Win32::UI::WindowsAndMessaging::{
        CallNextHookEx, GetForegroundWindow, GetMessageW, GetWindowTextLengthW, GetWindowTextW,
        KBDLLHOOKSTRUCT, MSLLHOOKSTRUCT, MSG, PostThreadMessageW, SetWindowsHookExW,
        UnhookWindowsHookEx, WH_KEYBOARD_LL, WH_MOUSE_LL, WM_KEYDOWN, WM_KEYUP, WM_LBUTTONDOWN,
        WM_LBUTTONUP, WM_MBUTTONDOWN, WM_MBUTTONUP, WM_MOUSEMOVE, WM_MOUSEWHEEL, WM_QUIT,
        WM_RBUTTONDOWN, WM_RBUTTONUP, WM_SYSKEYDOWN, WM_SYSKEYUP, WM_XBUTTONDOWN, WM_XBUTTONUP,
    };

    struct Shared {
        events: Arc<Mutex<Vec<KbEvent>>>,
        start: Instant,
        sensitive: Arc<AtomicBool>,
        redacted: Arc<AtomicU32>,
    }

    // 钩子回调是 extern "system" fn，无法捕获上下文，只能走全局表。
    static BUFFER: Mutex<Option<Shared>> = Mutex::new(None);

    pub struct RecordingSession {
        thread_id: u32,
        handle: std::thread::JoinHandle<()>,
        watch_handle: std::thread::JoinHandle<()>,
        watch_stop: Arc<AtomicBool>,
        events: Arc<Mutex<Vec<KbEvent>>>,
        redacted: Arc<AtomicU32>,
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
                // 键盘事件在敏感窗口下丢弃：只读原子布尔，不做阻塞式窗口查询。
                if key.is_some() && shared.sensitive.load(Ordering::Relaxed) {
                    shared.redacted.fetch_add(1, Ordering::Relaxed);
                    return;
                }
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

    /// 前台窗口标题读取：失败返回空串（视为非敏感），绝不 panic。
    fn foreground_title() -> String {
        let hwnd = unsafe { GetForegroundWindow() };
        if hwnd.0.is_null() {
            return String::new();
        }
        let length = unsafe { GetWindowTextLengthW(hwnd) };
        if length <= 0 {
            return String::new();
        }
        let mut buffer = vec![0u16; length as usize + 1];
        let copied = unsafe { GetWindowTextW(hwnd, &mut buffer) };
        if copied <= 0 {
            return String::new();
        }
        String::from_utf16_lossy(&buffer[..copied as usize])
    }

    /// 低频轮询前台窗口标题，把敏感结论写入原子标志。
    /// 独立于钩子线程，避免阻塞输入；用 stop 标志干净退出。
    fn watch_foreground(flag: Arc<AtomicBool>, stop: Arc<AtomicBool>) {
        while !stop.load(Ordering::Relaxed) {
            let sensitive = is_sensitive_window_title(&foreground_title());
            flag.store(sensitive, Ordering::Relaxed);
            std::thread::sleep(Duration::from_millis(250));
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
        let sensitive = Arc::new(AtomicBool::new(false));
        let redacted = Arc::new(AtomicU32::new(0));
        let watch_stop = Arc::new(AtomicBool::new(false));
        let (tx, rx) = mpsc::channel::<Result<u32, String>>();
        let ev = events.clone();
        let sens = sensitive.clone();
        let red = redacted.clone();

        let handle = std::thread::Builder::new()
            .name("orbit-input-recorder".into())
            .spawn(move || {
                *BUFFER.lock().unwrap() = Some(Shared { events: ev, start, sensitive: sens, redacted: red });
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

        let watch_flag = sensitive.clone();
        let watch_stop_flag = watch_stop.clone();
        let watch_handle = std::thread::Builder::new()
            .name("orbit-sensitive-watcher".into())
            .spawn(move || watch_foreground(watch_flag, watch_stop_flag))
            .map_err(|e| format!("敏感窗口监视线程启动失败：{e}"))?;

        Ok(RecordingSession {
            thread_id,
            handle,
            watch_handle,
            watch_stop,
            events,
            redacted,
            start,
        })
    }

    pub fn stop(session: RecordingSession) -> Result<(Vec<KbEvent>, u64, u32), String> {
        session.watch_stop.store(true, Ordering::Relaxed);
        unsafe { PostThreadMessageW(session.thread_id, WM_QUIT, WPARAM(0), LPARAM(0)) }
            .map_err(|e| format!("无法通知录制线程退出：{e}"))?;
        let _ = session.watch_handle.join();
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
        let redacted = session.redacted.load(Ordering::Relaxed);
        Ok((events, duration_ms, redacted))
    }
}

#[cfg(test)]
mod recording_store_tests {
    use super::*;

    fn temp_dir() -> std::path::PathBuf {
        let dir = std::env::temp_dir().join("orbit-recordings-test");
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn sample_events() -> Vec<KbEvent> {
        vec![
            KbEvent {
                timestamp: 0,
                kind: "key_press".to_string(),
                key: Some("A".to_string()),
                x: None,
                y: None,
                button: None,
                direction: None,
            },
            KbEvent {
                timestamp: 120,
                kind: "key_release".to_string(),
                key: Some("A".to_string()),
                x: None,
                y: None,
                button: None,
                direction: None,
            },
        ]
    }

    #[test]
    fn save_list_and_load_round_trip() {
        let dir = temp_dir();
        let id = save_recording(&dir, &sample_events(), 150).unwrap();
        let items = list_recordings(&dir).unwrap();
        let item = items.iter().find(|item| item.id == id).expect("列表缺少刚保存的录制");
        assert_eq!(item.event_count, 2);
        assert_eq!(item.duration_ms, 150);
        assert_eq!(item.platform, std::env::consts::OS);

        let (events, duration_ms) = load_recording(&dir, &id).unwrap();
        assert_eq!(events.len(), 2);
        assert_eq!(duration_ms, 150);
        assert_eq!(events[0].key.as_deref(), Some("A"));
    }

    #[test]
    fn load_missing_recording_is_explicit_error() {
        let dir = temp_dir();
        let error = load_recording(&dir, "rec_missing").unwrap_err();
        assert!(error.contains("找不到录制"), "错误文案应明确指出缺失：{error}");
    }

    #[test]
    fn replay_rejects_empty_recording_and_bad_speed() {
        let empty = replay(&[], "rec_x", 1.0).unwrap_err();
        assert!(empty.contains("录制为空"));
        let bad_speed = replay(&sample_events(), "rec_x", 0.01).unwrap_err();
        assert!(bad_speed.contains("倍速"), "应校验倍速范围：{bad_speed}");
    }

    fn mouse_params(value: serde_json::Value) -> orbit_core::InputMouseParams {
        serde_json::from_value(value).expect("测试参数应可反序列化")
    }

    #[test]
    fn plan_mouse_move_requires_coordinates() {
        let no_x = mouse_params(serde_json::json!({ "action": "move", "y": 5 }));
        assert!(plan_mouse(&no_x).unwrap_err().contains("x"));
        let no_y = mouse_params(serde_json::json!({ "action": "move", "x": 5 }));
        assert!(plan_mouse(&no_y).unwrap_err().contains("y"));
    }

    #[test]
    fn plan_mouse_move_returns_absolute_command() {
        let params = mouse_params(serde_json::json!({ "action": "move", "x": 100, "y": 200 }));
        match plan_mouse(&params).unwrap() {
            MouseCommand::Move { x, y } => assert_eq!((x, y), (100, 200)),
            other => panic!("move 动作应归类为 Move，实际：{other:?}"),
        }
    }

    #[test]
    fn plan_mouse_click_rejects_unknown_button() {
        let params = mouse_params(serde_json::json!({
            "action": "click", "button": "Middle2"
        }));
        let error = plan_mouse(&params).unwrap_err();
        assert!(error.contains("按钮"), "应提示按钮非法：{error}");
    }

    #[test]
    fn plan_mouse_click_defaults_count_to_one_and_checks_range() {
        let params = mouse_params(serde_json::json!({ "action": "click", "button": "Left" }));
        match plan_mouse(&params).unwrap() {
            MouseCommand::Click { count, .. } => assert_eq!(count, 1),
            other => panic!("click 动作应归类为 Click，实际：{other:?}"),
        }
        let bad = mouse_params(serde_json::json!({
            "action": "click", "button": "Left", "count": 99
        }));
        assert!(plan_mouse(&bad).is_err(), "连击次数应限制在合理范围");
    }

    #[test]
    fn plan_mouse_scroll_requires_nonzero_amount() {
        let params = mouse_params(serde_json::json!({ "action": "scroll", "amount": 3 }));
        match plan_mouse(&params).unwrap() {
            MouseCommand::Scroll { amount } => assert_eq!(amount, 3),
            other => panic!("scroll 动作应归类为 Scroll，实际：{other:?}"),
        }
        let zero = mouse_params(serde_json::json!({ "action": "scroll", "amount": 0 }));
        assert!(plan_mouse(&zero).is_err(), "滚轮步数不能为 0");
    }

    #[test]
    fn sensitive_window_title_matches_password_and_credential_keywords() {
        assert!(is_sensitive_window_title("密码管理器 — Chrome"));
        assert!(is_sensitive_window_title("Password Manager"));
        assert!(is_sensitive_window_title("网银登录 - Bank Login"));
    }

    #[test]
    fn sensitive_window_title_ignores_ordinary_windows() {
        assert!(!is_sensitive_window_title("记事本"));
        assert!(!is_sensitive_window_title("Orbit - 悬浮球"));
        assert!(!is_sensitive_window_title(""));
    }
}

#[cfg(all(test, target_os = "windows", feature = "smoke"))]
mod tests {
    use super::KbEvent;
    use enigo::{Direction, Enigo, Key, Keyboard, Settings};

    #[test]
    fn records_synthetic_input_and_stops_cleanly() {
        let session = super::start().expect("录制启动失败");
        let mut enigo = Enigo::new(&Settings::default()).expect("enigo 初始化失败");
        enigo.key(Key::Unicode('a'), Direction::Press).unwrap();
        enigo.key(Key::Unicode('a'), Direction::Release).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(250));

        let (events, duration_ms, _redacted) = super::stop(session).expect("录制停止失败");
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

    /// 实机回放冒烟：合成一次按键按下/抬起，验证排程耗时与漂移上报。
    /// 会向当前前台窗口输入一个 'a'（与录制冒烟同类副作用）。
    #[test]
    fn replay_synthetic_key_events_reports_drift() {
        let events = vec![
            KbEvent {
                timestamp: 0,
                kind: "key_press".to_string(),
                key: Some("a".to_string()),
                x: None,
                y: None,
                button: None,
                direction: None,
            },
            KbEvent {
                timestamp: 80,
                kind: "key_release".to_string(),
                key: Some("a".to_string()),
                x: None,
                y: None,
                button: None,
                direction: None,
            },
        ];
        let result = super::replay(&events, "rec_smoke", 1.0).expect("回放失败");
        assert_eq!(result.event_count, 2);
        assert!(result.elapsed_ms >= 70, "回放应至少排程到 80ms：{}", result.elapsed_ms);
        assert!(
            result.drift_ms.abs() < 60,
            "短序列漂移应远小于 60ms，实际 {}",
            result.drift_ms
        );
    }

    /// 未映射键位必须在合成前报错，避免静默丢事件。
    #[test]
    fn replay_rejects_unmapped_key_before_synthesizing() {
        let events = vec![KbEvent {
            timestamp: 0,
            kind: "key_press".to_string(),
            key: Some("vk1b".to_string()),
            x: None,
            y: None,
            button: None,
            direction: None,
        }];
        let error = super::replay(&events, "rec_smoke", 1.0).unwrap_err();
        assert!(error.contains("无法映射"), "错误应指出键位缺失映射：{error}");
    }
}
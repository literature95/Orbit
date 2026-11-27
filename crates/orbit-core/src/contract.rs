use crate::bindings::{BindingAction, BindingEvent, BindingsConfig, Modifier};
use crate::error::ErrorKind;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;
use ts_rs::TS;

#[derive(Clone, Copy, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum CommandCategory {
    Capture,
    Clipboard,
    Input,
    Ocr,
    Convert,
    Theme,
    System,
    Binding,
    Remote,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum RiskLevel {
    ReadOnly,
    Mutating,
    Destructive,
}

#[derive(Clone, Copy, Debug, Deserialize, JsonSchema, Serialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum CommandStatus {
    Ok,
    Error,
}

#[derive(Clone, Debug, Deserialize, JsonSchema, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[serde(deny_unknown_fields)]
#[ts(export)]
pub struct CommandRequest {
    pub command_id: String,
    #[ts(type = "unknown")]
    pub params: Value,
}

#[derive(Clone, Debug, Deserialize, JsonSchema, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct CommandResponse {
    pub command_id: String,
    pub status: CommandStatus,
    #[ts(type = "unknown | null")]
    pub data: Option<Value>,
    pub error: Option<String>,
    /// 失败时的错误类别；成功时为 null。前端据此区分参数/权限/超时等分支。
    pub error_kind: Option<ErrorKind>,
    #[ts(type = "number")]
    pub duration_ms: u64,
}

#[derive(Clone, Debug, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct CommandSpec {
    pub id: &'static str,
    pub label: &'static str,
    pub category: CommandCategory,
    pub risk: RiskLevel,
    #[ts(type = "unknown")]
    pub params_schema: Value,
    #[ts(type = "unknown")]
    pub returns_schema: Value,
    pub idempotent: bool,
}

#[derive(Clone, Copy, Debug, Deserialize, JsonSchema, Serialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum ScreenshotMode {
    Fullscreen,
    Region,
}

#[derive(Clone, Debug, Deserialize, JsonSchema, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[serde(deny_unknown_fields)]
#[ts(export)]
pub struct ScreenshotRequest {
    pub mode: ScreenshotMode,
    pub monitor_id: Option<u32>,
    pub x: Option<u32>,
    pub y: Option<u32>,
    pub width: Option<u32>,
    pub height: Option<u32>,
}

/// 无参数命令的统一请求体：强制 `{}`，拒绝任何多余字段。
#[derive(Clone, Copy, Debug, Default, Deserialize, JsonSchema, TS)]
#[serde(deny_unknown_fields)]
#[ts(export)]
pub struct EmptyParams {}

/// 悬浮球所在显示器的屏幕与内存概况。
#[derive(Clone, Debug, JsonSchema, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct SystemInfo {
    pub os_name: String,
    pub os_version: String,
    pub screen_width: u32,
    pub screen_height: u32,
    pub scale_factor: f64,
    #[ts(type = "number")]
    pub total_memory_mb: u64,
    #[ts(type = "number")]
    pub available_memory_mb: u64,
}

/// 灵动岛仪表盘实时数据（CPU / 内存 / 开机时长 / 温度 / 剪贴板条数）。
#[derive(Clone, Debug, JsonSchema, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct DashInfo {
    pub cpu_percent: f32,
    pub memory_used_percent: f32,
    #[ts(type = "number")]
    pub memory_used_mb: u64,
    #[ts(type = "number")]
    pub memory_total_mb: u64,
    #[ts(type = "number")]
    pub uptime_secs: u64,
    pub time_now: String,
    pub clipboard_count: usize,
    pub temperature_c: Option<f32>,
}

/// 悬浮球停靠方位：悬空、左侧、右侧、顶部。
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(rename_all = "kebab-case")]
#[ts(export)]
pub enum DockPosition {
    #[default]
    Floating,
    Left,
    Right,
    Top,
}

/// 覆盖层当前的停靠与展开状态。
#[derive(Clone, Copy, Debug, JsonSchema, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct OverlaySnapshot {
    pub dock: DockPosition,
    pub expanded: bool,
}

/// 逻辑像素尺寸，与 tauri.conf.json 的 width / height 单位一致。
#[derive(Clone, Copy, Debug, JsonSchema, PartialEq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct WindowSize {
    pub width: f64,
    pub height: f64,
}

/// 所有窗口与弹窗的尺寸配置：初始尺寸来自 tauri.conf.json，状态尺寸来自主进程。
/// 前端通过 `overlay.layout` 读取后注入 CSS 变量，避免多处硬编码尺寸漂移。
#[derive(Clone, Copy, Debug, JsonSchema, PartialEq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct WindowLayout {
    pub floating: WindowSize,
    pub side: WindowSize,
    pub notch: WindowSize,
    pub expanded: WindowSize,
    pub clipboard: WindowSize,
    pub selection: WindowSize,
    pub panel_inset: f64,
}

/// OCR 文本框 `[x1, y1, x2, y2]`：以图片左上角为原点、单位像素，
/// 与 LiteParse 的 OCR HTTP 契约一致，便于以后接入可替换的 OCR 后端。
#[derive(Clone, Debug, Deserialize, JsonSchema, PartialEq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct OcrWord {
    pub text: String,
    pub bbox: [f64; 4],
}

/// 一行识别结果；bbox 由其 words 的包围盒合并得到。
#[derive(Clone, Debug, Deserialize, JsonSchema, PartialEq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct OcrLine {
    pub text: String,
    pub bbox: [f64; 4],
    pub words: Vec<OcrWord>,
}

/// 结构化 OCR 结果。Windows.Media.Ocr 不提供置信度，因此不伪造该字段；
/// 未来若接入 PaddleOCR 等 HTTP 后端，可按 LiteParse 规范补 confidence。
#[derive(Clone, Debug, Deserialize, JsonSchema, PartialEq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct OcrResult {
    pub text: String,
    pub lines: Vec<OcrLine>,
    pub text_angle: Option<f64>,
}

/// 录屏区域：x/y 为该显示器内的物理像素，monitorId 用于换算虚拟桌面绝对坐标。
/// 与 `RegionFinishParams` 使用同一套选区几何语义。
#[derive(Clone, Copy, Debug, Deserialize, JsonSchema, PartialEq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[serde(deny_unknown_fields)]
#[ts(export)]
pub struct CaptureRegion {
    pub monitor_id: u32,
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}

/// 开始录制参数；region 缺省表示录制整块桌面，output_dir 缺省为「桌面/Orbit」。
/// 暂停采用分段录制：每段独立时间戳，停止时拼接成单个 MP4，使暂停期间不计入时间轴。
#[derive(Clone, Debug, Deserialize, JsonSchema, PartialEq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[serde(deny_unknown_fields)]
#[ts(export)]
pub struct RecordStartParams {
    pub region: Option<CaptureRegion>,
    pub output_dir: Option<String>,
}

/// 录屏状态变化：phase ∈ started / saving / saved / recovering；paused 表示当前处于暂停态。
#[derive(Clone, Debug, Deserialize, JsonSchema, PartialEq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct RecordStateEvent {
    pub phase: String,
    pub path: Option<String>,
    pub message: Option<String>,
    pub paused: bool,
}

/// 录制收尾失败后保留的可恢复文件。
#[derive(Clone, Debug, Deserialize, JsonSchema, PartialEq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct RecoveryItem {
    pub path: String,
    pub reason: String,
    pub created_at: String,
    #[ts(type = "number")]
    pub size_bytes: u64,
}

/// 展开或收起圆盘覆盖层。
#[derive(Clone, Copy, Debug, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[serde(deny_unknown_fields)]
#[ts(export)]
pub struct SetOverlayOpenParams {
    pub open: bool,
}

/// 设置中心球半径（px）：全局统一，折叠态窗口边长随之变为半径 × 2。
#[derive(Clone, Copy, Debug, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[serde(deny_unknown_fields)]
#[ts(export)]
pub struct SetBallRadiusParams {
    pub radius: f64,
}

/// 系统权限状态：当前仅占位，等待平台授权流程接入。
#[derive(Clone, Copy, Debug, Deserialize, JsonSchema, Serialize, TS)]
#[serde(rename_all = "kebab-case")]
#[ts(export)]
pub enum PermissionState {
    Unknown,
}

/// 一键设置结果：权限状态与登录启动是否已生效。
#[derive(Clone, Debug, JsonSchema, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct SetupResult {
    pub permissions: HashMap<String, PermissionState>,
    pub autostart_configured: bool,
}

/// 登录启动开关。
#[derive(Clone, Copy, Debug, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[serde(deny_unknown_fields)]
#[ts(export)]
pub struct SetAutostartParams {
    pub enabled: bool,
}

/// 剪贴板历史条目：文本 / 文件列表 / 图片。
#[derive(Clone, Debug, JsonSchema, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ClipItem {
    #[ts(type = "number")]
    pub id: i64,
    /// text | files | image
    pub kind: String,
    /// 文本内容 / 文件列表 JSON 数组 / 图片 PNG 文件的绝对路径
    pub content: String,
    pub preview: String,
    pub hash: String,
    pub pinned: bool,
    pub favorite: bool,
    #[ts(type = "number")]
    pub use_count: i64,
    #[ts(type = "number")]
    pub created_at: i64,
    #[ts(type = "number")]
    pub updated_at: i64,
}

/// 剪贴板历史查询条件。
#[derive(Clone, Debug, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[serde(deny_unknown_fields)]
#[ts(export)]
pub struct ClipListParams {
    pub query: Option<String>,
    pub filter: Option<String>,
    pub limit: Option<usize>,
}

/// 按 id 定位历史条目。
#[derive(Clone, Copy, Debug, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[serde(deny_unknown_fields)]
#[ts(export)]
pub struct ClipIdParams {
    #[ts(type = "number")]
    pub id: i64,
}

/// 置顶 / 收藏开关。
#[derive(Clone, Copy, Debug, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[serde(deny_unknown_fields)]
#[ts(export)]
pub struct ClipFlagParams {
    #[ts(type = "number")]
    pub id: i64,
    pub value: bool,
}

/// 写回系统剪贴板的纯文本。
#[derive(Clone, Debug, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[serde(deny_unknown_fields)]
#[ts(export)]
pub struct ClipWriteParams {
    pub text: String,
}

/// 剪贴板监听开关。
#[derive(Clone, Copy, Debug, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[serde(deny_unknown_fields)]
#[ts(export)]
pub struct ClipMonitorParams {
    pub enabled: bool,
}

/// 系统监视器快照：总体与每核 CPU、内存、开机时长、温度。
#[derive(Clone, Debug, JsonSchema, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct MonitorSnapshot {
    pub cpu_percent: f32,
    pub per_core: Vec<f32>,
    pub memory_used_percent: f32,
    #[ts(type = "number")]
    pub memory_used_mb: u64,
    #[ts(type = "number")]
    pub memory_total_mb: u64,
    #[ts(type = "number")]
    pub uptime_secs: u64,
    pub temperature_c: Option<f32>,
    pub temperature_enabled: bool,
}

// ============ 事件通道契约 ============

/// 事件契约：id 与载荷 schema 一一对应，后端发射与前端监听共用同一份定义。
#[derive(Clone, Debug, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct EventSpec {
    pub id: &'static str,
    pub label: &'static str,
    #[ts(type = "unknown")]
    pub payload_schema: Value,
}

/// `clip://changed`：剪贴板历史新增或去重后的增量通知。
#[derive(Clone, Debug, JsonSchema, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ClipboardChanged {
    #[ts(type = "number")]
    pub id: i64,
    pub kind: String,
    pub deduplicated: bool,
    pub count: usize,
}

/// `region-selection-start`：选区窗口已按显示器尺寸铺满。
#[derive(Clone, Copy, Debug, JsonSchema, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct RegionSelectionStart {
    pub monitor_id: u32,
}

/// OCR 识别指定图片文件。
#[derive(Clone, Debug, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[serde(deny_unknown_fields)]
#[ts(export)]
pub struct OcrImageParams {
    pub image_path: String,
}

/// 媒体格式转换请求。
#[derive(Clone, Debug, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[serde(deny_unknown_fields)]
#[ts(export)]
pub struct ConvertFormatParams {
    pub input_path: String,
    pub output_path: String,
    pub format: String,
}

/// 转换规划请求：源文件路径 + 目标格式（只读，不执行转换）。
#[derive(Clone, Debug, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[serde(deny_unknown_fields)]
#[ts(export)]
pub struct ConvertPlanParams {
    pub input_path: String,
    pub format: String,
}

/// 路由中的一跳：哪个引擎、从什么格式到什么格式。
#[derive(Clone, Debug, JsonSchema, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ConvertPlanStep {
    pub converter_id: String,
    pub from: String,
    pub to: String,
}

/// 规划结果：由引擎声明的直接边经最短路推导得到。
#[derive(Clone, Debug, JsonSchema, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ConvertPlan {
    pub from: String,
    pub to: String,
    pub steps: Vec<ConvertPlanStep>,
    pub cost: u32,
}

/// 异步转换任务的生命周期状态。
#[derive(Clone, Copy, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum ConvertJobState {
    Queued,
    Running,
    Completed,
    CompletedWithErrors,
    Failed,
    Cancelled,
}

/// 异步转换启动参数。
#[derive(Clone, Debug, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[serde(deny_unknown_fields)]
#[ts(export)]
pub struct ConvertStartParams {
    pub input_path: String,
    pub output_path: String,
    pub format: String,
}

/// 按任务 id 查询或取消。
#[derive(Clone, Debug, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[serde(deny_unknown_fields)]
#[ts(export)]
pub struct ConvertJobParams {
    pub job_id: String,
}

/// 任务快照：`convert.status` 的返回，也是 `convert-state` 事件的载荷。
#[derive(Clone, Debug, JsonSchema, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ConvertJobStatus {
    pub job_id: String,
    pub state: ConvertJobState,
    /// 0..1；按路由跳数推进，单跳任务在完成后直接到 1。
    pub progress: f32,
    pub from: String,
    pub to: String,
    pub output_path: Option<String>,
    pub error: Option<String>,
}

/// 管理员增强开关；开启会触发一次 UAC 并拉起常驻 helper。
#[derive(Clone, Copy, Debug, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[serde(deny_unknown_fields)]
#[ts(export)]
pub struct SetElevatedParams {
    pub enabled: bool,
}

/// 区域截图完成：选区窗口回传的显示器与像素矩形。
///
/// 结果通过命令响应同步返回（内部转调 `screenshot.capture`），
/// 不额外发 `region-selection-end` 事件：只有一个调用方（选区窗口），
/// 响应已携带 data/error，事件反而会引入顺序歧义。拖拽过程由选区窗口
/// 本地状态渲染，不需要 `update` 事件。取消是幂等命令，不产生截图与剪贴板副作用。
#[derive(Clone, Copy, Debug, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[serde(deny_unknown_fields)]
#[ts(export)]
pub struct RegionFinishParams {
    pub monitor_id: u32,
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}

/// 键鼠录制事件：时间戳 + 类型 + 对应键位/坐标/按钮。
#[derive(Clone, Debug, Deserialize, JsonSchema, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct KbEvent {
    #[ts(type = "number")]
    pub timestamp: u64,
    pub kind: String,
    pub key: Option<String>,
    pub x: Option<i32>,
    pub y: Option<i32>,
    pub button: Option<String>,
    pub direction: Option<String>,
}

/// 事件绑定 upsert：同一 (event, modifier) 已存在时覆盖动作。
#[derive(Clone, Debug, Deserialize, JsonSchema, Serialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct UpsertEventBindingParams {
    pub event: BindingEvent,
    #[serde(default)]
    pub modifier: Modifier,
    pub action: BindingAction,
}

/// 事件绑定键：删除与分发共用。
#[derive(Clone, Debug, Deserialize, JsonSchema, Serialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EventBindingKeyParams {
    pub event: BindingEvent,
    #[serde(default)]
    pub modifier: Modifier,
}

/// 全局快捷键 upsert：同一加速键已存在时覆盖动作。
#[derive(Clone, Debug, Deserialize, JsonSchema, Serialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct UpsertShortcutParams {
    pub accelerator: String,
    pub action: BindingAction,
}

/// 删除全局快捷键。
#[derive(Clone, Debug, Deserialize, JsonSchema, Serialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RemoveShortcutParams {
    pub accelerator: String,
}

/// 已保存的键鼠录制（列表项，不含事件体）。
#[derive(Clone, Debug, Deserialize, JsonSchema, Serialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[ts(export)]
pub struct RecordingItem {
    pub id: String,
    pub name: String,
    #[ts(type = "number")]
    pub created_at: i64,
    #[ts(type = "number")]
    pub duration_ms: u64,
    pub platform: String,
    pub event_count: u32,
}

/// 回放参数：录制 id + 可选倍速（0.1 ~ 10，缺省 1.0）。
#[derive(Clone, Debug, Deserialize, JsonSchema, Serialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[ts(export)]
pub struct KbReplayParams {
    pub recording_id: String,
    pub speed: Option<f64>,
}

/// 回放结果：实际耗时与相对录制时间轴的漂移（P-09 指标）。
#[derive(Clone, Debug, Deserialize, JsonSchema, Serialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[ts(export)]
pub struct KbReplayResult {
    pub recording_id: String,
    pub event_count: u32,
    #[ts(type = "number")]
    pub elapsed_ms: u64,
    #[ts(type = "number")]
    pub drift_ms: i64,
}

/// 鼠标控制动作：绝对移动 / 点击 / 滚轮。仅本地会话内合成，不跨进程转发。
#[derive(Clone, Copy, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum MouseAction {
    Move,
    Click,
    Scroll,
}

/// 本地鼠标控制参数：`action` 决定读取哪组字段，无关字段应省略。
/// `button` 取值 Left / Right / Middle / X1 / X2；`count` 为连击次数（缺省 1）；
/// `amount` 为滚轮步数，正值向上。
#[derive(Clone, Debug, Deserialize, JsonSchema, Serialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[ts(export)]
pub struct InputMouseParams {
    pub action: MouseAction,
    pub x: Option<i32>,
    pub y: Option<i32>,
    pub button: Option<String>,
    pub count: Option<u32>,
    pub amount: Option<i32>,
}

/// 键鼠录制状态事件：录制启停时由主进程广播，前端据此同步悬浮球状态。
/// 敏感上下文过滤跳过的按键数以 `redactedCount` 暴露，便于用户知晓。
#[derive(Clone, Debug, JsonSchema, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct KbRecordStateEvent {
    pub active: bool,
    #[ts(type = "number")]
    pub redacted_count: u32,
}

/// 远程只读服务启动参数：端口缺省或 0 时由系统分配回环端口。
/// 服务始终只绑定 127.0.0.1，跨机访问必须经公司 VPN 或 SSH 隧道。
#[derive(Clone, Copy, Debug, Default, Deserialize, JsonSchema, Serialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[ts(export)]
pub struct RemoteStartParams {
    pub port: Option<u16>,
}

/// 远程只读服务端点与一次性配对令牌。
/// 令牌只保存在内存中，服务停止或应用退出即失效。
#[derive(Clone, Debug, JsonSchema, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct RemoteEndpoint {
    pub host: String,
    pub port: u16,
    pub token: String,
}

/// 远程只读服务状态；状态查询不返回令牌。
#[derive(Clone, Debug, JsonSchema, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct RemoteStatus {
    pub running: bool,
    pub endpoint: Option<String>,
    pub active_sessions: u32,
}

/// 控制器连接参数。`endpoint` 只接受回环地址，避免绕过 VPN/隧道直连公网。
#[derive(Clone, Debug, Deserialize, JsonSchema, Serialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[ts(export)]
pub struct RemoteRequestParams {
    pub endpoint: String,
    pub token: String,
    pub monitor_id: Option<u32>,
}

/// 远程截图结果：PNG 已写入本机剪贴板，只返回尺寸。
#[derive(Clone, Copy, Debug, JsonSchema, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct RemoteScreenshotResult {
    pub width: u32,
    pub height: u32,
}

/// 已登记事件目录。新增事件必须同时在此登记，
/// `bus::emit` 在发送前校验 id，避免出现前端监听不到的事件名。
pub fn event_catalog() -> Result<Vec<EventSpec>, serde_json::Error> {
    let changed_payload = serde_json::to_value(schemars::schema_for!(ClipboardChanged))?;
    let selection_payload = serde_json::to_value(schemars::schema_for!(RegionSelectionStart))?;
    let record_state_payload = serde_json::to_value(schemars::schema_for!(RecordStateEvent))?;
    let kb_record_state_payload = serde_json::to_value(schemars::schema_for!(KbRecordStateEvent))?;
    let bindings_changed_payload = serde_json::to_value(schemars::schema_for!(BindingsConfig))?;
    let remote_state_payload = serde_json::to_value(schemars::schema_for!(RemoteStatus))?;
    let convert_status_payload = serde_json::to_value(schemars::schema_for!(ConvertJobStatus))?;
    let null_payload = serde_json::json!({ "type": "null" });
    Ok(vec![
        EventSpec {
            id: "clip://changed",
            label: "剪贴板历史变化",
            payload_schema: changed_payload,
        },
        EventSpec {
            id: "clip://shown",
            label: "剪贴板窗口已显示",
            payload_schema: null_payload,
        },
        EventSpec {
            id: "region-selection-start",
            label: "区域选区开始",
            payload_schema: selection_payload,
        },
        EventSpec {
            id: "record-state",
            label: "录屏状态变化",
            payload_schema: record_state_payload,
        },
        EventSpec {
            id: "kb-record-state",
            label: "键鼠录制状态变化",
            payload_schema: kb_record_state_payload,
        },
        EventSpec {
            id: "binding://changed",
            label: "事件绑定配置变化",
            payload_schema: bindings_changed_payload,
        },
        EventSpec {
            id: "remote://state",
            label: "远程只读服务状态变化",
            payload_schema: remote_state_payload,
        },
        EventSpec {
            id: "convert-state",
            label: "转换任务状态或进度变化",
            payload_schema: convert_status_payload,
        },
    ])
}

pub fn command_catalog() -> Result<Vec<CommandSpec>, serde_json::Error> {
    let screenshot_params = serde_json::to_value(schemars::schema_for!(ScreenshotRequest))?;
    let screenshot_returns = serde_json::to_value(schemars::schema_for!(serde_json::Value))?;
    let empty_params = serde_json::to_value(schemars::schema_for!(EmptyParams))?;
    let system_returns = serde_json::to_value(schemars::schema_for!(SystemInfo))?;
    let dash_returns = serde_json::to_value(schemars::schema_for!(DashInfo))?;
    let monitor_returns = serde_json::to_value(schemars::schema_for!(MonitorSnapshot))?;
    let overlay_returns = serde_json::to_value(schemars::schema_for!(OverlaySnapshot))?;
    let overlay_open_params = serde_json::to_value(schemars::schema_for!(SetOverlayOpenParams))?;
    let ball_radius_params = serde_json::to_value(schemars::schema_for!(SetBallRadiusParams))?;
    let setup_returns = serde_json::to_value(schemars::schema_for!(SetupResult))?;
    let autostart_params = serde_json::to_value(schemars::schema_for!(SetAutostartParams))?;
    let clip_list_params = serde_json::to_value(schemars::schema_for!(ClipListParams))?;
    let clip_list_returns = serde_json::to_value(schemars::schema_for!(Vec<ClipItem>))?;
    let clip_id_params = serde_json::to_value(schemars::schema_for!(ClipIdParams))?;
    let clip_flag_params = serde_json::to_value(schemars::schema_for!(ClipFlagParams))?;
    let clip_write_params = serde_json::to_value(schemars::schema_for!(ClipWriteParams))?;
    let clip_monitor_params = serde_json::to_value(schemars::schema_for!(ClipMonitorParams))?;
    let ocr_image_params = serde_json::to_value(schemars::schema_for!(OcrImageParams))?;
    let convert_format_params = serde_json::to_value(schemars::schema_for!(ConvertFormatParams))?;
    let convert_plan_params = serde_json::to_value(schemars::schema_for!(ConvertPlanParams))?;
    let convert_plan_returns = serde_json::to_value(schemars::schema_for!(ConvertPlan))?;
    let convert_start_params = serde_json::to_value(schemars::schema_for!(ConvertStartParams))?;
    let convert_job_params = serde_json::to_value(schemars::schema_for!(ConvertJobParams))?;
    let convert_status_returns = serde_json::to_value(schemars::schema_for!(ConvertJobStatus))?;
    let region_finish_params = serde_json::to_value(schemars::schema_for!(RegionFinishParams))?;
    let elevated_params = serde_json::to_value(schemars::schema_for!(SetElevatedParams))?;
    let command_response_returns = serde_json::to_value(schemars::schema_for!(CommandResponse))?;
    let kb_events_returns = serde_json::to_value(schemars::schema_for!(Vec<KbEvent>))?;
    let recording_list_returns = serde_json::to_value(schemars::schema_for!(Vec<RecordingItem>))?;
    let kb_replay_params = serde_json::to_value(schemars::schema_for!(KbReplayParams))?;
    let kb_replay_returns = serde_json::to_value(schemars::schema_for!(KbReplayResult))?;
    let input_mouse_params = serde_json::to_value(schemars::schema_for!(InputMouseParams))?;
    let layout_returns = serde_json::to_value(schemars::schema_for!(WindowLayout))?;
    let ocr_result_returns = serde_json::to_value(schemars::schema_for!(OcrResult))?;
    let record_start_params = serde_json::to_value(schemars::schema_for!(RecordStartParams))?;
    let recovery_returns = serde_json::to_value(schemars::schema_for!(Vec<RecoveryItem>))?;
    let bindings_returns = serde_json::to_value(schemars::schema_for!(BindingsConfig))?;
    let upsert_event_params = serde_json::to_value(schemars::schema_for!(UpsertEventBindingParams))?;
    let event_key_params = serde_json::to_value(schemars::schema_for!(EventBindingKeyParams))?;
    let upsert_shortcut_params = serde_json::to_value(schemars::schema_for!(UpsertShortcutParams))?;
    let remove_shortcut_params = serde_json::to_value(schemars::schema_for!(RemoveShortcutParams))?;
    let dispatch_returns = serde_json::to_value(schemars::schema_for!(serde_json::Value))?;
    let remote_start_params = serde_json::to_value(schemars::schema_for!(RemoteStartParams))?;
    let remote_endpoint_returns = serde_json::to_value(schemars::schema_for!(RemoteEndpoint))?;
    let remote_status_returns = serde_json::to_value(schemars::schema_for!(RemoteStatus))?;
    let remote_request_params = serde_json::to_value(schemars::schema_for!(RemoteRequestParams))?;
    let remote_screenshot_returns =
        serde_json::to_value(schemars::schema_for!(RemoteScreenshotResult))?;
    let string_returns = serde_json::json!({ "type": "string" });
    let optional_string_returns = serde_json::json!({ "type": ["string", "null"] });
    let number_returns = serde_json::json!({ "type": "number" });
    let bool_returns = serde_json::json!({ "type": "boolean" });
    let null_returns = serde_json::json!({ "type": "null" });
    let string_array_returns = serde_json::json!({ "type": "array", "items": { "type": "string" } });
    Ok(vec![
        CommandSpec {
            id: "screenshot.capture",
            label: "截图到剪贴板",
            category: CommandCategory::Capture,
            risk: RiskLevel::Mutating,
            params_schema: screenshot_params,
            returns_schema: screenshot_returns,
            idempotent: false,
        },
        CommandSpec {
            id: "system.info",
            label: "屏幕与系统信息",
            category: CommandCategory::System,
            risk: RiskLevel::ReadOnly,
            params_schema: empty_params.clone(),
            returns_schema: system_returns,
            idempotent: true,
        },
        CommandSpec {
            id: "system.dash",
            label: "灵动岛仪表盘数据",
            category: CommandCategory::System,
            risk: RiskLevel::ReadOnly,
            params_schema: empty_params.clone(),
            returns_schema: dash_returns,
            idempotent: true,
        },
        CommandSpec {
            id: "system.monitor",
            label: "系统监视器快照",
            category: CommandCategory::System,
            risk: RiskLevel::ReadOnly,
            params_schema: empty_params.clone(),
            returns_schema: monitor_returns,
            idempotent: true,
        },
        CommandSpec {
            id: "overlay.dock",
            label: "按屏幕边缘停靠覆盖层",
            category: CommandCategory::System,
            risk: RiskLevel::Mutating,
            params_schema: empty_params.clone(),
            returns_schema: overlay_returns,
            idempotent: true,
        },
        CommandSpec {
            id: "overlay.set_open",
            label: "展开或收起覆盖层",
            category: CommandCategory::System,
            risk: RiskLevel::Mutating,
            params_schema: overlay_open_params,
            returns_schema: null_returns.clone(),
            idempotent: true,
        },
        CommandSpec {
            id: "overlay.set_ball_radius",
            label: "设置中心球半径（折叠态窗口随之缩放）",
            category: CommandCategory::System,
            risk: RiskLevel::Mutating,
            params_schema: ball_radius_params,
            returns_schema: null_returns.clone(),
            idempotent: true,
        },
        CommandSpec {
            id: "overlay.layout",
            label: "读取窗口尺寸配置",
            category: CommandCategory::System,
            risk: RiskLevel::ReadOnly,
            params_schema: empty_params.clone(),
            returns_schema: layout_returns,
            idempotent: true,
        },
        CommandSpec {
            id: "app.setup.status",
            label: "读取一键设置状态",
            category: CommandCategory::System,
            risk: RiskLevel::ReadOnly,
            params_schema: empty_params.clone(),
            returns_schema: setup_returns.clone(),
            idempotent: true,
        },
        CommandSpec {
            id: "app.setup.complete",
            label: "完成首次设置",
            category: CommandCategory::System,
            risk: RiskLevel::Mutating,
            params_schema: empty_params.clone(),
            returns_schema: setup_returns.clone(),
            idempotent: true,
        },
        CommandSpec {
            id: "app.autostart.set",
            label: "切换登录启动",
            category: CommandCategory::System,
            risk: RiskLevel::Mutating,
            params_schema: autostart_params,
            returns_schema: setup_returns,
            idempotent: true,
        },
        CommandSpec {
            id: "app.quit",
            label: "退出 Orbit",
            category: CommandCategory::System,
            risk: RiskLevel::Mutating,
            params_schema: empty_params.clone(),
            returns_schema: null_returns.clone(),
            idempotent: false,
        },
        CommandSpec {
            id: "clipboard.read",
            label: "读取系统剪贴板文本",
            category: CommandCategory::Clipboard,
            risk: RiskLevel::ReadOnly,
            params_schema: empty_params.clone(),
            returns_schema: string_returns.clone(),
            idempotent: true,
        },
        CommandSpec {
            id: "clipboard.list",
            label: "查询剪贴板历史",
            category: CommandCategory::Clipboard,
            risk: RiskLevel::ReadOnly,
            params_schema: clip_list_params,
            returns_schema: clip_list_returns,
            idempotent: true,
        },
        CommandSpec {
            id: "clipboard.count",
            label: "统计剪贴板历史条数",
            category: CommandCategory::Clipboard,
            risk: RiskLevel::ReadOnly,
            params_schema: empty_params.clone(),
            returns_schema: number_returns.clone(),
            idempotent: true,
        },
        CommandSpec {
            id: "clipboard.image_preview",
            label: "生成图片缩略图",
            category: CommandCategory::Clipboard,
            risk: RiskLevel::ReadOnly,
            params_schema: clip_id_params.clone(),
            returns_schema: string_returns.clone(),
            idempotent: true,
        },
        CommandSpec {
            id: "clipboard.get_monitor",
            label: "读取剪贴板监听状态",
            category: CommandCategory::Clipboard,
            risk: RiskLevel::ReadOnly,
            params_schema: empty_params.clone(),
            returns_schema: bool_returns.clone(),
            idempotent: true,
        },
        CommandSpec {
            id: "clipboard.shortcut",
            label: "读取剪贴板快捷键",
            category: CommandCategory::Clipboard,
            risk: RiskLevel::ReadOnly,
            params_schema: empty_params.clone(),
            returns_schema: string_returns.clone(),
            idempotent: true,
        },
        CommandSpec {
            id: "clipboard.write",
            label: "写入系统剪贴板文本",
            category: CommandCategory::Clipboard,
            risk: RiskLevel::Mutating,
            params_schema: clip_write_params,
            returns_schema: null_returns.clone(),
            idempotent: false,
        },
        CommandSpec {
            id: "clipboard.write_back",
            label: "写回历史条目",
            category: CommandCategory::Clipboard,
            risk: RiskLevel::Mutating,
            params_schema: clip_id_params.clone(),
            returns_schema: null_returns.clone(),
            idempotent: false,
        },
        CommandSpec {
            id: "clipboard.set_pin",
            label: "置顶剪贴板条目",
            category: CommandCategory::Clipboard,
            risk: RiskLevel::Mutating,
            params_schema: clip_flag_params.clone(),
            returns_schema: null_returns.clone(),
            idempotent: true,
        },
        CommandSpec {
            id: "clipboard.set_favorite",
            label: "收藏剪贴板条目",
            category: CommandCategory::Clipboard,
            risk: RiskLevel::Mutating,
            params_schema: clip_flag_params,
            returns_schema: null_returns.clone(),
            idempotent: true,
        },
        CommandSpec {
            id: "clipboard.set_monitor",
            label: "切换剪贴板监听",
            category: CommandCategory::Clipboard,
            risk: RiskLevel::Mutating,
            params_schema: clip_monitor_params,
            returns_schema: bool_returns.clone(),
            idempotent: true,
        },
        CommandSpec {
            id: "clipboard.delete",
            label: "删除剪贴板条目",
            category: CommandCategory::Clipboard,
            risk: RiskLevel::Destructive,
            params_schema: clip_id_params.clone(),
            returns_schema: null_returns.clone(),
            idempotent: false,
        },
        CommandSpec {
            id: "clipboard.clear",
            label: "清空剪贴板历史",
            category: CommandCategory::Clipboard,
            risk: RiskLevel::Destructive,
            params_schema: empty_params.clone(),
            returns_schema: null_returns.clone(),
            idempotent: false,
        },
        CommandSpec {
            id: "clipboard.open_window",
            label: "打开剪贴板窗口",
            category: CommandCategory::Clipboard,
            risk: RiskLevel::Mutating,
            params_schema: empty_params.clone(),
            returns_schema: null_returns.clone(),
            idempotent: true,
        },
        CommandSpec {
            id: "clipboard.close_window",
            label: "关闭剪贴板窗口",
            category: CommandCategory::Clipboard,
            risk: RiskLevel::Mutating,
            params_schema: empty_params.clone(),
            returns_schema: null_returns.clone(),
            idempotent: true,
        },
        CommandSpec {
            id: "record.start",
            label: "开始屏幕录制",
            category: CommandCategory::Capture,
            risk: RiskLevel::Mutating,
            params_schema: record_start_params,
            returns_schema: string_returns.clone(),
            idempotent: false,
        },
        CommandSpec {
            id: "record.pause",
            label: "暂停屏幕录制",
            category: CommandCategory::Capture,
            risk: RiskLevel::Mutating,
            params_schema: empty_params.clone(),
            returns_schema: null_returns.clone(),
            idempotent: false,
        },
        CommandSpec {
            id: "record.resume",
            label: "继续屏幕录制",
            category: CommandCategory::Capture,
            risk: RiskLevel::Mutating,
            params_schema: empty_params.clone(),
            returns_schema: null_returns.clone(),
            idempotent: false,
        },
        CommandSpec {
            id: "record.stop",
            label: "停止屏幕录制",
            category: CommandCategory::Capture,
            risk: RiskLevel::Mutating,
            params_schema: empty_params.clone(),
            returns_schema: optional_string_returns,
            idempotent: false,
        },
        CommandSpec {
            id: "record.recovery.list",
            label: "列出可恢复的录屏文件",
            category: CommandCategory::Capture,
            risk: RiskLevel::ReadOnly,
            params_schema: empty_params.clone(),
            returns_schema: recovery_returns,
            idempotent: true,
        },
        CommandSpec {
            id: "ocr.image",
            label: "识别图片文字",
            category: CommandCategory::Ocr,
            risk: RiskLevel::ReadOnly,
            params_schema: ocr_image_params,
            returns_schema: ocr_result_returns.clone(),
            idempotent: true,
        },
        CommandSpec {
            id: "ocr.current_screen",
            label: "识别当前屏幕文字",
            category: CommandCategory::Ocr,
            risk: RiskLevel::ReadOnly,
            params_schema: empty_params.clone(),
            returns_schema: ocr_result_returns.clone(),
            idempotent: true,
        },
        CommandSpec {
            id: "kb_record.start",
            label: "开始键鼠录制",
            category: CommandCategory::Input,
            risk: RiskLevel::Mutating,
            params_schema: empty_params.clone(),
            returns_schema: null_returns.clone(),
            idempotent: false,
        },
        CommandSpec {
            id: "kb_record.stop",
            label: "停止键鼠录制",
            category: CommandCategory::Input,
            risk: RiskLevel::Mutating,
            params_schema: empty_params.clone(),
            returns_schema: kb_events_returns,
            idempotent: false,
        },
        CommandSpec {
            id: "kb_record.list",
            label: "列出已保存的键鼠录制",
            category: CommandCategory::Input,
            risk: RiskLevel::ReadOnly,
            params_schema: empty_params.clone(),
            returns_schema: recording_list_returns,
            idempotent: true,
        },
        CommandSpec {
            id: "kb.replay",
            label: "回放键鼠录制",
            category: CommandCategory::Input,
            risk: RiskLevel::Mutating,
            params_schema: kb_replay_params,
            returns_schema: kb_replay_returns,
            idempotent: false,
        },
        CommandSpec {
            id: "input.mouse",
            label: "鼠标控制",
            category: CommandCategory::Input,
            risk: RiskLevel::Mutating,
            params_schema: input_mouse_params,
            returns_schema: null_returns.clone(),
            idempotent: false,
        },
        CommandSpec {
            id: "convert.format",
            label: "转换媒体格式",
            category: CommandCategory::Convert,
            risk: RiskLevel::Mutating,
            params_schema: convert_format_params,
            returns_schema: string_returns,
            idempotent: false,
        },
        CommandSpec {
            id: "convert.formats",
            label: "列出可转换的目标格式",
            category: CommandCategory::Convert,
            risk: RiskLevel::ReadOnly,
            params_schema: empty_params.clone(),
            returns_schema: string_array_returns,
            idempotent: true,
        },
        CommandSpec {
            id: "convert.plan",
            label: "规划转换路由（不执行）",
            category: CommandCategory::Convert,
            risk: RiskLevel::ReadOnly,
            params_schema: convert_plan_params,
            returns_schema: convert_plan_returns,
            idempotent: true,
        },
        CommandSpec {
            id: "convert.start",
            label: "异步启动转换任务",
            category: CommandCategory::Convert,
            risk: RiskLevel::Mutating,
            params_schema: convert_start_params,
            returns_schema: convert_status_returns.clone(),
            idempotent: false,
        },
        CommandSpec {
            id: "convert.cancel",
            label: "取消转换任务",
            category: CommandCategory::Convert,
            risk: RiskLevel::Mutating,
            params_schema: convert_job_params.clone(),
            returns_schema: null_returns.clone(),
            idempotent: true,
        },
        CommandSpec {
            id: "convert.status",
            label: "查询转换任务状态",
            category: CommandCategory::Convert,
            risk: RiskLevel::ReadOnly,
            params_schema: convert_job_params,
            returns_schema: convert_status_returns,
            idempotent: true,
        },
        CommandSpec {
            id: "region.start",
            label: "开始区域选区",
            category: CommandCategory::Capture,
            risk: RiskLevel::Mutating,
            params_schema: empty_params.clone(),
            returns_schema: null_returns.clone(),
            idempotent: true,
        },
        CommandSpec {
            id: "region.finish",
            label: "完成区域截图",
            category: CommandCategory::Capture,
            risk: RiskLevel::Mutating,
            params_schema: region_finish_params,
            returns_schema: command_response_returns,
            idempotent: false,
        },
        CommandSpec {
            id: "region.cancel",
            label: "取消区域选区",
            category: CommandCategory::Capture,
            risk: RiskLevel::Mutating,
            params_schema: empty_params.clone(),
            returns_schema: null_returns.clone(),
            idempotent: true,
        },
        CommandSpec {
            id: "elevated.set",
            label: "切换管理员增强",
            category: CommandCategory::System,
            risk: RiskLevel::Mutating,
            params_schema: elevated_params,
            returns_schema: bool_returns.clone(),
            idempotent: true,
        },
        CommandSpec {
            id: "elevated.status",
            label: "读取管理员增强状态",
            category: CommandCategory::System,
            risk: RiskLevel::ReadOnly,
            params_schema: empty_params.clone(),
            returns_schema: bool_returns.clone(),
            idempotent: true,
        },
        CommandSpec {
            id: "binding.list",
            label: "读取事件绑定配置",
            category: CommandCategory::Binding,
            risk: RiskLevel::ReadOnly,
            params_schema: empty_params.clone(),
            returns_schema: bindings_returns.clone(),
            idempotent: true,
        },
        CommandSpec {
            id: "binding.dispatch",
            label: "按事件绑定执行动作",
            category: CommandCategory::Binding,
            risk: RiskLevel::Mutating,
            params_schema: event_key_params.clone(),
            returns_schema: dispatch_returns,
            idempotent: false,
        },
        CommandSpec {
            id: "binding.upsert_event",
            label: "写入事件绑定",
            category: CommandCategory::Binding,
            risk: RiskLevel::Mutating,
            params_schema: upsert_event_params,
            returns_schema: bindings_returns.clone(),
            idempotent: false,
        },
        CommandSpec {
            id: "binding.remove_event",
            label: "删除事件绑定",
            category: CommandCategory::Binding,
            risk: RiskLevel::Mutating,
            params_schema: event_key_params,
            returns_schema: bindings_returns.clone(),
            idempotent: false,
        },
        CommandSpec {
            id: "binding.upsert_shortcut",
            label: "写入全局快捷键",
            category: CommandCategory::Binding,
            risk: RiskLevel::Mutating,
            params_schema: upsert_shortcut_params,
            returns_schema: bindings_returns.clone(),
            idempotent: false,
        },
        CommandSpec {
            id: "binding.remove_shortcut",
            label: "删除全局快捷键",
            category: CommandCategory::Binding,
            risk: RiskLevel::Mutating,
            params_schema: remove_shortcut_params,
            returns_schema: bindings_returns,
            idempotent: false,
        },
        CommandSpec {
            id: "remote.serve.start",
            label: "启动远程只读服务",
            category: CommandCategory::Remote,
            risk: RiskLevel::Mutating,
            params_schema: remote_start_params,
            returns_schema: remote_endpoint_returns,
            idempotent: false,
        },
        CommandSpec {
            id: "remote.serve.stop",
            label: "停止远程只读服务",
            category: CommandCategory::Remote,
            risk: RiskLevel::Mutating,
            params_schema: empty_params.clone(),
            returns_schema: bool_returns.clone(),
            idempotent: true,
        },
        CommandSpec {
            id: "remote.status",
            label: "读取远程只读服务状态",
            category: CommandCategory::Remote,
            risk: RiskLevel::ReadOnly,
            params_schema: empty_params.clone(),
            returns_schema: remote_status_returns,
            idempotent: true,
        },
        CommandSpec {
            id: "remote.screenshot",
            label: "抓取远程截图到本机剪贴板",
            category: CommandCategory::Remote,
            risk: RiskLevel::Mutating,
            params_schema: remote_request_params.clone(),
            returns_schema: remote_screenshot_returns,
            idempotent: false,
        },
        CommandSpec {
            id: "remote.ocr",
            label: "远程 OCR 识别",
            category: CommandCategory::Remote,
            // 刻意标记为 Mutating：避免只读 MCP 白名单自动暴露远程屏幕读取能力。
            risk: RiskLevel::Mutating,
            params_schema: remote_request_params,
            returns_schema: ocr_result_returns,
            idempotent: false,
        },
    ])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catalog_covers_only_registered_commands() {
        let catalog = command_catalog().unwrap();
        let ids: Vec<&str> = catalog.iter().map(|spec| spec.id).collect();
        assert_eq!(
            ids,
            [
                "screenshot.capture",
                "system.info",
                "system.dash",
                "system.monitor",
                "overlay.dock",
                "overlay.set_open",
                "overlay.set_ball_radius",
                "overlay.layout",
                "app.setup.status",
                "app.setup.complete",
                "app.autostart.set",
                "app.quit",
                "clipboard.read",
                "clipboard.list",
                "clipboard.count",
                "clipboard.image_preview",
                "clipboard.get_monitor",
                "clipboard.shortcut",
                "clipboard.write",
                "clipboard.write_back",
                "clipboard.set_pin",
                "clipboard.set_favorite",
                "clipboard.set_monitor",
                "clipboard.delete",
                "clipboard.clear",
                "clipboard.open_window",
                "clipboard.close_window",
                "record.start",
                "record.pause",
                "record.resume",
                "record.stop",
                "record.recovery.list",
                "ocr.image",
                "ocr.current_screen",
                "kb_record.start",
                "kb_record.stop",
                "kb_record.list",
                "kb.replay",
                "input.mouse",
                "convert.format",
                "convert.formats",
                "convert.plan",
                "convert.start",
                "convert.cancel",
                "convert.status",
                "region.start",
                "region.finish",
                "region.cancel",
                "elevated.set",
                "elevated.status",
                "binding.list",
                "binding.dispatch",
                "binding.upsert_event",
                "binding.remove_event",
                "binding.upsert_shortcut",
                "binding.remove_shortcut",
                "remote.serve.start",
                "remote.serve.stop",
                "remote.status",
                "remote.screenshot",
                "remote.ocr",
            ]
        );
        let screenshot = &catalog[0];
        assert_eq!(screenshot.risk, RiskLevel::Mutating);
        let system_info = &catalog[1];
        assert_eq!(system_info.risk, RiskLevel::ReadOnly);
        assert_eq!(system_info.category, CommandCategory::System);
    }

    #[test]
    fn record_start_params_accept_default_and_region() {
        let fullscreen: RecordStartParams = serde_json::from_value(serde_json::json!({}))
            .expect("整屏录制应允许缺省 region");
        assert_eq!(fullscreen.region, None);
        let regioned: RecordStartParams = serde_json::from_value(serde_json::json!({
            "region": { "monitorId": 0, "x": 10, "y": 20, "width": 320, "height": 240 }
        }))
        .expect("区域录制参数应可反序列化");
        assert_eq!(regioned.region.map(|region| region.width), Some(320));
        assert!(
            serde_json::from_value::<RecordStartParams>(serde_json::json!({ "fullscreen": true }))
                .is_err(),
            "多余字段必须被拒绝"
        );
    }

    #[test]
    fn recording_contracts_cover_region_events_and_recovery() {
        let catalog = command_catalog().unwrap();
        assert_eq!(
            catalog.len(),
            61,
            "命令目录应包含绑定层 6 条、回放 2 条、远程只读 5 条、鼠标控制 1 条、球半径 1 条与转换 5 条命令"
        );
        let start = catalog
            .iter()
            .find(|spec| spec.id == "record.start")
            .expect("命令目录缺少 record.start");
        let properties = start
            .params_schema
            .as_object()
            .and_then(|schema| schema.get("properties"))
            .and_then(|value| value.as_object())
            .expect("record.start 参数 schema 应为对象");
        assert!(
            properties.contains_key("region"),
            "record.start 缺少可选区域参数"
        );
        assert!(catalog.iter().any(|spec| spec.id == "record.recovery.list"));
        let events = event_catalog().unwrap();
        assert!(
            events.iter().any(|spec| spec.id == "record-state"),
            "事件目录缺少录屏状态事件"
        );
    }

    #[test]
    fn ocr_commands_return_structured_result() {
        let catalog = command_catalog().unwrap();
        for id in ["ocr.image", "ocr.current_screen"] {
            let spec = catalog
                .iter()
                .find(|spec| spec.id == id)
                .expect("命令目录缺少 OCR 命令");
            let schema = spec
                .returns_schema
                .as_object()
                .expect("OCR 返回 schema 应为对象");
            assert_eq!(
                schema.get("type").and_then(|value| value.as_str()),
                Some("object"),
                "{id} 仍声明返回纯字符串"
            );
            let properties = schema
                .get("properties")
                .and_then(|value| value.as_object())
                .unwrap();
            assert!(properties.contains_key("text"), "{id} 缺少 text 字段");
            assert!(properties.contains_key("lines"), "{id} 缺少 lines 字段");
        }
    }

    #[test]
    fn layout_catalog_entry_is_read_only_and_idempotent() {
        let catalog = command_catalog().unwrap();
        let layout = catalog
            .iter()
            .find(|spec| spec.id == "overlay.layout")
            .expect("命令目录缺少 overlay.layout");
        assert_eq!(layout.risk, RiskLevel::ReadOnly);
        assert_eq!(layout.category, CommandCategory::System);
        assert!(layout.idempotent);
        assert!(layout.returns_schema.is_object());
    }

    #[test]
    fn event_catalog_has_no_unconsumed_push_event() {
        let ids: Vec<&str> = event_catalog().unwrap().iter().map(|spec| spec.id).collect();
        assert!(
            !ids.contains(&"system-metrics"),
            "前端无消费者的指标推送事件不应继续常驻采样：{ids:?}"
        );
    }

    #[test]
    fn event_catalog_covers_all_emitted_events() {
        let catalog = event_catalog().unwrap();
        let ids: Vec<&str> = catalog.iter().map(|spec| spec.id).collect();
        assert_eq!(
            ids,
            [
                "clip://changed",
                "clip://shown",
                "region-selection-start",
                "record-state",
                "kb-record-state",
                "binding://changed",
                "remote://state",
            ]
        );
        assert!(catalog.iter().all(|spec| spec.payload_schema.is_object()));
    }

    #[test]
    fn input_mouse_catalog_entry_is_mutating_input() {
        let catalog = command_catalog().unwrap();
        let spec = catalog
            .iter()
            .find(|spec| spec.id == "input.mouse")
            .expect("命令目录缺少 input.mouse");
        assert_eq!(spec.label, "鼠标控制");
        assert_eq!(spec.category, CommandCategory::Input);
        assert_eq!(spec.risk, RiskLevel::Mutating);
        assert!(!spec.idempotent);

        let props = spec.params_schema["properties"]
            .as_object()
            .expect("input.mouse 参数 schema 缺少 properties");
        assert!(props.contains_key("action"), "缺少 action 字段");
        assert!(props.contains_key("button"), "缺少 button 字段");
    }

    #[test]
    fn input_mouse_params_reject_unknown_fields() {
        let request = serde_json::from_value::<InputMouseParams>(serde_json::json!({
            "action": "move",
            "x": 100,
            "y": 200,
            "unexpected": true
        }));
        assert!(request.is_err());
    }

    #[test]
    fn input_mouse_params_accept_move_click_scroll() {
        let move_req = serde_json::from_value::<InputMouseParams>(serde_json::json!({
            "action": "move", "x": 100, "y": 200
        }))
        .expect("move 应可反序列化");
        assert_eq!(move_req.action, MouseAction::Move);
        assert_eq!(move_req.x, Some(100));

        let click_req = serde_json::from_value::<InputMouseParams>(serde_json::json!({
            "action": "click", "button": "Left", "count": 2
        }))
        .expect("click 应可反序列化");
        assert_eq!(click_req.action, MouseAction::Click);
        assert_eq!(click_req.count, Some(2));

        let scroll_req = serde_json::from_value::<InputMouseParams>(serde_json::json!({
            "action": "scroll", "amount": 3
        }))
        .expect("scroll 应可反序列化");
        assert_eq!(scroll_req.action, MouseAction::Scroll);
        assert_eq!(scroll_req.amount, Some(3));
    }

    #[test]
    fn screenshot_request_rejects_unknown_fields() {
        let request = serde_json::from_value::<ScreenshotRequest>(serde_json::json!({
            "mode": "fullscreen",
            "outputPath": "unexpected.png"
        }));
        assert!(request.is_err());
    }

    #[test]
    fn non_empty_params_reject_unknown_fields() {
        let overlay = serde_json::from_value::<SetOverlayOpenParams>(serde_json::json!({
            "open": true,
            "expanded": true
        }));
        assert!(overlay.is_err());
        let autostart = serde_json::from_value::<SetAutostartParams>(serde_json::json!({
            "enabled": true,
            "startup": true
        }));
        assert!(autostart.is_err());
    }

    /// 绑定参数的 TS 绑定需要显式导出（自动导出只覆盖叶子类型）。
    #[test]
    fn exports_binding_param_typescript_bindings() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("bindings");
        let cfg = ts_rs::Config::new().with_out_dir(&dir);
        UpsertEventBindingParams::export_all(&cfg).expect("导出 UpsertEventBindingParams 失败");
        EventBindingKeyParams::export_all(&cfg).expect("导出 EventBindingKeyParams 失败");
        UpsertShortcutParams::export_all(&cfg).expect("导出 UpsertShortcutParams 失败");
        RemoveShortcutParams::export_all(&cfg).expect("导出 RemoveShortcutParams 失败");
        RecordingItem::export_all(&cfg).expect("导出 RecordingItem 失败");
        KbReplayParams::export_all(&cfg).expect("导出 KbReplayParams 失败");
        KbReplayResult::export_all(&cfg).expect("导出 KbReplayResult 失败");
        MouseAction::export_all(&cfg).expect("导出 MouseAction 失败");
        InputMouseParams::export_all(&cfg).expect("导出 InputMouseParams 失败");
        KbRecordStateEvent::export_all(&cfg).expect("导出 KbRecordStateEvent 失败");
        ConvertPlanParams::export_all(&cfg).expect("导出 ConvertPlanParams 失败");
        ConvertPlan::export_all(&cfg).expect("导出 ConvertPlan 失败");
        ConvertStartParams::export_all(&cfg).expect("导出 ConvertStartParams 失败");
        ConvertJobParams::export_all(&cfg).expect("导出 ConvertJobParams 失败");
        ConvertJobStatus::export_all(&cfg).expect("导出 ConvertJobStatus 失败");
    }
}

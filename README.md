# Orbit
Orbit 桌面悬浮球项目计划书（完整版 v2.0）
项目代号 Orbit · 中文名 轨道 · 技术栈 Tauri 2 + Rust 原生管线 · 定位 速度优先、可编程、可换肤的常驻自动化中枢

一、项目概述
Orbit 是一个以悬浮球为唯一常驻入口、以轮盘和事件绑定为交互壳、以命令总线为执行内核、以 MCP 为外部扩展面的桌面自动化工具。悬浮球是中心天体，轮盘是径向轨道，命令是环绕运行的卫星，MCP 是外部轨道接口——四层含义统一在一个名字里。

产品的核心承诺只有一条：点击到响应的体感延迟不可感知，开机到可用的沉默感不可打破。功能可以迭代增加，但如果悬浮球在长时间常驻后出现卡顿、轮盘弹出有肉眼可见的延迟、键鼠回放漂移，产品就失去了存在理由。

1.1 目标用户
用户类型	核心诉求	对应能力
效率工具重度用户	一键完成截图/OCR/剪贴板/翻译	轮盘 + 事件绑定
自动化爱好者	录制键鼠操作、串成宏、绑定到任意事件	宏录制 + binding
开发者 / AI 工作流用户	让大模型调用桌面能力	MCP 只读工具集
桌面美化用户	换主题、换皮肤、换交互手感	主题包系统
1.2 不做什么（明确边界）
不做云端服务：所有能力本地执行，OCR、录屏、录制数据不出机器。MCP 只绑定 127.0.0.1。

不做全平台键鼠支持承诺：Linux 明确限定 X11 / XWayland，纯 Wayland 下录制降级或不可用。

不做无确认的高风险远程调用：mouse_click、keyboard_type、replay_macro 默认不暴露给 MCP。

不追求三平台录屏同时齐活：按平台逐个落地，先做目标用户最集中的平台。

二、硬性指标与验收基线
以下指标是项目的第一验收标准。任何后续功能如果导致任一项超标，必须降级或重新设计实现路径，而不是调整指标：

编号	指标	目标值	测量条件	验收阶段
P-01	冷启动时间	< 500 ms	进程启动到悬浮球可点击	M0
P-02	空闲内存占用	< 60 MB（整进程族）	悬浮球静止、轮盘关闭、无录屏/OCR	M0
P-03	轮盘弹出延迟	< 16 ms	点击悬浮球到轮盘完全绘制	M1
P-04	截图 → 剪贴板就绪	< 150 ms	全屏捕获、编码、写入剪贴板	M1
P-05	全局键鼠录制抖动	< 5 ms	高频鼠标移动事件时间戳偏差	M0
P-06	MCP 工具调用开销	< 50 ms（不含执行）	IPC 往返	M2
P-07	绑定/快捷键变更生效	即时，无需重启	修改后下一次触发即按新绑定执行	M1
P-08	主题切换首帧延迟	< 16 ms	CSS 变量更新后首次渲染	M3
P-09	键鼠回放漂移	< 20 ms / 10s	30 秒录制回放的累计时间偏差	M2
P-10	长时录制内存增长	< 50 MB / 30min	30 分钟录屏期间 RSS 增量	M3
三、技术选型与架构
3.1 选型决策
Tauri 2 + Rust 原生管线。基准数据：空闲内存 30–50 MB、冷启动 < 0.5 秒，而 Electron 对应 200–300 MB 和 1–2 秒。有迁移案例显示托盘类应用从 Electron 迁到 Tauri 后，空闲内存从 185 MB 降至 28.5 MB，冷启动从 3.2 秒压到 0.5 秒以内。

两个必须提前接受的硬约束：

录屏不走 WebView 媒体栈。 macOS 的 WKWebView 不支持 getDisplayMedia，Windows 的 WebView2 也拿不到系统级区域捕获，且 macOS 的 App Nap 会节流后台 JavaScript，导致快捷键触发的录制初始化被静默排队。录屏必须作为独立的 Rust 原生采集管线存在。

MCP 不嵌入主进程。 MCP 工具调用如果跑在 UI 进程的事件循环里，大模型的一次批量调用就可能阻塞悬浮球的点击响应。MCP Server 作为独立 sidecar 运行，通过本地 IPC 转发到命令总线。

3.2 五层架构
text
┌─────────────────────────────────────────────────────────────┐
│  L1 入口层：悬浮球 / 双模式轮盘 / 全局快捷键 / 托盘菜单        │
│     （Tauri WebView，薄 UI，不承载重型逻辑）                  │
└────────────────────────┬────────────────────────────────────┘
                         │ 事件命中
┌────────────────────────▼────────────────────────────────────┐
│  L2 事件绑定层：event + modifier → { command | macro | menu } │
│     bindings.json 持久化（Tauri Store）                       │
└────────────────────────┬────────────────────────────────────┘
                         │ execute(commandId, params, source)
┌────────────────────────▼────────────────────────────────────┐
│  L3 命令总线：统一注册、路由、权限、审计日志、风险分级          │
│     命令契约层：Rust 类型 → 前端 TS + MCP schema 单一生成源    │
└───────┬─────────────────────────────────┬───────────────────┘
        │                                 │
┌───────▼──────────────┐      ┌───────────▼───────────────────┐
│  L4a 快路径（主进程）  │      │  L4b 重路径（独立管线/子进程）  │
│  截图 │ OCR │ 剪贴板   │      │  录屏（原生采集）              │
│  键鼠监听 │ 窗口查询  │      │  格式转换（FFmpeg 子进程）     │
└──────────────────────┘      └───────────────────────────────┘
                         │
┌────────────────────────▼────────────────────────────────────┐
│  L5 MCP 适配层（独立 sidecar 进程）                           │
│     stdio JSON-RPC → 工具定义 → IPC → 命令总线                │
│     风险分级暴露：只读默认开，破坏性需 confirm                │
└─────────────────────────────────────────────────────────────┘
快路径与重路径分离是性能设计的核心。 毫秒级操作留在主进程内走轻量异步；耗时或需要独立权限的操作放到子进程，崩溃不拖垮主进程，也不占用 UI 事件循环。

四、工程目录结构
text
orbit/
├── Cargo.toml                      # workspace 根
├── crates/
│   ├── orbit-core/                 # 命令总线、契约层、审计日志
│   │   ├── src/
│   │   │   ├── registry.rs         # 命令注册与路由
│   │   │   ├── contract.rs         # CommandSpec / RiskLevel
│   │   │   ├── executor.rs         # execute() 统一入口
│   │   │   └── audit.rs            # 审计日志（SQLite）
│   ├── orbit-capabilities/         # 快路径能力
│   │   ├── src/
│   │   │   ├── screenshot.rs       # xcap 封装
│   │   │   ├── ocr.rs              # 系统原生 OCR
│   │   │   ├── clipboard.rs        # 剪贴板读写 + 历史
│   │   │   └── windows.rs          # 窗口枚举
│   ├── orbit-input/                # 键鼠监听与回放
│   │   ├── src/
│   │   │   ├── recorder.rs         # rdev listen（独立线程）
│   │   │   ├── player.rs           # enigo 回放
│   │   │   └── format.rs           # 录制脚本数据格式
│   ├── orbit-capture/              # 录屏原生管线
│   │   ├── src/
│   │   │   ├── macos.rs            # ScreenCaptureKit
│   │   │   ├── windows.rs          # Windows.Graphics.Capture
│   │   │   ├── linux.rs            # PipeWire/portal
│   │   │   └── writer.rs           # 渐进式写盘
│   ├── orbit-mcp/                  # MCP sidecar 二进制
│   │   ├── src/
│   │   │   ├── main.rs             # stdio JSON-RPC 入口
│   │   │   ├── tools.rs            # 工具定义（从契约生成）
│   │   │   └── ipc.rs              # 与主进程通信
│   └── orbit-ipc/                  # 进程间通信协议
│       └── src/lib.rs
├── src-tauri/                      # Tauri 主进程
│   ├── src/
│   │   ├── main.rs
│   │   ├── window.rs               # 悬浮窗创建、DWM 禁用
│   │   ├── shortcuts.rs            # 全局快捷键注册/注销
│   │   ├── bindings.rs             # binding 加载与热更新
│   │   └── theme.rs                # 主题管理
│   ├── tauri.conf.json
│   └── capabilities/
├── src/                            # 前端（建议 Svelte 或 Solid，体积小）
│   ├── components/
│   │   ├── Orb.svelte              # 悬浮球
│   │   ├── RadialMenu.svelte       # 轮盘
│   │   ├── BindingEditor.svelte    # 绑定编辑器
│   │   └── Settings.svelte
│   ├── stores/
│   └── styles/
│       └── themes/                 # 预置主题
├── bindings.json                   # Tauri Store 持久化
├── scripts/
│   ├── perf-baseline/              # 性能基准脚本
│   └── build-sidecar/              # MCP sidecar 打包
└── docs/
    ├── SAFETY.md                   # MCP 安全边界
    ├── CONTRACTS.md                # 命令契约文档
    └── PERF.md                     # 性能基线报告
为什么拆成 workspace crates：orbit-capture 和 orbit-mcp 需要独立编译为 sidecar 二进制；orbit-core 作为被依赖的库，编译产物可被主进程和 MCP sidecar 共享；前端类型通过 specta 或 ts-rs 从 orbit-core 的契约类型自动生成，保证单一事实源。

五、核心数据结构与契约
5.1 命令契约（orbit-core::contract）
rust
#[derive(Serialize, Deserialize, Clone)]
pub struct CommandSpec {
    pub id: &'static str,          // "screenshot.region"
    pub label: &'static str,       // "区域截图"
    pub category: Category,        // Capture | Ocr | Clipboard | Input | Convert
    pub params: JsonSchema,        // 参数 schema
    pub returns: JsonSchema,       // 返回 schema
    pub risk: RiskLevel,           // ReadOnly | Mutating | Destructive
    pub idempotent: bool,
    pub requires_confirm: bool,    // MCP 调用时是否强制 confirm: true
}

#[derive(Serialize, Deserialize, Clone, Copy)]
pub enum RiskLevel { ReadOnly, Mutating, Destructive }
单一事实源原则：CommandSpec 的 Rust 类型是唯一的契约定义。通过 specta 生成前端 TypeScript 类型，通过 schemars 生成 JSON Schema 供 MCP 工具定义使用。手写三份契约是禁止的——这会导致同一个 screenshot 命令在本地轮盘能跑、MCP 调用时因参数格式漂移而失败。

5.2 事件绑定（bindings.json）
json
{
  "version": 1,
  "bindings": [
    {
      "event": "left-click",
      "modifier": "none",
      "action": { "type": "menu", "menu": "radial-main" }
    },
    {
      "event": "left-click",
      "modifier": "ctrl",
      "action": { "type": "command", "id": "screenshot.region" }
    },
    {
      "event": "middle-click",
      "modifier": "none",
      "action": { "type": "menu", "menu": "radial-cursor", "anchor": "cursor" }
    },
    {
      "event": "drop",
      "modifier": "none",
      "action": { "type": "command", "id": "convert.auto", "args": { "source": "$payload" } }
    },
    {
      "event": "wheel-up",
      "modifier": "alt",
      "action": { "type": "command", "id": "theme.next" }
    }
  ],
  "shortcuts": [
    {
      "accelerator": "CmdOrCtrl+Shift+Space",
      "action": { "type": "menu", "menu": "radial-cursor", "anchor": "cursor" }
    }
  ]
}
事件槽位枚举：left-click、left-double-click、middle-click、right-click、wheel-up、wheel-down、drop、hover-enter、hover-leave。

修饰键枚举：none、ctrl、alt、shift、ctrl+alt、ctrl+shift、alt+shift、ctrl+alt+shift。

5.3 录制脚本格式（SQLite 表）
sql
CREATE TABLE recordings (
  id          TEXT PRIMARY KEY,      -- UUID
  name        TEXT NOT NULL,
  created_at  INTEGER NOT NULL,      -- Unix ms
  duration_ms INTEGER NOT NULL,
  platform    TEXT NOT NULL,         -- macos | windows | linux
  events      BLOB NOT NULL,         -- 序列化事件流（MessagePack）
  metadata    TEXT                   -- JSON：屏幕分辨率、缩放比等
);

CREATE TABLE audit_log (
  id          INTEGER PRIMARY KEY AUTOINCREMENT,
  ts          INTEGER NOT NULL,
  command_id  TEXT NOT NULL,
  source      TEXT NOT NULL,         -- "orb" | "hotkey" | "mcp"
  params_hash TEXT NOT NULL,
  result      TEXT NOT NULL,         -- "ok" | "err" | "denied"
  duration_ms INTEGER
);
user_version 作为迁移幂等闸门，首启自动备份旧数据库到 orbit.db.bak。

六、模块详细拆解
P0 — 速度基座与冲突地基（第 1–2 周）
P0-1 悬浮窗创建与平台适配

子任务	实现要点	验收
Tauri 透明置顶窗口	transparent: true、decorations: false、alwaysOnTop: true	三平台可见
Windows DWM 过渡禁用	Win32 DwmSetWindowAttribute(DWMWA_TRANSITIONS_FORCEDISABLED)	show() < 16 ms
Linux 透明渲染	设置 WEBKIT_DISABLE_DMABUF_RENDERER=1	无黑底
拖拽权限	capabilities/default.json 加入 core:window:allow-start-dragging	拖拽可用
位置记忆与离屏重置	持久化坐标，多显示器变化时重置到主屏中心	重启后位置正确
P0-2 拖拽/点击冲突框架

这是事件绑定层第零条规则，不能等到 P1 再做。Tauri 已知问题（tauri#10767）会导致拖拽交接吞掉 mouseup 和焦点事件。

规则	实现
移动阈值拖拽	按压后位移 > 4px 才调用 startDragging()
双击检测	e.detail === 2 在 mousedown 阶段判定，第二次按压不启动拖拽
右键菜单	Tauri 原生 ContextMenu::popup(window)，菜单项 ID 加 orb-- 前缀避免与托盘冲突
事件阻断	拖拽启动后，当前按压周期内的 click 绑定不触发
P0-3 命令契约层与单一事实源

CommandSpec 定义于 orbit-core，通过 specta 生成 TS 类型、schemars 生成 JSON Schema。

命令总线 execute(commandId, params, source) 为唯一入口，source ∈ {orb, hotkey, mcp}。

执行日志（成功/失败/拒绝）写入 SQLite audit_log，异步批量落盘，不阻塞主路径。

P0-4 截图管线（xcap）

全屏截图、区域截图、多显示器支持。

内存直出：xcap 捕获 → RGBA buffer → 编码 → 剪贴板/文件，不经过磁盘中转。

首版只实现全屏 + 区域，窗口截图放 P1。

P1 — 高频核心能力与可编程交互（第 3–6 周）
P1-1 双模式轮盘

模式	触发	锚点
球心轮盘	点击悬浮球	球心
光标轮盘	全局快捷键 / 中键	鼠标当前位置
两者共享扇区渲染与命令路由，仅锚点计算不同。扇区从命令总线动态读取，支持分组（Capture / Clipboard / Input / Convert），滚轮或二级扇区切换分组。轮盘半径根据视口动态夹取，避免贴近屏幕边缘时被裁切。

P1-2 事件绑定层与编辑器

设置面板提供 binding 编辑器：事件槽位下拉 → 修饰键下拉 → 动作类型（command / macro / menu）→ 具体目标。

保存后通过 Tauri Store 写 bindings.json，同时通知主进程热重载 binding 表。

热更新路径：前端 invoke('reload_bindings') → 主进程读 Store → 更新内存 binding 表 → 返回新表快照。不需要重启。

冲突检测：同一 event + modifier 不允许重复绑定，编辑器实时校验。

P1-3 全局快捷键录制与安全注销

子任务	关键细节
Recorder UI	点击进入录制态，按下组合键捕获，Escape 取消，清除按钮禁用
冲突检测	GlobalShortcut::register 返回 Err 时必须通知用户，不能假设注册成功
安全注销	变更时先 unregister 旧键，再注册新键；unregister_all() 在 WindowEvent::Destroyed 兜底
事务边界	binding 更新与快捷键注销在同一逻辑事务内，失败回滚
持久化	与 binding 一同写入 bindings.json
P1-4 持久化分工

数据	存储	理由
事件绑定、快捷键、主题选择	Tauri Store（JSON）	键值型、低查询复杂度、前后端均可访问
剪贴板历史	SQLite（WAL）	关系查询、按时间/类型过滤
键鼠录制脚本	SQLite（BLOB）	持久性要求高、需元数据关联
审计日志	SQLite（WAL）	追加写、按时间检索
bindings.json 的版本化：version 字段 + 启动时迁移函数，新增事件槽位不触发 SQLite 迁移。

P1-5 剪贴板历史

底层 tauri-plugin-clipboard-next 或平台 API。

监听变化走 Tauri 事件，主进程收到后写入 SQLite（去重：内容哈希相同且 5 秒内不重复写）。

轮盘子菜单展示最近 20 条，按类型（文本/图片/文件路径）分组。

P1-6 OCR（系统原生）

macOS：Vision framework VNRecognizeTextRequest

Windows：Windows.Media.Ocr

返回 { text, blocks: [{ bbox, text, confidence }] }，支持“截图 → OCR → 剪贴板 / 翻译命令”链路。

中文识别率不达标时，PaddleOCR sidecar 作为可选增强，用户显式启用，不拖慢冷启动。

P2 — 重型能力、主题系统与 MCP 暴露（第 7–15 周）
P2-1 录屏原生管线

平台	API	编码
macOS	ScreenCaptureKit	VideoToolbox (H.264/HEVC)
Windows	Windows.Graphics.Capture	Media Foundation
Linux	PipeWire / xdg-desktop-portal	VA-API（可选）
不引入 FFmpeg，控制包体和合规风险。

采集线程独立于主进程，帧数据通过共享内存或本地 socket 传给写入线程。

渐进式写盘：边采集边写入临时容器，崩溃后可通过索引恢复已写入部分，优于 MediaRecorder 的一次性内存缓冲。

音轨采集必须作为独立验收项（SkyDesk2 明确声明“暂无法录制声音”，这是必须补上的缺陷）。

验收：音画同步偏差 < 50 ms、意外退出可恢复、30 分钟内存增量 < 50 MB。

P2-2 键鼠录制与回放

录制：rdev::listen 放独立线程（阻塞），事件序列化到 SQLite。

回放：enigo 模拟，按时间戳调度，支持倍速和循环。

兼容性边界：rdev::listen 依赖 X11，Wayland 原生会话下不工作；rdev::grab 走 evdev 在 X11 和 Wayland 下均可拦截但权限模型不同。Linux 支持范围明确限定为 X11 / XWayland。

回放精度：通过“绝对时间戳 + 补偿调度”而非“sleep 累加”，减少累积漂移。

P2-3 格式转换

FFmpeg 子进程，命令参数封装为 convert.audio、convert.video、convert.image。

走重路径，进度通过 Tauri 事件推送到前端。

支持拖拽文件到悬浮球触发（与 drop 事件绑定联动）。

P2-4 主题系统

v1（CSS 变量） ：预置主题在构建时静态注入 CSS，运行时切换只更新 :root 变量；接入 appWindow.onThemeChanged 跟随系统主题。

v2（ZIP 主题包） ：主题包含 theme.json、index.css、可选 preview.jpg / locales/ / fonts/，运行时安装/删除/备份，通过自定义协议加载资源。tauri-plugin-custom-themes-api 当前 0.1.0，成熟度有限，v2 落地时做充分评估，必要时自建轻量管理器。

P2-5 MCP 适配层

独立 sidecar 二进制，stdio JSON-RPC。

工具定义从 CommandSpec 的 JSON Schema 自动生成。

通过本地 IPC（Unix socket / Windows named pipe）转发到命令总线。

分发形态：Tauri shell 负责 UI 和进程生命周期，MCP server 作为 sidecar 随安装包分发。

绑定 127.0.0.1，要求 bearer token，默认关闭，用户主动启动。

七、里程碑与周计划
M0：性能脚手架与冲突验证（第 1–2 周）
周	任务	交付物
W1	Tauri 最小悬浮窗、DWM 禁用、xcap 全屏截图、rdev 监听	三平台性能基线初稿
W1	4px 阈值 + 双击检测 + 原生右键菜单 demo	冲突框架可运行 demo
W2	三平台完整性能基线（冷启动、内存、截图、抖动）	docs/PERF.md 初版
W2	全局快捷键注册失败路径与注销验证	失败路径测试报告
M0 出口条件：P-01、P-02、P-05 达标；冲突框架 demo 在 Windows 和 Linux X11 通过；若某项指标不达标，在进入 M1 前调整实现路径（例如轮盘改为“常驻窗口 + 移动位置”而非 show/hide）。

M1：命令总线 + 双模式轮盘 + 事件绑定 + 截图/剪贴板闭环（第 3–6 周）
周	任务
W3	命令契约层（Rust → TS + Schema 生成）、命令总线执行日志
W4	球心轮盘 + 光标轮盘、扇区动态读取、分组切换
W5	binding 编辑器、Tauri Store 持久化、热重载路径
W5	全局快捷键 recorder + 冲突检测 + 安全注销
W6	截图 → 剪贴板全链路、剪贴板历史层、SQLite 迁移框架
M1 出口条件：P-03、P-04、P-07 达标；修改 binding 或快捷键后无需重启立即生效；旧快捷键被可靠释放。

M2：OCR + 键鼠录制 + MCP 只读工具（第 7–10 周）
周	任务
W7	系统原生 OCR（macOS + Windows），截图 → OCR → 剪贴板链路
W8	键鼠录制侧完成，录制数据格式定型，回放原型
W9	MCP sidecar 跑通，只读工具暴露（screenshot / ocr / clipboard_read / list_windows / get_screen_info）
W10	Claude Desktop / Cursor 实测工具发现与调用；回放精度调优
M2 出口条件：P-06、P-09 达标；MCP 只读工具在至少一个主流客户端上可用；30 秒录制回放漂移 < 20 ms/10s。

M3：录屏 + 回放 + 格式转换 + 主题包（第 11–15 周）
周	任务
W11	录屏管线第一个平台（按用户分布定），全屏录制 + 音轨
W12	录屏区域/窗口模式，渐进式写盘与崩溃恢复
W13	录屏第二平台；键鼠回放与录制联调
W14	FFmpeg 子进程格式转换；主题 v1（CSS 变量 + 系统跟随）
W15	MCP 写操作按风险分级暴露；主题 v2 评估；整体回归
M3 出口条件：P-08、P-10 达标；录屏至少两平台可用且含音轨；MCP 写操作默认关闭或强制 confirm；24 小时常驻内存曲线平稳。

八、测试策略
8.1 测试分层
层级	范围	工具	纳入 CI
单元测试	契约序列化、binding 解析、快捷键冲突检测、录制格式编解码	cargo test	是
集成测试	命令总线端到端、binding 热重载、剪贴板历史写入	cargo test + Tauri test harness	是
性能测试	P-01 ~ P-10 全指标	scripts/perf-baseline/ 自定义 bench	是（阈值告警）
跨平台矩阵	Windows 10/11、macOS 13+（Intel + Apple Silicon）、Ubuntu 22.04 X11	GitHub Actions matrix + 自托管 runner	是（合并前）
长时稳定性	24 小时常驻内存曲线、1000 次截图循环、30 分钟录屏	定时任务	每周
安全测试	MCP 未授权调用拒绝、确认机制绕过尝试、审计日志完整性	手工 + 自动化	每里程碑
8.2 性能基线脚本
scripts/perf-baseline/ 提供可重复执行的基准：

bash
# 冷启动
orbit-bench cold-start --runs 20 --report cold-start.json

# 截图延迟
orbit-bench screenshot --mode fullscreen --runs 100 --report screenshot.json

# 轮盘延迟
orbit-bench radial-open --runs 100 --report radial.json

# 键鼠抖动
orbit-bench input-jitter --duration 10s --report jitter.json
每次 CI 跑出的结果与 docs/PERF.md 中的基线对比，超过阈值 20% 时告警，超过 50% 时阻断合并。

8.3 平台兼容性矩阵
平台	版本	键鼠录制	录屏	OCR
Windows	10 21H2+ / 11	✅	✅ (Graphics.Capture)	✅
macOS	13+	✅（需辅助功能权限）	✅ (ScreenCaptureKit)	✅
Linux	Ubuntu 22.04 X11	✅	✅ (PipeWire)	⚠️ 依赖 Tesseract
Linux	Wayland 原生	❌ 降级	✅ (portal)	⚠️
九、安全与 MCP 暴露策略
9.1 工具分级
工具	风险	默认状态	调用要求
screenshot	ReadOnly	开启	—
ocr	ReadOnly	开启	—
clipboard_read	ReadOnly	开启	—
list_windows	ReadOnly	开启	—
get_screen_info	ReadOnly	开启	—
clipboard_write	Mutating	关闭	confirm: true + 来源标记
mouse_click	Destructive	关闭	confirm: true + 来源标记
keyboard_type	Destructive	关闭	confirm: true + 来源标记
replay_macro	Destructive	关闭	confirm: true + 录制会话 ID
录屏启停	—	不暴露	仅本地轮盘触发
格式转换	—	不暴露	仅本地轮盘触发
9.2 来源标记机制
写操作不能凭空构造。clipboard_write 必须引用一次截图或 OCR 的结果 ID，replay_macro 必须引用录制会话 ID。MCP schema 中不出现裸的 x/y 坐标字段或任意文本写入，大模型无法绕过观察直接操作。

9.3 审计与拒绝
所有 MCP 调用写入 audit_log，source = "mcp"。拒绝的调用记录 result = "denied" 及原因。审计日志按周轮转，保留 90 天。

9.4 产品文档定性声明
产品文档（docs/SAFETY.md）明确写出：任何做会话级捕获、低级钩子或无值守桌面控制的东西，都与大量攻击性工具共享能力面。这不是靠“做得小心”能绕过的评价，而是键鼠全局监听和回放模块的固有属性。请只在可信环境中运行本工具。

十、发布与分发
平台	格式	签名	更新
Windows	NSIS 安装包 + portable zip	Authenticode	Tauri updater
macOS	DMG + App Bundle	Developer ID + 公证	Tauri updater
Linux	AppImage + deb	GPG	Tauri updater
sidecar 打包：MCP server 作为 externalBin 打包进安装包，Tauri shell 启动时按需拉起。安装后无需预装任何运行时。

版本策略：0.x 阶段每 2 周一个 minor，1.0 后每 4 周一个 minor，patch 按需。

十一、文档体系
文档	内容	维护者
README.md	产品介绍、安装、快速上手	全员
docs/PERF.md	性能基线报告、基准脚本用法	Rust 工程师
docs/CONTRACTS.md	命令契约清单、参数/返回/风险说明	Rust 工程师
docs/SAFETY.md	MCP 安全边界、工具分级、审计说明	安全负责人
docs/BINDINGS.md	事件槽位、修饰键、动作类型参考	前端
docs/THEMES.md	主题包格式、CSS 变量清单	前端
CHANGELOG.md	按 Keep a Changelog 格式	全员
十二、资源与风险登记册
12.1 最小人员配置
角色	投入	负责范围
Rust / Tauri 工程师（主）	1.5 人	命令总线、契约层、MCP、键鼠、录屏
Rust 工程师（辅）	0.5 人	录屏平台适配、格式转换
前端工程师	1 人	悬浮球/轮盘、binding 编辑器、设置、主题
测试 / 平台联调	0.5 人	三平台矩阵、权限路径、长时稳定性
产品 / 文档	0.5 人	README、SAFETY、用户文档
12.2 风险登记册
编号	风险	概率	影响	缓解措施	负责阶段
R-01	Wayland 下键鼠监听不可用	高	中	明确限定 X11/XWayland，文档前置声明	M0
R-02	Windows WebView2 内存超 60 MB	中	高	测量边界定义为整进程族；必要时精简 UI 依赖	M0
R-03	录屏三平台是三套实现	高	高	按平台拆分，先做主力平台，不做齐活承诺	M3
R-04	全局快捷键被占用静默失败	中	中	捕获 Err 并通知用户；启动时自检	M1
R-05	binding 热更新导致状态不一致	中	中	事务边界 + 失败回滚 + 快照校验	M1
R-06	主题包加载不安全资源	低	高	自定义协议 + 资源白名单 + 签名校验	M3
R-07	MCP 被提示注入利用	中	高	来源标记 + 默认关闭 + 审计 + 不暴露裸坐标	M2
R-08	录屏长时内存增长	中	中	渐进式写盘 + 环形缓冲 + 30 分钟压测	M3
R-09	键鼠回放累积漂移	中	中	绝对时间戳 + 补偿调度，而非 sleep 累加	M2
R-10	Tauri 拖拽吞事件（#10767）	高	高	4px 阈值 + 双击前置检测，M0 验证	M0
十三、第一周行动清单
第一周不写功能代码，只做五件验证：

Windows 透明置顶窗口 show() 延迟。Tauri 2 创建悬浮窗，禁用 DWM 过渡，测调用到窗口可见耗时。做不到 16 ms 以内，轮盘方案改为“常驻窗口 + 移动位置”。

xcap 全屏截图端到端耗时。调用到 PNG 字节就绪。超过 150 ms 检查是否走了磁盘中转，改为内存直出。

rdev 高频鼠标事件时间戳偏差。Windows 和 Linux X11 各录 10 秒，统计偏差分布。这个数据决定回放精度承诺能写到什么程度。

4px 阈值 + 双击检测 + 右键菜单的冲突框架 demo。确认拖拽不再吞 click 和 mouseup，双击在拖拽启动前被正确识别。

全局快捷键注册失败与注销验证。在已被占用的组合键上测 register 失败路径，确认能捕获 Err 并通知；测变更后旧键是否被可靠释放。

这五件事的结论，比任何功能列表都更能判断 Orbit 能否在“速度优先、可编程、可换肤”的前提下成立。先锁地基，再叠功能。

# Orbit
**桌面悬浮球项目计划与进度**

> 项目代号：Orbit · 中文名：轨道 · 技术栈：Tauri 2 + Rust 原生管线 · 定位：速度优先、可编程、可换肤的常驻自动化中枢
>
> 进度更新：2026-11-06。当前已有桌面覆盖层原型；文中的性能数字仍是待验证的初始目标，不代表已达标。

## 开发环境

桌面端使用 Tauri 2。Linux 开发环境需要 Node.js 20.19+（或 22.12+）、Rust stable 和 WebKitGTK 4.1。可从 [rustup.rs](https://rustup.rs) 安装 Rust；Ubuntu/Debian 系统依赖如下：

```bash
sudo apt update
sudo apt install build-essential curl wget file libxdo-dev libssl-dev librsvg2-dev libwebkit2gtk-4.1-dev libpipewire-0.3-dev libgbm-dev libpipewire-0.3-dev
```

安装 Rust stable 后，在仓库根目录执行：

```bash
npm install
npm run tauri:dev
```

`npm run check` 和 `npm run build` 用于前端检查与构建；`npm run tauri:build` 构建桌面安装包。首次 Rust 构建需要下载并编译依赖。macOS 和 Windows 的系统依赖请按 Tauri 2 对应平台的开发环境要求安装。

## 覆盖层行为与平台边界

Orbit 启动后显示透明置顶悬浮球。拖到屏幕顶部吸附为刘海样式状态条，拖到左右边缘吸附为侧边入口；点击后展开覆盖层，再点击关闭按钮收起。窗口位置会保存，并在启动后恢复吸附状态。顶部状态条可显示系统 CPU 使用率和系统提供的 CPU 温度传感器数据；没有可读传感器时显示 `--`。

置顶窗口只能请求操作系统窗口管理器提升层级，不能保证覆盖独占全屏游戏、受保护视频或系统安全界面。Windows/macOS 全屏空间、Linux Wayland 合成器及游戏/显卡驱动可能隐藏普通桌面覆盖层；需要实际平台验收。游戏 FPS 没有跨游戏通用读取接口，当前显示 `--`，不能将屏幕采样伪装成游戏帧率。接入帧率需后续按游戏/平台选择受支持的遥测接口。

## 当前项目进度（2026-11-06）

当前处于 **M0/P0 原型阶段，M0 尚未验收通过；M1 正在实现**。本次首发目标确定为 Windows x64 NSIS 安装包；已有一个未实机验证的 0.1.0 安装器原型和 Windows 安装器 CI 工作流，尚未达到正式 v0.1 发布条件。

| 范围 | 状态 | 说明 |
| --- | --- | --- |
| Tauri 桌面工程 | 已实现 | Svelte/Vite 前端与 Tauri 2/Rust 桌面端已接通；Node.js 用于前端及打包工具链，不是打包后应用的运行时依赖。 |
| 悬浮球与窗口行为 | 原型已实现，待实机验收 | 透明置顶无边框窗口、拖动、顶部/左右吸附、展开面板、原生右键菜单及窗口位置保存已接入。多显示器变化和离屏位置恢复仍需验证/完善。 |
| 系统状态 | 部分实现 | CPU 使用率和可读的 CPU 温度传感器数据可采集；没有可读温度传感器时显示 `--`。游戏 FPS 未接入。 |
| 一键设置与权限 | 部分实现 | 首次点击悬浮球会注册登录启动；面板可查看并开关登录启动。屏幕录制和辅助功能权限目前明确显示为“未检测”，授权流程及设置页跳转尚未实现。 |
| P0-2 拖拽/点击冲突 | 逻辑已实现，待平台验收 | 包含 4px 拖动阈值、双击预判、拖动后抑制点击和原生右键菜单；尚未完成各目标平台的手工验证。 |
| P0-3 命令契约与命令总线 | 首个能力已接入 | `orbit-core` 提供命令请求/响应契约、TS 类型生成、JSON Schema 和 SQLite 审计；当前仅注册截图命令，其他命令路由与权限策略未完成。 |
| P0-4 截图管线 | 基础闭环已实现，待桌面验收 | xcap 支持全屏多显示器合成与按显示器坐标区域截图，并直接写入剪贴板；已增加独立透明选区窗口和 Escape/右键取消，仍待 Windows 桌面验证坐标缩放、剪贴板与恢复行为。 |
| P1 及后续能力 | 部分开始 | 命令总线/截图已开始；轮盘、事件绑定、全局快捷键、剪贴板历史、OCR、键鼠录制回放、录屏和 MCP 尚未实现。 |
| Windows 安装器 | 原型已构建，待验收 | Linux 交叉构建已生成 NSIS 0.1.0 原型；因当前没有 Windows 桌面运行环境，尚未安装/启动验收。GitHub Actions 可手动触发或由匹配 `package.json` 版本的 `v*` 标签触发。 |

**已完成的工程验证：** 前端检查/构建通过；Rust 单测覆盖贴边判定、审计记录和命令类型生成；Linux 构建及 Windows GNU 目标交叉检查通过。Windows NSIS 原型包已生成（4.09 MiB），未签名且未在 Windows 上执行。以上不等于已完成目标桌面的 UI、拖动、截图/剪贴板、全屏游戏或长时稳定性验收。尚无 macOS 实机验证，也未测量 P-01 至 P-10 的性能基线。

**当前主要风险与下一步：** 先在 Windows 实机验收安装、启动、登录启动、拖动/吸附、截图/剪贴板和卸载；补齐区域选取 UI，再推进 M1 的轮盘/绑定/快捷键/剪贴板闭环。随后按 M2、M3 完成 OCR、键鼠录制回放、只读 MCP、录屏和主题能力，并补性能、安全、长时测试。普通置顶窗口无法保证显示在独占全屏游戏之上；若游戏覆盖和 FPS 是优先目标，需另选游戏/遥测接口，不作为普通窗口已有能力。

## 一、项目概述
Orbit 是一个以悬浮球为唯一常驻入口、以轮盘和事件绑定为交互壳、以命令总线为执行内核、以 MCP 为外部扩展面的桌面自动化工具。悬浮球是中心天体，轮盘是径向轨道，命令是环绕运行的卫星，MCP 是外部轨道接口——四层含义统一在一个名字里。

产品的核心承诺只有一条：点击到响应的体感延迟不可感知，开机到可用的沉默感不可打破。功能可以迭代增加，但如果悬浮球在长时间常驻后出现卡顿、轮盘弹出有肉眼可见的延迟、键鼠回放漂移，产品就失去了存在理由。

### 1.1 目标用户
| 用户类型 | 核心诉求 | 对应能力 |
| --- | --- | --- |
| 效率工具重度用户 | 一键完成截图/OCR/剪贴板/翻译 | 轮盘 + 事件绑定 |
| 自动化爱好者 | 录制键鼠操作、串成宏、绑定到任意事件 | 宏录制 + binding |
| 开发者 / AI 工作流用户 | 让大模型调用桌面能力 | MCP 只读工具集 |
| 桌面美化用户 | 换主题、换皮肤、换交互手感 | 主题包系统 |
### 1.2 不做什么（明确边界）
不做云端服务：所有能力本地执行，OCR、录屏、录制数据不出机器。MCP 首版采用 stdio；如未来增加 HTTP transport，只允许绑定 127.0.0.1 并启用认证。

首次启动直接显示悬浮球；点击悬浮球触发一键设置并注册登录启动。各项系统权限仍由操作系统分别授权，不承诺一个跨平台的“全部权限”按钮。管理员 / root 权限仅在具体功能确实需要时通过平台授权流程申请。

不做全平台键鼠支持承诺：Linux 明确限定 X11 / XWayland，纯 Wayland 下录制降级或不可用。

不做无确认的高风险远程调用：mouse_click、keyboard_type、replay_macro 默认不暴露给 MCP。

不追求三平台录屏同时齐活：按平台逐个落地，先做目标用户最集中的平台。

## 二、硬性指标与验收基线
以下指标是待验证的初始目标，不是未经测量的产品承诺。基线报告必须注明设备、操作系统、分辨率、采样次数和统计口径；建议至少报告 p50 / p95。只有在固定测试环境下复现后，指标才作为阶段验收线。超标时先定位瓶颈，再决定降级或调整实现路径。

| 编号 | 指标 | 初始目标（待实测校准） | 测量条件 | 验收阶段 |
| --- | --- | --- | --- | --- |
| P-01 | 冷启动时间 | < 500 ms | 进程启动到悬浮球可点击 | M0 |
| P-02 | 空闲内存占用 | < 60 MB（整进程族） | 悬浮球静止、轮盘关闭、无录屏/OCR | M0 |
| P-03 | 轮盘弹出延迟 | < 16 ms | 点击悬浮球到轮盘完全绘制 | M1 |
| P-04 | 截图 → 剪贴板就绪 | < 150 ms | 全屏捕获、编码、写入剪贴板 | M1 |
| P-05 | 全局键鼠录制抖动 | < 5 ms | 高频鼠标移动事件时间戳偏差 | M0 |
| P-06 | MCP 工具调用开销 | < 50 ms（不含执行） | IPC 往返 | M2 |
| P-07 | 绑定/快捷键变更生效 | 即时，无需重启 | 修改后下一次触发即按新绑定执行 | M1 |
| P-08 | 主题切换首帧延迟 | < 16 ms | CSS 变量更新后首次渲染 | M3 |
| P-09 | 键鼠回放漂移 | < 20 ms / 10s | 30 秒录制回放的累计时间偏差 | M2 |
| P-10 | 长时录制内存增长 | < 50 MB / 30min | 30 分钟录屏期间 RSS 增量 | M3 |
## 三、技术选型与架构
### 3.1 选型决策
Tauri 2 + Rust 原生管线。基准数据：空闲内存 30–50 MB、冷启动 < 0.5 秒，而 Electron 对应 200–300 MB 和 1–2 秒。有迁移案例显示托盘类应用从 Electron 迁到 Tauri 后，空闲内存从 185 MB 降至 28.5 MB，冷启动从 3.2 秒压到 0.5 秒以内。

两个必须提前接受的硬约束：

录屏不走 WebView 媒体栈。 macOS 的 WKWebView 不支持 getDisplayMedia，Windows 的 WebView2 也拿不到系统级区域捕获，且 macOS 的 App Nap 会节流后台 JavaScript，导致快捷键触发的录制初始化被静默排队。录屏必须作为独立的 Rust 原生采集管线存在。

MCP 不嵌入主进程。 MCP 工具调用如果跑在 UI 进程的事件循环里，大模型的一次批量调用就可能阻塞悬浮球的点击响应。MCP Server 作为独立 sidecar 运行，通过本地 IPC 转发到命令总线。

### 3.2 五层架构
```text
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
│     风险分级暴露：只读按用户配置启用，写操作由服务端授权        │
└─────────────────────────────────────────────────────────────┘
```
快路径与重路径分离是性能设计的核心。 毫秒级操作留在主进程内走轻量异步；耗时或需要独立权限的操作放到子进程，崩溃不拖垮主进程，也不占用 UI 事件循环。

## 四、工程目录结构
```text
orbit/
├── README.md
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
```
为什么拆成 workspace crates：orbit-capture 和 orbit-mcp 需要独立编译为 sidecar 二进制；orbit-core 作为被依赖的库，编译产物可被主进程和 MCP sidecar 共享；前端类型通过 specta 或 ts-rs 从 orbit-core 的契约类型自动生成，保证单一事实源。

## 五、核心数据结构与契约
### 5.1 命令契约（orbit-core::contract）
```rust
#[derive(Serialize, Clone)]
pub struct CommandSpec {
    pub id: &'static str,          // "screenshot.region"
    pub label: &'static str,       // "区域截图"
    pub category: Category,        // Capture | Ocr | Clipboard | Input | Convert
  pub params_schema: serde_json::Value,
  pub returns_schema: serde_json::Value,
    pub risk: RiskLevel,           // ReadOnly | Mutating | Destructive
    pub idempotent: bool,
}

#[derive(Serialize, Clone, Copy)]
pub enum RiskLevel { ReadOnly, Mutating, Destructive }
```

这里的 `CommandSpec` 是运行时命令目录示意：不从外部反序列化，因此不派生 `Deserialize`；`JsonSchema` 是 trait，不应直接作为字段类型。参数与返回值应定义为具体 Rust 类型，再由选定的类型工具生成 TypeScript、由 schemars 生成 JSON Schema，避免手写多份契约。该代码片段仍需按选定的 crate 版本补齐生成实现。

单一事实源原则：命令参数与返回类型是契约定义；命令目录记录风险、类别和 schema。生成产物必须纳入 CI 检查，避免本地命令和 MCP 工具参数漂移。

### 5.2 事件绑定（bindings.json）
```json
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
```
事件槽位枚举：left-click、left-double-click、middle-click、right-click、wheel-up、wheel-down、drop、hover-enter、hover-leave。

修饰键枚举：none、ctrl、alt、shift、ctrl+alt、ctrl+shift、alt+shift、ctrl+alt+shift。

### 5.3 录制脚本格式（SQLite 表）
```sql
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
```

数据库迁移使用 `PRAGMA user_version` 作为版本闸门；迁移前备份数据库，并明确备份保留与恢复策略。

## 六、模块详细拆解
### P0 — 速度基座与冲突地基（第 1–2 周）
#### P0-1 悬浮窗创建与平台适配

进度：透明置顶窗口、拖拽、顶部/左右吸附、位置保存和相关平台配置已接入；多显示器离屏恢复及跨平台交互尚待验收。普通置顶行为不构成独占全屏覆盖保证。

| 子任务 | 实现要点 | 验收 |
| --- | --- | --- |
| Tauri 透明置顶窗口 | transparent: true、decorations: false、alwaysOnTop: true | 桌面合成层置顶；全屏游戏需逐平台验证 |
| Windows DWM 过渡禁用 | Win32 DwmSetWindowAttribute(DWMWA_TRANSITIONS_FORCEDISABLED) | show() < 16 ms |
| Linux 透明渲染 | 设置 WEBKIT_DISABLE_DMABUF_RENDERER=1 | 无黑底 |
| 拖拽权限 | capabilities/default.json 加入 core:window:allow-start-dragging | 拖拽可用 |
| 位置记忆、贴边吸附与离屏重置 | 顶部刘海、左右侧边栏、持久化坐标，多显示器变化时重置到主屏中心 | 重启后位置正确 |
#### P0-2 拖拽/点击冲突框架

这是事件绑定层第零条规则，不能等到 P1 再做。Tauri 已知问题（tauri#10767）会导致拖拽交接吞掉 mouseup 和焦点事件。

进度：核心交互逻辑已接入，吸附判定单元测试通过；Windows、Linux X11 等目标平台的手工冲突验证仍未完成。

| 规则 | 实现 |
| --- | --- |
| 移动阈值拖拽 | 按压后位移 > 4px 才调用 startDragging() |
| 双击检测 | e.detail === 2 在 mousedown 阶段判定，第二次按压不启动拖拽 |
| 右键菜单 | Tauri 原生 ContextMenu::popup(window)，菜单项 ID 加 orb-- 前缀避免与托盘冲突 |
| 事件阻断 | 拖拽启动后，当前按压周期内的 click 绑定不触发 |
#### P0-3 命令契约层与单一事实源

进度：首个命令契约、TypeScript 类型、JSON Schema、SQLite `user_version` 迁移与审计写入已接入；当前命令路由仅包含截图，完整命令目录、安全策略和审计轮转尚未完成。

CommandSpec 定义于 orbit-core；当前采用 ts-rs 生成 TypeScript、schemars 生成 JSON Schema。生成文件由 Rust 契约测试维护。

命令总线 execute(commandId, params, source) 为唯一入口，source ∈ {orb, hotkey, mcp}。

执行日志（成功/失败/拒绝）写入 SQLite audit_log，异步批量落盘，不阻塞主路径。

#### P0-4 截图管线（xcap）

进度：xcap 全屏多显示器合成、区域截图以及 RGBA 内存直写剪贴板已接入；前端提供全屏截图和透明屏幕框选入口，区域选取期间隐藏主窗口以避免选区层进入截图。Windows 实机延迟及坐标缩放验收待完成。

全屏截图、区域截图、多显示器支持。

内存直出：xcap 捕获 → RGBA buffer → 编码 → 剪贴板/文件，不经过磁盘中转。

首版只实现全屏 + 区域，窗口截图放 P1。

#### P0-5 首次启动权限与自启动引导

进度：首次点击悬浮球触发登录启动注册、读取注册状态、在面板关闭登录启动已接入；屏幕录制与辅助功能权限只返回“未检测”，逐项授权/降级提示和设置页跳转尚未完成，也尚未在 Windows 桌面验收完整流程。

- 启动后直接显示悬浮球；首次点击悬浮球触发一键设置并注册“登录时启动”。系统权限仍由操作系统分别授权，应用不得把未检测或未授权的权限标记为完成。
- 通过操作系统提供的授权流程或设置页逐项完成授权；不能绕过系统确认。未授权时明确提示对应能力不可用，并提供重试或打开系统设置的入口。
- “登录时启动”由首次点击触发注册；之后用户可在设置中关闭，关闭后移除对应启动项，卸载时清理应用创建的启动配置。
- 若某项能力确实需要管理员 / root 辅助程序，设置流程应单独解释原因、权限范围和撤销方式，并由操作系统确认安装；不得因此提升整个 UI / MCP 进程。

验收：应用启动后直接显示可拖动悬浮球；点击可触发一次设置并反馈登录启动结果；权限未经操作系统确认时不得报告为已授权；用户后续关闭登录启动后设置立即生效。

### P1 — 高频核心能力与可编程交互（第 3–6 周）
进度：命令总线与截图到剪贴板的首个可运行闭环已开始实现；双模式轮盘、事件绑定、快捷键录制与剪贴板历史尚未实现。

#### P1-1 双模式轮盘

| 模式 | 触发 | 锚点 |
| --- | --- | --- |
| 球心轮盘 | 点击悬浮球 | 球心 |
| 光标轮盘 | 全局快捷键 / 中键 | 鼠标当前位置 |
两者共享扇区渲染与命令路由，仅锚点计算不同。扇区从命令总线动态读取，支持分组（Capture / Clipboard / Input / Convert），滚轮或二级扇区切换分组。轮盘半径根据视口动态夹取，避免贴近屏幕边缘时被裁切。

#### P1-2 事件绑定层与编辑器

设置面板提供 binding 编辑器：事件槽位下拉 → 修饰键下拉 → 动作类型（command / macro / menu）→ 具体目标。

保存后通过 Tauri Store 写 bindings.json，同时通知主进程热重载 binding 表。

热更新路径：前端 invoke('reload_bindings') → 主进程读 Store → 更新内存 binding 表 → 返回新表快照。不需要重启。

冲突检测：同一 event + modifier 不允许重复绑定，编辑器实时校验。

#### P1-3 全局快捷键录制与安全注销

| 子任务 | 关键细节 |
| --- | --- |
| Recorder UI | 点击进入录制态，按下组合键捕获，Escape 取消，清除按钮禁用 |
| 冲突检测 | GlobalShortcut::register 返回 Err 时必须通知用户，不能假设注册成功 |
| 安全注销 | 变更时先 unregister 旧键，再注册新键；unregister_all() 在 WindowEvent::Destroyed 兜底 |
| 事务边界 | binding 更新与快捷键注销在同一逻辑事务内，失败回滚 |
| 持久化 | 与 binding 一同写入 bindings.json |
#### P1-4 持久化分工

| 数据 | 存储 | 理由 |
| --- | --- | --- |
| 事件绑定、快捷键、主题选择 | Tauri Store（JSON） | 键值型、低查询复杂度、前后端均可访问 |
| 剪贴板历史 | SQLite（WAL） | 关系查询、按时间/类型过滤 |
| 键鼠录制脚本 | SQLite（BLOB） | 持久性要求高、需元数据关联 |
| 审计日志 | SQLite（WAL） | 追加写、按时间检索 |
bindings.json 的版本化：version 字段 + 启动时迁移函数，新增事件槽位不触发 SQLite 迁移。

#### P1-5 剪贴板历史

底层 tauri-plugin-clipboard-next 或平台 API。

监听变化走 Tauri 事件，主进程收到后写入 SQLite（去重：内容哈希相同且 5 秒内不重复写）。

轮盘子菜单展示最近 20 条，按类型（文本/图片/文件路径）分组。

#### P1-6 OCR（系统原生）

macOS：Vision framework VNRecognizeTextRequest

Windows：Windows.Media.Ocr

返回 { text, blocks: [{ bbox, text, confidence }] }，支持“截图 → OCR → 剪贴板 / 翻译命令”链路。

Linux 首版 OCR 能力尚未定案：可将 Tesseract 作为可选后端，或明确标记为暂不支持；不要在兼容矩阵中承诺尚未进入排期的能力。中文识别率不达标时，可评估 PaddleOCR sidecar 作为显式启用的可选增强。

### P2 — 重型能力、主题系统与 MCP 暴露（第 7–15 周）
#### P2-1 录屏原生管线

| 平台 | API | 编码 |
| --- | --- | --- |
| macOS | ScreenCaptureKit | VideoToolbox (H.264/HEVC) |
| Windows | Windows.Graphics.Capture | Media Foundation |
| Linux | PipeWire / xdg-desktop-portal | VA-API（可选） |
录屏采集与编码首版不依赖 FFmpeg，以控制包体和部署复杂度；格式转换模块单独评估 FFmpeg 的打包、许可与安全更新策略。

采集线程独立于主进程，帧数据通过共享内存或本地 socket 传给写入线程。

渐进式写盘：边采集边写入临时容器，崩溃后可通过索引恢复已写入部分，优于 MediaRecorder 的一次性内存缓冲。

音轨采集必须作为独立验收项（SkyDesk2 明确声明“暂无法录制声音”，这是必须补上的缺陷）。

验收：音画同步偏差 < 50 ms、意外退出可恢复、30 分钟内存增量 < 50 MB。

#### P2-2 键鼠录制与回放

录制：rdev::listen 放独立线程（阻塞），事件序列化到 SQLite。

回放：enigo 模拟，按时间戳调度，支持倍速和循环。

兼容性边界：rdev::listen 依赖 X11，Wayland 原生会话下不工作；rdev::grab 走 evdev 在 X11 和 Wayland 下均可拦截但权限模型不同。Linux 支持范围明确限定为 X11 / XWayland。

回放精度：通过“绝对时间戳 + 补偿调度”而非“sleep 累加”，减少累积漂移。

#### P2-3 格式转换

FFmpeg 子进程，命令参数封装为 convert.audio、convert.video、convert.image。

走重路径，进度通过 Tauri 事件推送到前端。

支持拖拽文件到悬浮球触发（与 drop 事件绑定联动）。

#### P2-4 主题系统

v1（CSS 变量） ：预置主题在构建时静态注入 CSS，运行时切换只更新 :root 变量；接入 appWindow.onThemeChanged 跟随系统主题。

v2（ZIP 主题包） ：主题包含 theme.json、index.css、可选 preview.jpg / locales/ / fonts/，运行时安装/删除/备份，通过自定义协议加载资源。tauri-plugin-custom-themes-api 当前 0.1.0，成熟度有限，v2 落地时做充分评估，必要时自建轻量管理器。

#### P2-5 MCP 适配层

独立 sidecar 二进制，由用户配置的 MCP 客户端通过 stdio JSON-RPC 启动和通信。

工具定义从 CommandSpec 的 JSON Schema 自动生成。

通过带操作系统访问控制的本地 IPC（Unix socket / Windows named pipe）转发到命令总线；主进程必须校验请求权限和来源。

分发形态：MCP server 作为 sidecar 随安装包分发，由 MCP 客户端按用户配置启动；不自动向网络监听端口。stdio transport 不使用 bearer token。若未来增加 HTTP transport，需单独设计认证、来源校验和授权策略。

## 七、里程碑与周计划
### M0：性能脚手架与冲突验证（第 1–2 周）
排期按约 4 FTE 估算，并假设目标平台、系统权限和关键 API 能及时验证。录屏双平台、音轨和 MCP 写操作属于高风险项；若 M0 未验证通过或实际投入不足，应优先交付悬浮球、轮盘、绑定与截图闭环，将这些扩展项顺延，不以跳过安全或性能验收来追赶日期。

| 周 | 任务 | 交付物 |
| --- | --- | --- |
| W1 | Tauri 最小悬浮窗、DWM 禁用、xcap 全屏截图、rdev 监听 | 三平台性能基线初稿 |
| W1 | 4px 阈值 + 双击检测 + 原生右键菜单 demo | 冲突框架可运行 demo |
| W2 | 三平台完整性能基线（冷启动、内存、截图、抖动） | docs/PERF.md 初版 |
| W2 | 全局快捷键注册失败路径与注销验证 | 失败路径测试报告 |
| W2 | 权限能力矩阵与登录启动原型 | 悬浮球一键设置反馈、启动项开关验证 |
当前状态：M0 出口条件尚未满足。虽然悬浮层原型和部分平台编译已完成，但 P-01/P-02/P-05 尚无完整验收记录，性能基线、权限拒绝降级验证、登录启动开关验证及 Windows/Linux X11 冲突 demo 验收仍待完成。达到出口条件后再进入 M1。

### M1：命令总线 + 双模式轮盘 + 事件绑定 + 截图/剪贴板闭环（第 3–6 周）
| 周 | 任务 |
| --- | --- |
| W3 | 命令契约层（Rust → TS + Schema 生成）、命令总线执行日志 |
| W4 | 球心轮盘 + 光标轮盘、扇区动态读取、分组切换 |
| W5 | binding 编辑器、Tauri Store 持久化、热重载路径 |
| W5 | 全局快捷键 recorder + 冲突检测 + 安全注销 |
| W6 | 截图 → 剪贴板全链路、剪贴板历史层、SQLite 迁移框架 |
M1 出口条件：P-03、P-04、P-07 达标；修改 binding 或快捷键后无需重启立即生效；旧快捷键被可靠释放。

### M2：OCR + 键鼠录制 + MCP 只读工具（第 7–10 周）
| 周 | 任务 |
| --- | --- |
| W7 | 系统原生 OCR（macOS + Windows），截图 → OCR → 剪贴板链路 |
| W8 | 键鼠录制侧完成，录制数据格式定型，回放原型 |
| W9 | MCP sidecar 跑通，只读工具暴露（screenshot / ocr / clipboard_read / list_windows / get_screen_info） |
| W10 | Claude Desktop / Cursor 实测工具发现与调用；回放精度调优 |
M2 出口条件：P-06、P-09 达标；MCP 只读工具在至少一个主流客户端上可用；30 秒录制回放漂移 < 20 ms/10s。

### M3：录屏 + 回放 + 格式转换 + 主题包（第 11–15 周）
| 周 | 任务 |
| --- | --- |
| W11 | 录屏管线第一个平台（按用户分布定），全屏录制 + 音轨 |
| W12 | 录屏区域/窗口模式，渐进式写盘与崩溃恢复 |
| W13 | 录屏第二平台；键鼠回放与录制联调 |
| W14 | FFmpeg 子进程格式转换；主题 v1（CSS 变量 + 系统跟随） |
| W15 | MCP 写操作按风险分级暴露；主题 v2 评估；整体回归 |
M3 出口条件：P-08、P-10 达标；录屏至少两平台可用且含音轨；MCP 写操作默认关闭或强制 confirm；24 小时常驻内存曲线平稳。

## 八、测试策略
### 8.1 测试分层
| 层级 | 范围 | 工具 | 纳入 CI |
| --- | --- | --- | --- |
| 单元测试 | 契约序列化、binding 解析、快捷键冲突检测、录制格式编解码 | cargo test | 是 |
| 集成测试 | 命令总线端到端、binding 热重载、剪贴板历史写入 | cargo test + Tauri test harness | 是 |
| 性能测试 | P-01 ~ P-10 全指标 | scripts/perf-baseline/ 自定义 bench | 是（阈值告警） |
| 跨平台矩阵 | Windows 10/11、macOS 13+（Intel + Apple Silicon）、Ubuntu 22.04 X11 | GitHub Actions matrix + 自托管 runner | 是（合并前） |
| 长时稳定性 | 24 小时常驻内存曲线、1000 次截图循环、30 分钟录屏 | 定时任务 | 每周 |
| 安全测试 | MCP 未授权调用拒绝、确认机制绕过尝试、审计日志完整性 | 手工 + 自动化 | 每里程碑 |
### 8.2 性能基线脚本
scripts/perf-baseline/ 提供可重复执行的基准：

```bash
# 冷启动
orbit-bench cold-start --runs 20 --report cold-start.json

# 截图延迟
orbit-bench screenshot --mode fullscreen --runs 100 --report screenshot.json

# 轮盘延迟
orbit-bench radial-open --runs 100 --report radial.json

# 键鼠抖动
orbit-bench input-jitter --duration 10s --report jitter.json
```
每次 CI 跑出的结果与 docs/PERF.md 中的基线对比，超过阈值 20% 时告警，超过 50% 时阻断合并。

### 8.3 平台兼容性矩阵
| 平台 | 版本 | 键鼠录制 | 录屏 | OCR |
| --- | --- | --- | --- | --- |
| Windows | 10 21H2+ / 11 | ✅ | ✅ (Graphics.Capture) | ✅ |
| macOS | 13+ | ✅（需辅助功能权限） | ✅ (ScreenCaptureKit) | ✅ |
| Linux | Ubuntu 22.04 X11 | ✅（需验证） | ✅（portal / PipeWire，需验证） | 待定，首版不承诺 |
| Linux | Wayland 原生 | ❌ 键鼠能力降级 | ✅（portal，需验证） | 待定，首版不承诺 |
## 九、安全与 MCP 暴露策略
### 9.1 工具分级
| 工具 | 风险 | 默认状态 | 调用要求 |
| --- | --- | --- | --- |
| screenshot | ReadOnly | 用户显式启用后可用 | — |
| ocr | ReadOnly | 用户显式启用后可用 | — |
| clipboard_read | ReadOnly | 用户显式启用后可用 | — |
| list_windows | ReadOnly | 用户显式启用后可用 | — |
| get_screen_info | ReadOnly | 用户显式启用后可用 | — |
| clipboard_write | Mutating | 默认关闭 | 用户显式批准；由服务端校验授权 |
| mouse_click | Destructive | 默认关闭 | 默认不暴露；如启用，必须由服务端执行用户批准和范围校验 |
| keyboard_type | Destructive | 默认关闭 | 默认不暴露；如启用，必须由服务端执行用户批准和范围校验 |
| replay_macro | Destructive | 默认关闭 | 默认不暴露；如启用，需用户批准并限定录制会话 |
| 录屏启停 | — | 不暴露 | 仅本地轮盘触发 |
| 格式转换 | — | 不暴露 | 仅本地轮盘触发 |
### 9.2 来源标记机制
来源 ID 用于审计和限制操作对象，不等同于用户授权，也不能单独抵御提示注入。写操作必须由服务端实施权限检查；高风险操作默认不暴露，并要求清晰、可撤销的本地用户批准。首版不提供任意坐标点击或不受限的文本输入。

### 9.3 审计与拒绝
所有 MCP 调用写入 audit_log，source = "mcp"。拒绝的调用记录 result = "denied" 及原因。审计日志按周轮转，保留 90 天。

### 9.4 产品文档定性声明
产品文档（docs/SAFETY.md）明确写出：任何做会话级捕获、低级钩子或无值守桌面控制的东西，都与大量攻击性工具共享能力面。这不是靠“做得小心”能绕过的评价，而是键鼠全局监听和回放模块的固有属性。请只在可信环境中运行本工具。

### 9.5 权限与性能档位
- 启动后直接显示悬浮球；首次点击触发登录启动配置并显示屏幕录制、辅助功能等权限状态。权限逐项由系统授权，不能伪装成一次性“全部授权”。
- 开机启动配置失败时明确显示错误并允许重试；未实现或未检查的权限不得显示为已授权。
- 系统隐私或辅助功能权限按平台流程申请；只有具体操作确需管理员 / root 权限时才由独立辅助程序申请并限定范围，不提升整个 UI 或 MCP 进程。
- 提供“平衡 / 性能”档位，默认平衡。性能档只调整 Orbit 自身的工作负载预算，例如录屏帧率、编码质量、并行任务数和缓存上限；不得调整其他进程或静默修改系统电源计划、服务、安全设置。
- 不使用实时调度优先级，不以耗尽 CPU、内存或 GPU 为目标。系统资源紧张时应限制或暂停后台任务，优先保证桌面交互响应。
- 若未来某项功能确实离不开提权，必须单独说明原因、权限范围、影响和撤销方式；不得通过启动时自动提权或通用 shell helper 绕过授权。
- 验收性能档：在固定测试设备上比较 UI 响应 p95、CPU 和 RSS；性能提升不能以悬浮球卡顿、系统失去响应或突破既定资源上限为代价。

## 十、发布与分发
| 平台 | 格式 | 签名 | 更新 |
| --- | --- | --- | --- |
| Windows | NSIS 安装包 + portable zip | Authenticode | Tauri updater |
| macOS | DMG + App Bundle | Developer ID + 公证 | Tauri updater |
| Linux | AppImage + deb | GPG | Tauri updater |

首个桌面发行版目标为 Windows x64 NSIS 安装包。`.github/workflows/windows-installer.yml` 可通过 GitHub Actions 手动触发，或推送与 `package.json` 版本匹配的 `v*` 标签后构建并上传安装器产物；在 M0–M3 出口条件通过前，不应将中间构建标记为正式 v0.1。工作区使用根目录 `Cargo.lock`；Windows runner 的安装器产物位于 `target/release/bundle/nsis/`。

sidecar 打包：MCP server 作为 externalBin 打包进安装包，Tauri shell 启动时按需拉起。安装后无需预装任何运行时。

版本策略：0.x 阶段每 2 周一个 minor，1.0 后每 4 周一个 minor，patch 按需。

## 十一、文档体系
| 文档 | 内容 | 维护者 |
| --- | --- | --- |
| README.md | 产品介绍、安装、快速上手 | 全员 |
| docs/PERF.md | 性能基线报告、基准脚本用法 | Rust 工程师 |
| docs/CONTRACTS.md | 命令契约清单、参数/返回/风险说明 | Rust 工程师 |
| docs/SAFETY.md | MCP 安全边界、工具分级、审计说明 | 安全负责人 |
| docs/BINDINGS.md | 事件槽位、修饰键、动作类型参考 | 前端 |
| docs/THEMES.md | 主题包格式、CSS 变量清单 | 前端 |
| CHANGELOG.md | 按 Keep a Changelog 格式 | 全员 |
## 十二、资源与风险登记册
### 12.1 最小人员配置
| 角色 | 投入 | 负责范围 |
| --- | --- | --- |
| Rust / Tauri 工程师（主） | 1.5 人 | 命令总线、契约层、MCP、键鼠、录屏 |
| Rust 工程师（辅） | 0.5 人 | 录屏平台适配、格式转换 |
| 前端工程师 | 1 人 | 悬浮球/轮盘、binding 编辑器、设置、主题 |
| 测试 / 平台联调 | 0.5 人 | 三平台矩阵、权限路径、长时稳定性 |
| 产品 / 文档 | 0.5 人 | README、SAFETY、用户文档 |
### 12.2 风险登记册
| 编号 | 风险 | 概率 | 影响 | 缓解措施 | 负责阶段 |
| --- | --- | --- | --- | --- | --- |
| R-01 | Wayland 下键鼠监听不可用 | 高 | 中 | 明确限定 X11/XWayland，文档前置声明 | M0 |
| R-02 | Windows WebView2 内存超 60 MB | 中 | 高 | 测量边界定义为整进程族；必要时精简 UI 依赖 | M0 |
| R-03 | 录屏三平台是三套实现 | 高 | 高 | 按平台拆分，先做主力平台，不做齐活承诺 | M3 |
| R-04 | 全局快捷键被占用静默失败 | 中 | 中 | 捕获 Err 并通知用户；启动时自检 | M1 |
| R-05 | binding 热更新导致状态不一致 | 中 | 中 | 事务边界 + 失败回滚 + 快照校验 | M1 |
| R-06 | 主题包加载不安全资源 | 低 | 高 | 自定义协议 + 资源白名单 + 签名校验 | M3 |
| R-07 | MCP 被提示注入利用 | 中 | 高 | 高风险工具默认不暴露；服务端授权、用户批准、范围限制与审计 | M2 |
| R-08 | 录屏长时内存增长 | 中 | 中 | 渐进式写盘 + 环形缓冲 + 30 分钟压测 | M3 |
| R-09 | 键鼠回放累积漂移 | 中 | 中 | 绝对时间戳 + 补偿调度，而非 sleep 累加 | M2 |
| R-10 | Tauri 拖拽吞事件（#10767） | 高 | 高 | 4px 阈值 + 双击前置检测，M0 验证 | M0 |
| R-11 | 权限授权失败或性能档影响系统响应 | 中 | 高 | 明确提示授权原因并提供重试；限制 Orbit 自身工作负载并做压力验收 | M0 / M1 |
## 十三、第一周行动清单
第一周只做五项可丢弃的验证原型，不进入产品功能实现；原型测量结果用于决定后续架构和验收线。该安排与 M0/W1 的“demo/基线”任务一致。

1. **透明置顶窗口延迟**：先在 Windows 测量 Tauri 窗口从调用显示到首帧可见的延迟；再在其他目标平台复测。若达不到目标，不直接归因于 DWM，先记录平台、分位数和瓶颈，再比较常驻窗口与显示/隐藏方案。

2. **全屏截图耗时**：测量调用到 PNG 字节就绪，并分别记录捕获、编码阶段；超过目标时确认是否存在磁盘中转及编码瓶颈。

3. **鼠标事件时间分布**：在 Windows 和 Linux X11 各采集 10 秒高频事件，记录事件间隔和回调延迟；用结果校准回放精度目标，不将未经校准的时间戳偏差作为保证。

4. **拖拽与点击冲突**：验证 4px 阈值、双击检测和右键菜单原型，覆盖 click、mouseup 丢失及双击误判场景。

5. **全局快捷键失败路径**：在组合键已被占用时验证注册错误能被捕获并反馈；变更快捷键后确认旧键释放、新键注册失败时状态可恢复。

这五件事的结论，比任何功能列表都更能判断 Orbit 能否在“速度优先、可编程、可换肤”的前提下成立。先锁地基，再叠功能。

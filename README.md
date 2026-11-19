# Orbit

## 项目介绍

Orbit（轨道）是一个桌面悬浮球自动化中枢：以悬浮球为唯一常驻入口、轮盘与事件绑定为交互壳、命令总线为执行内核、MCP 为外部扩展面。核心承诺只有一条——**点击到响应的体感延迟不可感知，开机到可用的沉默感不可打破**。

悬浮球是中心天体，轮盘是径向轨道，命令是环绕运行的卫星，MCP 是外部轨道接口，四层含义统一在一个名字里。能力覆盖截图、OCR、剪贴板历史、录屏、键鼠录制回放、格式转换、系统监控、只读 MCP 工具与远程只读配对，全部本地执行，默认不产生任何网络流量。

## 环境要求

| 依赖 | 版本要求 |
|------|---------|
| Node.js | >= 20.19（或 22.12+） |
| Rust | stable（[rustup.rs](https://rustup.rs) 安装） |
| WebKitGTK | 4.1（仅 Linux 开发环境需要） |
| FFmpeg | 录屏与格式转换需要，需自行安装；支持 PATH / `FFMPEG_PATH` 环境变量 / sidecar 资源目录 |
| Windows.Media.Ocr | Windows 10+ 内置，OCR 默认后端使用 |
| WebView2 | Windows x64 随 Tauri 分发，无需单独安装 |

Linux 开发环境额外系统依赖：

```bash
sudo apt update
sudo apt install build-essential curl wget file libxdo-dev libssl-dev librsvg2-dev libwebkit2gtk-4.1-dev libpipewire-0.3-dev libgbm-dev
```

> macOS / Linux 后端尚未实机验证，当前仅在 Windows x64 实机安装运行验证。

## 技术栈

| 分类 | 技术 |
|------|------|
| 前端 | Svelte 5 + TypeScript + Vite 6 |
| 桌面壳 | Tauri 2（Windows 使用 WebView2） |
| 主进程/原生管线 | Rust（命令总线、截图、OCR、剪贴板、录屏、键鼠、格式转换） |
| 数据库 | SQLite（rusqlite，WAL）：剪贴板历史、键鼠录制、审计日志 |
| 采集层 | xcap（截图）、WinRT（OCR + WGC 采集 POC）、FFmpeg（录屏/转换）、enigo（键鼠回放 + 本地鼠标控制） |
| 外部扩展 | 独立 MCP sidecar 二进制（stdio JSON-RPC，只读白名单） |
| 构建/发布 | Tauri NSIS 安装包 + GitHub Actions CI |

**选型理由**：基准数据（Tauri 2 + Rust）空闲内存 30–50 MB、冷启动 < 0.5 s，而 Electron 对应 200–300 MB 和 1–2 s。两个必须接受的硬约束：录屏不走 WebView 媒体栈（WebView2 拿不到系统级区域捕获，macOS App Nap 会节流后台 JS），必须独立 Rust 原生采集管线；MCP 不嵌入主进程（大模型批量调用不能阻塞悬浮球点击响应），必须独立 sidecar。

## 项目结构

```
orbit/
├── Cargo.toml                      # workspace 根：orbit-core / orbit-ocr / orbit-ocr-server / src-tauri
├── package.json                    # 前端依赖与脚本（dev/check/build/tauri:*）
├── crates/
│   ├── orbit-core/                 # 命令契约、TS 绑定生成、SQLite 审计、事件绑定
│   ├── orbit-ocr/                  # 共享 OCR 引擎：原生 WinRT 优先 + PowerShell 回退
│   └── orbit-ocr-server/           # 远程 OCR：服务端（落盘+识别）与客户端（截屏上传）
├── src-tauri/                      # Tauri 主进程
│   ├── src/
│   │   ├── main.rs / lib.rs        # 窗口布局、overlay 状态、setup/autostart、区域选区、run() 装配
│   │   ├── bus.rs                  # 命令总线：注册表、路由、事件发射、审计、错误分类
│   │   ├── bindings.rs             # 事件绑定层：bindings.json 持久化、快捷键注册、动作分发
│   │   ├── windows.rs              # 惰性窗口创建（clipboard/selection 首次使用时创建）
│   │   ├── mcp_bridge.rs           # MCP 本地桥：127.0.0.1 + 一次性 token
│   │   ├── recording.rs            # 录屏：ffmpeg 分段录制、暂停/恢复、拼接、恢复区
│   │   ├── wgc.rs                  # WGC 采集 POC（D3D11 抓帧，尚未接入录屏）
│   │   ├── ocr.rs                  # OcrEngine 抽象 + 后端起用顺序
│   │   ├── ocr_backend.rs          # PowerShell 后端（回退实现）
│   │   ├── convert.rs              # FFmpeg 格式转换
│   │   ├── monitor.rs              # 系统指标采样
│   │   ├── elevated.rs             # 管理员 helper 握手、UAC 拉起、错误映射
│   │   ├── clipboard.rs            # 剪贴板事件采集 + SQLite 历史 + 独立面板窗口
│   │   ├── input_recorder.rs       # 原生键鼠钩子录制 + enigo 回放
│   │   ├── screenshot.rs           # xcap 捕获与剪贴板直写
│   │   ├── remote.rs               # 远程只读配对服务（回环 + 一次性令牌）
│   │   ├── mcp/main.rs             # orbit-mcp sidecar（stdio JSON-RPC，不链接 Tauri）
│   │   └── helper/main.rs          # orbit-elevated-helper 提权 sidecar（温度常驻 watch）
│   ├── build.rs                    # 图标资源 / tauri-build
│   ├── tauri.conf.json             # 窗口配置、构建目标、NSIS 安装器配置
│   └── capabilities/               # Tauri 能力声明
├── src/                            # Svelte 5 前端
│   ├── App.svelte                  # 悬浮球交互、可编辑扇区圆盘、灵动岛、设置入口
│   ├── app.css                     # 全局样式与窗口尺寸首帧兜底 CSS 变量
│   ├── bus.ts                      # 前端唯一命令入口 runCommand → execute_command
│   ├── OrbGalaxy.svelte            # Canvas 2D 银河旋涡
│   ├── SystemMonitor.svelte        # 系统指标面板
│   ├── DiscSettings.svelte         # 轮盘布局编辑
│   ├── ClipboardPanel.svelte       # 剪贴板历史面板
│   └── main.ts
├── docs/
│   ├── CONTRACTS.md                # 命令/事件/绑定契约文档
│   ├── PERF.md                     # 性能基线口径与结果
│   ├── SAFETY.md                   # 安全边界：两线定性、Meterpreter 对照、负面清单、出网守卫
│   ├── REMOTE.md                   # 远程只读服务边界与协议
│   ├── REMOTE_OCR.md               # 远程 OCR 链路
│   └── ACCEPTANCE-2026-11-13.md    # 验收清单
├── scripts/
│   ├── egress-audit/check-egress.ps1  # 出网出口守卫（CI 不变量）
│   └── perf-baseline/measure.ps1      # P-01/P-02 性能测量
├── .github/workflows/
│   ├── ci.yml                      # PR/主干 CI：单测 + 前端检查 + 绑定漂移 + 出网守卫
│   └── windows-installer.yml       # Windows 安装器 CI
└── .screenshots/                   # 实机截图
```

## 项目架构

五层架构，**快路径与重路径分离**是性能设计的核心：

```text
┌─────────────────────────────────────────────────────────────┐
│  L1 入口层：悬浮球 / 扇区圆盘 / 全局快捷键 / 托盘菜单         │
│     （Tauri WebView，薄 UI，不承载重型逻辑）                  │
└────────────────────────┬────────────────────────────────────┘
                          │ 事件命中
┌────────────────────────▼────────────────────────────────────┐
│  L2 事件绑定层：event + modifier → { command | macro | menu } │
│     bindings.json 持久化（启动期加载与校验回落，改键即时生效）  │
└────────────────────────┬────────────────────────────────────┘
                          │ execute(commandId, params, source)
┌────────────────────────▼────────────────────────────────────┐
│  L3 命令总线：统一注册、路由、权限、审计日志、风险分级           │
│     命令契约层：Rust 类型 → 前端 TS + MCP schema 单一生成源    │
└───────┬─────────────────────────────────┬───────────────────┘
        │                                 │
┌───────▼──────────────┐      ┌───────────▼───────────────────┐
│  L4a 快路径（主进程）  │      │  L4b 重路径（独立管线/子进程）  │
│  截图 │ OCR │ 剪贴板   │      │  录屏（FFmpeg 子进程）              │
│  键鼠监听 │ 窗口查询  │      │  格式转换（FFmpeg 子进程）        │
└──────────────────────┘      └───────────────────────────────┘
                          │
┌────────────────────────▼────────────────────────────────────┐
│  L5 MCP 适配层（独立 sidecar 进程）                           │
│     stdio JSON-RPC → 工具定义 → IPC → 命令总线                │
│     风险分级暴露：只读按用户配置启用，写操作由服务端授权          │
└─────────────────────────────────────────────────────────────┘
```

**模块职责与数据流转：**

- **L1 入口层**：透明置顶悬浮球（点击原位展开扇区圆盘、贴边吸附为状态条/侧边入口），拖动/点击冲突用 4px 阈值 + 双击预判化解。悬浮球大小在编辑页拖动调整并全局统一（含折叠态），三层圆联动撑开不卡手，球/内圆/外圈可到窗口物理极限 136/148/166；折叠态窗口边长随球直径变化（`overlay.set_ball_radius` 同步主进程）。编辑态把 SVG 提到球之上保证手柄可命中，"+"添加功能改由编辑栏按钮承担。灵动岛刘海常驻显示时间、CPU、内存，首次采样就绪前只显示时钟、不渲染占位数字。
- **L2 事件绑定层**：`bindings.json` 持久化事件 × 修饰键 → 命令/菜单/宏，启动期原子加载与校验回落；全局快捷键改为配置驱动，改键即时生效（P-07）。前端右键/滚轮事件已接入 `binding.dispatch`，未绑定则维持原行为。
- **L3 命令总线**：**56 条命令全部走总线**，`invoke_handler` 只注册 `get_command_catalog` 与 `execute_command`，无 Tauri IPC 直连能力；契约来自 `orbit-core::command_catalog` 单一事实源，ts-rs 生成 TypeScript、schemars 生成 JSON Schema，CI 检查绑定漂移。同步处理器跑阻塞线程池，审计异步写入不阻塞响应；执行来源（orb/hotkey/mcp/remote）与被拒调用留痕。
- **L4 快慢路径**：毫秒级操作留在主进程轻量异步；耗时或需要独立权限的操作（录屏、转换）放子进程，崩溃不拖垮主进程、不占 UI 事件循环。
- **L5 MCP 适配层**：`orbit-mcp` 独立二进制（不链接 Tauri），stdio JSON-RPC，工具由 `RiskLevel::ReadOnly` 命令自动派生，写/破坏性命令双重拦截（sidecar 与桥各校验一次），越权调用写审计 `denied`。

**关键设计决策：**

- **单一事实源**：命令参数/返回类型是契约定义，TS 绑定与 JSON Schema 由 Rust 类型生成，生成产物纳入 CI 检查。
- **内存直出**：截图捕获 → RGBA buffer → 编码 → 剪贴板/文件，不经过磁盘中转。
- **惰性窗口**：`clipboard`/`selection` 窗口首次使用时创建，进程族空闲内存从 553 MB 降至 445 MB（见 `docs/PERF.md`）。
- **安全边界**：安装协议前置披露 RAT 同类能力与植入端负面清单；出网出口由 `scripts/egress-audit/check-egress.ps1` 守卫，把「作者监听线为空」变成 CI 不变量（见 `docs/SAFETY.md`）。

## 功能计划

| 阶段 | 功能 | 状态 |
|------|------|------|
| M0/P0 | Tauri 工程、透明置顶悬浮球、贴边吸附、位置保存、拖拽/点击冲突 | ✅ 已实现 |
| M0/P0 | xcap 全屏/区域截图、内存直写剪贴板 | ✅ 已实现 |
| M0/P0 | 命令总线（56 条命令、事件通道、SQLite 审计、风险分级） | ✅ 已实现 |
| M0/P0 | 一键设置与登录启动（HKCU Run，显式可撤） | ✅ 已实现 |
| M1 | 双模式轮盘：球心/光标圆盘、扇区拖拽排序/删除、内圆/外圈/中心球半径调整（1–12 扇区等分；联动撑开不卡手，球/内圆/外圈可到窗口物理极限 136/148/166；折叠态窗口边长随球直径变化，全局统一） | ✅ 球心轮盘已实现，光标轮盘未实现 |
| M1 | 事件绑定层（bindings.json + 6 条 binding.* 命令 + 改键即时生效） | ✅ 已实现 |
| M1 | 剪贴板历史（200 条、文本/文件/图片、密钥跳过、`Alt+V` 唤起） | ✅ 已实现 |
| M1 | 灵动岛仪表盘（时间/CPU/内存/温度，管理员增强按需） | ✅ 已实现 |
| M2 | OCR（WinRT 原生后端 + PowerShell 回退，结构化返回行/词/bbox） | ✅ Windows 已实现 |
| M2 | 键鼠录制与回放（原生低级钩子、enigo 合成、0.1~10 倍速、driftMs 观测；敏感窗口按键自动跳过并计数） | ✅ Windows 已实现 |
| M2 | 本地鼠标控制（`input.mouse`：移动/点击/滚轮，纯校验 + enigo 合成，走总线审计，无网络） | ✅ Windows 已实现 |
| M2 | 只读 MCP sidecar（回环桥 + 一次性 token + 双重白名单 + 审计） | ✅ 已实现 |
| M2 | 远程 OCR（服务端/客户端，Bearer token，结构化返回） | ✅ MVP 已落地 |
| M3 | 录屏（FFmpeg gdigrab 分段录制、暂停/恢复拼接、恢复区） | 🚧 原型可用，原生管线（WGC/Media Foundation）未接入 |
| M3 | 格式转换（FFmpeg：MP3/WAV/MP4/GIF/WebP） | ✅ 已实现 |
| M3 | 主题系统（5 套 CSS 变量预置主题） | 🚧 v1 部分实现，ZIP 主题包未实现 |
| M3 | 远程只读配对（回环、一次性令牌、截图/OCR 白名单） | ✅ MVP 已落地 |
| 待办 | WGC 采集正式接入录屏、音轨、渐进式写盘 | 📋 规划中 |
| 待办 | macOS / Linux 后端（OCR、录屏、键鼠） | 📋 规划中 |
| 待办 | 性能全指标测量（P-03 ~ P-10）与长时稳定性 | 📋 规划中 |

### 扩展功能池与技术选型（候选，尚未实现）

除上方按里程碑排期的功能外，下表记录一组候选扩展能力及其建议的 Rust crate / 方案，作为后续规划池。其中截图 / 录屏 / 系统信息 / 开机自启等与现有能力主题重叠，表中列出的 crate 为可评估的替代或跨平台方案，不代表已采用；其余均为未实现候选。任何涉及出网、远程写操作或音视频采集的能力，落地前须与 `docs/SAFETY.md` 的安全边界（本地执行、只读白名单、无默认出网）对齐。

| 功能 | Rust crate / 方案 | 说明 |
|------|------------------|------|
| 键盘捕获 | `rdev`、`device_query` | 全局键盘 hook |
| 鼠标控制 | `rdev`、`enigo` | 发送鼠标/键盘事件 |
| 屏幕截图 | `screenshots`、`xcap` | 跨平台截屏 |
| 屏幕录制 | `ffmpeg-next` 或调 ffmpeg | 视频流采集 |
| 摄像头 | `nokhwa` | 跨平台摄像头访问 |
| 麦克风录制 | `cpal` | 音频采集 |
| 远程命令执行 | `std::process::Command` | 调用系统 shell |
| 网络通信 | `tokio + axum` / `tokio-tungstenite` | 异步 WebSocket/HTTP |
| 加密传输 | `rustls`、`ring`、`aes-gcm` | 端到端加密 |
| 系统信息 | `sysinfo` | 进程/系统状态 |
| 开机自启 | `auto-launch` | 自启动管理 |
| 托盘图标 | `tray-icon`、`tao` | 系统托盘 |
| 文件传输 | `tokio::fs` | 远程文件读写 |

**性能基线（2026-11-18，Windows x64 实测）：**

| 指标 | 目标 | 实测 | 结论 |
| --- | --- | --- | --- |
| P-01 冷启动（窗口就绪代理） | < 500 ms | p50 **344 ms** / p95 356 ms | ✅ 达标（热缓存） |
| P-02 空闲内存（主进程） | < 60 MB | **35.5 MB** | ✅ 达标 |
| P-02 空闲内存（进程族） | < 60 MB | **445 MB** | ⚠️ WebView2 底座成本，目标需重定口径 |

P-03 ~ P-10 尚无自动化测量，不得据本表推断达标。完整口径与数据见 `docs/PERF.md`。

## 开发指南

```bash
# 安装依赖
npm install

# 开发模式（前端 + Tauri 桌面）
npm run tauri:dev

# 前端检查与构建
npm run check
npm run build

# 桌面端单测（默认纯单测，不触实机）
cargo test --workspace

# 实机冒烟（需要 ffmpeg/ffprobe，--features smoke）
cargo test -p orbit-desktop --lib --features smoke

# 出网出口守卫（作者监听线为空的 CI 不变量）
powershell -NoProfile -ExecutionPolicy Bypass -File scripts/egress-audit/check-egress.ps1

# 构建 Windows 安装包
npm run tauri:build
```

> **构建方式约束**：验证 release 产物必须走 Tauri 打包脚本（`npm run tauri -- build --no-bundle`）；直接 `cargo build --release` 产出的 exe **未内嵌前端资源**，启动后 WebView2 会去连 `localhost:1420` 并显示"无法访问此页面"。
>
> **实机冒烟**：从自动化终端用 `Start-Process` 启动的进程会在该条命令结束时被回收（表现为"启动后自行退出"），这不是应用缺陷。手动验证请用 `explorer.exe <exe 路径>` 脱离启动。

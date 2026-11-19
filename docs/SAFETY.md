# Orbit 安全边界（SAFETY）

> 状态：2026-11-19 首版。本文是 README §9 的展开与落地文档，与
> `src-tauri/license-agreement.txt`（安装协议）和 `docs/REMOTE.md` §3
> （明确不做清单）三处互为引用，修订需同步更新三处。
>
> 维护者：安全负责人（README §11 文档体系分配）。

## 1. 两线定性：用户功能线与作者监听线

对一切 RAT（Remote Administration Tool）同类软件，判定其性质的不是
「有哪些能力」，而是**第二条线握在谁手里**：

| | 用户功能线 | 作者监听线 |
| --- | --- | --- |
| 定义 | 设备所有者可见、可配置、可审计的能力 | 向作者/第三方发送数据或接受其指令的通道 |
| Orbit | 全部 49 条命令（见 docs/CONTRACTS.md） | **空——没有任何代码路径** |

功能性能力（键鼠钩子、屏幕捕获、剪贴板监听、OCR、键鼠回放）属于用户线：
它们的存在使 Orbit 与攻击性工具**共享能力面**，这是固有属性（README §9.4
已如实声明），但能力面的方向决定了性质——功能线 + 作者持有远端 =
间谍软件；功能线 + 用户持有远端 = 管理工具。

### 1.1 作者线为空的代码级证据（2026-11-19 grep 快照）

1. **无作者侧地址**：全部 `.rs` 源码中不存在任何硬编码域名、公网 IP 或
   云端配置拉取；出网地址只能由用户在运行时显式提供。
2. **无 DNS 解析能力**：`to_socket_addrs` / `lookup_host` /
   `gethostbyname` / `DnsQuery` 全仓零命中；`remote.rs` 的
   `parse_endpoint` 解析字面 `SocketAddr` 且强制 `is_loopback()`，
   域名过不了类型这一关（remote.rs 测试断言 `example.com` 被拒）。
3. **监听仅回环两处**：MCP 桥（`mcp_bridge.rs`，`Ipv4Addr::LOCALHOST`
   + 一次性 token）与远程只读配对（`remote.rs`，`LOOPBACK_HOST` 常量
   + 默认关闭）；无 `0.0.0.0` 分支，无 NAT 穿透。
4. **出站连接全部用户显式配置**：远程 OCR 客户端的 `--server` 参数、
   远程配对端点（且只接受回环，跨机需用户自建 VPN/SSH 隧道）。
5. **无遥测/更新/崩溃上报**：`tauri = { features = [] }` 未启用
   updater/http 插件；无 sentry/telemetry 类依赖；前端 `fetch`/
   `XMLHttpRequest`/`WebSocket`/`sendBeacon` 零命中。
6. **Cargo.lock 里的 `reqwest` 是 Tauri 的传递依赖**，本项目四个 crate
   无一调用它；若未来启用 `tauri-plugin-updater` 或 http feature，
   出网能力会在不出现「作者线式代码」的情况下诞生——这正是 §4 守卫存在的理由。

## 2. 控制面：控制长在架构里，不是使用纪律

| 控制机制 | 代码事实 |
| --- | --- |
| 单一执行路径 | `invoke_handler` 仅 `execute_command`；全部能力过 bus.rs 注册表，绕过路由/校验/审计的第二入口不存在 |
| 风险分级挂在单一事实源 | `RiskLevel` 是 `CommandSpec` 字段；MCP 工具从命令目录自动派生（`orbit_core::read_only_tools()`），新增命令忘分类时目录本身就是控制面 |
| 双重白名单 | sidecar 校验一次，主进程桥再校验一次（`is_exposed`），绕过 sidecar 也拿不到写命令 |
| 远程能力面封闭枚举 | `remote.rs` 的 `kind` 只匹配 `screenshot` 与 `ocr` 两个字面量，无「任意命令」分支 |
| 默认关闭 + 一次性令牌 | `RemoteState` 初始为 `None`；令牌只在内存、常数时间比较；进程退出即 `stop_server` |
| 全程审计 | 每次调用带 `source`（orb/hotkey/mcp/remote）写 SQLite；**被拒绝的调用也留痕**（`record_denied`） |
| 运行时可见 | 远程服务运行时悬浮球 `R` 角标常驻，随时可停 |
| 持久化仅 HKCU Run 一项 | tauri-plugin-autostart，设置页显式开关，可移除 |

已知薄弱点（诚实清单，加固优先级待排）：

1. **令牌文件边界是 OS 用户账户**：`mcp-bridge.json` 在 `app_data_dir`，
   同用户任何进程都能读令牌连桥拉只读命令（CONTRACTS.md 已声明
   "not a multi-tenant auth scheme"）。同用户进程本可直读
   `clipboard.db`，故非新增暴露面。
2. **静态数据无额外保护**：`recordings.db`（完整击键历史）、
   `clipboard.db`（剪贴板历史）明文落盘；机器账户被攻破即凭据金矿。
   候选加固：at-rest 加密。
3. **`kb.replay` 无执行前确认**：不进 MCP（已防提示注入路径 R-07），
   但本地 UI 触发也无二次确认；「可控」目前依赖「触发 UI 只在用户手上」。
4. **回环协议无 TLS**：跨机配对依赖用户自建隧道（文档已声明）；
   属使用纪律而非代码保证。

## 3. Meterpreter 命令对照（负面清单证据）

以 Meterpreter 标准命令表为镜，Orbit 能力面重叠 4 项且语义全部相反
（缓冲区所有者与数据流向不同）；其余 10 项原语**零命中**，逐条 grep
证据（2026-11-19，`grep -r <原语> --include=*.rs` 全部 0 matches）：

### 3.1 能力重叠但语义相反（4 项）

| Meterpreter | Orbit | 语义反转轴 |
| --- | --- | --- |
| `keyscan_start/stop/dump` | `kb_record.start/stop` + `kb_record.list` | Meterpreter 缓冲等远端拉取；Orbit 录制写入用户自己的 SQLite，无「另一个方向」的拉取通道 |
| `screenshot` | `screenshot.capture` | Meterpreter 像素回传 operator；Orbit 直写本机剪贴板，用户立即可见 |
| `keyevent/mouse/keyboard_send` | `kb.replay` | Meterpreter 输入来自远端；Orbit 回放由本机 UI 触发，且 MCP 白名单显式排除 `kb.replay` |
| `shell` | `convert.format`（同 spawn 子进程） | Meterpreter 接受任意命令行；Orbit 子进程仅 ffmpeg/powershell/ffprobe 三个固定程序名，参数由代码构造 |

### 3.2 零命中原语（10 项，grep 快照）

| Meterpreter 命令 | 对应原语 | grep 结果 |
| --- | --- | --- |
| `record_mic` | `waveIn`/`waveOut`/`IAudioClient` | 0 |
| `webcam_snap/stream/chat/list` | `avicap`/`capCreate`/Media Foundation 视频捕获 | 0 |
| `enumdesktops/getdesktop/setdesktop` | `EnumDesktops`/`OpenInputDesktop`/`SwitchDesktop`/`SetThreadDesktop` | 0 |
| `reboot/shutdown` | `ExitWindowsEx`/`InitiateSystemShutdown`/`SetSuspendState` | 0 |
| `getsystem` | 令牌窃取类 `DuplicateTokenEx`；唯一提权是 UAC 弹窗 + 用户确认 | 0 |
| `uictl` | `BlockInput` | 0 |
| `idletime` | `GetLastInputInfo` | 0 |
| `play` | `mciSendString`/`PlaySound`/winmm | 0 |
| `execute -f notepad` | 任意进程启动；子进程白名单固定 3 个程序名 | 0 |
| 注入类（注入/迁移/反射） | `CreateRemoteThread`/`WriteProcessMemory`/`VirtualAllocEx`/`NtCreateThreadEx`/`QueueUserAPC`/`LoadLibrary*`/`GetProcAddress`/`VirtualProtect`/`MapViewOfFile` | 0 |

### 3.3 Orbit 有而 Meterpreter 没有的（全部是「控制」而非「侵入」）

`clipboard.list/read/count`（密钥过滤）、`audit_log` + source 字段、
`binding.*` 事件绑定编辑器、`elevated.status`（意图≠存活）、MCP 只读
白名单、远程配对白名单——RAT 不给受害者看采集了什么，Orbit 给。

## 4. 出网出口守卫（CI 不变量）

`scripts/egress-audit/check-egress.ps1` 把「作者线为空」变成机器检查：

- **枚举**全部 `.rs` 中的 `TcpListener::bind` / `TcpStream::connect` /
  `UdpSocket::*` / DNS 解析调用点，与白名单（9 条，各注明绑定方向与
  用途）比对；**未登记出口即 CI 失败**。
- **负面清单**：§3.2 全部原语任何代码行命中即失败，无豁免。
- **白名单保鲜**：登记行号若不再是出口调用（行号漂移）也失败，防止
  守卫失明。
- 已接入 `.github/workflows/ci.yml` 的 `egress-guard` job（push/PR 均跑）。

新增出口的流程：先在脚本白名单登记（写明绑定方向：仅回环 / 用户显式
配置，与用途），再过 PR 公开评审——**那一刻就是出网能力诞生并被公开
质询的时刻**。

> 脚本为 ASCII-only：PowerShell 5.1 读无 BOM 的 UTF-8 会按 ANSI 解析，
> CJK 注释行尾字节可能吞掉换行符把下一行代码吃进注释（本仓库实测踩过）。

## 5. 已知限制与不做清单

- MCP 仅 stdio transport；HTTP transport 若引入须绑定 127.0.0.1 + 认证
  并单独评审（README §1.2）。
- 远程服务无 TLS，跨机必须走用户自建 VPN/SSH 隧道（docs/REMOTE.md §7）。
- `docs/REMOTE.md` §3「明确不做」清单（stager、反向 shell、凭据抓取、
  公网监听、键鼠注入远程化等）与 license-agreement.txt 第二节负面清单
  同源，均为「不提供接口、不写文档步骤」的硬边界。
- 本文档的 grep 证据是时点快照；机器化的部分由 §4 守卫持续保证，
  其余（如 ffmpeg PATH 解析的用户环境信任）属用户线内部风险，另行评估。

## 6. 修订记录

- 2026-11-19：首版。两线定性、Meterpreter 对照、负面清单 grep 证据、
  出网守卫（check-egress.ps1 + CI 接入）。

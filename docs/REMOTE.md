# 远程只读服务（REMOTE）

> 状态：2026-11-18 MVP。默认关闭，只绑定回环地址，按需启动。
> 目标：在两台 Orbit 之间完成**可见、可授权、可审计**的远程截图与远程 OCR。
> 明确不是 Meterpreter 替代品。

## 1. 能做什么

| 能力 | 命令 | 说明 |
|---|---|---|
| 启动服务 | `remote.serve.start` | 绑定 `127.0.0.1:0`，返回端口与一次性令牌 |
| 停止服务 | `remote.serve.stop` | 关闭监听，令牌立即失效 |
| 查询状态 | `remote.status` | 是否运行、端点、当前会话数；不返回令牌 |
| 远程截图 | `remote.screenshot` | 远端抓一帧 PNG，写入**本机**剪贴板，返回尺寸 |
| 远程 OCR | `remote.ocr` | 远端本地 OCR，只回传 `text + lines + bbox` |

配套：

- 事件 `remote://state`：启动/停止时刷新前端状态；
- 前端设置页“远程只读服务”区块：启动/停止、显示端点与令牌、填写对方端点/令牌、远程截图/OCR；
- 悬浮层右上角 `R` 角标：服务运行时常驻可见；
- 审计：远端每次请求写 `audit_log`，`source = remote`，命令 id 为 `remote.screenshot` / `remote.ocr`。

## 2. 安全边界

### 已实现

- **默认关闭**：不随应用启动，不加开机自启，不写注册表。
- **只绑定回环**：服务端固定 `127.0.0.1`，控制器也只接受回环端点。
- **一次性令牌**：每次启动生成 256-bit 随机令牌，只存在内存中；停止或退出即失效。
- **能力白名单**：服务端只认 `screenshot` 和 `ocr` 两种请求，其他一律拒绝。
- **审计**：每次请求记录来源、能力、显示器、结果与耗时；参数只存哈希。
- **可见**：服务运行时悬浮层有 `R` 角标，设置页可随时停止。
- **不暴露给 MCP**：`remote.screenshot` / `remote.ocr` 刻意标记为 `Mutating`，不会进入只读 MCP 工具列表。

### 跨机访问方式

公司电脑与家庭网络之间不要做公网端口映射。推荐：

1. 公司 VPN 已连通时，在家庭电脑建立到公司电脑的隧道；
2. 或使用公司允许的 SSH 隧道，把公司电脑的回环端口映射到家庭电脑回环：

```text
ssh -N -L 47100:127.0.0.1:47100 user@company-pc
```

然后在家庭 Orbit 中填写 `127.0.0.1:47100` 与公司 Orbit 显示的配对令牌。

> 服务本身不提供 NAT 穿透、中继、UDP 打洞或公网监听；这是刻意设计，避免把 Orbit 变成可被公网扫描的远控端。

## 3. 明确不做

以下能力**不实现、不提供接口、不写文档步骤**：

| 类别 | 具体项 |
|---|---|
| 载荷与加载 | stager / stage、反射式 DLL 注入、进程注入、内存加载、无文件执行 |
| 持久化 | 注册表 Run 键、计划任务、服务驻留、开机静默自启、隐藏进程 |
| 提权 | `getsystem`、UAC 绕过、令牌窃取、SYSTEM 提权 |
| 凭据 | 键盘记录、`hashdump`、mimikatz/kiwi、浏览器/聊天凭据抓取 |
| 采集 | 摄像头、麦克风、屏幕静默录制、剪贴板静默同步 |
| 控制 | 任意 shell、远程命令执行、键鼠注入（当前 MVP 未开放） |
| 横向 | SMB 传输、端口转发、内网扫描、pivoting |
| 对抗 | 免杀、EDR/杀软绕过、日志清除、rootkit |
| 网络 | 公网监听、NAT 穿透、隐蔽 C2、多传输切换 |

如果后续要加“远程键鼠控制”，必须单独做权限模型、可见提示和审计，不能默认开启。

## 4. 协议

传输：TCP，仅回环。

帧格式：4 字节大端长度 + JSON，请求上限 16 KiB，响应上限 64 MiB。

请求：

```json
{
  "token": "<64 位十六进制>",
  "kind": "screenshot",
  "monitorId": null
}
```

响应：

```json
{ "ok": true, "data": { "width": 1920, "height": 1080, "pngBase64": "..." } }
```

或：

```json
{ "ok": false, "error": "配对令牌无效。" }
```

OCR 的 `data` 为 `OcrResult`：

```json
{
  "text": "...",
  "lines": [{ "text": "...", "words": [{ "text": "...", "bbox": [0, 0, 10, 10] }] }],
  "textAngle": null
}
```

## 5. 实现位置

| 文件 | 职责 |
|---|---|
| `crates/orbit-core/src/contract.rs` | `RemoteStartParams` / `RemoteEndpoint` / `RemoteStatus` / `RemoteRequestParams` / `RemoteScreenshotResult`、5 条命令、`remote://state` 事件 |
| `src-tauri/src/remote.rs` | 回环 TCP 服务、令牌、帧协议、截图/OCR 能力、审计、客户端请求 |
| `src-tauri/src/screenshot.rs` | 抽出 `capture_image()`，本地截图与远程截图共用 |
| `src-tauri/src/bus.rs` | 注册 `remote.*` 命令 |
| `src/App.svelte` / `src/DiscSettings.svelte` | 设置页操作、令牌显示、远程截图/OCR、运行角标 |
| `src/app.css` | 远程区块与角标样式 |

## 6. 验证

```powershell
npm run check
cargo check --workspace
cargo test -p orbit-core
cargo test -p orbit-desktop --lib
```

手动验证（同一台机器即可）：

1. 打开 Orbit 设置 → 启动“远程只读服务”；
2. 复制端点与令牌到下方输入框；
3. 点“远程截图”，剪贴板应得到当前屏幕；
4. 点“远程 OCR”，设置页应显示识别文本；
5. 点“停止”，角标消失，旧令牌请求返回“配对令牌无效”。

## 7. 已知限制

- 只支持单帧截图，没有持续屏幕流；
- 没有远程键鼠控制；
- 没有文件传输、剪贴板同步；
- 锁屏/安全桌面下无法抓屏；
- 依赖公司 VPN/SSH 隧道，服务本身不做 NAT 穿透；
- 令牌只存内存，应用重启后需要重新配对；
- 当前协议未做 TLS，安全性依赖回环 + VPN/SSH 隧道；如需跨不可信网络，应在隧道层加密。

## 8. 合规前提

仅用于**本人管理、明确授权**的设备。公司电脑需取得 IT/安全团队许可，并遵守公司数据与远程访问政策。远程屏幕可能包含客户信息、代码、凭据，默认只回传 OCR 文本，截图仅在需要时按次抓取。

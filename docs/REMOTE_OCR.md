# 远程 OCR（orbit-ocr-server / orbit-ocr-client）

> 2026-11-18 落地 MVP。这条链路只做一件事：把图片变成结构化文字。
> 不提供 shell、文件传输、进程控制或任何通用远程执行能力——需要那种能力
> 的场景不属于 Orbit，本文档也不会涉及。

## 数据流

```text
客户端机器（orbit-ocr-client）            服务端机器（orbit-ocr-server）
────────────────────────────            ────────────────────────────
xcap 截取主显示器 → PNG 字节
        │
        ▼
POST /v1/ocr  (Authorization: Bearer <token>, body = PNG bytes)
        │                                  1. 校验 token（常数时间比较）
        │                                  2. 校验 Content-Length ≤ 32 MiB
        │                                  3. 落盘 <data-dir>/images/<unix_ms>.png
        │                                  4. orbit_ocr::recognize(path)
        │                                     （原生 WinRT 优先，PowerShell 回退）
        ▼
{"ok":true,"image":"...png","result":{"text":"...","lines":[...],"textAngle":...}}
```

- 客户端是**显式运行**的一次性命令行工具：不常驻、不隐藏、不注册自启。
- 服务端只暴露两个路由：`GET /health` 与 `POST /v1/ocr`；其余 404。
- 被拒绝的请求（token 错误、超限、路由错误）**不落盘**（e2e 断言）。

## 使用

```powershell
# 服务端（生成并打印一次性 token）
orbit-ocr-server --bind 127.0.0.1:8788 --data-dir D:\ocr-data

# 客户端：截当前桌面
orbit-ocr-client --server 127.0.0.1:8788 --token <64hex> --capture

# 客户端：上传本地图片
orbit-ocr-client --server 127.0.0.1:8788 --token <64hex> --image shot.png
```

token 也可用环境变量 `ORBIT_OCR_TOKEN` 传递，避免进入 shell 历史。

## 实测（2026-11-18，本机回环）

| 场景 | 结果 |
| --- | --- |
| 端到端冒烟（合成 "ORBIT 123" 图） | 200 + 结构化结果；落盘字节与上传一致 |
| 错误 token | `ok:false`，提示未授权；`images/` 目录为空 |
| 真实截屏上传（579 KB PNG） | 往返 691 ms，返回 124 行结构化文本 |

## 安全边界与部署建议

1. **认证**：Bearer token，常数时间比较；同一 token 只给可信客户端。
2. **传输**：MVP 为明文 HTTP/1.1（零依赖实现）。跨公网部署必须放在 TLS
   终端的反向代理后面（Caddy / nginx），或在 VPN / SSH 隧道内使用。
3. **限额**：单请求正文上限 32 MiB；超出返回 413 且不落盘。
4. **隐私**：截屏内容可能包含敏感信息。上传前应确认接收方有权限保存与
   处理这些图片；服务端保存的图片请按需定期清理（`images/` 目录）。
5. **不做的事**：不接受任意命令、不回传目录列表、不执行服务端脚本、
   不做进程注入/持久化。协议白名单只有 `/health` 与 `/v1/ocr`。

## 与应用内 remote 服务的区别

| | `src-tauri/src/remote.rs`（应用内） | `orbit-ocr-server`（独立服务端） |
| --- | --- | --- |
| 模式 | **拉**：外部调用方连上 Orbit，请求它截屏/OCR | **推**：客户端主动上传图片到服务端 |
| 绑定 | 仅 127.0.0.1（跨机需 VPN/SSH 隧道） | 可绑任意地址（建议放 TLS 代理后） |
| 数据流 | 调用方拿走截图/OCR 结果 | 图片由客户端推给服务端，服务端落盘 |
| 适用 | 本机自动化 / 同机集成测试 | 远程机器识别，NAT 后可用（出站连接） |

两者共用同一 OCR 引擎（`crates/orbit-ocr`），结果语义一致。

## 与 MSF / Meterpreter 的关系

借鉴的只有**会话生命周期机制**：握手、鉴权、心跳、断线重连与退避、
任务队列与回执、限流与吊销。这些机制服务于"OCR-only"通道。
不借鉴、也不实现：stager/stage 加载、进程注入/迁移、内存反射执行、
任意命令会话、文件系统漫游。能力边界由协议固定，而不是靠使用者自觉。

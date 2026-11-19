# 性能基线（PERF）

> 口径：可自动化、可复现的代理指标。基线报告必须注明设备、系统、采样次数与统计方式。
> 当前脚本覆盖 P-01（冷启动）与 P-02（空闲内存）；P-03 ~ P-10 尚无自动化测量。

## 如何测量

```powershell
# 需要 release 产物（内嵌前端资源；cargo 裸构建的 exe 会被脚本告警）
npm run tauri -- build --no-bundle

# 默认 5 轮、窗口出现后空闲 8s、内存采样 5 次
powershell -NoProfile -File scripts/perf-baseline/measure.ps1

# 归档 JSON
powershell -NoProfile -File scripts/perf-baseline/measure.ps1 -Json -OutFile docs/perf/baseline-YYYY-MM-DD.json
```

脚本行为：先拒绝在已有实例运行时测量（`-Force` 可强制结束）；每轮启动 exe，测「进程启动 → 主窗口句柄非零」，然后空闲等待并采内存；结束即杀进程；最后校验 exe 是否内嵌 `assets/index-`（README 的产物判定法）。

## 测量口径与局限

- **P-01 冷启动 = 进程启动到主窗口句柄就绪**。这是「悬浮球可点击」的可自动化代理：WebView 首帧内容就绪没有外部可观测信号，因此真实可点击延迟**略高于**本值。首轮含磁盘冷缓存，p95 会显著高于 p50；报告需同时给出两者。
- **P-02 两组数字**：`main` = `orbit-desktop.exe` 自身 WorkingSet；`tree` = 该进程族（含 WebView2 子进程）WorkingSet 之和，1s 间隔采样取最后一次。
- WorkingSet 会受页面活动、GPU 进程、系统内存压力影响；跨机器比较必须在同机、同分辨率、同系统版本下进行。

## 2026-11-18 基线（惰性窗口改造前）

设备：Windows NT 10.0.26200（x64），exe 15.34 MiB，内嵌资源校验通过，5 轮。
原始数据：[docs/perf/baseline-2026-11-18.json](perf/baseline-2026-11-18.json)。

| 指标 | 目标 | p50 | p95 | 结论 |
| --- | --- | --- | --- | --- |
| P-01 冷启动（窗口就绪代理） | < 500 ms | **344 ms** | **356 ms** | 达标（热缓存；首轮冷缓存 1359 ms 已单列观察） |
| P-02 空闲内存（主进程） | < 60 MB | **35.5 MB** | — | 达标 |
| P-02 空闲内存（进程族） | < 60 MB | **553 MB** | 554 MB | **严重超标（约 9×）** |

> 目标值来自 README「硬性指标」表，属于待校准的初始目标；本次测量表明 P-02 的「进程族 < 60 MB」在 WebView2 架构下不可达，需要重新定义口径，而不是继续按旧数字宣传。

### 诊断：超标来自 WebView2 的多窗口渲染进程

单次空闲（12s）进程族构成：

| 进程 | WorkingSet |
| --- | --- |
| orbit-desktop（主进程） | 35.5 MB |
| msedgewebview2 × 8 | 21.8 – 175.6 MB（合计 ≈ 518 MB） |
| 合计 | ≈ 553.6 MB |

`tauri.conf.json` 在启动时创建了 **三个 webview 窗口**（`main` / `clipboard` / `selection`），每个窗口各自拉起渲染进程，外加 GPU / network / utility 等共享子进程。主进程本身已达 Tauri 的预期区间，超标全部来自「三个窗口都常驻」。

### 后续动作（按收益排序）

1. ~~**延迟创建 `clipboard` / `selection` 窗口**~~ —— **2026-11-18 已完成**，见下节；实测节省约 108 MB（553 → 445 MB），低于预估的 100–150 MB 上限，因为 WebView2 固定底座（browser / GPU / utility 进程）与窗口数无关。
2. **降低空闲渲染开销**：空闲时暂停或降频银河 Canvas（`requestAnimationFrame`），减少渲染进程与 GPU 进程常驻。
3. **重新定义 P-02 口径**：主进程 35 MB 达标；进程族在 WebView2 架构下 ≈ 400 MB 属于底座成本。若要满足「进程族 < 60 MB」，只能放弃 WebView2（不符合技术选型）或改口径为「主进程 < 60 MB + 进程族参考值」。
4. 在 `docs/PERF.md` 记录 P-03（轮盘弹出）、P-04（截图→剪贴板）、P-07（绑定改键即时生效）的测量脚本与结果。

## 2026-11-18 惰性窗口改造后

改动：`tauri.conf.json` 的 `clipboard` / `selection` 窗口加 `"create": false`，`src-tauri/src/windows.rs` 在首次使用时按同一份窗口配置经 `run_on_main_thread` 创建（窗口配置保留在 `app.windows`，`layout_config` 的尺寸单一事实源不变）。

原始数据：[docs/perf/baseline-2026-11-18-lazy-windows.json](perf/baseline-2026-11-18-lazy-windows.json)，5 轮。

| 指标 | 改造前 | 改造后 | 变化 |
| --- | --- | --- | --- |
| P-01 冷启动 p50 | 344 ms | **345 ms** | 无变化 |
| P-02 空闲内存（进程族）p50 | 553 MB | **445 MB** | **-108 MB（-20%）** |
| 启动时 WebView2 子进程数 | 8 | 7 | -1（另 2 个渲染进程延后到首次使用） |

行为验证（实机，2026-11-18）：启动后 `msedgewebview2` 子进程数为 7；发送 `Alt+V` 后子进程数变为 8，窗口列表出现「Orbit 剪贴板」——说明惰性创建与快捷键链路正常。区域选区的 `selection` 窗口走同一条 `ensure()` 路径，但未做 UI 交互验证，首次使用需实机确认。

剩余结论：进程族仍在 445 MB 量级，其中 WebView2 固定底座占大头（单窗口空闲时仍有 7 个 `msedgewebview2` 进程，最大者 ≈165 MB）。继续压缩空间有限，P-02 的「进程族 < 60 MB」目标应与产品口径一起重定。

回放功能合入后的回归（2026-11-18，3 轮）：P-01 p50 363 ms、P-02 p50 448 MB（exe 15.83 MiB），相对惰性窗口改造无退化。

## OCR 延迟（2026-11-18，非 P-指标，作为 P-04 前置参考）

同一张 800×200 合成图（`ORBIT 123`），`cargo test --features smoke` 内计时：

| 后端 | 耗时 | 说明 |
| --- | --- | --- |
| 原生 WinRT（`ocr_native.rs`） | **69 ms** | `windows` crate 直连 Windows.Media.Ocr，无子进程 |
| PowerShell（旧实现，现为回退） | **1792 ms** | 每次识别 spawn `powershell.exe` + 反射 AsTask |

经 MCP 端到端（`orbit-mcp` 冷启动 + 回环桥 + OCR）：首次 397 ms，热态 **42 ms**。两个后端对同一图片的词数与 bbox 逐词一致（冒烟测试断言）。这解释了为什么"OCR 慢"此前主要来自进程启动而不是识别本身。

## WGC 采集 POC（2026-11-18，release 构建）

目的：验证替换 ffmpeg gdigrab 的可行性（不装 FFmpeg、可排除自身窗口、原生暂停）。POC 在 `src-tauri/src/wgc.rs`，**尚未接入 `record.*`**。

| 指标 | 结果 |
| --- | --- |
| 抓主显示器一帧（D3D 设备 + 会话 + 首帧 + 回读 RGBA） | 首次 264 ms；后续每次新建会话 81–95 ms |
| 同帧 PNG 编码（`image` crate，仅 POC 用） | 11 ms |
| 帧尺寸 | 与 `xcap` 枚举的主显示器尺寸一致（冒烟断言） |
| 自身窗口排除 | `SetWindowDisplayAffinity(hwnd, WDA_EXCLUDEFROMCAPTURE)` 读写回环通过（Win10 2004+ API） |

注意：debug 构建下同一 POC 单帧 860–1133 ms，差异来自未优化的 BGRA→RGBA 逐像素循环——**不要用 debug 数字评估采集成本**。正式录屏应保持单一 D3D 设备与会话、把纹理直接交给 Media Foundation（NV12 + 硬编），不落 PNG；届时单帧成本远低于这里的"每次新建会话"数字。

缺口（POC 未覆盖）：连续帧节奏（WGC 按内容变化出帧，静态桌面会稀疏，编码器需补帧）、Media Foundation H.264、WASAPI 音频、区域裁剪、与暂停时间轴的对接。

## 已知未测项

P-03 轮盘弹出延迟、P-04 截图→剪贴板、P-05 键鼠录制抖动、P-06 MCP 调用开销、P-07 绑定生效、P-08 主题首帧、P-10 长时录制内存增长——均无自动化测量，不得以本文件的 P-01/P-02 结果推断它们达标。

**P-09 键鼠回放漂移**：已有观测手段——`kb.replay` 返回 `driftMs`（实际耗时 − 录制时间轴），设置面板回放后会显示该值；实机短序列冒烟断言 < 60 ms。但「30 秒录制、< 20 ms/10s」的正式测量尚未执行，需要一条真实 30 秒录制 + 多次回放脚本（可复用 `kb.replay` 的返回字段）。

**P-06 MCP 工具调用开销**：链路已可用（`orbit-mcp` → 回环桥 → 命令总线，`CommandResponse.durationMs` 含执行时间），但「IPC 往返 < 50 ms（不含执行）」的正式测量尚未做；需要一次只读 `tools/list` 或空参命令的往返计时脚本。

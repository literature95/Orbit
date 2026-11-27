# 格式转换框架

Orbit 的格式转换不再是一张硬编码的格式白名单：引擎只声明**直接转换能力**，
多跳路由由 `RoutePlanner` 在格式图上用 Dijkstra 自动推导。这个设计借鉴了
Filee 的转换子系统（引擎 → 边 → 路由 → 队列）。

## 分层

```
src-tauri/src/conversion/
  formats.rs             格式注册表：id / 分类 / 扩展名别名 / 常用度
  model.rs               ConversionEdge / Converter / ConversionStep / ConverterStatus
  engine/ffmpeg.rs        FFmpeg 引擎：视频抽帧、音视频转码、图片转动图（真实编码参数）
  engine/image.rs         Rust image 引擎：png/jpg/webp/bmp/tiff/tga/gif 互转
  registry.rs             ConverterRegistry：能力发现唯一入口
  planner.rs              RoutePlanner：Dijkstra 最短路 + 独占边过滤 + 3 跳上限
  output.rs               JobAllocator：路径预占 + 归属标记 + 失败回滚 + 工作目录
  process.rs              外部进程运行器：超时 + 停滞检测 + 取消 + 进程树清理
  job.rs                  异步任务状态 / 取消令牌 / 任务注册表
src-tauri/src/convert.rs  命令总线门面：formats / plan / format / start / cancel / status
```

## 命令

| 命令 | 风险 | 说明 |
|---|---|---|
| `convert.formats` | ReadOnly | 列出「常用且可到达」的目标格式（圆盘扇区）；可被 MCP 安全暴露 |
| `convert.plan` | ReadOnly | 输入路径 + 目标格式 → 路由预览（`{ from, to, steps, cost }`） |
| `convert.format` | Mutating | 同步执行，返回最终输出路径 |
| `convert.start` | Mutating | 异步启动，立即返回任务快照（`ConvertJobStatus`） |
| `convert.cancel` | Mutating | 按 jobId 置取消令牌，后台任务在检查点停下并回滚 |
| `convert.status` | ReadOnly | 按 jobId 查询状态 / 进度 / 输出 / 错误 |

状态变化通过 `convert-state` 事件回流，载荷为 `ConvertJobStatus`。

## 真正的多跳路由（已实现）

两个引擎的边合起来形成一张图，规划器用 Dijkstra 找最便宜路径：

```text
mp4 → png   （FFmpeg 抽帧）
png → tiff  （image 引擎）
----------------------------
mp4 → tiff  = 2 跳，自动推导，没有任何引擎单独声明过这条边
```

类似还有 `mov → png → bmp`、`mp4 → png → tga`。`convert.plan` 可直接预览这些路由。
新增一个引擎就能自动解锁一片新格式对——这是 Filee 设计的核心价值。

## 格式注册表与进程健壮性（已实现）

- `formats.rs` 注册 19 种格式（图片/视频/音频），支持扩展名别名（`jpeg→jpg`、`tif→tiff`、`m4v→mp4`）
- `convert.formats` 只返回常用且可到达的目标，保证圆盘扇区数量可控；其余格式仍可规划
- `ProcessRunner` 给 FFmpeg 加 10 分钟超时 + 60 秒停滞检测 + 取消令牌 + 进程树 kill，卡住不会拖垮任务
- FFmpeg 输出按格式走真实编码参数（`libmp3lame` / `pcm_s16le` / `flac` / `libx264` / `fps` 滤镜等），不是改后缀

## 输出安全（P1 已实现）

`conversion/output.rs` 的 `JobAllocator` 负责所有输出路径：

- **带锁预占**：并发任务不会拿到同名；已有文件追加 ` (2)` / ` (3)`，**永不覆盖用户输入**
- **归属标记**：记录路径分配前是否存在；失败/取消只删除本次独占的文件，用户原有文件保持不动
- **工作目录隔离**：中间文件写入 `app_data_dir/convert/<job-id>/`，任务结束回收自建空目录
- **成功才提交**：`finish()` 释放预占并保留输出；`discard()` 回滚全部独占路径

## 设计约束

- **单一事实源**：命令契约在 `orbit-core::command_catalog`，TS 绑定与 JSON Schema 由 Rust 类型生成，CI 检查漂移
- **不碰输入文件**：引擎只读 `step.input_path`，输出只写 `step.output_path`
- **外部程序统一解析**：总线用 `recording::resolve_ffmpeg` 解析路径，引擎通过 `step.tool_path` 使用，不各自找程序
- **无窗口**：外部进程统一走 `NoWindow` trait（Windows `CREATE_NO_WINDOW`）
- **本地执行**：无网络、无 shell、无路径注入；参数由代码构造

## 前端接入（已实现）

两种方式进入同一流程：点击圆盘「格式转换」扇区选文件，或**把文件直接拖到悬浮球/圆盘**
（Tauri 原生 `onDragDropEvent`，`dragDropEnabled` 默认开启，无需额外 capability）。
拖入时圆盘立即切换为目标格式扇区（MP3/WAV/MP4/GIF/WebP…），末尾追加「取消」扇区；
**拖拽悬停到哪个扇区就高亮哪个，松手即转换到该格式或取消**，不再有「松手转换」文字提示。
点击方式则是点格式扇区 → 选保存路径。两者都走 `convert.start` 异步启动 →
监听 `convert-state` 显示进度/结果；点「取消」扇区或按 Esc 静默回到主轮盘。
后端不可用时回退到内置格式列表；一次拖入多个文件时取第一个。

## 后续阶段（未实现）

- 引擎状态缓存与后台探测、每引擎并发闸门（当前 `GetStatus` 是即时探测）
- 更多引擎（PDFium / LibreOffice / Ghostscript / Tesseract）解锁文档类多跳路由

参考：Filee 项目的 `docs/ADDING-A-CONVERTER.md`、`docs/ARCHITECTURE.md` 与
`src/Filee.Core/Conversion/{IConverter,RoutePlanner,ConverterCatalog,JobQueue}.cs`。

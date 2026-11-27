# 格式转换框架（P0）

Orbit 的格式转换不再是一张硬编码的格式白名单：引擎只声明**直接转换能力**，
多跳路由由 `RoutePlanner` 在格式图上用 Dijkstra 自动推导。这个设计借鉴了
Filee 的转换子系统（引擎 → 边 → 路由 → 队列）。

## 分层

```
src-tauri/src/conversion/
  model.rs               ConversionEdge / Converter / ConversionStep / ConverterStatus
  engine/ffmpeg.rs        FFmpeg 引擎：5 个目标格式各一条边
  registry.rs             ConverterRegistry：能力发现唯一入口
  planner.rs              RoutePlanner：最短路 + 独占边过滤 + 3 跳上限
  output.rs               JobAllocator：路径预占 + 归属标记 + 失败回滚 + 工作目录
src-tauri/src/convert.rs  命令总线门面：formats / plan / format
```

## 命令

| 命令 | 风险 | 说明 |
|---|---|---|
| `convert.formats` | ReadOnly | 列出引擎声明的所有目标格式；可被 MCP 安全暴露 |
| `convert.plan` | ReadOnly | 输入路径 + 目标格式 → 路由预览（`{ from, to, steps, cost }`） |
| `convert.format` | Mutating | 同步执行，返回最终输出路径 |
| `convert.start` | Mutating | 异步启动，立即返回任务快照（`ConvertJobStatus`） |
| `convert.cancel` | Mutating | 按 jobId 置取消令牌，后台任务在检查点停下并回滚 |
| `convert.status` | ReadOnly | 按 jobId 查询状态 / 进度 / 输出 / 错误 |

状态变化通过 `convert-state` 事件回流，载荷为 `ConvertJobStatus`。

## 如何新增一个目标格式（P0 范围）

1. 在 `conversion/engine/ffmpeg.rs` 的 `EDGES` 加一条 `ConversionEdge::new("*", "<format>")`
2. 在同文件的 `convert_with_ffmpeg` 参数表里加一个分支
3. 运行 `cargo test -p orbit-core` 重新生成 TS 绑定（新增类型时）并提交
4. 加一个纯单测验证边存在；需要真机验证的加在 `#[cfg(feature = "smoke")]`

规划器、`convert.formats`、`convert.plan` 会自动包含新格式，无需改动。

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
拖入时高亮提示「松手转换」，松手后自动展开圆盘 → 从 `convert.formats` 动态加载目标 →
`convert.start` 异步启动 → 监听 `convert-state` 显示进度/结果，`convert.cancel` 可中途取消。
后端不可用时回退到内置格式列表；一次拖入多个文件时取第一个并提示。

## 后续阶段（未实现）

- P3：引擎状态缓存与后台探测、每引擎并发闸门、`ProcessRunner` 超时与停滞检测；更多引擎（PDFium / ImageMagick 等）与中间格式路由自动解锁

参考：Filee 项目的 `docs/ADDING-A-CONVERTER.md`、`docs/ARCHITECTURE.md` 与
`src/Filee.Core/Conversion/{IConverter,RoutePlanner,ConverterCatalog,JobQueue}.cs`。

<#
.SYNOPSIS
    Orbit 性能基线测量：P-01 冷启动、P-02 空闲内存占用。

.DESCRIPTION
    对已打包的 release exe 重复测量，输出 p50/p95。测量口径（写进 docs/PERF.md）：

    P-01 冷启动（代理指标）：进程启动 → 主窗口句柄非零（窗口已创建并可见）。
      这是"悬浮球可点击"的可自动化代理；WebView 首帧内容就绪没有外部可观测信号，
      因此真实可点击延迟会略高于本值，报告时必须注明。

    P-02 空闲内存：窗口出现后等待 -IdleSeconds，再按 1s 间隔采样 -IdleSamples 次，
      取最后一次；统计口径是整进程族（主进程 + WebView2 子进程）的 WorkingSet64 之和。

    另外校验产物是否内嵌前端资源（README 约定：exe 中应能检索到 `assets/index-`，
    直接 cargo build 的 exe 不含该字符串，启动会去连 localhost:1420）。

.PARAMETER ExePath
    release exe 路径，默认 <仓库>/target/release/orbit-desktop.exe。

.PARAMETER Iterations
    冷启动测量轮数，默认 5。

.PARAMETER IdleSeconds
    窗口出现后等待多久再采内存，默认 8 秒（覆盖首次 CPU 采样与剪贴板监听启动）。

.PARAMETER IdleSamples
    空闲内存采样次数，默认 5。

.PARAMETER Force
    若已有 orbit-desktop 进程在运行，默认拒绝执行；加 -Force 先结束它们。

.PARAMETER Json
    以 JSON 输出（便于 CI 归档），默认输出表格。

.EXAMPLE
    pwsh -File scripts/perf-baseline/measure.ps1

.EXAMPLE
    pwsh -File scripts/perf-baseline/measure.ps1 -Iterations 10 -Json -OutFile perf.json
#>
[CmdletBinding()]
param(
    [string]$ExePath = "",
    [int]$Iterations = 5,
    [int]$IdleSeconds = 8,
    [int]$IdleSamples = 5,
    [switch]$Force,
    [switch]$Json,
    [string]$OutFile = ""
)

$ErrorActionPreference = 'Stop'

function Resolve-RepoRoot {
    $scriptDir = Split-Path -Parent $PSCommandPath
    return (Resolve-Path (Join-Path $scriptDir '..\..')).Path
}

function Get-ProcessTreeIds {
    param([int]$RootId)
    $all = Get-CimInstance Win32_Process -Property ProcessId, ParentProcessId
    $byParent = @{}
    foreach ($proc in $all) {
        if (-not $byParent.ContainsKey([int]$proc.ParentProcessId)) {
            $byParent[[int]$proc.ParentProcessId] = New-Object System.Collections.Generic.List[int]
        }
        $byParent[[int]$proc.ParentProcessId].Add([int]$proc.ProcessId)
    }
    $result = New-Object System.Collections.Generic.List[int]
    $queue = New-Object System.Collections.Generic.Queue[int]
    $result.Add($RootId)
    $queue.Enqueue($RootId)
    while ($queue.Count -gt 0) {
        $current = $queue.Dequeue()
        if ($byParent.ContainsKey($current)) {
            foreach ($child in $byParent[$current]) {
                if (-not $result.Contains($child)) { $result.Add($child); $queue.Enqueue($child) }
            }
        }
    }
    return $result
}

function Measure-TreeWorkingSetMb {
    param([int]$RootId)
    $ids = Get-ProcessTreeIds -RootId $RootId
    $bytes = 0L
    foreach ($id in $ids) {
        try {
            $proc = Get-Process -Id $id -ErrorAction Stop
            $bytes += $proc.WorkingSet64
        } catch { }
    }
    return [math]::Round($bytes / 1MB, 1)
}

function Measure-Percentile {
    param([double[]]$Values, [double]$Percentile)
    if ($Values.Count -eq 0) { return $null }
    $sorted = $Values | Sort-Object
    $index = [math]::Ceiling(($Percentile / 100) * $sorted.Count) - 1
    $index = [math]::Max(0, [math]::Min($index, $sorted.Count - 1))
    return $sorted[$index]
}

function Test-EmbeddedAssets {
    param([string]$Path)
    # 判定法来自 README：内嵌前端资源的 exe 里能检索到 `assets/index-` 字符串。
    $bytes = [System.IO.File]::ReadAllBytes($Path)
    $text = [System.Text.Encoding]::ASCII.GetString($bytes)
    return [bool]($text.Contains('assets/index-'))
}

$repoRoot = Resolve-RepoRoot
if ([string]::IsNullOrWhiteSpace($ExePath)) {
    $ExePath = Join-Path $repoRoot 'target\release\orbit-desktop.exe'
}
if (-not (Test-Path $ExePath)) {
    throw "找不到 release exe：$ExePath。先执行 npm run tauri -- build --no-bundle（或 npm run tauri:build 出安装包）。"
}
$ExePath = (Resolve-Path $ExePath).Path

$existing = Get-Process -Name 'orbit-desktop' -ErrorAction SilentlyContinue
if ($existing) {
    if (-not $Force) {
        throw "已有 orbit-desktop 进程在运行（$($existing.Id -join ', ')）。测量需要独占实例：关闭它或加 -Force。"
    }
    $existing | Stop-Process -Force
    Start-Sleep -Milliseconds 500
}

if (-not (Test-EmbeddedAssets -Path $ExePath)) {
    Write-Warning "exe 中未检索到 assets/index-：可能是 cargo 裸构建产物，启动会去连 localhost:1420。请用 tauri build 重新打包。"
}

$coldStartMs = New-Object System.Collections.Generic.List[double]
$idleMb = New-Object System.Collections.Generic.List[double]
$failures = New-Object System.Collections.Generic.List[string]

for ($i = 1; $i -le $Iterations; $i++) {
    Write-Host ("[{0}/{1}] 启动 {2}" -f $i, $Iterations, (Split-Path -Leaf $ExePath))
    $timer = [System.Diagnostics.Stopwatch]::StartNew()
    $proc = Start-Process -FilePath $ExePath -PassThru -WindowStyle Normal
    $handle = [IntPtr]::Zero
    $deadline = [DateTime]::UtcNow.AddSeconds(30)
    while ([DateTime]::UtcNow -lt $deadline) {
        if ($proc.HasExited) {
            $failures.Add("第 $i 轮：进程在窗口出现前退出（ExitCode=$($proc.ExitCode)）")
            break
        }
        $proc.Refresh()
        $handle = $proc.MainWindowHandle
        if ($handle -ne [IntPtr]::Zero) { break }
        Start-Sleep -Milliseconds 10
    }
    $timer.Stop()

    if ($proc.HasExited) { continue }
    if ($handle -eq [IntPtr]::Zero) {
        $failures.Add("第 $i 轮：30s 内未出现主窗口")
        try { $proc.Kill() } catch { }
        continue
    }
    $coldStartMs.Add([math]::Round($timer.Elapsed.TotalMilliseconds, 1))
    Write-Host ("    冷启动（窗口句柄就绪）：{0} ms" -f $coldStartMs[$coldStartMs.Count - 1])

    Start-Sleep -Seconds $IdleSeconds
    $lastMb = 0.0
    for ($s = 0; $s -lt $IdleSamples; $s++) {
        $lastMb = Measure-TreeWorkingSetMb -RootId $proc.Id
        if ($s -lt $IdleSamples - 1) { Start-Sleep -Seconds 1 }
    }
    if (-not $proc.HasExited) {
        $idleMb.Add($lastMb)
        Write-Host ("    空闲内存（进程族）：{0} MB" -f $lastMb)
    }

    try { $proc.Kill() } catch { }
    Start-Sleep -Milliseconds 800
}

$report = [ordered]@{
    generatedAt   = (Get-Date).ToString('o')
    exe           = $ExePath
    exeBytes      = (Get-Item $ExePath).Length
    embeddedAssets = (Test-EmbeddedAssets -Path $ExePath)
    os            = "$([System.Environment]::OSVersion.VersionString)"
    machine       = $env:COMPUTERNAME
    iterations    = $Iterations
    p01ColdStartMs = @{
        samples = $coldStartMs.ToArray()
        p50     = Measure-Percentile -Values $coldStartMs.ToArray() -Percentile 50
        p95     = Measure-Percentile -Values $coldStartMs.ToArray() -Percentile 95
        target  = 500
    }
    p02IdleTreeMb = @{
        samples = $idleMb.ToArray()
        p50     = Measure-Percentile -Values $idleMb.ToArray() -Percentile 50
        p95     = Measure-Percentile -Values $idleMb.ToArray() -Percentile 95
        target  = 60
    }
    failures      = $failures.ToArray()
}

if ($Json) {
    $text = $report | ConvertTo-Json -Depth 6
} else {
    $lines = @(
        "Orbit 性能基线 — $(Get-Date -Format 'yyyy-MM-dd HH:mm:ss')",
        "  exe: $ExePath ($([math]::Round($report.exeBytes / 1MB, 2)) MiB, 内嵌资源: $($report.embeddedAssets))",
        "  P-01 冷启动(ms)  : 样本 [$($report.p01ColdStartMs.samples -join ', ')]  p50=$($report.p01ColdStartMs.p50)  p95=$($report.p01ColdStartMs.p95)  目标<500",
        "  P-02 空闲内存(MB): 样本 [$($report.p02IdleTreeMb.samples -join ', ')]  p50=$($report.p02IdleTreeMb.p50)  p95=$($report.p02IdleTreeMb.p95)  目标<60"
    )
    if ($failures.Count -gt 0) { $lines += "  失败：$($failures -join '；')" }
    $text = $lines -join "`n"
}

if (-not [string]::IsNullOrWhiteSpace($OutFile)) {
    Set-Content -Path $OutFile -Value $text -Encoding utf8
    Write-Host "已写入 $OutFile"
} else {
    Write-Output $text
}

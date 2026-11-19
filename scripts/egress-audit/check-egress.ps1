# Orbit egress guard (outbound-connection allowlist)
#
# Goal: turn "the author surveillance line is empty" from a one-off grep
# conclusion into a CI invariant. The script enumerates every network
# egress call site in all Rust sources (listen / connect / DNS resolve),
# then compares them against the allowlist below. Any unregistered egress
# fails the run.
#
# To add a new egress point you MUST first register it in the allowlist
# with its binding direction (loopback-only / user-configured) and purpose,
# then pass public PR review -- that moment is exactly when outbound
# capability is born and gets publicly challenged. See docs/SAFETY.md.
#
# The script also checks the implant-side forbidden-primitive list
# (Meterpreter-class primitives): any code-line hit fails, no exemptions.
#
# Usage:
#   pwsh -NoProfile -File scripts/egress-audit/check-egress.ps1
#   (Windows PowerShell 5.1 also works:
#    powershell -NoProfile -ExecutionPolicy Bypass -File ...)
# CI:
#   pwsh -NoProfile -File scripts/egress-audit/check-egress.ps1 -RepoRoot $env:GITHUB_WORKSPACE
# Exit code: 0 = pass; 1 = unregistered egress, forbidden primitive, or stale allowlist.
#
# ENCODING NOTE: keep this file ASCII-only. PowerShell 5.1 reads BOM-less
# UTF-8 as ANSI; a CJK comment line can swallow the newline and merge the
# next code line into the comment (verified experimentally in this repo).
# The write tool stores files as UTF-8 without BOM, so ASCII is the only
# portable choice. Chinese lives in docs/SAFETY.md, not in this script.

param(
    # Repo root (the directory containing Cargo.toml).
    # In CI pass $env:GITHUB_WORKSPACE.
    [string]$RepoRoot = (Split-Path -Parent (Split-Path -Parent $PSScriptRoot)),

    # Loose mode: report only, never fail. For local triage; CI never uses it.
    [switch]$ReportOnly
)

$ErrorActionPreference = 'Stop'
if (-not (Test-Path (Join-Path $RepoRoot 'Cargo.toml'))) {
    throw "RepoRoot is not the Orbit repo root (Cargo.toml not found): $RepoRoot"
}
$repoRoot = $RepoRoot

# ======== allowlist: ALL legitimate network egress ========
# Format: "<relative/path>:<line>" => "description"
# Registration requires: binding direction (loopback-only / user-configured)
# AND purpose. Line numbers must track the code -- the script verifies each
# registered line is still an egress call, so line drift fails the guard.
$allowed = @{
    'src-tauri/src/mcp_bridge.rs:34'          = 'MCP bridge listen: Ipv4Addr::LOCALHOST random port + one-time token first-line auth; loopback only'
    'src-tauri/src/remote.rs:51'             = 'Remote read-only pairing listen: LOOPBACK_HOST=127.0.0.1 + one-time token + capability whitelist (screenshot/ocr), off by default'
    'src-tauri/src/remote.rs:284'            = 'Remote pairing client connect: parse_endpoint enforces is_loopback(); cross-machine needs user-built VPN/SSH tunnel'
    'src-tauri/src/mcp/main.rs:148'          = 'orbit-mcp sidecar connects to local bridge: 127.0.0.1 + one-time token from bridge file'
    'crates/orbit-ocr-server/src/lib.rs:74'   = 'orbit-ocr-server listen: user-deployed standalone program, default 127.0.0.1:8787, not bundled in installer'
    'crates/orbit-ocr-server/src/lib.rs:201'  = 'orbit-ocr-client connect: server address passed explicitly via --server, no default remote address'
    # ---- test code (loopback ports, never shipped) ----
    'src-tauri/src/remote.rs:426'            = 'test: frame_roundtrip loopback self-connect'
    'src-tauri/src/remote.rs:433'            = 'test: frame_roundtrip loopback self-connect'
    'src-tauri/tests/mcp_stdio.rs:10'         = 'test: fake bridge listens on loopback random port'
}

# ======== egress detection patterns (Rust std + common socket APIs) ========
$egressPatterns = @(
    'TcpListener::bind'
    'TcpStream::connect'
    'UdpSocket::(bind|connect|send_to)'
    'to_socket_addrs'
    'lookup_host'
    'gethostbyname'
    'DnsQuery'
)

# ======== forbidden list: Meterpreter-class implant primitives ========
# Mirrors docs/SAFETY.md section 3 and license-agreement.txt section 2.
$forbiddenPatterns = @(
    # process injection and migration
    'CreateRemoteThread', 'WriteProcessMemory', 'VirtualAllocEx', 'NtCreateThreadEx',
    'QueueUserAPC',
    # reflective in-memory execution
    'LoadLibraryA', 'LoadLibraryW', 'GetProcAddress', 'VirtualProtect', 'MapViewOfFile',
    # privilege escalation
    'getsystem', 'DuplicateTokenEx',
    # power control
    'ExitWindowsEx', 'InitiateSystemShutdown', 'SetSuspendState',
    # input blocking and desktop manipulation
    'BlockInput', 'EnumDesktops', 'OpenInputDesktop', 'SwitchDesktop', 'SetThreadDesktop',
    'GetLastInputInfo',
    # audio / webcam capture surface
    'waveIn', 'waveOut', 'IAudioClient', 'avicap', 'capCreate', 'mciSendString', 'PlaySound',
    # PowerShell download primitives
    'DownloadString', 'DownloadFile', 'Net\.WebClient', 'Invoke-WebRequest', 'Invoke-RestMethod',
    'Start-BitsTransfer', 'bitsadmin'
)

# Scan all .rs files; directories without permission are silently skipped.
function Find-RepoRustFiles {
    Get-ChildItem -Path $repoRoot -Recurse -File -Filter '*.rs' -ErrorAction SilentlyContinue |
        Where-Object { $_.FullName -notmatch '[\\/](target|node_modules)[\\/]' }
}

$violations = New-Object System.Collections.Generic.List[string]
$forbiddenHits = New-Object System.Collections.Generic.List[string]
$foundCount = 0

foreach ($file in Find-RepoRustFiles) {
    $relative = $file.FullName.Substring($repoRoot.Length).TrimStart('\', '/') -replace '\\', '/'

    $lines = @(Get-Content -LiteralPath $file.FullName -ErrorAction SilentlyContinue)
    if ($lines.Count -eq 0) { continue }
    for ($i = 0; $i -lt $lines.Count; $i++) {
        $line = $lines[$i]
        $lineNo = $i + 1
        $key = "${relative}:${lineNo}"

        foreach ($pattern in $egressPatterns) {
            if ($line -match $pattern) {
                $foundCount++
                if (-not $allowed.ContainsKey($key)) {
                    $violations.Add("unregistered egress: ${key}: $($line.Trim())  [match: $pattern]")
                }
                break
            }
        }

        foreach ($pattern in $forbiddenPatterns) {
            if ($line -match $pattern) {
                # Comment/doc lines do not count (the forbidden-list docs mention
                # these symbol names themselves). Only code-shaped lines count.
                $trimmed = $line.TrimStart()
                $isComment = $trimmed.StartsWith('//') -or $trimmed.StartsWith('*') -or
                             $trimmed.StartsWith('/*') -or $trimmed.StartsWith('#[') -or
                             $trimmed.StartsWith('#!')
                $isDocQuote = $line -match 'license-agreement|SAFETY\.md|forbidden|Meterpreter'
                if (-not $isComment -and -not $isDocQuote) {
                    $forbiddenHits.Add("forbidden primitive: ${key}: $($line.Trim())  [match: $pattern]")
                }
            }
        }
    }
}

# ======== allowlist freshness: registered lines must still be egress ========
# Without this check, line drift would let the guard go blind silently.
$staleEntries = New-Object System.Collections.Generic.List[string]
foreach ($key in $allowed.Keys) {
    $parts = $key -split ':'
    $relPath = $parts[0]
    $lineNo = [int]$parts[1]
    $fullPath = Join-Path $repoRoot ($relPath -replace '/', '\')
    if (-not (Test-Path -LiteralPath $fullPath)) {
        $staleEntries.Add("allowlist entry points to missing file: $key")
        continue
    }
    $content = @(Get-Content -LiteralPath $fullPath -ErrorAction SilentlyContinue)
    if ($content.Count -eq 0 -or $lineNo -gt $content.Count) {
        $staleEntries.Add("allowlist line number out of range: $key (file has $($content.Count) lines)")
        continue
    }
    $lineText = $content[$lineNo - 1]
    $stillEgress = $false
    foreach ($pattern in $egressPatterns) {
        if ($lineText -match $pattern) { $stillEgress = $true; break }
    }
    if (-not $stillEgress) {
        $staleEntries.Add("allowlist entry no longer an egress call (update or remove): $key")
    }
}

# ======== report ========
Write-Host "=== Orbit egress guard ==="
Write-Host "repo root: $repoRoot"
Write-Host "scan scope: all .rs files (target/node_modules excluded)"
Write-Host "egress calls found: $foundCount; allowlist entries: $($allowed.Count)"
Write-Host ""

if ($staleEntries.Count -gt 0) {
    Write-Host "!! stale allowlist entries ($($staleEntries.Count)):" -ForegroundColor Yellow
    $staleEntries | ForEach-Object { Write-Host "   $_" -ForegroundColor Yellow }
    Write-Host ""
}

if ($violations.Count -gt 0) {
    Write-Host "X unregistered egress calls ($($violations.Count)):" -ForegroundColor Red
    $violations | ForEach-Object { Write-Host "   $_" -ForegroundColor Red }
    Write-Host ""
    Write-Host "New egress must be registered in scripts/egress-audit/check-egress.ps1"
    Write-Host "with its binding direction (loopback-only / user-configured) and purpose,"
    Write-Host "and reviewed publicly in a PR. See docs/SAFETY.md."
} else {
    Write-Host "OK: all egress calls are registered" -ForegroundColor Green
}

if ($forbiddenHits.Count -gt 0) {
    Write-Host ""
    Write-Host "X forbidden implant primitives ($($forbiddenHits.Count)):" -ForegroundColor Red
    $forbiddenHits | ForEach-Object { Write-Host "   $_" -ForegroundColor Red }
    Write-Host ""
    Write-Host "These primitives conflict with license-agreement.txt section 2."
} else {
    Write-Host "OK: no forbidden implant primitives" -ForegroundColor Green
}

$failed = ($violations.Count -gt 0) -or ($forbiddenHits.Count -gt 0) -or ($staleEntries.Count -gt 0)
if ($failed -and -not $ReportOnly) {
    exit 1
}
Write-Host ""
if ($failed) {
    Write-Host "result: FAIL (ReportOnly mode, not exiting)"
} else {
    Write-Host "result: PASS"
}

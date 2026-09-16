<#
.SYNOPSIS
    zcode-switcher Windows 工具链安装/状态脚本（PowerShell 5.1+）。

.DESCRIPTION
    幂等地把 Windows 上构建 zcode-switcher（前端 + Tauri / Rust）所需的工具链
    装到本地：
      - Node.js 20 LTS  便携 zip  → <repo>/.tools/node-v20.19.5-win-x64/
      - MinGW-w64 14 (UCRT)       → <repo>/.tools/mingw64/
      - Rust 1.98 stable           → $env:USERPROFILE\.cargo\  (rustup-init)
      - NSIS 3.11                  → $env:LOCALAPPDATA\tauri\NSIS\  (避免 tauri bundler 网络超时)

    已装的工具全部跳过；不会重新下载。
    不动全局 PATH（激活通过 scripts/dev-env.sh 在 shell 内 export）。

.PARAMETER SelfContained
    仅把仓库根下 .tools/ 与 ~/.cargo 装齐；不动 NSIS（不联网）。
    适合 CI 与不希望走 %LOCALAPPDATA% 的场景。

.PARAMETER Status
    只打印当前工具链状态、退出码 0/1。不修改任何文件。

.PARAMETER ForceMsvc
    默认走 GNU 工具链（与现有 INSTALL.md 一致；w64devkit 链接器已确认缺
    libgcc_eh，build script 必失败——仅用于 cargo test 的 GNU 路径）。
    走 MSVC 路径时：先 rustup-init 装 stable，再 `rustup default
    stable-x86_64-pc-windows-msvc`（自动安装 MSVC 目标），不依赖本机已装
    Visual Studio Build Tools。**注意**：MSVC 链接器本身仍需由 Visual
    Studio Build Tools / Windows SDK 提供；本脚本不替你装那个。

.EXAMPLE
    powershell -NoProfile -ExecutionPolicy Bypass -File scripts\install-toolchain\windows.ps1

.EXAMPLE
    powershell -NoProfile -ExecutionPolicy Bypass -File scripts\install-toolchain\windows.ps1 -Status

.EXAMPLE
    powershell -NoProfile -ExecutionPolicy Bypass -File scripts\install-toolchain\windows.ps1 -SelfContained

.EXAMPLE
    powershell -NoProfile -ExecutionPolicy Bypass -File scripts\install-toolchain\windows.ps1 -ForceMsvc

.NOTES
    任何阶段失败都立即退出、非零退出码。输出包含具体失败原因。
    下载源：nodejs.org、winlibs.com、rustup.rs、nsis.sourceforge.io。
#>
[CmdletBinding()]
param(
    [switch]$SelfContained,
    [switch]$Status,
    [switch]$ForceMsvc
)

$ErrorActionPreference = 'Stop'
$ProgressPreference = 'SilentlyContinue'  # 避免 Write-Progress 干扰 CI 日志

$RepoRoot = (Resolve-Path (Join-Path $PSScriptRoot '..\..')).Path
$ToolsDir = Join-Path $RepoRoot '.tools'
$NodeDir  = Join-Path $ToolsDir 'node-v20.19.5-win-x64'
$MingwDir = Join-Path $ToolsDir 'mingw64'

$UserProfile = $env:USERPROFILE
$CargoBin    = Join-Path $UserProfile '.cargo\bin'
$LocalAppData = $env:LOCALAPPDATA
$NsisDir     = Join-Path $LocalAppData 'tauri\NSIS'

$NodeVersion = '20.19.5'
$NodeUrl     = "https://nodejs.org/dist/v$NodeVersion/node-v$NodeVersion-win-x64.zip"
$MingwUrl    = 'https://github.com/brechtsanders/winlibs_mingw/releases/download/14.2.0posix-19.1.1-12.0.0-msvcrt-r2/winlibs-x86_64-posix-seh-gcc-14.2.0-mingw-w64msvcrt-12.0.0-r2.zip'
$RustInitUrl = 'https://win.rustup.rs/x86_64'
$NsisUrl     = 'https://sourceforge.net/projects/nsis/files/NSIS%203/3.11/nsis-3.11.zip/download'

# ---------------------------------------------------------------------------
# 工具函数
# ---------------------------------------------------------------------------

function Write-Section {
    param([string]$Title)
    Write-Host ''
    Write-Host ('=== {0} ===' -f $Title) -ForegroundColor Cyan
}

function Test-Command {
    param([string]$Name)
    $cmd = Get-Command $Name -ErrorAction SilentlyContinue
    return [bool]$cmd
}

function Get-CachedPath {
    # 在 %TEMP% 下缓存下载；同名文件存在则跳过。
    param([string]$Url, [string]$FileName)
    $dir = Join-Path $env:TEMP 'zcode-switcher-toolchain-cache'
    if (-not (Test-Path $dir)) { New-Item -ItemType Directory -Force -Path $dir | Out-Null }
    $cached = Join-Path $dir $FileName
    if (Test-Path $cached) { return $cached }
    Write-Host "下载：$Url"
    try {
        [Net.ServicePointManager]::SecurityProtocol = [Net.SecurityProtocolType]::Tls12
        Invoke-WebRequest -Uri $Url -OutFile $cached -UseBasicParsing -TimeoutSec 300
    } catch {
        throw "下载失败：$Url`n$_"
    }
    return $cached
}

function Expand-Zip {
    param([string]$ZipPath, [string]$DestDir)
    Write-Host "解压到：$DestDir"
    if (-not (Test-Path $DestDir)) { New-Item -ItemType Directory -Force -Path $DestDir | Out-Null }
    # Expand-Archive 在 PS 5.1 上对大 zip 偶发挂死；用 System.IO.Compression 走文件流。
    Add-Type -AssemblyName System.IO.Compression.FileSystem
    [System.IO.Compression.ZipFile]::ExtractToDirectory($ZipPath, $DestDir)
}

# ---------------------------------------------------------------------------
# 状态检查（与安装共用）
# ---------------------------------------------------------------------------

function Get-ToolStatus {
    $rows = New-Object System.Collections.Generic.List[object]

    # node
    if (Test-Path (Join-Path $NodeDir 'node.exe')) {
        $v = & (Join-Path $NodeDir 'node.exe') --version 2>$null
        $rows.Add([pscustomobject]@{ Tool='node'; Status='installed'; Detail="local $v at .tools/" })
    } else {
        $rows.Add([pscustomobject]@{ Tool='node'; Status='missing'; Detail="expected at $NodeDir" })
    }

    # rustc
    $rustcCmd = Get-Command rustc -ErrorAction SilentlyContinue
    if ($rustcCmd) {
        $v = & rustc --version 2>$null
        $default = (& rustup show active-toolchain 2>$null) -replace '`n',' '
        $rows.Add([pscustomobject]@{ Tool='rustc'; Status='installed'; Detail="$v  ($default)" })
    } else {
        $rows.Add([pscustomobject]@{ Tool='rustc'; Status='missing'; Detail="expected at $CargoBin\rustc.exe" })
    }

    # gcc (仅 GNU 工具链需要)
    $gccCmd = Get-Command gcc -ErrorAction SilentlyContinue
    if ($gccCmd) {
        $v = (gcc --version 2>$null | Select-Object -First 1)
        $rows.Add([pscustomobject]@{ Tool='gcc'; Status='installed'; Detail=$v })
    } else {
        $rows.Add([pscustomobject]@{ Tool='gcc'; Status='missing'; Detail="needed for GNU toolchain; ignored if MSVC" })
    }

    # NSIS（只有需要打 NSIS 安装包才用）
    if (Test-Path (Join-Path $NsisDir 'makensis.exe')) {
        $rows.Add([pscustomobject]@{ Tool='nsis'; Status='installed'; Detail=$NsisDir })
    } else {
        $rows.Add([pscustomobject]@{ Tool='nsis'; Status='missing'; Detail="needed only for `tauri build`" })
    }

    return $rows
}

# ---------------------------------------------------------------------------
# 安装步骤
# ---------------------------------------------------------------------------

function Install-Node {
    if (Test-Path (Join-Path $NodeDir 'node.exe')) {
        Write-Host "[node] 已存在，跳过。"
        return
    }
    $zip = Get-CachedPath -Url $NodeUrl -FileName 'node-win.zip'
    if (Test-Path $ToolsDir) { Remove-Item -Recurse -Force $ToolsDir }  # 干净解压
    New-Item -ItemType Directory -Force -Path $ToolsDir | Out-Null
    Expand-Zip -ZipPath $zip -DestDir $ToolsDir
    if (-not (Test-Path (Join-Path $NodeDir 'node.exe'))) {
        throw "node.exe 不在预期位置：$NodeDir"
    }
    Write-Host "[node] 安装完成：$( & (Join-Path $NodeDir 'node.exe') --version )"
}

function Install-Mingw {
    if (Test-Path (Join-Path $MingwDir 'bin\gcc.exe')) {
        Write-Host "[mingw] 已存在，跳过。"
        return
    }
    $zip = Get-CachedPath -Url $MingwUrl -FileName 'mingw.zip'
    Expand-Zip -ZipPath $zip -DestDir $ToolsDir
    # winlibs 解出来是 mingw64/ 子目录
    $actual = Join-Path $ToolsDir 'mingw64'
    if (-not (Test-Path (Join-Path $actual 'bin\gcc.exe'))) {
        throw "mingw64\bin\gcc.exe 不在预期位置：$actual"
    }
    if ($actual -ne $MingwDir) {
        Rename-Item $actual $MingwDir
    }
    Write-Host "[mingw] 安装完成：$( & (Join-Path $MingwDir 'bin\gcc.exe') --version | Select-Object -First 1 )"
}

function Install-Rust {
    $rustcCmd = Get-Command rustc -ErrorAction SilentlyContinue
    if ($rustcCmd) {
        Write-Host "[rust] 已存在：$(& rustc --version)"
    } else {
        $exe = Get-CachedPath -Url $RustInitUrl -FileName 'rustup-init.exe'
        Write-Host "运行 rustup-init（默认安装到 $CargoBin）..."
        # /quiet: 无 UI；-y: 全默认；--no-modify-path: 不动注册表 PATH
        $proc = Start-Process -FilePath $exe -ArgumentList @('-q','-y','--no-modify-path','--default-toolchain','stable') -NoNewWindow -Wait -PassThru
        if ($proc.ExitCode -ne 0) { throw "rustup-init 退出 $($proc.ExitCode)" }
    }

    if ($ForceMsvc) {
        Write-Host "[rust] ForceMsvc：切默认到 stable-x86_64-pc-windows-msvc"
        & rustup default stable-x86_64-pc-windows-msvc
    } else {
        $cur = (& rustup show active-toolchain 2>$null)
        if ($cur -notmatch 'msvc') {
            Write-Host "[rust] 当前默认：$cur（GNU）。如要切 MSVC，重跑并加 -ForceMsvc。"
        }
    }
}

function Install-Nsis {
    if (Test-Path (Join-Path $NsisDir 'makensis.exe')) {
        Write-Host "[nsis] 已存在，跳过。"
        return
    }
    if ($SelfContained) {
        Write-Host "[nsis] SelfContained 模式跳过 NSIS 安装（需要时手工跑 tauri build 触发的下载可能超时）。"
        return
    }
    New-Item -ItemType Directory -Force -Path $NsisDir | Out-Null
    $zip = Get-CachedPath -Url $NsisUrl -FileName 'nsis.zip'
    Expand-Zip -ZipPath $zip -DestDir $NsisDir
    # nsis-3.11 解出来是 nsis-3.11/ 子目录
    $inner = Get-ChildItem -Directory $NsisDir | Where-Object { $_.Name -like 'nsis-*' } | Select-Object -First 1
    if ($inner) {
        Get-ChildItem $inner.FullName -Force | Move-Item -Destination $NsisDir -Force
        Remove-Item $inner.FullName -Recurse -Force
    }
    if (-not (Test-Path (Join-Path $NsisDir 'makensis.exe'))) {
        throw "makensis.exe 不在预期位置：$NsisDir"
    }
    Write-Host "[nsis] 安装完成：$NsisDir"
}

# ---------------------------------------------------------------------------
# 主流程
# ---------------------------------------------------------------------------

Write-Section 'zcode-switcher Windows 工具链检查'
$status = Get-ToolStatus
$status | Format-Table -AutoSize | Out-String | Write-Host

if ($Status) {
    $missing = @($status | Where-Object { $_.Status -eq 'missing' -and $_.Tool -ne 'gcc' })
    if ($missing.Count -eq 0) {
        Write-Host '全部就绪。' -ForegroundColor Green
        exit 0
    } else {
        Write-Host ("缺失: " + (($missing | ForEach-Object { $_.Tool }) -join ', ')) -ForegroundColor Yellow
        exit 1
    }
}

Write-Section '安装（幂等：已装跳过）'
try {
    Install-Node
    Install-Mingw
    Install-Rust
    Install-Nsis
} catch {
    Write-Host ''
    Write-Host ("[FATAL] " + $_.Exception.Message) -ForegroundColor Red
    exit 1
}

Write-Section '安装完成'
$status = Get-ToolStatus
$status | Format-Table -AutoSize | Out-String | Write-Host

Write-Host ''
Write-Host '下一步：' -ForegroundColor Green
Write-Host '  1. 启动 Git Bash / MSYS shell'
Write-Host '  2. source scripts/dev-env.sh           # 把 node / cargo / gcc 加入 PATH'
Write-Host '  3. npm install'
Write-Host '  4. npm run tauri build                 # 打 NSIS 安装包'
Write-Host ''
if (-not $ForceMsvc) {
    Write-Host '提示：本机默认是 GNU 工具链，cargo test 时切 MSVC 更稳：' -ForegroundColor Yellow
    Write-Host '  rustup default stable-x86_64-pc-windows-msvc' -ForegroundColor Yellow
    Write-Host '  cargo +stable-x86_64-pc-windows-msvc test --lib' -ForegroundColor Yellow
}

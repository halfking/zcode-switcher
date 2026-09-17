#requires -Version 5.1
<#
.SYNOPSIS
    zcode-switcher Windows 实机回归检查（在真实登录用户的桌面会话中运行）。

.DESCRIPTION
    覆盖跨平台凭据迁移（commit 3dfc58c）之后必须在真实用户会话里验证的项目：

      1. 已安装的 zcode-switcher.exe 版本（默认期望 1.1.16）。
      2. Switcher 与 ZCode 进程运行在交互登录用户（console 会话）下，而不是 SYSTEM。
      3. ~/.zcode/v2/credentials.json 存在、可解析，敏感字符串字段为 enc:v1: 密文。
      4. Switcher 账号池 profiles.json 可读，且每个档案的凭据副本均为 enc:v1: 密文。
      5. zcode.cdp：仅当 ZCode 是带 --remote-debugging-port=9229（增强启动）拉起时
         才要求 CDP 上存在 renderer 页面；用户手动启动的 ZCode 没有调试端口是正常
         状态，记为 SKIP 而不是 FAIL。带 -RepairCdp 时会用增强参数重启 ZCode（先
         复用 Switcher 记录的 exe 路径，回落到运行中进程/常见安装位置）后再复查。

    脚本只输出路径、版本、计数、布尔值与掩码后的邮箱，绝不输出任何 token / 密文内容。

.PARAMETER ExpectedVersion
    期望的 Switcher 版本号，默认 1.1.17。

.PARAMETER RepairCdp
    一键修复：当 ZCode 未带调试参数运行时，用 --remote-debugging-port=<CdpPort>
    重启 ZCode（会先结束当前 ZCode 进程），然后复查 CDP。

.EXAMPLE
    powershell -NoProfile -ExecutionPolicy Bypass -File scripts\windows-regression.ps1

.EXAMPLE
    powershell -NoProfile -ExecutionPolicy Bypass -File scripts\windows-regression.ps1 -ExpectedVersion 1.1.17

.EXAMPLE
    powershell -NoProfile -ExecutionPolicy Bypass -File scripts\windows-regression.ps1 -RepairCdp
#>
param(
    [string]$ExpectedVersion = "1.1.17",
    [int]$CdpPort = 9229,
    [switch]$RepairCdp
)

$ErrorActionPreference = 'Stop'
$script:failures = New-Object System.Collections.Generic.List[string]
$script:checks = 0

function Write-Pass([string]$name, [string]$detail) {
    $script:checks++
    Write-Output ("[PASS] {0} -- {1}" -f $name, $detail)
}

function Write-Fail([string]$name, [string]$detail) {
    $script:checks++
    $script:failures.Add($name)
    Write-Output ("[FAIL] {0} -- {1}" -f $name, $detail)
}

function Mask-Email([string]$email) {
    if ([string]::IsNullOrWhiteSpace($email)) { return '<empty>' }
    $at = $email.IndexOf('@')
    if ($at -lt 0) { return ($email.Substring(0, [Math]::Min(2, $email.Length)) + '***') }
    $local = $email.Substring(0, $at)
    $domain = $email.Substring($at + 1)
    $prefix = $local.Substring(0, [Math]::Min(2, $local.Length))
    return ("{0}***@{1}" -f $prefix, $domain)
}

function Test-EncV1Value([string]$value) {
    return ($value -is [string] -and $value.StartsWith('enc:v1:'))
}

function Get-PropValue($obj, [string]$name) {
    if ($obj -is [System.Collections.IDictionary]) {
        if ($obj.Contains($name)) { return $obj[$name] }
        return $null
    }
    $prop = $obj.PSObject.Properties[$name]
    if ($prop) { return $prop.Value }
    return $null
}

function Count-EncFields($obj) {
    $enc = 0
    $plain = New-Object System.Collections.Generic.List[string]
    if ($obj -is [System.Collections.IDictionary]) {
        foreach ($key in @($obj.Keys)) {
            if (Test-EncV1Value ([string]$obj[$key])) { $enc++ } else { $plain.Add([string]$key) }
        }
    } else {
        foreach ($prop in $obj.PSObject.Properties) {
            if (Test-EncV1Value ([string]$prop.Value)) { $enc++ } else { $plain.Add($prop.Name) }
        }
    }
    return @{ Encrypted = $enc; PlainKeys = $plain }
}

Write-Output ("== zcode-switcher Windows 回归 {0:yyyy-MM-dd HH:mm:ss} ==" -f (Get-Date))
Write-Output ("user: {0}\{1}   IsElevated(SYSTEM-like): {2}" -f $env:USERDOMAIN, $env:USERNAME, ([Security.Principal.WindowsPrincipal][Security.Principal.WindowsIdentity]::GetCurrent()).IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator))
Write-Output ("USERPROFILE: {0}" -f $env:USERPROFILE)

# ---- 1. 已安装 Switcher 版本 -------------------------------------------------
$switcherExe = Join-Path $env:LOCALAPPDATA 'ZCode Switcher\zcode-switcher.exe'
if (Test-Path $switcherExe) {
    $installed = (Get-Item $switcherExe).VersionInfo.ProductVersion
    # 版本号带编译次数（如 1.1.15+43），ProductVersion 可能是元数据截断后的 1.1.15.0，
    # 所以按前缀匹配而不是全等。
    if ($installed -like "$ExpectedVersion*") {
        Write-Pass 'switcher.version' ("{0} ({1})" -f $installed, $switcherExe)
    } else {
        Write-Fail 'switcher.version' ("installed={0} expected={1} ({2})" -f $installed, $ExpectedVersion, $switcherExe)
    }
} else {
    Write-Fail 'switcher.installed' ("missing: {0}" -f $switcherExe)
}

# ---- 2. 进程归属：真实用户会话 ------------------------------------------------
$sessionOwner = $null
try {
    $explorer = Get-CimInstance Win32_Process -Filter "Name='explorer.exe'" |
        Where-Object { $_.SessionId -eq 1 } | Select-Object -First 1
    if ($explorer) {
        $o = Invoke-CimMethod -InputObject $explorer -MethodName GetOwner
        $sessionOwner = "{0}\{1}" -f $o.Domain, $o.User
    }
} catch { }

if ($sessionOwner) {
    Write-Pass 'session.console-user' $sessionOwner
} else {
    Write-Fail 'session.console-user' 'no explorer.exe in session 1'
}

$procs = Get-CimInstance Win32_Process |
    Where-Object { $_.Name -in @('zcode-switcher.exe', 'ZCode.exe') -and $_.SessionId -eq 1 }
foreach ($name in @('zcode-switcher.exe', 'ZCode.exe')) {
    $inSession = @($procs | Where-Object { $_.Name -eq $name })
    if ($inSession.Count -gt 0) {
        Write-Pass ("process.{0}" -f $name) ("{0} process(es) in session 1" -f $inSession.Count)
    } else {
        Write-Fail ("process.{0}" -f $name) 'not running in interactive session 1'
    }
}

# ---- 3. credentials.json 加密形态 ---------------------------------------------
$credPath = Join-Path $env:USERPROFILE '.zcode\v2\credentials.json'
if (Test-Path $credPath) {
    try {
        $raw = Get-Content $credPath -Raw -Encoding UTF8
        try { $cred = ConvertFrom-Json $raw -AsHashtable } catch { $cred = ConvertFrom-Json $raw }
        $stat = Count-EncFields $cred
        Write-Pass 'credentials.parse' ("path={0} fields={1} enc:v1={2}" -f $credPath, $cred.Count, $stat.Encrypted)
        if ($stat.Encrypted -eq 0) {
            Write-Fail 'credentials.encrypted' 'no enc:v1: fields found; ZCode would store plaintext'
        } else {
            Write-Pass 'credentials.encrypted' ("{0} field(s) are enc:v1: ciphertext" -f $stat.Encrypted)
        }
        foreach ($k in @('oauth:active_provider', 'oauth:bigmodel:access_token', 'oauth:zai:access_token')) {
            $v = Get-PropValue $cred $k
            if ($null -ne $v) {
                if (Test-EncV1Value ([string]$v)) {
                    Write-Pass ("credentials.field[{0}]" -f $k) 'present, enc:v1:'
                } else {
                    Write-Fail ("credentials.field[{0}]" -f $k) 'present but NOT enc:v1: encrypted'
                }
            }
        }
    } catch {
        Write-Fail 'credentials.parse' $_.Exception.Message
    }
} else {
    Write-Fail 'credentials.exists' ("missing: {0}" -f $credPath)
}

# ---- 4. 账号池档案与凭据副本 ----------------------------------------------------
$profilesPath = Join-Path $env:USERPROFILE '.zcode\v2\account-profiles\profiles.json'
if (Test-Path $profilesPath) {
    try {
        $profiles = Get-Content $profilesPath -Raw -Encoding UTF8 | ConvertFrom-Json
        $credDir = Split-Path $profilesPath
        $okSnapshots = 0
        $badSnapshots = New-Object System.Collections.Generic.List[string]
        foreach ($p in @($profiles)) {
            if ([string]::IsNullOrEmpty($p.cred_file)) { continue }
            $snap = Join-Path $credDir $p.cred_file
            if (-not (Test-Path $snap)) {
                $badSnapshots.Add($p.cred_file); continue
            }
            $snapRaw = Get-Content $snap -Raw -Encoding UTF8
            try { $snapObj = ConvertFrom-Json $snapRaw -AsHashtable } catch { $snapObj = ConvertFrom-Json $snapRaw }
            $stat = Count-EncFields $snapObj
            if ($stat.Encrypted -gt 0) { $okSnapshots++ } else { $badSnapshots.Add($p.cred_file) }
        }
        if ($badSnapshots.Count -eq 0 -and $okSnapshots -gt 0) {
            Write-Pass 'profiles.snapshots' ("{0} profile(s), all credential snapshots enc:v1:" -f $okSnapshots)
        } else {
            Write-Fail 'profiles.snapshots' ("ok={0} bad={1}" -f $okSnapshots, ($badSnapshots -join ','))
        }
        $masked = @($profiles | ForEach-Object { Mask-Email $_.email })
        Write-Output ("       profiles.emails(masked): {0}" -f ($masked -join ', '))
    } catch {
        Write-Fail 'profiles.parse' $_.Exception.Message
    }
} else {
    Write-Output ("[SKIP] profiles.index -- missing: {0}" -f $profilesPath)
}

# ---- 5. ZCode CDP（9229） ------------------------------------------------------
# 只有 ZCode 由增强启动（命令行带 --remote-debugging-port=9229）拉起时，CDP 才是
# 本工具链路的必要条件；用户手动启动的 ZCode 没有调试端口属于正常环境状态，
# 记为 SKIP，不再当作回归失败。-RepairCdp 提供一键修复：带增强参数重启 ZCode。
$debugFlag = "--remote-debugging-port={0}" -f $CdpPort
$zcodeCmdlines = @()
try {
    $zcodeCmdlines = @(Get-CimInstance Win32_Process -Filter "Name='ZCode.exe'" |
        Where-Object { $_.SessionId -eq 1 } |
        ForEach-Object { [string]$_.CommandLine } |
        Where-Object { $_ })
} catch { }
$launchedWithFlag = @($zcodeCmdlines | Where-Object { $_.Contains($debugFlag) }).Count -gt 0

function Test-CdpRenderer {
    # /json/list 偶发瞬时不稳定（进程忙时可能短暂返回空列表），最多重试 3 次。
    $cdpPage = $null
    $cdpLastError = ''
    foreach ($attempt in 1..3) {
        try {
            $list = Invoke-RestMethod -Uri ("http://127.0.0.1:{0}/json/list" -f $CdpPort) -TimeoutSec 5
            $cdpPage = @($list) | Where-Object { $_.type -eq 'page' -and $_.url -like '*renderer/index.html*' } | Select-Object -First 1
            if ($cdpPage) { break }
            $cdpLastError = 'no renderer/index.html page'
        } catch {
            $cdpLastError = $_.Exception.Message
        }
        Start-Sleep -Seconds 2
    }
    return @{ Page = $cdpPage; Error = $cdpLastError }
}

function Repair-ZcodeWithDebugFlag([string]$flag) {
    # 优先用 Switcher 自己记录的 exe 路径（restart.rs 的 settings_file），回落到
    # 运行中进程的可执行路径，最后试常见安装位置。
    $exe = $null
    $settingsPath = Join-Path $env:USERPROFILE '.zcode\v2\zcode-switcher-settings.json'
    if (Test-Path $settingsPath) {
        try {
            $settings = Get-Content $settingsPath -Raw -Encoding UTF8 | ConvertFrom-Json
            if ($settings.zcode_exe_path -and (Test-Path $settings.zcode_exe_path)) {
                $exe = $settings.zcode_exe_path
            }
        } catch { }
    }
    if (-not $exe) {
        try {
            $proc = Get-CimInstance Win32_Process -Filter "Name='ZCode.exe'" | Select-Object -First 1
            if ($proc -and $proc.ExecutablePath -and (Test-Path $proc.ExecutablePath)) {
                $exe = $proc.ExecutablePath
            }
        } catch { }
    }
    if (-not $exe) {
        foreach ($cand in @(
            (Join-Path $env:LOCALAPPDATA 'Programs\ZCode\ZCode.exe'),
            (Join-Path $env:ProgramFiles 'ZCode\ZCode.exe')
        )) {
            if (Test-Path $cand) { $exe = $cand; break }
        }
    }
    if (-not $exe) {
        Write-Output ("[REPAIR] zcode.cdp -- 找不到 ZCode.exe，无法自动修复")
        return $false
    }

    Write-Output ("[REPAIR] zcode.cdp -- 结束 ZCode 并以 '{0}' 重启: {1}" -f $flag, $exe)
    try {
        Get-CimInstance Win32_Process -Filter "Name='ZCode.exe'" |
            ForEach-Object { Stop-Process -Id $_.ProcessId -Force -ErrorAction SilentlyContinue }
    } catch { }
    $deadline = (Get-Date).AddSeconds(15)
    while ((Get-Date) -lt $deadline) {
        $left = @(Get-CimInstance Win32_Process -Filter "Name='ZCode.exe'" -ErrorAction SilentlyContinue)
        if ($left.Count -eq 0) { break }
        Start-Sleep -Milliseconds 300
    }
    try {
        Start-Process -FilePath $exe -ArgumentList $flag | Out-Null
    } catch {
        Write-Output ("[REPAIR] zcode.cdp -- 启动失败: {0}" -f $_.Exception.Message)
        return $false
    }
    return $true
}

if (-not $launchedWithFlag) {
    if ($zcodeCmdlines.Count -eq 0) {
        Write-Output ("[SKIP] zcode.cdp -- ZCode 未运行（进程检查已在上面单独判定），不要求 CDP")
    } else {
        Write-Output ("[SKIP] zcode.cdp -- ZCode 未带 {0} 启动（非增强启动拉起），不要求 CDP；可加 -RepairCdp 一键修复" -f $debugFlag)
    }
    if ($RepairCdp) {
        $repaired = Repair-ZcodeWithDebugFlag $debugFlag
        if ($repaired) {
            # 等 Electron 起来并打开 CDP 端口。
            $cdpReady = $null
            $repairDeadline = (Get-Date).AddSeconds(30)
            while ((Get-Date) -lt $repairDeadline) {
                Start-Sleep -Seconds 2
                $probe = Test-CdpRenderer
                if ($probe.Page) { $cdpReady = $probe.Page; break }
            }
            if ($cdpReady) {
                Write-Pass 'zcode.cdp' ("port={0} renderer page present (after repair)" -f $CdpPort)
            } else {
                Write-Fail 'zcode.cdp' ("port={0}: repair applied but CDP still unavailable" -f $CdpPort)
            }
        }
    }
} else {
    $probe = Test-CdpRenderer
    if ($probe.Page) {
        Write-Pass 'zcode.cdp' ("port={0} renderer page present" -f $CdpPort)
    } else {
        Write-Fail 'zcode.cdp' ("port={0}: {1}" -f $CdpPort, $probe.Error)
    }
}

# ---- 汇总 ----------------------------------------------------------------------
Write-Output ("== summary: {0} check(s), {1} failure(s) ==" -f $script:checks, $script:failures.Count)
if ($script:failures.Count -gt 0) {
    Write-Output ("failed: {0}" -f ($script:failures -join ', '))
    exit 1
}
exit 0

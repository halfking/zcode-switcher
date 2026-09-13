#requires -Version 5.1
<#
.SYNOPSIS
    zcode-switcher Windows 实机回归检查（在真实登录用户的桌面会话中运行）。

.DESCRIPTION
    覆盖跨平台凭据迁移（commit 3dfc58c）之后必须在真实用户会话里验证的项目：

      1. 已安装的 zcode-switcher.exe 版本（默认期望 1.1.12）。
      2. Switcher 与 ZCode 进程运行在交互登录用户（console 会话）下，而不是 SYSTEM。
      3. ~/.zcode/v2/credentials.json 存在、可解析，敏感字符串字段为 enc:v1: 密文。
      4. Switcher 账号池 profiles.json 可读，且每个档案的凭据副本均为 enc:v1: 密文。
      5. ZCode 以 --remote-debugging-port=9229 启动，CDP 上存在 renderer 页面。

    脚本只输出路径、版本、计数、布尔值与掩码后的邮箱，绝不输出任何 token / 密文内容。

.PARAMETER ExpectedVersion
    期望的 Switcher 版本号，默认 1.1.12。

.EXAMPLE
    powershell -NoProfile -ExecutionPolicy Bypass -File scripts\windows-regression.ps1

.EXAMPLE
    powershell -NoProfile -ExecutionPolicy Bypass -File scripts\windows-regression.ps1 -ExpectedVersion 1.1.12
#>
param(
    [string]$ExpectedVersion = "1.1.12",
    [int]$CdpPort = 9229
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
    if ($installed -eq $ExpectedVersion) {
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
try {
    $list = Invoke-RestMethod -Uri ("http://127.0.0.1:{0}/json/list" -f $CdpPort) -TimeoutSec 5
    $page = @($list) | Where-Object { $_.type -eq 'page' -and $_.url -like '*renderer/index.html*' } | Select-Object -First 1
    if ($page) {
        Write-Pass 'zcode.cdp' ("port={0} renderer page present" -f $CdpPort)
    } else {
        Write-Fail 'zcode.cdp' ("port={0} reachable but no renderer/index.html page" -f $CdpPort)
    }
} catch {
    Write-Fail 'zcode.cdp' ("port={0} not reachable: {1}" -f $CdpPort, $_.Exception.Message)
}

# ---- 汇总 ----------------------------------------------------------------------
Write-Output ("== summary: {0} check(s), {1} failure(s) ==" -f $script:checks, $script:failures.Count)
if ($script:failures.Count -gt 0) {
    Write-Output ("failed: {0}" -f ($script:failures -join ', '))
    exit 1
}
exit 0

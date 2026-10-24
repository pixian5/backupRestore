# br-launch-agent.ps1 - deploy the LATEST br-agent-tcp.ps1 into the VM and start it
# elevated (High IL, Session 1), detached, with a long idle timeout.
#
# Run from the host via:  ./br-s1.sh br-launch-agent.ps1
#
# Why this exists: the legacy `br-agent-tcp.sh start` path does
#     prlctl exec --current-user powershell -File 'X:\tools\win-clicker\br-gui-exec.ps1'
# which (a) reads br-gui-exec.ps1 from the X: share, a STALE copy whose $src is null,
# and (b) uses --current-user, which is broken on this Parallels build. Result: the
# script silently deployed NOTHING and launched an OUTDATED agent binary.
# This script instead refreshes from the repo UNC share (which IS fresh).

$ErrorActionPreference = 'Continue'
$log = New-Object System.Collections.ArrayList
function W($s) { [void]$log.Add($s) }

$pkg = "H:\brwork"
# 源文件优先读客体本地副本（宿主侧 br-agent-tcp.sh start 经 prlctl/SYSTEM 通道刷入
# C:\Users\Public\pkg\br-agent-tcp.in.ps1，二进制 UTF-8 原样）——2026-10-14 实测提权
# (High IL) 上下文读 \\Mac\... UNC 会间歇性「系统找不到指定的文件」，而 SYSTEM 通道
# 的共享盘复制从没失败过（build-win.sh 每次部署都走它）。UNC 只作未预刷时的回退。
$localIn = "C:\Users\Public\pkg\br-agent-tcp.in.ps1"
$src = $localIn
if (-not (Test-Path -LiteralPath $src)) { $src = "\\Mac\backupRestore\tools\win-clicker\br-agent-tcp.ps1" }
$dst = "$pkg\br-agent-tcp.ps1"

W ("TIME=" + (Get-Date -Format "yyyy-MM-dd HH:mm:ss"))
W ("session=" + (Get-Process -Id $PID).SessionId + " admin=" + ([Security.Principal.WindowsPrincipal]([Security.Principal.WindowsIdentity]::GetCurrent())).IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator))
W ("src=" + $src)

# 1) refresh the agent script
$deployOk = $false
try {
    $bytes  = [System.IO.File]::ReadAllBytes($src)
    $utf8s  = [System.Text.Encoding]::UTF8.GetString($bytes)
    $gbk    = [System.Text.Encoding]::GetEncoding(936).GetBytes($utf8s)
    [System.IO.File]::WriteAllBytes($dst, $gbk)
    $fi = Get-Item -LiteralPath $dst
    $has = ([System.IO.File]::ReadAllText($dst)).Contains("'exec'")
    W ("deployed " + $dst + " bytes=" + $fi.Length + " mtime=" + $fi.LastWriteTime.ToString("yyyy-MM-dd HH:mm:ss") + " hasExecOp=" + $has)
    $deployOk = ($fi.Length -ge 1000) -and $has
} catch {
    W ("DEPLOY_FAIL " + $_.Exception.Message)
}
# 部署失败且本地没有可用旧副本 → 直接退出，**不杀现有 agent**（旧版在这里继续
# 往下走：先杀旧进程再 Start-Process 一个不存在的文件，把本来能用的 agent 也弄死）。
if (-not $deployOk -and -not (Test-Path -LiteralPath $dst)) {
    W "ABORT_NO_AGENT_SCRIPT"
    [System.IO.File]::WriteAllText("C:\Users\Public\pkg\launchagent.txt", ($log -join "`r`n"), (New-Object System.Text.UTF8Encoding($false)))
    Write-Output "LAUNCH_AGENT_ABORT"
    exit 4
}
if (-not $deployOk) { W "FALLBACK_TO_EXISTING_AGENT_SCRIPT" }

# 2) kill any already-running agent
$me = $PID
Get-CimInstance Win32_Process -Filter "Name='powershell.exe'" -ErrorAction SilentlyContinue | ForEach-Object {
    $cl = ""
    try { $cl = [string]$_.CommandLine } catch { }
    if ($cl -like '*br-agent-tcp.ps1*' -and $_.ProcessId -ne $me) {
        try { Stop-Process -Id $_.ProcessId -Force -ErrorAction Stop; W ("killed old pid=" + $_.ProcessId) }
        catch { W ("kill_fail pid=" + $_.ProcessId + " " + $_.Exception.Message) }
    }
}
Start-Sleep -Seconds 2

# 3) start the new agent detached (idle timeout 4h: a full restore can outlast 15 min)
Start-Process -FilePath "powershell.exe" -ArgumentList @(
    '-NoProfile', '-ExecutionPolicy', 'Bypass', '-File', $dst, '-Port', '9124', '-IdleTimeoutSec', '14400'
) -WindowStyle Hidden
Start-Sleep -Seconds 4

$lis = @(Get-NetTCPConnection -LocalPort 9124 -State Listen -ErrorAction SilentlyContinue)
foreach ($c in $lis) {
    $pr = Get-Process -Id $c.OwningProcess -ErrorAction SilentlyContinue
    W ("listener " + $c.LocalAddress + ":" + $c.LocalPort + " pid=" + $c.OwningProcess + " session=" + $pr.SessionId)
}
if ($lis.Count -eq 0) { W "NO_LISTENER" }

[System.IO.File]::WriteAllText("C:\Users\Public\pkg\launchagent.txt", ($log -join "`r`n"), (New-Object System.Text.UTF8Encoding($false)))
Write-Output "LAUNCH_AGENT_DONE"
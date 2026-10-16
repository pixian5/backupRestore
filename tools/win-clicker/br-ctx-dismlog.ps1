# br-ctx-dismlog.ps1 - root-cause probe #2:
#   1) process bitness / WOW64 redirection check (elevated context saw OLD file versions)
#   2) ACL + writability of C:\Windows\Logs\DISM (elevated DISM said "Logging is disabled")
#   3) run the 64-bit reagentc explicitly via UNC (bypasses any path weirdness)

$ErrorActionPreference = 'Continue'
function P($n, $v) { Write-Output ("{0,-52}: {1}" -f $n, $v) }
function SEC($n) { Write-Output ("=" * 62); Write-Output "== $n" }

SEC "CONTEXT/BITNESS"
P "timestamp" (Get-Date -Format "yyyy-MM-dd HH:mm:ss")
P "whoami" (& whoami.exe)
P "sessionId" (Get-Process -Id $PID).SessionId
P "Is64BitProcess" [Environment]::Is64BitProcess
P "PROCESSOR_ARCHITECTURE" $env:PROCESSOR_ARCHITECTURE
P "PROCESSOR_ARCHITEW6432" $env:PROCESSOR_ARCHITEW6432
P "powershell.exe path" (Get-Process -Id $PID).Path

SEC "FILE VERSIONS: System32 vs SysWOW64 vs UNC"
foreach ($f in @(
    "C:\Windows\System32\reagentc.exe", "C:\Windows\SysWOW64\reagentc.exe",
    "C:\Windows\System32\dism.exe",     "C:\Windows\SysWOW64\dism.exe",
    "C:\Windows\System32\cmd.exe",      "C:\Windows\SysWOW64\cmd.exe",
    "C:\Windows\System32\ntoskrnl.exe", "C:\Windows\SysWOW64\ntoskrnl.exe"
)) {
    if (Test-Path -LiteralPath $f) {
        $i = Get-Item -LiteralPath $f
        P ("ver " + $f) ($i.VersionInfo.FileVersion + "  size=" + $i.Length)
    } else { P ("ver " + $f) "MISSING" }
}
foreach ($u in @("\\localhost\C$\Windows\System32\reagentc.exe", "\\localhost\C$\Windows\System32\dism.exe")) {
    try { P ("unc " + $u) ((Get-Item -LiteralPath $u -ErrorAction Stop).VersionInfo.FileVersion) }
    catch { P ("unc " + $u) ("FAIL: " + $_.Exception.Message) }
}

SEC "PATH RESOLUTION"
foreach ($c in @("reagentc.exe", "dism.exe", "cmd.exe")) {
    $g = Get-Command $c -ErrorAction SilentlyContinue
    if ($g) { P ("Get-Command " + $c) $g.Source } else { P ("Get-Command " + $c) "NOT FOUND" }
}

SEC "DISM LOG FILE: metadata + ACL + writability"
$dl = "C:\Windows\Logs\DISM\dism.log"
if (Test-Path -LiteralPath $dl) {
    $i = Get-Item -LiteralPath $dl
    P "dism.log created" $i.CreationTime.ToString("yyyy-MM-dd HH:mm:ss")
    P "dism.log modified" $i.LastWriteTime.ToString("yyyy-MM-dd HH:mm:ss")
    P "dism.log size" $i.Length
    P "dism.log owner" (Get-Acl -LiteralPath $dl).Owner
    (Get-Acl -LiteralPath $dl).Access | ForEach-Object {
        P ("  ace " + $_.IdentityReference) ($_.FileSystemRights.ToString() + " / " + $_.AccessControlType + " / " + $_.IsInherited)
    }
} else { P "dism.log" "MISSING" }
P "-- DISM dir ACL --"
(Get-Acl -LiteralPath "C:\Windows\Logs\DISM").Access | ForEach-Object {
    P ("  dace " + $_.IdentityReference) ($_.FileSystemRights.ToString() + " / " + $_.AccessControlType + " / " + $_.IsInherited)
}
P "-- Logs dir ACL (parent) --"
(Get-Acl -LiteralPath "C:\Windows\Logs").Access | ForEach-Object {
    P ("  pace " + $_.IdentityReference) ($_.FileSystemRights.ToString() + " / " + $_.AccessControlType + " / " + $_.IsInherited)
}

P "-- write tests --"
foreach ($p in @("C:\Windows\Logs\DISM\dism.log", "C:\Windows\Logs\DISM\probe-write-test.tmp", "C:\Windows\Logs\probe-write-test.tmp")) {
    try { $fs = [System.IO.File]::OpenWrite($p); $fs.Close(); P ("write " + $p) "OK" }
    catch { P ("write " + $p) ("DENIED: " + $_.Exception.Message) }
}
Remove-Item -LiteralPath "C:\Windows\Logs\DISM\probe-write-test.tmp" -ErrorAction SilentlyContinue
Remove-Item -LiteralPath "C:\Windows\Logs\probe-write-test.tmp" -ErrorAction SilentlyContinue

SEC "RUN REAGENTC VIA UNC (definitely the 64-bit file)"
$rc = & cmd.exe /c "\\localhost\C$\Windows\System32\reagentc.exe /info 2>&1"
P "UNC 64bit reagentc /info EXIT" $LASTEXITCODE
$rc | Select-Object -First 10 | ForEach-Object { Write-Output ("  U| " + $_) }

SEC "RUN SYWOW64 REAGENTC EXPLICITLY (if present)"
if (Test-Path -LiteralPath "C:\Windows\SysWOW64\reagentc.exe") {
    $rc2 = & cmd.exe /c "C:\Windows\SysWOW64\reagentc.exe /info 2>&1"
    P "SysWOW64 reagentc /info EXIT" $LASTEXITCODE
    $rc2 | Select-Object -First 6 | ForEach-Object { Write-Output ("  W| " + $_) }
} else { P "SysWOW64 reagentc" "MISSING" }

Write-Output "PROBE2_DONE"

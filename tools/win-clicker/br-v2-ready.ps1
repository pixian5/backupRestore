# br-v2-ready.ps1 - verify ACL fix is in place and read the GUI state before re-running V2
$ErrorActionPreference = 'Continue'
function P($n, $v) { Write-Output ("{0,-44}: {1}" -f $n, $v) }
function SEC($n) { Write-Output ("=" * 62); Write-Output "== $n" }

SEC "ACL STATE (should now include Administrators)"
& cmd.exe /c "icacls C:\Windows\Logs\DISM 2>&1" | ForEach-Object { Write-Output ("  d| " + $_) }
& cmd.exe /c "icacls C:\Windows\Logs\DISM\dism.log 2>&1" | ForEach-Object { Write-Output ("  f| " + $_) }

SEC "GUI PROCESS"
$g = Get-Process BackupRestore -ErrorAction SilentlyContinue
if ($g) { $g | ForEach-Object { P ("pid " + $_.Id) ($_.Path + " session=" + $_.SessionId + " start=" + $_.StartTime.ToString("HH:mm:ss")) } }
else { P "BackupRestore" "NOT RUNNING" }

SEC "GUI PROCESSES (all BackupRestore* variants)"
Get-Process | Where-Object { $_.Name -match "BackupRestore|br-" } | ForEach-Object {
    P ("pid " + $_.Id + " " + $_.Name) ($_.Path + " start=" + $_.StartTime.ToString("HH:mm:ss"))
}

SEC "GUI LOG TAIL (last 20)"
$gui = "C:\Users\Public\backupRestore-package\logs\gui.log"
if (Test-Path -LiteralPath $gui) { Get-Content -LiteralPath $gui -Tail 20 | ForEach-Object { Write-Output ("  G| " + $_) } }

SEC "TASK DIR LIST"
Get-ChildItem "H:\brwork\tasks" -Directory -ErrorAction SilentlyContinue | ForEach-Object {
    P ("task " + $_.Name) ($_.LastWriteTime.ToString("HH:mm:ss"))
}

SEC "PENDING NON-TERMINAL TASKS (status.json)"
Get-ChildItem "H:\brwork\tasks" -Directory -ErrorAction SilentlyContinue | ForEach-Object {
    $s = Join-Path $_.FullName "status.json"
    if (Test-Path -LiteralPath $s) {
        try { $j = Get-Content -LiteralPath $s -Raw | ConvertFrom-Json; P ("status " + $_.Name) ($j.state + " stage=" + $j.stage + " success=" + $j.success) }
        catch { P ("status " + $_.Name) "PARSE_FAIL" }
    } else { P ("status " + $_.Name) "no status.json" }
}
Write-Output "V2READY_DONE"
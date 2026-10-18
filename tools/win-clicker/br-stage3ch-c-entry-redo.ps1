# br-stage3ch-c-entry-redo.ps1 -- C entry redo (system to non-C) -> F:, online channel, v2.6.16
# WHY from H:\brwork via scheduled task: the package-dir workspace still holds the
# stuck ef71fccc (Backup/Capturing) task; create_task would pop the
# "unfinished task exists" MB_YESNOCANCEL trap (yes=reboot-resume, no=cleanup
# refused for Capturing, cancel=abort). H:\brwork tasks are all terminal, so the
# hook flow runs dialog-free (same proven pattern as br-stage5-a-restore.ps1).
# The C-entry task itself is created under H:\brwork\tasks (phase 5 A entry did
# the same). Target F: != program volume H:, so no self-destruct guard issue.
# ASCII-only body (GBK-safe for prlctl exec).
$ErrorActionPreference = 'Continue'
$lines = New-Object System.Collections.ArrayList
function W($s) { [void]$lines.Add($s) }

# 1) kill old GUI
Get-Process -Name BackupRestore -ErrorAction SilentlyContinue | ForEach-Object {
  W ("killing old pid=" + $_.Id)
  Stop-Process -Id $_.Id -Force
}
Start-Sleep -Milliseconds 1500

# 2) write test hook config: tab=restore-other (C entry) -> op_index=3; channel=online
$json = @'
{
  "tab": "restore-other",
  "image": "H:\\BRImages\\BR-sys-20261018.wim",
  "target_volume": "F",
  "channel": "online",
  "auto_install": true
}
'@
[System.IO.File]::WriteAllText("C:\br-test.json", $json, (New-Object System.Text.UTF8Encoding($false)))
W "br-test.json written"

# 3) launch GUI from H:\brwork with --test-hook via scheduled task (Session 1, elevated)
$exe = 'H:\brwork\BackupRestore.exe'
$arg = '--test-hook'
$action = New-ScheduledTaskAction -Execute $exe -Argument $arg
if ($null -eq $action) { W "ACTION_NULL" } else { W ("action ok: " + $action.Execute + " " + $action.Arguments) }
$principal = New-ScheduledTaskPrincipal -UserId 'x' -LogonType Interactive -RunLevel Highest
$settings = New-ScheduledTaskSettingsSet -ExecutionTimeLimit (New-TimeSpan -Minutes 120) -AllowStartIfOnBatteries
Register-ScheduledTask -TaskName 'BRGUIC' -Action $action -Principal $principal -Settings $settings -Force | Out-Null
Start-ScheduledTask -TaskName 'BRGUIC'
Start-Sleep -Seconds 8
$info = Get-ScheduledTaskInfo -TaskName 'BRGUIC'
W ("C launched lastResult=" + $info.LastTaskResult)
Unregister-ScheduledTask -TaskName 'BRGUIC' -Confirm:$false
$p = Get-Process -Name BackupRestore -ErrorAction SilentlyContinue | Select-Object -First 1
if ($p) { W ("GUI_UP pid=" + $p.Id) } else { W "GUI_DOWN" }
$utf8 = New-Object System.Text.UTF8Encoding($false)
[System.IO.File]::WriteAllText("H:\brwork\stage3ch-relaunch.txt", ($lines -join "`r`n"), $utf8)
Write-Output "RELAUNCH_DONE"

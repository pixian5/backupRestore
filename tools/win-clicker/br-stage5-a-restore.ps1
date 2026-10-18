# br-stage5-a-restore.ps1 -- A entry (restore system to C:): launch GUI from H:\brwork + test hook
# WHY from H:: A entry target = C: = the program volume triggers the
# "program workspace volume matches restore target" self-destruct guard,
# so the GUI must run from H:\brwork (same as restart-gui-h.ps1).
# system_drive_choice=2: A entry is always offline; the hook adopts the
# "enter Windows RE" route directly, skipping the 3-button shunt window and
# the RE confirmation (established acceptance methodology; the destructive
# confirmation is still auto-accepted via TEST_AUTO_CONFIRM and logged).
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

# 2) write test hook config: tab=restore (A entry) -> op_index=1
$json = @'
{
  "tab": "restore",
  "image": "H:\\BRImages\\BR-sys-20261018.wim",
  "target_volume": "C",
  "system_drive_choice": 2,
  "auto_install": true
}
'@
[System.IO.File]::WriteAllText("C:\br-test.json", $json, (New-Object System.Text.UTF8Encoding($false)))
W "br-test.json written"

# 3) launch GUI from H:\brwork with --test-hook
$exe = 'H:\brwork\BackupRestore.exe'
$arg = '--test-hook'
$action = New-ScheduledTaskAction -Execute $exe -Argument $arg
if ($null -eq $action) { W "ACTION_NULL" } else { W ("action ok: " + $action.Execute + " " + $action.Arguments) }
$principal = New-ScheduledTaskPrincipal -UserId 'x' -LogonType Interactive -RunLevel Highest
$settings = New-ScheduledTaskSettingsSet -ExecutionTimeLimit (New-TimeSpan -Minutes 120) -AllowStartIfOnBatteries
Register-ScheduledTask -TaskName 'BRGUIA' -Action $action -Principal $principal -Settings $settings -Force | Out-Null
Start-ScheduledTask -TaskName 'BRGUIA'
Start-Sleep -Seconds 8
$info = Get-ScheduledTaskInfo -TaskName 'BRGUIA'
W ("A launched lastResult=" + $info.LastTaskResult)
Unregister-ScheduledTask -TaskName 'BRGUIA' -Confirm:$false
$p = Get-Process -Name BackupRestore -ErrorAction SilentlyContinue | Select-Object -First 1
if ($p) { W ("GUI_UP pid=" + $p.Id) } else { W "GUI_DOWN" }
$utf8 = New-Object System.Text.UTF8Encoding($false)
[System.IO.File]::WriteAllText("H:\brwork\stage5a-relaunch.txt", ($lines -join "`r`n"), $utf8)
Write-Output "RELAUNCH_DONE"

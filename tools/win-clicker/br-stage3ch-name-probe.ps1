# br-stage3ch-name-probe.ps1 -- zero-disk verification of the v2.6.16 menu-name fix
# Launches the GUI from H:\brwork with the C-entry hook but WITHOUT auto_install,
# so TEST_AUTO_CONFIRM stays false: after read_image prefills the menu name, the
# destructive confirmation (MB_YESNO) blocks and the operator can read the
# "startup menu name" line in its summary, then click No to cancel cleanly
# (no task, no format, no apply, zero disk writes).
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

# 2) write test hook config: C entry -> F:, online; NO auto_install
$json = @'
{
  "tab": "restore-other",
  "image": "H:\\BRImages\\BR-sys-20261018.wim",
  "target_volume": "F",
  "channel": "online"
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
Register-ScheduledTask -TaskName 'BRGUIP' -Action $action -Principal $principal -Settings $settings -Force | Out-Null
Start-ScheduledTask -TaskName 'BRGUIP'
Start-Sleep -Seconds 8
$info = Get-ScheduledTaskInfo -TaskName 'BRGUIP'
W ("probe launched lastResult=" + $info.LastTaskResult)
Unregister-ScheduledTask -TaskName 'BRGUIP' -Confirm:$false
$p = Get-Process -Name BackupRestore -ErrorAction SilentlyContinue | Select-Object -First 1
if ($p) { W ("GUI_UP pid=" + $p.Id) } else { W "GUI_DOWN" }
$utf8 = New-Object System.Text.UTF8Encoding($false)
[System.IO.File]::WriteAllText("H:\brwork\stage3ch-probe-launch.txt", ($lines -join "`r`n"), $utf8)
Write-Output "PROBE_LAUNCH_DONE"

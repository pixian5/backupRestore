# br-stage4b-pe-harddisk.ps1 -- Phase 4b (install PE, hard disk mode): launch GUI
# from H:\brwork + test hook WITHOUT auto_install (auto_install would fire the
# install before the RAM/DISK radio can be switched). Flow after launch:
#   agent click 1413 (radio DISK) -> click 1003 (create task) -> click 6 (yes on
#   the destructive confirm) -> [format + dism apply + bcd] -> click 2 (OK).
# target_volume=T: the hook sets the shared target combo exactly what
# install_pe_harddisk reads (selected_drive_letter(state, state.controls.target)).
# image = same marker WIM as the RAM test (winpeshl echoes SYSDRV, so the
# capture shows SYSDRV=T: when booted from the partition).
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

# 2) write test hook config: tab=pe -> op_index=4, no auto_install
$json = @'
{
  "tab": "pe",
  "image": "H:\\BRImages\\BR-PE-marker.wim",
  "target_volume": "T",
  "pe_name": "BR PE marker DISK"
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
Register-ScheduledTask -TaskName 'BRGUIPE' -Action $action -Principal $principal -Settings $settings -Force | Out-Null
Start-ScheduledTask -TaskName 'BRGUIPE'
Start-Sleep -Seconds 8
$info = Get-ScheduledTaskInfo -TaskName 'BRGUIPE'
W ("PE launched lastResult=" + $info.LastTaskResult)
Unregister-ScheduledTask -TaskName 'BRGUIPE' -Confirm:$false
$p = Get-Process -Name BackupRestore -ErrorAction SilentlyContinue | Select-Object -First 1
if ($p) { W ("GUI_UP pid=" + $p.Id) } else { W "GUI_DOWN" }
$utf8 = New-Object System.Text.UTF8Encoding($false)
[System.IO.File]::WriteAllText("H:\brwork\stage4b-pe-launch.txt", ($lines -join "`r`n"), $utf8)
Write-Output "RELAUNCH_DONE"

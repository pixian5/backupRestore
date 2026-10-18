# br-stage4-pe-install.ps1 -- Phase 4 (install PE, RAM disk mode): launch GUI
# from H:\brwork + test hook. Program volume = H: so pe_dir on T: passes the
# "target volume must differ from program volume" guard.
# image = marker WIM (copy of D:\sources\boot.wim index 1 + injected winpeshl.ini
# that launches cmd /k echo marker, so the PE boot is visible in a screen shot).
# pe_dir boot.sdi pre-staged from D:\Boot\boot.sdi (the program requires it and
# only looks in C:\WinPE_arm64\media\boot or the ADK path).
# tab=pe -> op_index=4 (install-pe-entry); RAM radio is default (WS_GROUP first).
# auto_install -> TEST_AUTO_CONFIRM + WM_APP_TEST_INSTALL -> create_task.
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

# 2) write test hook config: tab=pe -> op_index=4
$json = @'
{
  "tab": "pe",
  "image": "H:\\BRImages\\BR-PE-marker.wim",
  "pe_dir": "T:\\BackupRestorePE",
  "pe_name": "BR PE marker RAM",
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
Register-ScheduledTask -TaskName 'BRGUIPE' -Action $action -Principal $principal -Settings $settings -Force | Out-Null
Start-ScheduledTask -TaskName 'BRGUIPE'
Start-Sleep -Seconds 8
$info = Get-ScheduledTaskInfo -TaskName 'BRGUIPE'
W ("PE launched lastResult=" + $info.LastTaskResult)
Unregister-ScheduledTask -TaskName 'BRGUIPE' -Confirm:$false
$p = Get-Process -Name BackupRestore -ErrorAction SilentlyContinue | Select-Object -First 1
if ($p) { W ("GUI_UP pid=" + $p.Id) } else { W "GUI_DOWN" }
$utf8 = New-Object System.Text.UTF8Encoding($false)
[System.IO.File]::WriteAllText("H:\brwork\stage4-pe-launch.txt", ($lines -join "`r`n"), $utf8)
Write-Output "RELAUNCH_DONE"

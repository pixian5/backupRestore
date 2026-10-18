# br-run3-launch.ps1 -- v2.6.18 run 3: BCD rollback fix verification (C entry -> E:, online)
# v2.6.18 fixes (this run validates):
#   1. rollback_boot_request no longer falls back to `bcdedit /import` when the raw
#      ESP BCD copy is locked (error 32) -- that import poisoned the session's BFSVC
#      (run 2: bcdboot all failed afterwards until reboot). Now it fails cleanly.
#   2. trigger_bcd_rollback shows a modal error box on failure (was log-only).
# Test plan: press B mid-STEP 3 (pre-bcdboot) -> expect clean SUCCESS;
#            press B after STEP 3.6 (bcdboot wrote BCD, locked) -> expect clean
#            FAILURE + modal, task continues to completion.
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

# 2) write test hook config: tab=restore-other (C entry) -> target E:; channel=online
$json = @'
{
  "tab": "restore-other",
  "image": "H:\\BRImages\\BR-sys-20261018.wim",
  "target_volume": "E",
  "channel": "online",
  "auto_install": true
}
'@
[System.IO.File]::WriteAllText("C:\br-test.json", $json, (New-Object System.Text.UTF8Encoding($false)))
W "br-test.json written"

# 3) sentinel on target volume root (must disappear after restore; iron evidence)
$stamp = Get-Date -Format 'HHmmss'
$sentinel = "E:\BR-$stamp.txt"
[System.IO.File]::WriteAllText($sentinel, "run3 sentinel $stamp v2.6.18`r`n", (New-Object System.Text.UTF8Encoding($false)))
W ("SENTINEL=$sentinel")
$v = Get-CimInstance Win32_LogicalDisk | Where-Object { $_.DeviceID -eq 'E:' }
W ("E_SERIAL_BEFORE=" + $v.VolumeSerialNumber)

# 4) launch GUI from H:\brwork with --test-hook via scheduled task (Session 1, elevated)
$exe = 'H:\brwork\BackupRestore.exe'
$arg = '--test-hook'
$action = New-ScheduledTaskAction -Execute $exe -Argument $arg
$principal = New-ScheduledTaskPrincipal -UserId 'x' -LogonType Interactive -RunLevel Highest
$settings = New-ScheduledTaskSettingsSet -ExecutionTimeLimit (New-TimeSpan -Minutes 120) -AllowStartIfOnBatteries
Register-ScheduledTask -TaskName 'BRGUIC' -Action $action -Principal $principal -Settings $settings -Force | Out-Null
Start-ScheduledTask -TaskName 'BRGUIC'
Start-Sleep -Seconds 8
$info = Get-ScheduledTaskInfo -TaskName 'BRGUIC'
W ("launched lastResult=" + $info.LastTaskResult)
Unregister-ScheduledTask -TaskName 'BRGUIC' -Confirm:$false
$p = Get-Process -Name BackupRestore -ErrorAction SilentlyContinue | Select-Object -First 1
if ($p) { W ("GUI_UP pid=" + $p.Id) } else { W "GUI_DOWN" }
$utf8 = New-Object System.Text.UTF8Encoding($false)
[System.IO.File]::WriteAllText("\\Mac\backupRestore\tools\win-clicker\_evidence\phase6-progress-ui\run3-launch.txt", ($lines -join "`r`n"), $utf8)
Write-Output "RUN3_LAUNCH_DONE"

# br-stage3c-c-entry-redo.ps1 -- relaunch GUI with --test-hook: C entry (system to non-C) -> F:, online channel, v2.6.16
$ErrorActionPreference = 'Continue'
$lines = New-Object System.Collections.ArrayList
function W($s) { [void]$lines.Add($s) }

# 1) kill old GUI
Get-Process -Name BackupRestore -ErrorAction SilentlyContinue | ForEach-Object {
  W ("killing old pid=" + $_.Id)
  Stop-Process -Id $_.Id -Force
}
Start-Sleep -Milliseconds 1500

# 2) write test hook config: restore-other = C entry; channel=online (no reboot)
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

# 3) relaunch with --test-hook from the package dir (target F: != program volume C:, no self-destruct protection)
Start-Process -FilePath 'H:\brwork\BackupRestore.exe' -ArgumentList '--test-hook'
Start-Sleep -Milliseconds 3000
$p = Get-Process -Name BackupRestore -ErrorAction SilentlyContinue | Select-Object -First 1
if ($p) { W ("new pid=" + $p.Id) } else { W "NO_PROCESS" }
$utf8 = New-Object System.Text.UTF8Encoding($false)
[System.IO.File]::WriteAllText("H:\brwork\stage3c-relaunch.txt", ($lines -join "`r`n"), $utf8)
Write-Output "RELAUNCH_DONE"

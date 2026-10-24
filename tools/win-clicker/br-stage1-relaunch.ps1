# br-stage1-relaunch.ps1 -- 带 --test-hook 重启 GUI：自填备份字段 + RE 路由 + 自动创建
$ErrorActionPreference = 'Continue'
$lines = New-Object System.Collections.ArrayList
function W($s) { [void]$lines.Add($s) }

# 1) 关掉旧 GUI
Get-Process -Name BackupRestore -ErrorAction SilentlyContinue | ForEach-Object {
  W ("killing old pid=" + $_.Id)
  Stop-Process -Id $_.Id -Force
}
Start-Sleep -Milliseconds 1500

# 2) 写测试钩子配置（GUI 启动时自读）
$json = @'
{
  "tab": "backup",
  "image": "H:\\BRImages\\BR-sys-20261018.wim",
  "system_drive_choice": 2,
  "auto_install": true
}
'@
[System.IO.File]::WriteAllText("C:\br-test.json", $json, (New-Object System.Text.UTF8Encoding($false)))
W "br-test.json written"

# 3) 带 --test-hook 重启（agent 本身已提权，GUI 继承 High IL）
$env:BACKUPRESTORE_OPEN_IMAGE = "H:\BRImages\BR-sys-20261018.wim"
Start-Process -FilePath 'H:\brwork\BackupRestore.exe' -ArgumentList '--test-hook'
Start-Sleep -Milliseconds 3000
$p = Get-Process -Name BackupRestore -ErrorAction SilentlyContinue | Select-Object -First 1
if ($p) { W ("new pid=" + $p.Id + " session=" + $p.SessionId) } else { W "NO_PROCESS" }
$utf8 = New-Object System.Text.UTF8Encoding($false)
[System.IO.File]::WriteAllText("H:\brwork\stage1-relaunch.txt", ($lines -join "`r`n"), $utf8)
Write-Output "RELAUNCH_DONE"

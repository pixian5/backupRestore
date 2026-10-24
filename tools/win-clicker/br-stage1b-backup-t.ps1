# br-stage1b-backup-t.ps1 -- 带 --test-hook 重启 GUI：备份 T: 数据卷（在线通道，自动创建）
$ErrorActionPreference = 'Continue'
$lines = New-Object System.Collections.ArrayList
function W($s) { [void]$lines.Add($s) }

# 1) 关掉旧 GUI
Get-Process -Name BackupRestore -ErrorAction SilentlyContinue | ForEach-Object {
  W ("killing old pid=" + $_.Id)
  Stop-Process -Id $_.Id -Force
}
Start-Sleep -Milliseconds 1500

# 2) 写测试钩子配置（GUI 启动时自读）；source_volume=T 数据卷 → 在线备份，无 PE/RE 弹窗
$json = @'
{
  "tab": "backup",
  "source_volume": "T",
  "image": "H:\\BRImages\\BR-data-20261018.wim",
  "auto_install": true
}
'@
[System.IO.File]::WriteAllText("C:\br-test.json", $json, (New-Object System.Text.UTF8Encoding($false)))
W "br-test.json written"

# 3) 带 --test-hook 重启
Start-Process -FilePath 'H:\brwork\BackupRestore.exe' -ArgumentList '--test-hook'
Start-Sleep -Milliseconds 3000
$p = Get-Process -Name BackupRestore -ErrorAction SilentlyContinue | Select-Object -First 1
if ($p) { W ("new pid=" + $p.Id) } else { W "NO_PROCESS" }
$utf8 = New-Object System.Text.UTF8Encoding($false)
[System.IO.File]::WriteAllText("H:\brwork\stage1b-relaunch.txt", ($lines -join "`r`n"), $utf8)
Write-Output "RELAUNCH_DONE"

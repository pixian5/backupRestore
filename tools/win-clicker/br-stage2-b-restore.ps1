# br-stage2-b-restore.ps1 -- 带 --test-hook 重启 GUI：B 入口（还原数据到非 C 盘）→ T:
# 默认勾选「格式化目标分区」；auto_install 自动接受破坏性确认
$ErrorActionPreference = 'Continue'
$lines = New-Object System.Collections.ArrayList
function W($s) { [void]$lines.Add($s) }

# 1) 关掉旧 GUI
Get-Process -Name BackupRestore -ErrorAction SilentlyContinue | ForEach-Object {
  W ("killing old pid=" + $_.Id)
  Stop-Process -Id $_.Id -Force
}
Start-Sleep -Milliseconds 1500

# 2) 写测试钩子配置
$json = @'
{
  "tab": "restore-data",
  "image": "H:\\BRImages\\BR-data-20261018.wim",
  "target_volume": "T",
  "auto_install": true
}
'@
[System.IO.File]::WriteAllText("C:\br-test.json", $json, (New-Object System.Text.UTF8Encoding($false)))
W "br-test.json written"

# 3) 带 --test-hook 重启
Start-Process -FilePath 'C:\Users\Public\backupRestore-package\BackupRestore.exe' -ArgumentList '--test-hook'
Start-Sleep -Milliseconds 3000
$p = Get-Process -Name BackupRestore -ErrorAction SilentlyContinue | Select-Object -First 1
if ($p) { W ("new pid=" + $p.Id) } else { W "NO_PROCESS" }
$utf8 = New-Object System.Text.UTF8Encoding($false)
[System.IO.File]::WriteAllText("C:\Users\Public\backupRestore-package\stage2b-relaunch.txt", ($lines -join "`r`n"), $utf8)
Write-Output "RELAUNCH_DONE"

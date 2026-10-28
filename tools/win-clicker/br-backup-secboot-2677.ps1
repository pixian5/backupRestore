# br-backup-secboot-2677.ps1 -- 在安全启动开着的虚拟机上，用 GUI 测试钩子触发一次**真实系统备份**（C: → 离线通道）。
# 走的是与用户点「备份」页签 → 创建任务 完全相同的代码路径（钩子只负责把控件设成点完的状态 + 自动按「是」）。
# 备份必然重启进恢复环境，所以本脚本只负责「发起」，不等待结果。
#
# 用法：./br-s1.sh br-backup-secboot.ps1
$ErrorActionPreference = 'Continue'
$lines = New-Object System.Collections.ArrayList
function W($s) { [void]$lines.Add($s) }

# 1) 关掉可能在跑的旧 GUI（测试钩子只在启动时读配置）
Get-Process -Name BackupRestore -ErrorAction SilentlyContinue | ForEach-Object {
    W ("killing old pid=" + $_.Id)
    Stop-Process -Id $_.Id -Force
}
Start-Sleep -Milliseconds 2000

# 2) 测试钩子配置：备份页、离线通道（C: 是活动系统卷，必须走离线）、源卷 C、镜像放 H:
$json = @'
{
  "tab": "backup",
  "channel": "offline",
  "source_volume": "C",
  "image": "H:\\BRImages\\c-secboot-2677.wim",
  "auto_install": true
}
'@
[System.IO.File]::WriteAllText("C:\br-test.json", $json, (New-Object System.Text.UTF8Encoding($false)))
W "br-test.json written"

# 3) 带 --test-hook 启动 GUI
Start-Process -FilePath 'H:\brwork\BackupRestore.exe' -ArgumentList '--test-hook'
Start-Sleep -Seconds 30

# 4) 落结果（此时应当已经建好任务并进入准备/重启阶段）
$p = Get-Process -Name BackupRestore -ErrorAction SilentlyContinue | Select-Object -First 1
if ($p) { W ("GUI_UP pid=" + $p.Id) } else { W "GUI_DOWN" }
$utf8 = New-Object System.Text.UTF8Encoding($false)
[System.IO.File]::WriteAllText("H:\brwork\backup-secboot-launch.txt", ($lines -join "`r`n"), $utf8)
Write-Output "BACKUP_LAUNCH_DONE"

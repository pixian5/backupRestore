# br-restore-pe-2677.ps1 -- 用「离线还原到数据卷 + 选 PE」触发 PE 侧执行链
# 目的：让 PE 里的 execute() 真跑一次，目验 2.6.77 的两处修复：
#   a) 进度窗布局：阶段标题不被进度条压住、底部 5 按钮独占一行、步骤列表变矮、日志区变大
#   b) 后台 cmd：进度窗出现之前先弹出「最大化 + 在后台」的命令提示符（用户反馈 PE 链此前没有）
# 镜像用刚构建的 2.6.77 PE 镜像本身（412 MB）；还原完成后 T: 上就是新版 PE，终态干净。
# system_drive_choice=1 直接选 PE，跳过弹窗；auto_install 自动确认并直接创建任务。
# 用法：./br-s1.sh br-restore-pe-2677.ps1
$ErrorActionPreference = 'Continue'
$lines = New-Object System.Collections.ArrayList
function W($s) { [void]$lines.Add($s) }

# 1) 关掉可能在跑的旧 GUI（测试钩子只在启动时读配置）
Get-Process -Name BackupRestore -ErrorAction SilentlyContinue | ForEach-Object {
    W ("killing old pid=" + $_.Id)
    Stop-Process -Id $_.Id -Force
}
Start-Sleep -Milliseconds 2000

# 2) 写测试钩子配置：restore-data（B 数据卷还原）+ offline（离线通道）
#    target_volume=T（已格式化的 scratch 分区）；system_drive_choice=1 = 选 PE
$json = @'
{
  "tab": "restore-data",
  "channel": "offline",
  "image": "H:\\BRImages\\BackupRestorePE-2677-test.wim",
  "index": 1,
  "target_volume": "T",
  "format_target": true,
  "system_drive_choice": 1,
  "auto_install": true
}
'@
[System.IO.File]::WriteAllText("C:\br-test.json", $json, (New-Object System.Text.UTF8Encoding($false)))
W "br-test.json written"

# 3) 带 --test-hook 启动 GUI（本脚本已在 Session 1 提权上下文，直接起即可）
Start-Process -FilePath 'H:\brwork\BackupRestore.exe' -ArgumentList '--test-hook'
Start-Sleep -Seconds 30

# 4) 落结果（重启由产品侧 bootsequence 负责，这里只记录 GUI 起来没有）
$p = Get-Process -Name BackupRestore -ErrorAction SilentlyContinue | Select-Object -First 1
if ($p) { W ("GUI_UP pid=" + $p.Id) } else { W "GUI_DOWN" }
$utf8 = New-Object System.Text.UTF8Encoding($false)
[System.IO.File]::WriteAllText("H:\brwork\restore-pe-2677.txt", ($lines -join "`r`n"), $utf8)
Write-Output "RESTORE_PE_SCRIPT_DONE"

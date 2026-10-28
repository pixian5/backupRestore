# br-pe-install-secboot.ps1 -- 用 GUI 的测试钩子跑一次**真实**的「安装 PE（硬盘模式）」
# 目的：在安全启动开着的虚拟机上，验证产品自己的安装流程（格式化 + dism 展开 + 建 BCD 项）能不能走通。
# 与 br-stage4-pe-install.ps1 的区别：那个用 marker WIM（假镜像），这个用仓库里真实的 2.6.76 镜像。
#
# 前置：目标分区必须是「空的、非系统盘、非 ESP、非程序所在卷（H:）」，
#       否则 install_pe_harddisk 的安全校验会拦（目标分区含 Windows 安装也不行 → 先格式化）。
# 用法：./br-s1.sh br-pe-install-secboot.ps1
$ErrorActionPreference = 'Continue'
$lines = New-Object System.Collections.ArrayList
function W($s) { [void]$lines.Add($s) }

# 1) 关掉可能在跑的旧 GUI（测试钩子只在启动时读配置）
Get-Process -Name BackupRestore -ErrorAction SilentlyContinue | ForEach-Object {
    W ("killing old pid=" + $_.Id)
    Stop-Process -Id $_.Id -Force
}
Start-Sleep -Milliseconds 2000

# 2) 写测试钩子配置：tab=pe（安装 PE）、mode=disk（硬盘启动方式）、auto_install（自动接受破坏性确认）
$json = @'
{
  "tab": "pe",
  "mode": "disk",
  "image": "C:\\BackupRestorePE\\BackupRestorePE.wim",
  "pe_name": "BR PE secboot",
  "target_volume": "T",
  "auto_install": true
}
'@
[System.IO.File]::WriteAllText("C:\br-test.json", $json, (New-Object System.Text.UTF8Encoding($false)))
W "br-test.json written"

# 3) 带 --test-hook 启动 GUI（本脚本已在 Session 1 提权上下文，直接起即可）
Start-Process -FilePath 'H:\brwork\BackupRestore.exe' -ArgumentList '--test-hook'
Start-Sleep -Seconds 75

# 4) 落结果
$p = Get-Process -Name BackupRestore -ErrorAction SilentlyContinue | Select-Object -First 1
if ($p) { W ("GUI_UP pid=" + $p.Id) } else { W "GUI_DOWN" }
$utf8 = New-Object System.Text.UTF8Encoding($false)
[System.IO.File]::WriteAllText("H:\brwork\pe-install-secboot.txt", ($lines -join "`r`n"), $utf8)
Write-Output "PE_INSTALL_SCRIPT_DONE"

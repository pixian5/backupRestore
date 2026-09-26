# br-gui-exec.ps1 —— 在 VM 可见桌面里执行输入注入（宿主侧桥接，用普通 exec 权限运行）
#
# 为什么需要这一层：
#   1) 提权(runas)进程看不到 X: 网络盘（驱动器映射绑在中等完整性令牌上）
#      → 目标脚本必须先复制到客体本地磁盘 C:\ 再提权执行
#   2) 前台若是提权程序（BackupRestore GUI 就是），中等完整性进程注入会被 UIPI 拦截
#      （实测 SetCursorPos 返回 False、光标纹丝不动）→ 注入进程必须同为 High IL
#   3) Shell.Application.ShellExecute(...,"runas",0) 能在同一桌面提权启动，仍在 Session 1
#
# 用法（VM 内，普通用户上下文即可）：
#   powershell -File br-gui-exec.ps1 -Script clicker.ps1 -B64 <base64("-X 960 -Y 540 -Click")>
#   -Script 指 X:\tools\win-clicker\ 下的脚本名；-B64 用来绕开命令行引号/空格被吃掉的问题
#   （prlctl exec 传参会吞引号，这是踩过的坑）。
#   -Log 给定时，目标脚本的 stdout/stderr 会落到 C:\Users\Public\pkg\<Log>。

param(
    [string]$Script = "clicker.ps1",
    [string]$Args = "",
    [string]$B64 = "",
    [string]$Log = "",
    [switch]$NoElevate
)

if ($B64 -ne "") {
    $Args = [System.Text.Encoding]::UTF8.GetString([System.Convert]::FromBase64String($B64))
}

$p = "C:\Users\Public\pkg"
$pkg = "C:\Users\Public\backupRestore-package"
$src = "X:\tools\win-clicker\$Script"
if (-not (Test-Path $src)) { Write-Output "MISSING_SRC $src"; exit 2 }
Copy-Item $src "$pkg\$Script" -Force

$cmdline = "powershell -NoProfile -ExecutionPolicy Bypass -File `"" + $pkg + "\" + $Script + "`" " + $Args
if ($Log -ne "") { $cmdline += " > " + $p + "\" + $Log + " 2>&1" }

if ($NoElevate) {
    & cmd.exe /c $cmdline
    Write-Output "BRGUI_RAN_NOELEV $Script $Args"
    exit 0
}

$bat = "$p\_brgui.cmd"
Set-Content $bat "chcp 437 >nul" -Encoding ASCII
Add-Content $bat $cmdline -Encoding ASCII
$s = New-Object -ComObject Shell.Application
$s.ShellExecute("cmd.exe", "/c `"" + $bat + "`"", "", "runas", 0)
Write-Output "BRGUI_LAUNCHED $Script $Args"

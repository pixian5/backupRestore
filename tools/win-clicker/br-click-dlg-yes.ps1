# br-click-dlg-yes.ps1 - click a button (default "是(&Y)" id=6) of the BackupRestore
# native modal dialog (class #32770, owned by the BackupRestore process). Elevated,
# Session 1. Also used for completion dialogs' 确定 (id=2).
#
# 2026-10-24 增补「点击后复验」：实测 PostMessage(BM_CLICK) 后对话框可能仍在
# （首投丢失，疑为对话框刚创建尚未进入消息循环；当时无任何执行痕迹：无新日志、
# 无新任务目录、目标卷序列号未变可证伪"已执行"）。点击回执不能当验收依据——
# 本脚本点击后等待并复查：对话框消失 / 换成新对话框（如确认框→执行完成框）/
# 原对话框仍在（此时报 STILL_THERE，调用方应重试或改走键盘）。
param(
  [int]$ButtonId = 6,
  [string]$Title = "",
  [int]$VerifyMs = 2500,
  [int]$Retry = 1
)
Add-Type -TypeDefinition @"
using System;
using System.Runtime.InteropServices;
using System.Text;
public class BrCY {
  [DllImport("user32.dll")] public static extern bool EnumWindows(EnumCb cb, IntPtr l);
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern int GetClassNameW(IntPtr h, StringBuilder s, int n);
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern int GetWindowTextW(IntPtr h, StringBuilder s, int n);
  [DllImport("user32.dll")] public static extern IntPtr GetDlgItem(IntPtr h, int id);
  [DllImport("user32.dll")] public static extern IntPtr SendMessageW(IntPtr h, uint m, IntPtr w, IntPtr l);
  [DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr h);
  [DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(IntPtr h, out uint pid);
  [DllImport("user32.dll")] public static extern bool SetForegroundWindow(IntPtr h);
  public delegate bool EnumCb(IntPtr h, IntPtr l);
  public const uint BM_CLICK = 0x00F5;
  public static string Cls(IntPtr h) { StringBuilder sb = new StringBuilder(256); GetClassNameW(h, sb, 256); return sb.ToString(); }
  public static string Text(IntPtr h) { StringBuilder sb = new StringBuilder(2048); GetWindowTextW(h, sb, 2048); return sb.ToString(); }
}
"@
function Find-BrDialog {
  $p = Get-Process -Name BackupRestore -ErrorAction SilentlyContinue | Select-Object -First 1
  if (-not $p) { return $null }
  $found = [IntPtr]::Zero
  $cb = [BrCY+EnumCb]{ param($h,$l)
    $pid2 = [uint32]0
    [void][BrCY]::GetWindowThreadProcessId($h, [ref]$pid2)
    if ($pid2 -eq $p.Id -and [BrCY]::Cls($h) -eq "#32770" -and [BrCY]::IsWindowVisible($h)) {
      $script:found = $h; return $false
    }
    return $true
  }
  [void][BrCY]::EnumWindows($cb, [IntPtr]::Zero)
  return $found
}
$dlg = Find-BrDialog
if ($dlg -eq [IntPtr]::Zero -or $dlg -eq $null) { Write-Output "NO_DIALOG"; exit 2 }
[void][BrCY]::SetForegroundWindow($dlg)
Start-Sleep -Milliseconds 300
$b = [BrCY]::GetDlgItem($dlg, $ButtonId)
if ($b -eq [IntPtr]::Zero) { Write-Output ("NO_BUTTON id=" + $ButtonId); exit 3 }
Write-Output ("DIALOG=" + $dlg.ToInt64() + " body=[" + ([BrCY]::Text($dlg) -replace "`r`n"," / ") + "]")
$attempt = 0
while ($true) {
  $attempt++
  [void][BrCY]::SendMessageW($b, [BrCY]::BM_CLICK, [IntPtr]::Zero, [IntPtr]::Zero)
  Write-Output ("CLICKED id=" + $ButtonId + " btn=" + $b.ToInt64() + " attempt=" + $attempt)
  Start-Sleep -Milliseconds $VerifyMs
  $now = Find-BrDialog
  if ($now -eq [IntPtr]::Zero -or $now -eq $null) {
    Write-Output "VERIFY=DIALOG_GONE"
    exit 0
  }
  if ($now.ToInt64() -ne $dlg.ToInt64()) {
    Write-Output ("VERIFY=NEW_DIALOG hwnd=" + $now.ToInt64() + " title=[" + [BrCY]::Text($now) + "]")
    exit 0
  }
  if ($attempt -gt $Retry) {
    Write-Output "VERIFY=STILL_THERE (click did not take effect; do NOT treat as done)"
    exit 4
  }
  Write-Output "VERIFY=STILL_THERE retrying once"
}
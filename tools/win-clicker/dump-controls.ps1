$out = 'C:\Users\Public\brv15\status-read.txt'
$log = 'C:\Users\Public\brv15\status-run.log'
Add-Type @"
using System;
using System.Text;
using System.Runtime.InteropServices;
public class VD {
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern bool EnumWindows(EnumWindowsProc cb, IntPtr lp);
  public delegate bool EnumWindowsProc(IntPtr hWnd, IntPtr lParam);
  [DllImport("user32.dll")] public static extern IntPtr SendMessageW(IntPtr h, uint msg, IntPtr wp, IntPtr lp);
  [DllImport("user32.dll", CharSet=CharSet.Unicode, EntryPoint="SendMessageW")] public static extern IntPtr SendMessageStr(IntPtr h, uint msg, IntPtr wp, StringBuilder lp);
  [DllImport("user32.dll")] public static extern IntPtr GetDlgItem(IntPtr h, int id);
  [DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr h);
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern int GetClassNameW(IntPtr h, StringBuilder s, int n);
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern bool GetWindowRect(IntPtr h, out R r);
  public struct R { public int l,t,r,b; }
  public const uint WM_GETTEXT=0x000D, WM_GETTEXTLENGTH=0x000E;
  // 2026-10-24 修复：读别的进程的控件文本必须用 WM_GETTEXT 消息。GetWindowTextW
  // API 跨进程只返回「创建时缓存」的窗口名：编辑框创建时为空就恒返回空、之后被
  // 程序改过（apply_language 换中文等）就返回陈旧默认值。实测对照见
  // .test-artifacts/recon-38-lite-20261024/editprobe2.ps1（apiText=[] vs
  // msgText=[Windows (T:) 20261024]）。顶层窗口标题虽不受影响，统一走消息更稳。
  public static string Text(IntPtr h) {
    int n = (int)SendMessageW(h, WM_GETTEXTLENGTH, IntPtr.Zero, IntPtr.Zero);
    if (n <= 0) return "";
    StringBuilder sb = new StringBuilder(n + 1);
    SendMessageStr(h, WM_GETTEXT, (IntPtr)sb.Capacity, sb);
    return sb.ToString();
  }
}
"@
$mainH = [IntPtr]::Zero
$cb = [VD+EnumWindowsProc]{ param($hw,$lp)
    $t = New-Object System.Text.StringBuilder 512
    [VD]::SendMessageStr($hw, [VD]::WM_GETTEXT, [IntPtr]512, $t) | Out-Null
    if ($t.ToString() -like '*BackupRestore*' -and [VD]::IsWindowVisible($hw)) { $script:mainH = $hw; return $false }
    return $true
}
[VD]::EnumWindows($cb,[IntPtr]::Zero) | Out-Null
[System.IO.File]::WriteAllText($out, "MAIN=" + $mainH + [Environment]::NewLine)
# 枚举主窗口下所有子控件，打印 id、窗口类、文本、位置
foreach ($id in 1200..1400) {
    $c = [VD]::GetDlgItem($mainH, $id)
    if ($c -eq [IntPtr]::Zero) { continue }
    $txt = [VD]::Text($c)
    $cls = New-Object System.Text.StringBuilder 64
    [VD]::GetClassNameW($c,$cls,64) | Out-Null
    $r = New-Object VD+R
    [VD]::GetWindowRect($c,[ref]$r) | Out-Null
    [System.IO.File]::AppendAllText($out, "ID=$id CLASS=$($cls.ToString()) XY=$($r.l),$($r.t) TEXT=[$txt]" + [Environment]::NewLine)
}
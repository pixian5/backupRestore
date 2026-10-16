# br-list-windows.ps1 - list ALL top-level windows (class, title, pid, visibility).
# Diagnostic: find out whether a second BackupRestore-titled window exists that
# title-based tooling (set-image-path.ps1) might latch onto instead of the real GUI.
param()
Add-Type -TypeDefinition @"
using System;
using System.Runtime.InteropServices;
using System.Text;
public class BrLW {
  [DllImport("user32.dll")] public static extern bool EnumWindows(EnumCb cb, IntPtr l);
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern int GetWindowTextW(IntPtr h, StringBuilder s, int n);
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern int GetClassNameW(IntPtr h, StringBuilder s, int n);
  [DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr h);
  [DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(IntPtr h, out uint pid);
  public delegate bool EnumCb(IntPtr h, IntPtr l);
  public static string Text(IntPtr h) { StringBuilder sb = new StringBuilder(512); GetWindowTextW(h, sb, 512); return sb.ToString(); }
  public static string Cls(IntPtr h) { StringBuilder sb = new StringBuilder(256); GetClassNameW(h, sb, 256); return sb.ToString(); }
  public static string Esc(string s) {
    var sb = new StringBuilder();
    foreach (char c in s) {
      if (c >= 32 && c < 127) sb.Append(c);
      else sb.Append("\\u").Append(((int)c).ToString("x4"));
    }
    return sb.ToString();
  }
}
"@
$script:tops = New-Object System.Collections.ArrayList
$cb = [BrLW+EnumCb]{ param($h,$l) [void]$script:tops.Add($h); return $true }
[void][BrLW]::EnumWindows($cb, [IntPtr]::Zero)
foreach ($h in $script:tops) {
    $procId = 0
    [void][BrLW]::GetWindowThreadProcessId($h, [ref]$procId)
    $t = [BrLW]::Esc([BrLW]::Text($h))
    $c = [BrLW]::Cls($h)
    if ($t.Length -gt 0 -or $c -match "BackupRestore") {
        $vis = "0"
        if ([BrLW]::IsWindowVisible($h)) { $vis = "1" }
        Write-Output ("hwnd=" + $h.ToInt64() + " pid=" + $procId + " vis=" + $vis + " class=" + $c + " title=[" + $t + "]")
    }
}
Write-Output "LIST_DONE"

# br-findtext.ps1 - locate EVERY window on this desktop whose text looks like a WIM
# path, so we can find which control the running app really reads its image path from.
# ASCII only. Output: C:\Users\Public\pkg\findtext.txt (UTF-8, no BOM)
$ErrorActionPreference = 'Continue'
Add-Type -TypeDefinition @"
using System;
using System.Text;
using System.Collections.Generic;
using System.Runtime.InteropServices;
public class BrFT {
  [DllImport("user32.dll")] public static extern bool EnumWindows(EnumCb cb, IntPtr l);
  [DllImport("user32.dll")] public static extern bool EnumChildWindows(IntPtr h, EnumCb cb, IntPtr l);
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern int GetWindowTextW(IntPtr h, StringBuilder s, int n);
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern int GetClassNameW(IntPtr h, StringBuilder s, int n);
  [DllImport("user32.dll")] public static extern int GetDlgCtrlID(IntPtr h);
  [DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(IntPtr h, out uint pid);
  [DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr h);
  [DllImport("user32.dll")] public static extern bool IsWindow(IntPtr h);
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern IntPtr FindWindowW(string cls, string title);
  [DllImport("user32.dll")] public static extern IntPtr GetDlgItem(IntPtr h, int id);
  public delegate bool EnumCb(IntPtr h, IntPtr l);
  public static string Text(IntPtr h) { StringBuilder sb = new StringBuilder(2048); GetWindowTextW(h, sb, 2048); return sb.ToString(); }
  public static string Cls(IntPtr h) { StringBuilder sb = new StringBuilder(256); GetClassNameW(h, sb, 256); return sb.ToString(); }
  public static List<string> Scan(string needle) {
    var res = new List<string>();
    EnumWindows(delegate(IntPtr h, IntPtr l) {
      Check(h, "TOP", needle, res);
      EnumChildWindows(h, delegate(IntPtr c, IntPtr l2) { Check(c, "CHILD", needle, res); return true; }, IntPtr.Zero);
      return true;
    }, IntPtr.Zero);
    return res;
  }
  static void Check(IntPtr h, string kind, string needle, List<string> res) {
    string t = Text(h);
    if (t == null || t.Length == 0) return;
    if (t.IndexOf(needle, StringComparison.OrdinalIgnoreCase) < 0) return;
    uint pid; GetWindowThreadProcessId(h, out pid);
    res.Add(kind + " hwnd=" + h.ToInt64() + " pid=" + pid + " id=" + GetDlgCtrlID(h) + " vis=" + (IsWindowVisible(h) ? 1 : 0) + " class=" + Cls(h) + " text=[" + t + "]");
  }
  public static string HandleInfo(IntPtr h, string tag) {
    if (h == IntPtr.Zero) return tag + " hwnd=NULL";
    return tag + " hwnd=" + h.ToInt64() + " isWindow=" + (IsWindow(h) ? 1 : 0) + " id=" + GetDlgCtrlID(h) + " class=" + Cls(h) + " text=[" + Text(h) + "]";
  }
}
"@
$lines = New-Object System.Collections.ArrayList
function W($s) { [void]$lines.Add($s) }
$root = [BrFT]::FindWindowW("BackupRestoreNativeGui", $null)
W ("ROOT=" + $root)
$img = [BrFT]::GetDlgItem($root, 1203)
W ([BrFT]::HandleInfo($img, "GetDlgItem(root,1203)"))
W ""
W "== windows containing 'test-fast' =="
foreach ($l in [BrFT]::Scan("test-fast")) { W $l }
W ""
W "== windows containing 'zzprobe' =="
foreach ($l in [BrFT]::Scan("zzprobe")) { W $l }
W ""
W "== windows containing '.wim' =="
foreach ($l in [BrFT]::Scan(".wim")) { W $l }
$utf8 = New-Object System.Text.UTF8Encoding($false)
[System.IO.File]::WriteAllText("C:\Users\Public\pkg\findtext.txt", ($lines -join "`r`n"), $utf8)
Write-Output "FINDTEXT_DONE"
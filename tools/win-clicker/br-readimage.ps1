# br-readimage.ps1 - invoke the GUI "read image" handler via WM_COMMAND (id 1002)
# after switching to the restore tab (id 1102). The handler logs
# "GUI action started: read WIM metadata; image=<path>" so we can see exactly
# which image path string the running app reads from its image control.
# ASCII only. Output: C:\Users\Public\pkg\readimage.txt
$ErrorActionPreference = 'Continue'
Add-Type -TypeDefinition @"
using System;
using System.Runtime.InteropServices;
using System.Text;
public class BrRI {
  [DllImport("user32.dll")] public static extern bool EnumWindows(EnumCb cb, IntPtr l);
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern int GetWindowTextW(IntPtr h, StringBuilder s, int n);
  [DllImport("user32.dll")] public static extern IntPtr GetDlgItem(IntPtr h, int id);
  [DllImport("user32.dll")] public static extern IntPtr SendMessageW(IntPtr h, uint msg, IntPtr wp, IntPtr lp);
  public delegate bool EnumCb(IntPtr h, IntPtr l);
  public static string Text(IntPtr h) { StringBuilder sb = new StringBuilder(1024); GetWindowTextW(h, sb, 1024); return sb.ToString(); }
  public static IntPtr Find() {
    IntPtr found = IntPtr.Zero;
    EnumWindows(delegate(IntPtr h, IntPtr l) {
      if (Text(h).StartsWith("BackupRestore - Rust GUI")) { found = h; return false; }
      return true;
    }, IntPtr.Zero);
    return found;
  }
}
"@
$root = [BrRI]::Find()
$lines = New-Object System.Collections.ArrayList
function W($s) { [void]$lines.Add($s) }
W ("ROOT=" + $root)
if ($root -eq [IntPtr]::Zero) { $lines -join "`r`n" | Set-Content "C:\Users\Public\pkg\readimage.txt" -Encoding ASCII; Write-Output "NO_WINDOW"; exit 1 }

# 1. switch to restore tab
$tab = [BrRI]::GetDlgItem($root, 1102)
[void][BrRI]::SendMessageW($root, 0x0111, [IntPtr]1102, $tab)
Start-Sleep -Milliseconds 800
W ("after tab switch: image_ctrl_text=[" + [BrRI]::Text([BrRI]::GetDlgItem($root, 1203)) + "]")
W ("imageCtrl hwnd=" + [BrRI]::GetDlgItem($root, 1203))

# 2. click "read image" (id 1002)
$btn = [BrRI]::GetDlgItem($root, 1002)
[void][BrRI]::SendMessageW($root, 0x0111, [IntPtr]1002, $btn)
Start-Sleep -Milliseconds 2500
W ("image_ctrl_text_after=[" + [BrRI]::Text([BrRI]::GetDlgItem($root, 1203)) + "]")
$lines -join "`r`n" | Set-Content "C:\Users\Public\pkg\readimage.txt" -Encoding ASCII
Write-Output "READIMAGE_DONE"
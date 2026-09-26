# br-gui-click.ps1 - press a BackupRestore GUI button by control id using BM_CLICK,
# then report the status static (id 1300) before/after.
#
# Why not coordinates: the agent's click() injects real mouse input at physical screen
# coordinates, but the window gets moved/restored between runs (see br-win-move.ps1), so
# stale coordinate tables silently hit the wrong control. BM_CLICK addresses the control
# directly and is frame-independent.
#
# ASCII only (PowerShell 5.1 -File decodes sources as system ANSI/GBK).
# Output: C:\Users\Public\pkg\guiclick.txt

param(
  [int]$Id = 1003,
  [string]$Note = "",
  [int]$WaitMs = 2000
)

$ErrorActionPreference = 'Continue'

Add-Type -TypeDefinition @"
using System;
using System.Runtime.InteropServices;
using System.Text;
public class BrGC {
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern IntPtr GetDlgItem(IntPtr h, int id);
  [DllImport("user32.dll")] public static extern IntPtr SendMessageW(IntPtr h, uint msg, IntPtr wp, IntPtr lp);
  [DllImport("user32.dll", CharSet=CharSet.Unicode, EntryPoint="GetWindowTextW")] public static extern int GetWindowTextW(IntPtr h, StringBuilder s, int n);
  [DllImport("user32.dll")] public static extern bool IsWindowEnabled(IntPtr h);
  [DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr h);
  [DllImport("user32.dll")] public static extern bool SetForegroundWindow(IntPtr h);
  [DllImport("user32.dll")] public static extern bool EnumWindows(EnumCb cb, IntPtr l);
  public delegate bool EnumCb(IntPtr h, IntPtr l);
  public const uint BM_CLICK = 0x00F5;
  public static string Text(IntPtr h) {
    if (h == IntPtr.Zero) return "";
    StringBuilder sb = new StringBuilder(4096);
    GetWindowTextW(h, sb, 4096);
    return sb.ToString();
  }
  public static IntPtr FindByTitle(string needle) {
    IntPtr found = IntPtr.Zero;
    EnumWindows(delegate(IntPtr h, IntPtr l) {
      string t = Text(h);
      if (t != null && t.IndexOf(needle, StringComparison.OrdinalIgnoreCase) >= 0) { found = h; return false; }
      return true;
    }, IntPtr.Zero);
    return found;
  }
}
"@

$pkg = "C:\Users\Public\pkg"
$l = New-Object System.Collections.Generic.List[string]
function W($s) { [void]$l.Add($s) }

$hwnd = [BrGC]::FindByTitle('BackupRestore - Rust GUI')
W ("NOTE=" + $Note)
W ("TIME=" + (Get-Date -Format "yyyy-MM-dd HH:mm:ss"))
W ("HWND=" + $hwnd + " title=[" + [BrGC]::Text($hwnd) + "]")
if ($hwnd -eq [IntPtr]::Zero) {
  [System.IO.File]::WriteAllText("$pkg\guiclick.txt", ($l -join "`r`n"), (New-Object System.Text.UTF8Encoding($false)))
  Write-Output "NO_WINDOW"; exit 1
}

$btn = [BrGC]::GetDlgItem($hwnd, $Id)
$stat = [BrGC]::GetDlgItem($hwnd, 1300)
W ("BEFORE button id=" + $Id + " hwnd=" + $btn + " text=[" + [BrGC]::Text($btn) + "] enabled=" + [BrGC]::IsWindowEnabled($btn) + " visible=" + [BrGC]::IsWindowVisible($btn))
W ("BEFORE status=[" + [BrGC]::Text($stat) + "]")

if ($btn -eq [IntPtr]::Zero) {
  W "BTN_NOT_FOUND"
} else {
  [void][BrGC]::SetForegroundWindow($hwnd)
  Start-Sleep -Milliseconds 300
  [void][BrGC]::SendMessageW($btn, [BrGC]::BM_CLICK, [IntPtr]::Zero, [IntPtr]::Zero)
  W ("BM_CLICK sent to id=" + $Id)
  Start-Sleep -Milliseconds $WaitMs
  W ("AFTER status=[" + [BrGC]::Text($stat) + "]")
  W ("AFTER button enabled=" + [BrGC]::IsWindowEnabled($btn))
}

W "DONE"
[System.IO.File]::WriteAllText("$pkg\guiclick.txt", ($l -join "`r`n"), (New-Object System.Text.UTF8Encoding($false)))
Write-Output "GUICLICK_DONE"
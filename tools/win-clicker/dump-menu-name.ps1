# dump-menu-name.ps1 -- read the C-entry boot menu name field (ID_MENU=1207) and the
# destructive confirmation static text with a large buffer (enum truncates).
$ErrorActionPreference = 'Continue'
Add-Type -TypeDefinition @"
using System;
using System.Text;
using System.Runtime.InteropServices;
public class BrDump {
  [DllImport("user32.dll", CharSet=CharSet.Unicode)]
  public static extern int GetWindowTextW(IntPtr h, StringBuilder s, int n);
  [DllImport("user32.dll")]
  public static extern IntPtr GetDlgItem(IntPtr h, int id);
  public delegate bool EnumCb(IntPtr h, IntPtr l);
  [DllImport("user32.dll")]
  public static extern bool EnumChildWindows(IntPtr h, EnumCb cb, IntPtr l);
  [DllImport("user32.dll", CharSet=CharSet.Unicode)]
  public static extern int GetClassNameW(IntPtr h, StringBuilder s, int n);
}
"@
$lines = New-Object System.Collections.ArrayList
$kids = New-Object System.Collections.ArrayList
$cb = [BrDump+EnumCb]{ param($h,$l) [void]$kids.Add($h); return $true }
# main window hwnd from the probe launch: find the BackupRestoreNativeGui top window
$tops = New-Object System.Collections.ArrayList
$cbTop = [BrDump+EnumCb]{ param($h,$l) [void]$tops.Add($h); return $true }
[void][BrDump]::EnumChildWindows([IntPtr]::Zero, $cbTop, [IntPtr]::Zero)
foreach ($t in $tops) {
  $s = New-Object Text.StringBuilder 512
  [void][BrDump]::GetWindowTextW($t, $s, 512)
  if ($s.ToString() -like "*Rust GUI*") {
    $kids.Clear()
    [void][BrDump]::EnumChildWindows($t, $cb, [IntPtr]::Zero)
    foreach ($k in $kids) {
      $c = New-Object Text.StringBuilder 256
      [void][BrDump]::GetClassNameW($k, $c, 256)
      if ($c.ToString() -eq "Edit") {
        $txt = New-Object Text.StringBuilder 1024
        [void][BrDump]::GetWindowTextW($k, $txt, 1024)
        [void]$lines.Add("EDIT hwnd=" + $k.ToInt64() + " text=[" + $txt.ToString() + "]")
      }
    }
  }
  if ($s.ToString() -like "*破坏性确认*" -or $s.ToString() -like "*Destructive*") {
    $kids.Clear()
    [void][BrDump]::EnumChildWindows($t, $cb, [IntPtr]::Zero)
    foreach ($k in $kids) {
      $txt = New-Object Text.StringBuilder 4096
      [void][BrDump]::GetWindowTextW($k, $txt, 4096)
      if ($txt.ToString().Length -gt 0) {
        [void]$lines.Add("DLGKID hwnd=" + $k.ToInt64() + " text=[" + $txt.ToString() + "]")
      }
    }
  }
}
$utf8 = New-Object System.Text.UTF8Encoding($false)
[System.IO.File]::WriteAllText("C:\Users\Public\pkg\menuname.txt", ($lines -join "`r`n"), $utf8)
Write-Output ("DUMP_DONE lines=" + $lines.Count)

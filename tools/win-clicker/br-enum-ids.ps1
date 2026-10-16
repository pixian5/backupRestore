# br-enum-ids.ps1 - enumerate child controls of the main GUI window with ctrl id,
# class, visibility and text (text escaped as \uXXXX so CJK survives ASCII logs).
param([string]$WindowClass = "BackupRestoreNativeGui")
Add-Type -TypeDefinition @"
using System;
using System.Runtime.InteropServices;
using System.Text;
public class BrEnumI {
  [DllImport("user32.dll")] public static extern bool EnumWindows(EnumCb cb, IntPtr l);
  [DllImport("user32.dll")] public static extern bool EnumChildWindows(IntPtr h, EnumCb cb, IntPtr l);
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern int GetWindowTextW(IntPtr h, StringBuilder s, int n);
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern int GetClassNameW(IntPtr h, StringBuilder s, int n);
  [DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr h);
  [DllImport("user32.dll")] public static extern int GetDlgCtrlID(IntPtr h);
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
$h = [IntPtr]::Zero
$cb = [BrEnumI+EnumCb]{ param($hw,$lp)
    if ([BrEnumI]::Cls($hw) -eq $WindowClass) { $script:h = $hw; return $false }
    return $true
}
[void][BrEnumI]::EnumWindows($cb, [IntPtr]::Zero)
if ($h -eq [IntPtr]::Zero) { Write-Output 'NO_WINDOW'; exit 1 }
$script:kids = New-Object System.Collections.ArrayList
$cb2 = [BrEnumI+EnumCb]{ param($k,$l) [void]$script:kids.Add($k); return $true }
[void][BrEnumI]::EnumChildWindows($h, $cb2, [IntPtr]::Zero)
foreach ($k in $script:kids) {
    $id = [BrEnumI]::GetDlgCtrlID($k)
    $cls = [BrEnumI]::Cls($k)
    $vis = [BrEnumI]::IsWindowVisible($k)
    $t = [BrEnumI]::Esc([BrEnumI]::Text($k))
    if ($t.Length -gt 0 -or $cls -eq 'Edit' -or $cls -eq 'ComboBox' -or $cls -eq 'Button') {
        Write-Output ("id=" + $id + " class=" + $cls + " vis=" + $vis + " text=[" + $t + "]")
    }
}
Write-Output ("TOTAL=" + $script:kids.Count)

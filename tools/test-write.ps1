# Externally write/read status control to verify control is functional.
Add-Type @"
using System;
using System.Runtime.InteropServices;
using System.Text;
public class Win32W {
    [DllImport("user32.dll", CharSet = CharSet.Unicode)] public static extern bool SetWindowText(IntPtr h, string t);
    [DllImport("user32.dll", CharSet = CharSet.Unicode)] public static extern int GetWindowText(IntPtr h, StringBuilder s, int m);
}
"@
$h = [IntPtr]327822
$r = [Win32W]::SetWindowText($h, "WRITE_TEST_123")
Write-Output ("set_result=" + $r)
$sb = New-Object System.Text.StringBuilder 512
[void][Win32W]::GetWindowText($h, $sb, 512)
Write-Output ("readback=" + $sb.ToString())

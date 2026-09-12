# Direct write test on status control of current window.
Add-Type @"
using System;
using System.Runtime.InteropServices;
using System.Text;
public class Win32X {
    [DllImport("user32.dll", CharSet = CharSet.Unicode)] public static extern bool SetWindowText(IntPtr h, string t);
    [DllImport("user32.dll", CharSet = CharSet.Unicode)] public static extern int GetWindowText(IntPtr h, StringBuilder s, int m);
}
"@
$h = [IntPtr]393380
$before = New-Object System.Text.StringBuilder 512
[void][Win32X]::GetWindowText($h, $before, 512)
Write-Output ("before=" + $before.ToString())
$r = [Win32X]::SetWindowText($h, "DIRECT_TEST_PE_HINT")
Write-Output ("set_result=" + $r)
$after = New-Object System.Text.StringBuilder 512
[void][Win32X]::GetWindowText($h, $after, 512)
Write-Output ("after=" + $after.ToString())

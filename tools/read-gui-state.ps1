Add-Type @"
using System;
using System.Text;
using System.Runtime.InteropServices;
public class Win {
    [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern int GetWindowText(IntPtr hWnd, StringBuilder sb, int max);
    [DllImport("user32.dll")] public static extern IntPtr GetDlgItem(IntPtr hWnd, int nIDDlgItem);
}
"@
$sb = New-Object System.Text.StringBuilder 1024
$hStatus = [Win]::GetDlgItem([IntPtr]524488, 1205)
[Win]::GetWindowText($hStatus, $sb, 1024) | Out-Null
Write-Output ("STATUS: " + $sb.ToString())

$sbLog = New-Object System.Text.StringBuilder 4096
$hLog = [Win]::GetDlgItem([IntPtr]524488, 1302)
[Win]::GetWindowText($hLog, $sbLog, 4096) | Out-Null
Write-Output ("LOG: " + $sbLog.ToString())

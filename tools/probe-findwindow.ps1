# probe-findwindow.ps1 - ASCII only. Test FindWindow against known classes.
Add-Type @'
using System;
using System.Text;
using System.Runtime.InteropServices;
public class FW {
    [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern IntPtr FindWindow(string cls, string title);
    [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern IntPtr FindWindowEx(IntPtr parent, IntPtr child, string cls, string title);
}
'@
Write-Output ("tray=[" + [FW]::FindWindow("Shell_TrayWnd", $null) + "]")
Write-Output ("native-null-title=[" + [FW]::FindWindow("BackupRestoreNativeGui", $null) + "]")
Write-Output ("native-with-title=[" + [FW]::FindWindow("BackupRestoreNativeGui", "BackupRestore - Rust GUI v2.6.8") + "]")
Write-Output ("progman=[" + [FW]::FindWindow("Progman", $null) + "]")

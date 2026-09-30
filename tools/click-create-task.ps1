if (-not ([System.Management.Automation.PSTypeName]'WinClickTask').Type) {
Add-Type @"
using System;
using System.Runtime.InteropServices;
public class WinClickTask {
    [DllImport("user32.dll")] public static extern IntPtr FindWindow(string cls, string title);
    [DllImport("user32.dll")] public static extern IntPtr GetDlgItem(IntPtr hWnd, int nIDDlgItem);
    [DllImport("user32.dll")] public static extern IntPtr SendMessage(IntPtr hWnd, uint Msg, IntPtr wParam, IntPtr lParam);
    [DllImport("user32.dll")] public static extern bool SetForegroundWindow(IntPtr hWnd);
}
"@
}
$hMain = [WinClickTask]::FindWindow("BackupRestoreNativeGui", $null)
[WinClickTask]::SetForegroundWindow($hMain)
$btn = [WinClickTask]::GetDlgItem($hMain, 1003)
[WinClickTask]::SendMessage($btn, 0x00F5, [IntPtr]::Zero, [IntPtr]::Zero)
Write-Output "Clicked ID_CREATE_TASK"

if (-not ([System.Management.Automation.PSTypeName]'WinClickChoice').Type) {
Add-Type @"
using System;
using System.Runtime.InteropServices;
public class WinClickChoice {
    [DllImport("user32.dll")] public static extern IntPtr FindWindow(string cls, string title);
    [DllImport("user32.dll")] public static extern IntPtr GetDlgItem(IntPtr hWnd, int nIDDlgItem);
    [DllImport("user32.dll")] public static extern IntPtr SendMessage(IntPtr hWnd, uint Msg, IntPtr wParam, IntPtr lParam);
    [DllImport("user32.dll")] public static extern bool SetForegroundWindow(IntPtr hWnd);
}
"@
}
$hDlg = [WinClickChoice]::FindWindow("BackupRestoreSystemDriveChoice", $null)
if ($hDlg -ne [IntPtr]::Zero) {
    [WinClickChoice]::SetForegroundWindow($hDlg)
    $btn = [WinClickChoice]::GetDlgItem($hDlg, 2002) # ID_CHOICE_RE
    [WinClickChoice]::SendMessage($btn, 0x00F5, [IntPtr]::Zero, [IntPtr]::Zero)
    Write-Output "Clicked ID_CHOICE_RE"
} else {
    Write-Output "FAIL: BackupRestoreSystemDriveChoice not found"
}

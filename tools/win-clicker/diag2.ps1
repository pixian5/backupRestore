# diag2.ps1 - report focused control ID + class of foreground window (cross-thread)
Add-Type -TypeDefinition @"
using System;
using System.Text;
using System.Runtime.InteropServices;
public class FW2 {
    [StructLayout(LayoutKind.Sequential)] public struct RECT { public int left; public int top; public int right; public int bottom; }
    [StructLayout(LayoutKind.Sequential)] public struct GUITHREADINFO {
        public uint cbSize; public uint flags; public IntPtr hwndActive; public IntPtr hwndFocus;
        public IntPtr hwndCapture; public IntPtr hwndMenuOwner; public IntPtr hwndMoveSize; public IntPtr hwndCaret; public RECT rcCaret;
    }
    [DllImport("user32.dll")] public static extern IntPtr GetForegroundWindow();
    [DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(IntPtr h, out uint pid);
    [DllImport("user32.dll")] public static extern bool GetGUIThreadInfo(uint tid, ref GUITHREADINFO info);
    [DllImport("user32.dll")] public static extern int GetDlgCtrlID(IntPtr hwnd);
    [DllImport("user32.dll")] public static extern int GetClassName(IntPtr h, StringBuilder s, int n);
    [DllImport("user32.dll")] public static extern int GetWindowText(IntPtr h, StringBuilder s, int n);
}
"@
$fg = [FW2]::GetForegroundWindow()
$pid2 = 0
$tid = [FW2]::GetWindowThreadProcessId($fg, [ref]$pid2)
$gi = New-Object FW2+GUITHREADINFO
$gi.cbSize = [System.Runtime.InteropServices.Marshal]::SizeOf($gi)
$ok = [FW2]::GetGUIThreadInfo($tid, [ref]$gi)
$lines = @()
$lines += "fg=$fg tid=$tid pid=$pid2 ok=$ok focus=$($gi.hwndFocus) active=$($gi.hwndActive)"
if ($gi.hwndFocus -ne [IntPtr]::Zero) {
    $cls = New-Object System.Text.StringBuilder 64
    [FW2]::GetClassName($gi.hwndFocus, $cls, 64) | Out-Null
    $txt = New-Object System.Text.StringBuilder 128
    [FW2]::GetWindowText($gi.hwndFocus, $txt, 128) | Out-Null
    $lines += "focusid=$([FW2]::GetDlgCtrlID($gi.hwndFocus)) class='$($cls.ToString())' text='$($txt.ToString())'"
}
$lines | Out-File C:\Users\Public\backupRestore-package\diag2.txt -Encoding utf8

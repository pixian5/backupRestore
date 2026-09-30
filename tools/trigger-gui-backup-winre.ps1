# trigger-gui-backup-winre.ps1 -- Automated GUI trigger for C: to F:\6.wim backup via WinRE
param(
    [string]$TargetWim = "F:\6.wim"
)

$logFile = "C:\Users\Public\trigger.log"
function Log($msg) {
    $line = (Get-Date -Format "yyyy-MM-dd HH:mm:ss") + " " + $msg
    Write-Output $line
    Add-Content -Path $logFile -Value $line
}

"--- Starting trigger run ---" | Out-File $logFile -Encoding ascii

Add-Type -TypeDefinition @"
using System;
using System.Text;
using System.Runtime.InteropServices;

public class Win32Gui {
    public delegate bool EnumProc(IntPtr hWnd, IntPtr lParam);
    [DllImport("user32.dll")] public static extern bool EnumWindows(EnumProc cb, IntPtr lParam);
    [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern int GetWindowText(IntPtr hWnd, StringBuilder sb, int max);
    [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern int GetClassName(IntPtr hWnd, StringBuilder sb, int max);
    [DllImport("user32.dll")] public static extern IntPtr GetDlgItem(IntPtr hDlg, int id);
    [DllImport("user32.dll")] public static extern IntPtr SendMessage(IntPtr hWnd, uint msg, IntPtr wParam, IntPtr lParam);
    [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern bool SetDlgItemTextW(IntPtr hDlg, int id, string text);
    [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern int GetDlgItemTextW(IntPtr hDlg, int id, StringBuilder sb, int max);
    [DllImport("user32.dll")] public static extern bool PostMessageW(IntPtr hWnd, uint msg, IntPtr wParam, IntPtr lParam);
    [DllImport("user32.dll")] public static extern bool IsWindow(IntPtr hWnd);
    [DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr hWnd);
    [DllImport("user32.dll")] public static extern bool SetForegroundWindow(IntPtr hWnd);

    public static IntPtr FindMainWindow() {
        IntPtr found = IntPtr.Zero;
        EnumWindows((h, l) => {
            if (!IsWindowVisible(h)) return true;
            StringBuilder sb = new StringBuilder(512);
            GetWindowText(h, sb, 512);
            if (sb.ToString().StartsWith("BackupRestore - Rust GUI")) {
                found = h;
                return false;
            }
            return true;
        }, IntPtr.Zero);
        return found;
    }

    public static IntPtr FindDialogByClass(string clsName) {
        IntPtr found = IntPtr.Zero;
        EnumWindows((h, l) => {
            if (!IsWindowVisible(h)) return true;
            StringBuilder sb = new StringBuilder(256);
            GetClassName(h, sb, 256);
            if (sb.ToString() == clsName) {
                found = h;
                return false;
            }
            return true;
        }, IntPtr.Zero);
        return found;
    }
}
"@

$BM_CLICK = 0x00F5
$WM_COMMAND = 0x0111
$ID_OPERATION_BACKUP = 1101
$ID_IMAGE = 1203
$ID_CREATE_TASK = 1003
$ID_CHOICE_RE = 2002
$IDYES = 6

Log "Step 1: Find BackupRestore Main Window"
$mainHwnd = [Win32Gui]::FindMainWindow()
if ($mainHwnd -eq [IntPtr]::Zero) {
    Log "ERROR: Cannot find BackupRestore main window"
    exit 1
}

$sb = New-Object System.Text.StringBuilder 512
[Win32Gui]::GetWindowText($mainHwnd, $sb, 512) | Out-Null
Log ("Main window: " + $mainHwnd + " Title: " + $sb.ToString())
[Win32Gui]::SetForegroundWindow($mainHwnd) | Out-Null
Start-Sleep -Seconds 1

Log "Step 2: Switch to Backup Mode (ID 1101)"
[Win32Gui]::PostMessageW($mainHwnd, $WM_COMMAND, [IntPtr]$ID_OPERATION_BACKUP, [IntPtr]::Zero) | Out-Null
Start-Sleep -Seconds 2

Log ("Step 3: Set Image Path to " + $TargetWim)
$okSet = [Win32Gui]::SetDlgItemTextW($mainHwnd, $ID_IMAGE, $TargetWim)
Log ("SetDlgItemTextW returned: " + $okSet)
Start-Sleep -Milliseconds 500
$sbPath = New-Object System.Text.StringBuilder 512
$len = [Win32Gui]::GetDlgItemTextW($mainHwnd, $ID_IMAGE, $sbPath, 512)
Log ("Read back length=" + $len + " text=" + $sbPath.ToString())

if ($sbPath.ToString() -ne $TargetWim) {
    Log "ERROR: Path mismatch after set"
    exit 1
}

Log "Step 4: Click Create Task Button (ID 1003)"
$btnCreate = [Win32Gui]::GetDlgItem($mainHwnd, $ID_CREATE_TASK)
if ($btnCreate -eq [IntPtr]::Zero) {
    Log "ERROR: Create task button not found"
    exit 1
}
[Win32Gui]::PostMessageW($mainHwnd, $WM_COMMAND, [IntPtr]$ID_CREATE_TASK, $btnCreate) | Out-Null
Log "Posted create task command"

Log "Step 5: Wait for Environment Dialog"
$choiceHwnd = [IntPtr]::Zero
for ($i = 0; $i -lt 30; $i++) {
    Start-Sleep -Milliseconds 500
    $choiceHwnd = [Win32Gui]::FindDialogByClass("BackupRestoreSystemDriveChoice")
    if ($choiceHwnd -ne [IntPtr]::Zero) { break }
}

if ($choiceHwnd -eq [IntPtr]::Zero) {
    Log "ERROR: Environment dialog not found"
    exit 1
}
Log ("Environment dialog handle: " + $choiceHwnd)
Start-Sleep -Seconds 1

Log "Step 6: Click Enter Windows RE (ID 2002)"
$btnRe = [Win32Gui]::GetDlgItem($choiceHwnd, $ID_CHOICE_RE)
if ($btnRe -eq [IntPtr]::Zero) {
    Log "ERROR: Enter Windows RE button not found"
    exit 1
}
[Win32Gui]::PostMessageW($choiceHwnd, $WM_COMMAND, [IntPtr]$ID_CHOICE_RE, $btnRe) | Out-Null
Log "Posted Enter Windows RE command"

Log "Step 7: Wait for Confirmation Dialog"
$confirmHwnd = [IntPtr]::Zero
for ($i = 0; $i -lt 30; $i++) {
    Start-Sleep -Milliseconds 500
    $confirmHwnd = [Win32Gui]::FindDialogByClass("#32770")
    if ($confirmHwnd -ne [IntPtr]::Zero) { break }
}

if ($confirmHwnd -eq [IntPtr]::Zero) {
    Log "ERROR: Confirmation dialog not found"
    exit 1
}
Log ("Confirmation dialog handle: " + $confirmHwnd)
Start-Sleep -Seconds 1

Log "Step 8: Click IDYES (6) on Confirmation Dialog"
$btnYes = [Win32Gui]::GetDlgItem($confirmHwnd, $IDYES)
if ($btnYes -ne [IntPtr]::Zero) {
    [Win32Gui]::SendMessage($btnYes, $BM_CLICK, [IntPtr]::Zero, [IntPtr]::Zero) | Out-Null
} else {
    [Win32Gui]::SendMessage($confirmHwnd, $WM_COMMAND, [IntPtr]$IDYES, [IntPtr]::Zero) | Out-Null
}
Log "SUCCESS: WinRE backup triggered!"

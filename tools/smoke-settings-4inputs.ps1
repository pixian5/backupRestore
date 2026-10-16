# smoke-settings-4inputs.ps1
# 2026-10-17 v2.6.9 GUI smoke: open the settings window (4 inputs:
# failure threshold / failure action / DISM wait secs / pause wait secs),
# set distinctive values, save, and read the ini back.
# ASCII only on purpose (the guest decodes this file as GBK).
# Run ELEVATED in session 1 via tools/launch-as-gui-token.ps1 so the smoke
# process shares the GUI's High integrity level and UIPI no longer drops
# BM_CLICK (non-elevated --current-user runs hit settings-window=0).
#
# LOGGING: the borrowed-GUI-token child runs in a NEW logon session, so the
# per-user network drive X: is NOT visible (verified 2026-10-17 00:42).
# Write the transcript to a LOCAL path instead; the host reads it back through
# the SYSTEM channel. The marker also goes to a local path.
$ErrorActionPreference = 'Continue'

$logDir  = 'C:\Users\Public\backupRestore-package\logs'
$smokeLog = Join-Path $logDir 'smoke-settings.log'
$marker   = 'C:\Users\Public\backupRestore-package\logs\settings-populated.marker'
if (-not (Test-Path $logDir)) { New-Item -ItemType Directory -Path $logDir -Force | Out-Null }
Start-Transcript -Path $smokeLog -Force

Add-Type @'
using System;
using System.Text;
using System.Runtime.InteropServices;
public class SW {
    [DllImport("user32.dll")] public static extern bool EnumWindows(EnumProc cb, IntPtr l);
    public delegate bool EnumProc(IntPtr h, IntPtr l);
    [DllImport("user32.dll")] public static extern bool EnumChildWindows(IntPtr parent, EnumProc cb, IntPtr l);
    [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern int GetWindowText(IntPtr h, StringBuilder sb, int max);
    [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern int GetClassName(IntPtr h, StringBuilder sb, int max);
    [DllImport("user32.dll")] public static extern IntPtr GetDlgItem(IntPtr h, int id);
    // CharSet=Unicode is REQUIRED: the default (ANSI) marshals WM_SETTEXT's
    // UTF-16 pointer as an ANSI string across processes, so "1200" arrives as
    // "1" (the NUL right after the first wide char terminates it).
    // Verified 2026-10-17: without this, saved values were 6 instead of 600.
    [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern IntPtr SendMessage(IntPtr h, uint msg, IntPtr w, IntPtr l);
    // PostMessage (async) for BM_CLICK: a synchronous SendMessage(BM_CLICK)
    // blocks until the modal settings dialog is closed, because the GUI thread
    // enters a nested modal message loop inside the send. Verified 2026-10-17.
    [DllImport("user32.dll")] public static extern bool PostMessage(IntPtr h, uint msg, IntPtr w, IntPtr l);
    [DllImport("user32.dll")] public static extern bool SetForegroundWindow(IntPtr h);
    [DllImport("user32.dll")] public static extern int GetDlgCtrlID(IntPtr h);
    [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr h, out RECT r);
    [DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr h);
    [DllImport("user32.dll")] public static extern bool ShowWindow(IntPtr h, int cmd);
    [StructLayout(LayoutKind.Sequential)] public struct RECT { public int L, T, R, B; }
}
'@

$BM_CLICK = 0x00F5
$WM_SETTEXT = 0x000C
$EXE_DIR = "C:\Users\Public\backupRestore-package"

# --- collect all top-level windows: "hwnd|class|title" ---
$script:winList = New-Object System.Collections.ArrayList
$enumProc = [SW+EnumProc]{
    param($h, $l)
    $cn = New-Object System.Text.StringBuilder 256
    [void][SW]::GetClassName($h, $cn, 256)
    $ti = New-Object System.Text.StringBuilder 256
    [void][SW]::GetWindowText($h, $ti, 256)
    [void]$script:winList.Add($h.ToString() + "|" + $cn.ToString() + "|" + $ti.ToString())
    return $true
}
function Collect-Windows() {
    $script:winList.Clear()
    [void][SW]::EnumWindows($enumProc, [IntPtr]::Zero)
    return $script:winList
}
function Find-WindowInList($list, [string]$cls) {
    foreach ($entry in $list) {
        $parts = $entry.Split("|")
        if ($parts[1] -eq $cls) { return [IntPtr]([int64]$parts[0]) }
    }
    return [IntPtr]::Zero
}

$list = Collect-Windows
Write-Output ("windows-seen=" + $list.Count)
$main = Find-WindowInList $list "BackupRestoreNativeGui"
Write-Output ("main-window=" + $main)
if ($main -eq [IntPtr]::Zero) { Write-Output "FAIL: main window not found"; Stop-Transcript; exit 1 }

$btn = [SW]::GetDlgItem($main, 1007)
Write-Output ("settings-button=" + $btn)
if ($btn -eq [IntPtr]::Zero) { Write-Output "FAIL: settings button not found"; Stop-Transcript; exit 1 }
[SW]::SetForegroundWindow($main) | Out-Null
[SW]::PostMessage($btn, $BM_CLICK, [IntPtr]::Zero, [IntPtr]::Zero) | Out-Null
Start-Sleep -Seconds 4

$list = Collect-Windows
$dlg = Find-WindowInList $list "BackupRestoreSettingsWindow"
Write-Output ("settings-window=" + $dlg)
if ($dlg -eq [IntPtr]::Zero) { Write-Output "FAIL: settings window did not open"; Stop-Transcript; exit 1 }
$r = New-Object SW+RECT
[void][SW]::GetWindowRect($dlg, [ref]$r)
Write-Output ("settings-rect=" + $r.L + "," + $r.T + "," + $r.R + "," + $r.B + " visible=" + [SW]::IsWindowVisible($dlg))
[void][SW]::ShowWindow($dlg, 9)   # SW_RESTORE, in case it opened minimised
[void][SW]::SetForegroundWindow($dlg)
[void][SW]::ShowWindow($dlg, 5)   # SW_SHOW
Start-Sleep -Seconds 1
[void][SW]::SetForegroundWindow($dlg)
Write-Output "settings-window-open"

# --- enumerate every child control: id / class / text ---
$script:kidList = New-Object System.Collections.ArrayList
$kidProc = [SW+EnumProc]{
    param($h, $l)
    $id = [SW]::GetDlgCtrlID($h)
    $cn = New-Object System.Text.StringBuilder 256
    [void][SW]::GetClassName($h, $cn, 256)
    $ti = New-Object System.Text.StringBuilder 512
    [void][SW]::GetWindowText($h, $ti, 512)
    [void]$script:kidList.Add($id.ToString() + "|" + $cn.ToString() + "|" + $ti.ToString())
    return $true
}
[void][SW]::EnumChildWindows($dlg, $kidProc, [IntPtr]::Zero)
Write-Output ("child-count=" + $kidList.Count)
foreach ($entry in $kidList) {
    Write-Output ("child " + $entry)
}

function Get-EditText([int]$ctrlId) {
    # WM_GETTEXT (SendMessage) is the reliable cross-process read; plain
    # GetWindowText() returns empty for another process's EDIT control.
    $h = [SW]::GetDlgItem($dlg, $ctrlId)
    if ($h -eq [IntPtr]::Zero) { return "<no-control>" }
    $ptr = [Runtime.InteropServices.Marshal]::AllocHGlobal(256)
    $n = [SW]::SendMessage($h, 0x000D, [IntPtr]256, $ptr)
    $s = [Runtime.InteropServices.Marshal]::PtrToStringUni($ptr, [int]$n)
    [Runtime.InteropServices.Marshal]::FreeHGlobal($ptr)
    return $s
}

function Set-EditText([int]$ctrlId, [string]$value) {
    $h = [SW]::GetDlgItem($dlg, $ctrlId)
    if ($h -eq [IntPtr]::Zero) { Write-Output ("FAIL: edit " + $ctrlId + " not found"); return }
    $ptr = [Runtime.InteropServices.Marshal]::StringToHGlobalUni($value)
    [SW]::SendMessage($h, $WM_SETTEXT, [IntPtr]::Zero, $ptr) | Out-Null
    [Runtime.InteropServices.Marshal]::FreeHGlobal($ptr)
    # Read back so a truncated write can never pass unnoticed.
    Write-Output ("set edit " + $ctrlId + " = " + $value + " (readback=" + (Get-EditText $ctrlId) + ")")
}

Set-EditText 3100 "7"       # failure threshold
Set-EditText 3108 "900"     # DISM wait secs
Set-EditText 3110 "300"     # pause wait secs
$radio = [SW]::GetDlgItem($dlg, 3102)   # action = pause in recovery
Write-Output ("pause-radio=" + $radio)
[SW]::PostMessage($radio, $BM_CLICK, [IntPtr]::Zero, [IntPtr]::Zero) | Out-Null

# Signal the host (macOS) to grab a screenshot of the populated settings window.
Set-Content -Path $marker -Value '' -Encoding ascii
Write-Output ("marker-written=" + $marker)
Start-Sleep -Seconds 20

$save = [SW]::GetDlgItem($dlg, 3103)
Write-Output ("save-button=" + $save)
[SW]::PostMessage($save, $BM_CLICK, [IntPtr]::Zero, [IntPtr]::Zero) | Out-Null
Start-Sleep -Seconds 3

$ini = Join-Path $EXE_DIR "BackupRestore.settings.ini"
if (Test-Path $ini) {
    Write-Output "--- ini content after save ---"
    Get-Content $ini | ForEach-Object { Write-Output ("ini| " + $_) }
} else {
    Write-Output "FAIL: settings.ini not created after save"
}
# The modal dialog must be gone once save is handled.
$list = Collect-Windows
$still = Find-WindowInList $list "BackupRestoreSettingsWindow"
Write-Output ("settings-window-after-save=" + $still)
Write-Output "SMOKE-SETTINGS-DONE"
Stop-Transcript

# drive-gui-backup-t.ps1
# Real GUI (window-message) drive: backup source volume T: -> H:\BRImages\gui-backup.wim
# Derived from tools/drive-full-flow.ps1 with two changes:
#   1) select source volume combo (ID_SOURCE = 1202) to "T:"  (GUI defaults to system partition C:)
#   2) image path -> H:\BRImages\gui-backup.wim  (new volume convention: images live on H:)
# NOTE: output kept ASCII-only on purpose (script body is decoded as GBK on the guest).
# Add-Type compiles via %TEMP%; the session inherits SYSTEM's C:\Windows\TEMP which the
# interactive user cannot write -> "type not found". Redirect to a writable dir first.
$env:TEMP = "C:\Users\Public"
$env:TMP  = "C:\Users\Public"

Add-Type @'
using System;
using System.Text;
using System.Runtime.InteropServices;

public static class Native {
    [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern IntPtr FindWindow(string cls, string title);
    [DllImport("user32.dll")] public static extern bool EnumWindows(EnumProc cb, IntPtr lParam);
    public delegate bool EnumProc(IntPtr hWnd, IntPtr lParam);
    [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern int GetWindowText(IntPtr hWnd, StringBuilder sb, int max);
    [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern int GetClassName(IntPtr hWnd, StringBuilder sb, int max);
    [DllImport("user32.dll")] public static extern IntPtr GetDlgItem(IntPtr h, int id);
    [DllImport("user32.dll")] public static extern bool IsWindowEnabled(IntPtr h);
    [DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr h);
    [DllImport("user32.dll")] public static extern IntPtr SendMessage(IntPtr h, uint m, IntPtr w, IntPtr l);
    [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern IntPtr SendMessageS(IntPtr h, uint m, IntPtr w, string l);
    [DllImport("user32.dll")] public static extern bool PostMessage(IntPtr h, uint m, IntPtr w, IntPtr l);
    [DllImport("user32.dll")] public static extern bool SetForegroundWindow(IntPtr h);
    [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern bool SetDlgItemTextW(IntPtr hDlg, int id, string text);
    [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern int GetDlgItemTextW(IntPtr hDlg, int id, StringBuilder sb, int max);

    [DllImport("user32.dll", EntryPoint="SendMessageW")] public static extern IntPtr SendMessageI(IntPtr h, uint m, IntPtr w, IntPtr l);
    public static int GetComboCount(IntPtr combo) {
        return SendMessageI(combo, 0x014B, IntPtr.Zero, IntPtr.Zero).ToInt32();
    }
    public static string GetComboText(IntPtr combo, int idx) {
        int len = SendMessageI(combo, 0x0149, new IntPtr(idx), IntPtr.Zero).ToInt32();
        if (len <= 0) { return ""; }
        IntPtr buf = Marshal.AllocHGlobal((len + 2) * 2);
        try {
            SendMessageI(combo, 0x0148, new IntPtr(idx), buf);
            return Marshal.PtrToStringUni(buf, len);
        } finally {
            Marshal.FreeHGlobal(buf);
        }
    }

    public static IntPtr FindWindowByPrefix(string prefix) {
        IntPtr found = IntPtr.Zero;
        EnumWindows((h, l) => {
            if (!IsWindowVisible(h)) return true;
            StringBuilder sb = new StringBuilder(512);
            GetWindowText(h, sb, 512);
            if (sb.ToString().StartsWith(prefix)) { found = h; return false; }
            return true;
        }, IntPtr.Zero);
        return found;
    }

    public static IntPtr FindDialogByClass(string cls) {
        IntPtr found = IntPtr.Zero;
        EnumWindows((h, l) => {
            if (!IsWindowVisible(h)) return true;
            StringBuilder sb = new StringBuilder(512);
            GetClassName(h, sb, 512);
            if (sb.ToString().Equals(cls)) { found = h; return false; }
            return true;
        }, IntPtr.Zero);
        return found;
    }
}
'@

$BM_CLICK = 0x00F5
$WM_COMMAND = 0x0111
$WM_GETTEXTLENGTH = 0x000E
$CB_FINDSTRING = 0x014C
$CB_SETCURSEL = 0x014E
$CBN_SELCHANGE = 1

$ID_OPERATION_BACKUP = 1101
$ID_REFRESH = 1001
$ID_SOURCE = 1202
$ID_IMAGE = 1203
$ID_CREATE_TASK = 1003
$ID_CHOICE_RE = 2002
$ID_COMPRESS = 1208
$IDYES = 6

$IMAGE_PATH = "H:\BRImages\gui-backup.wim"
$SOURCE_PREFIX = "T:"

try { Start-Transcript -LiteralPath "H:\brwork\drive-gui.log" -Force | Out-Null } catch { }

Write-Output "== 1. find main window =="
$main = [Native]::FindWindowByPrefix("BackupRestore - Rust GUI")
if ($main -eq [IntPtr]::Zero) { throw "MAIN_WINDOW_NOT_FOUND" }
[void][Native]::SetForegroundWindow($main)
Write-Output ("main hwnd=" + $main)

Write-Output "== 2. switch to backup mode (1101) =="
$btnBackup = [Native]::GetDlgItem($main, $ID_OPERATION_BACKUP)
if ($btnBackup -ne [IntPtr]::Zero) {
    [void][Native]::SendMessage($btnBackup, $BM_CLICK, [IntPtr]::Zero, [IntPtr]::Zero)
} else {
    [void][Native]::PostMessage($main, $WM_COMMAND, [IntPtr]$ID_OPERATION_BACKUP, [IntPtr]::Zero)
}
Start-Sleep -Seconds 2

Write-Output "== 2.2 click refresh environment (1001) so volumes get enumerated =="
$comboSrc = [Native]::GetDlgItem($main, $ID_SOURCE)
if ($comboSrc -eq [IntPtr]::Zero) { throw "SOURCE_COMBO_NOT_FOUND" }
if ([Native]::GetComboCount($comboSrc) -eq 0) {
    $btnRefresh = [Native]::GetDlgItem($main, $ID_REFRESH)
    if ($btnRefresh -ne [IntPtr]::Zero) {
        [void][Native]::PostMessage($btnRefresh, $BM_CLICK, [IntPtr]::Zero, [IntPtr]::Zero)
        Write-Output "refresh clicked"
    } else {
        Write-Output "WARN refresh button not found"
    }
    for ($w = 0; $w -lt 120; $w++) {
        Start-Sleep -Milliseconds 500
        if ([Native]::GetComboCount($comboSrc) -gt 0) { break }
        if (($w % 10) -eq 0) {
            $sbS = New-Object System.Text.StringBuilder 1024
            [void][Native]::GetDlgItemTextW($main, 1205, $sbS, 1024)
            Write-Output ("  waiting... count=0 status='" + $sbS.ToString() + "'")
        }
    }
}
Write-Output ("after refresh combo count=" + [Native]::GetComboCount($comboSrc))
$sbS2 = New-Object System.Text.StringBuilder 1024
[void][Native]::GetDlgItemTextW($main, 1205, $sbS2, 1024)
Write-Output ("status(1205)='" + $sbS2.ToString() + "'")
$sbL2 = New-Object System.Text.StringBuilder 2048
[void][Native]::GetDlgItemTextW($main, 1302, $sbL2, 2048)
Write-Output ("log(1302)='" + $sbL2.ToString() + "'")

Write-Output "== 2.5 select source volume (1202) -> T: =="
$idx = -1
try {
    $cnt = [Native]::GetComboCount($comboSrc)
    Write-Output ("combo item count=" + $cnt)
    for ($i = 0; $i -lt $cnt; $i++) {
        $t = [Native]::GetComboText($comboSrc, $i)
        Write-Output ("  item[" + $i + "]='" + $t + "'")
        if ($t.StartsWith($SOURCE_PREFIX)) { $idx = $i; break }
    }
} catch {
    Write-Output ("COMBO_ENUM_FAIL: " + $_.Exception.Message)
}
Write-Output ("resolved source idx=" + $idx)
if ($idx -lt 0) { throw "SOURCE_T_NOT_FOUND" }
[void][Native]::SendMessage($comboSrc, $CB_SETCURSEL, [IntPtr]$idx, [IntPtr]::Zero)
Start-Sleep -Milliseconds 300
$wparam = [IntPtr](($ID_SOURCE -shl 16) -bor $CBN_SELCHANGE)
[void][Native]::PostMessage($main, $WM_COMMAND, $wparam, $comboSrc)
Start-Sleep -Seconds 1
$sbSrc = New-Object System.Text.StringBuilder 512
[void][Native]::GetDlgItemTextW($main, $ID_SOURCE, $sbSrc, 512)
Write-Output ("source combo text='" + $sbSrc.ToString() + "'")

Write-Output "== 3. set image path (1203) =="
if (-not [Native]::SetDlgItemTextW($main, $ID_IMAGE, $IMAGE_PATH)) { throw "SET_IMAGE_FAILED" }
Start-Sleep -Milliseconds 500
$sbImg = New-Object System.Text.StringBuilder 512
[void][Native]::GetDlgItemTextW($main, $ID_IMAGE, $sbImg, 512)
Write-Output ("image edit text='" + $sbImg.ToString() + "'")
if ($sbImg.ToString() -ne $IMAGE_PATH) { Write-Output "WARN image path mismatch" }

Write-Output "== 3.1 compression combo (1208) -> index 1 =="
$cboCompress = [Native]::GetDlgItem($main, $ID_COMPRESS)
if ($cboCompress -ne [IntPtr]::Zero) {
    [void][Native]::SendMessage($cboCompress, $CB_SETCURSEL, [IntPtr]1, [IntPtr]::Zero)
    Write-Output "compression set"
}

Write-Output "== 4. click create task (1003) =="
$btnCreate = [Native]::GetDlgItem($main, $ID_CREATE_TASK)
if ($btnCreate -eq [IntPtr]::Zero) { throw "ID_CREATE_TASK_NOT_FOUND" }
[void][Native]::PostMessage($btnCreate, $BM_CLICK, [IntPtr]::Zero, [IntPtr]::Zero)
Write-Output "create-task clicked"

Write-Output "== 5. wait environment choice dialog =="
$dlgChoice = [IntPtr]::Zero
for ($i = 0; $i -lt 40; $i++) {
    Start-Sleep -Milliseconds 300
    $dlgChoice = [Native]::FindDialogByClass("BackupRestoreSystemDriveChoice")
    if ($dlgChoice -ne [IntPtr]::Zero) { break }
}
if ($dlgChoice -eq [IntPtr]::Zero) { throw "SYSTEM_CHOICE_DIALOG_NOT_FOUND" }
Write-Output ("choice dialog hwnd=" + $dlgChoice)
[void][Native]::SetForegroundWindow($dlgChoice)

Write-Output "== 6. click enter Windows RE (2002) =="
$btnRe = [Native]::GetDlgItem($dlgChoice, $ID_CHOICE_RE)
if ($btnRe -eq [IntPtr]::Zero) { throw "ID_CHOICE_RE_NOT_FOUND" }
Start-Sleep -Milliseconds 300
[void][Native]::PostMessage($btnRe, $BM_CLICK, [IntPtr]::Zero, [IntPtr]::Zero)
Write-Output "enter-RE clicked"

Write-Output "== 7. wait confirm MessageBox =="
$dlgConfirm = [IntPtr]::Zero
for ($i = 0; $i -lt 40; $i++) {
    Start-Sleep -Milliseconds 300
    $dlgConfirm = [Native]::FindDialogByClass("#32770")
    if ($dlgConfirm -ne [IntPtr]::Zero) { break }
}
if ($dlgConfirm -eq [IntPtr]::Zero) { throw "CONFIRM_DIALOG_NOT_FOUND" }
Write-Output ("confirm dialog hwnd=" + $dlgConfirm)
[void][Native]::SetForegroundWindow($dlgConfirm)

Write-Output "== 8. click Yes (6) =="
$btnYes = [Native]::GetDlgItem($dlgConfirm, $IDYES)
if ($btnYes -eq [IntPtr]::Zero) { throw "IDYES_NOT_FOUND" }
Start-Sleep -Milliseconds 300
[void][Native]::PostMessage($btnYes, $BM_CLICK, [IntPtr]::Zero, [IntPtr]::Zero)
Write-Output "SUCCESS: task confirmed, preparing recovery boot..."

# drive-full-flow.ps1 —— 驱动 H:\brwork\BackupRestore.exe 进行真实的 GUI 备份
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
    [DllImport("user32.dll")] public static extern bool PostMessage(IntPtr h, uint m, IntPtr w, IntPtr l);
    [DllImport("user32.dll")] public static extern bool SetForegroundWindow(IntPtr h);
    [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern bool SetDlgItemTextW(IntPtr hDlg, int id, string text);
    [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern int GetDlgItemTextW(IntPtr hDlg, int id, StringBuilder sb, int max);

    public static IntPtr FindWindowByPrefix(string prefix) {
        IntPtr found = IntPtr.Zero;
        EnumWindows((h, l) => {
            if (!IsWindowVisible(h)) return true;
            StringBuilder sb = new StringBuilder(512);
            GetWindowText(h, sb, 512);
            if (sb.ToString().StartsWith(prefix)) {
                found = h;
                return false;
            }
            return true;
        }, IntPtr.Zero);
        return found;
    }

    public static IntPtr FindDialogByClass(string cls) {
        IntPtr found = IntPtr.Zero;
        EnumWindows((h, l) => {
            if (!IsWindowVisible(h)) return true;
            StringBuilder sb = new StringBuilder(256);
            GetClassName(h, sb, 256);
            if (sb.ToString() == cls) {
                found = h;
                return false;
            }
            return true;
        }, IntPtr.Zero);
        return found;
    }
}
'@

$BM_CLICK = 0x00F5
$WM_COMMAND = 0x0111
$ID_OPERATION_BACKUP = 1101
$ID_IMAGE = 1203
$ID_CREATE_TASK = 1003
$ID_CHOICE_RE = 2002
$IDYES = 6

Write-Output "== 1. 查找 BackupRestore 主窗口 =="
$main = [Native]::FindWindowByPrefix("BackupRestore - Rust GUI")
if ($main -eq [IntPtr]::Zero) {
    throw "MAIN_WINDOW_NOT_FOUND"
}
[void][Native]::SetForegroundWindow($main)
Write-Output ("主窗口句柄: " + $main)

Write-Output "== 2. 切换到「备份」模式 (ID 1101) =="
$btnBackup = [Native]::GetDlgItem($main, $ID_OPERATION_BACKUP)
if ($btnBackup -ne [IntPtr]::Zero) {
    [void][Native]::SendMessage($btnBackup, $BM_CLICK, [IntPtr]::Zero, [IntPtr]::Zero)
} else {
    [void][Native]::PostMessage($main, $WM_COMMAND, [IntPtr]$ID_OPERATION_BACKUP, [IntPtr]::Zero)
}
Start-Sleep -Seconds 1

Write-Output "== 3. 设置镜像路径为 F:\6.wim =="
[void][Native]::SetDlgItemTextW($main, $ID_IMAGE, "F:\6.wim")
Start-Sleep -Milliseconds 500
$sbPath = New-Object System.Text.StringBuilder 512
[void][Native]::GetDlgItemTextW($main, $ID_IMAGE, $sbPath, 512)
Write-Output ("已设置路径: " + $sbPath.ToString())

Write-Output "== 4. 点击「创建任务」(ID 1003) =="
$btnCreate = [Native]::GetDlgItem($main, $ID_CREATE_TASK)
if ($btnCreate -eq [IntPtr]::Zero) {
    throw "ID_CREATE_TASK_NOT_FOUND"
}
[void][Native]::SendMessage($btnCreate, $BM_CLICK, [IntPtr]::Zero, [IntPtr]::Zero)
Write-Output "已点击「创建任务」"

Write-Output "== 5. 等待环境选择对话框 (BackupRestoreSystemDriveChoice) =="
$dlgChoice = [IntPtr]::Zero
for ($i = 0; $i -lt 30; $i++) {
    Start-Sleep -Milliseconds 300
    $dlgChoice = [Native]::FindDialogByClass("BackupRestoreSystemDriveChoice")
    if ($dlgChoice -ne [IntPtr]::Zero) { break }
}
if ($dlgChoice -eq [IntPtr]::Zero) {
    throw "SYSTEM_CHOICE_DIALOG_NOT_FOUND"
}
Write-Output ("环境选择对话框句柄: " + $dlgChoice)
[void][Native]::SetForegroundWindow($dlgChoice)

Write-Output "== 6. 点击「进入 Windows RE」(ID 2002) =="
$btnRe = [Native]::GetDlgItem($dlgChoice, $ID_CHOICE_RE)
if ($btnRe -eq [IntPtr]::Zero) {
    throw "ID_CHOICE_RE_NOT_FOUND"
}
Start-Sleep -Milliseconds 300
[void][Native]::SendMessage($btnRe, $BM_CLICK, [IntPtr]::Zero, [IntPtr]::Zero)
Write-Output "已点击「进入 Windows RE」"

Write-Output "== 7. 等待「进入恢复环境」MessageBox 确认对话框 =="
$dlgConfirm = [IntPtr]::Zero
for ($i = 0; $i -lt 30; $i++) {
    Start-Sleep -Milliseconds 300
    $dlgConfirm = [Native]::FindDialogByClass("#32770")
    if ($dlgConfirm -ne [IntPtr]::Zero) { break }
}
if ($dlgConfirm -eq [IntPtr]::Zero) {
    throw "CONFIRM_DIALOG_NOT_FOUND"
}
Write-Output ("确认对话框句柄: " + $dlgConfirm)
[void][Native]::SetForegroundWindow($dlgConfirm)

Write-Output "== 8. 点击确认对话框的「是(Y)」按钮 (ID 6) =="
$btnYes = [Native]::GetDlgItem($dlgConfirm, $IDYES)
if ($btnYes -eq [IntPtr]::Zero) {
    throw "IDYES_NOT_FOUND"
}
Start-Sleep -Milliseconds 300
[void][Native]::SendMessage($btnYes, $BM_CLICK, [IntPtr]::Zero, [IntPtr]::Zero)
Write-Output "SUCCESS: 任务已确认触发！系统正在准备恢复环境引导事务..."

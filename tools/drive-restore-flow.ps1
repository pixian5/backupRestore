# drive-restore-flow.ps1 —— 驱动 H:\brwork\BackupRestore.exe 进行真实的 GUI 还原
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
    [DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr h);
    [DllImport("user32.dll")] public static extern IntPtr SendMessage(IntPtr h, uint m, IntPtr w, IntPtr l);
    [DllImport("user32.dll")] public static extern bool PostMessage(IntPtr h, uint m, IntPtr w, IntPtr l);
    [DllImport("user32.dll")] public static extern bool SetForegroundWindow(IntPtr h);

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
$ID_OPERATION_RESTORE = 1102
$ID_IMAGE = 1203
$ID_READ_IMAGE = 1204
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

Write-Output "== 2. 切换到「还原」模式 (ID 1102) =="
$btnRestore = [Native]::GetDlgItem($main, $ID_OPERATION_RESTORE)
if ($btnRestore -ne [IntPtr]::Zero) {
    [void][Native]::SendMessage($btnRestore, $BM_CLICK, [IntPtr]::Zero, [IntPtr]::Zero)
} else {
    [void][Native]::PostMessage($main, $WM_COMMAND, [IntPtr]$ID_OPERATION_RESTORE, [IntPtr]::Zero)
}
Start-Sleep -Seconds 1

$EM_SETSEL = 0x00B1
$WM_CHAR = 0x0102
$WM_GETTEXTLENGTH = 0x000E

function TypeInto([int]$id, [string]$value) {
    $ctl = [Native]::GetDlgItem($main, $id)
    if ($ctl -eq [IntPtr]::Zero) { Write-Output ("FAIL: control " + $id + " not found"); return $false }
    [void][Native]::SendMessage($ctl, $EM_SETSEL, [IntPtr]0, [IntPtr](-1))
    foreach ($ch in $value.ToCharArray()) {
        [void][Native]::SendMessage($ctl, $WM_CHAR, [IntPtr][int]$ch, [IntPtr]::Zero)
        Start-Sleep -Milliseconds 8
    }
    $len = [int][Native]::SendMessage($ctl, $WM_GETTEXTLENGTH, [IntPtr]::Zero, [IntPtr]::Zero)
    Write-Output ("  typed id=" + $id + " len=" + $len + " want=" + $value.Length)
    return ($len -eq $value.Length)
}

Write-Output "== 3. 设置镜像路径为 F:\6.wim =="
if (-not (TypeInto $ID_IMAGE "F:\6.wim")) { throw "TYPE_IMAGE_FAILED" }

Write-Output "== 3.1 点击读取镜像 (ID_READ_IMAGE = 1204) =="
$btnRead = [Native]::GetDlgItem($main, $ID_READ_IMAGE)
if ($btnRead -ne [IntPtr]::Zero) {
    [void][Native]::PostMessage($btnRead, $BM_CLICK, [IntPtr]::Zero, [IntPtr]::Zero)
}
Start-Sleep -Seconds 2

Write-Output "== 4. 点击「创建任务」(ID 1003) =="
$btnCreate = [Native]::GetDlgItem($main, $ID_CREATE_TASK)
if ($btnCreate -eq [IntPtr]::Zero) {
    throw "ID_CREATE_TASK_NOT_FOUND"
}
[void][Native]::PostMessage($btnCreate, $BM_CLICK, [IntPtr]::Zero, [IntPtr]::Zero)
Write-Output "已点击「创建任务」"

Write-Output "== 5. 等待「破坏性确认」MessageBox 对话框 =="
$dlgDestructive = [IntPtr]::Zero
for ($i = 0; $i -lt 30; $i++) {
    Start-Sleep -Milliseconds 300
    $dlgDestructive = [Native]::FindDialogByClass("#32770")
    if ($dlgDestructive -ne [IntPtr]::Zero) { break }
}
if ($dlgDestructive -ne [IntPtr]::Zero) {
    Write-Output ("破坏性确认对话框句柄: " + $dlgDestructive)
    [void][Native]::SetForegroundWindow($dlgDestructive)
    $btnYes = [Native]::GetDlgItem($dlgDestructive, $IDYES)
    if ($btnYes -ne [IntPtr]::Zero) {
        Start-Sleep -Milliseconds 300
        [void][Native]::PostMessage($btnYes, $BM_CLICK, [IntPtr]::Zero, [IntPtr]::Zero)
        Write-Output "已点击破坏性确认「是」"
    }
}

Write-Output "== 6. 等待环境选择对话框 (BackupRestoreSystemDriveChoice) =="
$dlgChoice = [IntPtr]::Zero
for ($i = 0; $i -lt 150; $i++) {
    Start-Sleep -Milliseconds 500
    $dlgChoice = [Native]::FindDialogByClass("BackupRestoreSystemDriveChoice")
    if ($dlgChoice -ne [IntPtr]::Zero) { break }
}
if ($dlgChoice -eq [IntPtr]::Zero) {
    throw "SYSTEM_CHOICE_DIALOG_NOT_FOUND"
}
Write-Output ("环境选择对话框句柄: " + $dlgChoice)
[void][Native]::SetForegroundWindow($dlgChoice)

Write-Output "== 7. 点击「进入 Windows RE」(ID 2002) =="
$btnRe = [Native]::GetDlgItem($dlgChoice, $ID_CHOICE_RE)
if ($btnRe -eq [IntPtr]::Zero) {
    throw "ID_CHOICE_RE_NOT_FOUND"
}
Start-Sleep -Milliseconds 300
[void][Native]::PostMessage($btnRe, $BM_CLICK, [IntPtr]::Zero, [IntPtr]::Zero)
Write-Output "已点击「进入 Windows RE」"

Write-Output "== 8. 等待「进入恢复环境」MessageBox 确认对话框 =="
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

Write-Output "== 9. 点击确认对话框的「是(Y)」按钮 (ID 6) =="
$btnYes = [Native]::GetDlgItem($dlgConfirm, $IDYES)
if ($btnYes -eq [IntPtr]::Zero) {
    throw "IDYES_NOT_FOUND"
}
Start-Sleep -Milliseconds 300
[void][Native]::PostMessage($btnYes, $BM_CLICK, [IntPtr]::Zero, [IntPtr]::Zero)
Write-Output "SUCCESS: 还原任务已确认触发！系统正在准备恢复环境引导事务..."

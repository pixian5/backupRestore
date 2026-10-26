# _t-refresh-status.ps1 - ASCII-source body for run-gui-token.ps1 (session 1, GUI token).
#
# Purpose: prove on the REAL machine that "Refresh task status" now reads status.json.
# Before v2.6.64 the GUI only printed taskId/operation/paths from last-task.json and
# never opened status.json, so a task that failed inside the recovery environment came
# back to Windows with no stage and no reason on screen.
#
# Method: drive four outcomes through the real button and read the real status box:
#   S1 ok + code      -> status.json with errorCodeText=BR-PE-039, error="DISM timeout TEST123"
#   S2 ok + no reason -> failed with error:"" (this shape exists on the real machine)
#   S3 missing        -> statusJson points at a file that does not exist
#   S4 malformed      -> statusJson file contains "not json"
#
# Assertions are ASCII ONLY (cmd channel mangles Chinese). Chinese label rendering is
# covered by the unit tests in status_receipt.rs; here we assert on
#   - the ASCII error code / ASCII reason text actually shown in the status box
#   - the structured gui.log line: status_outcome=... code=... error_len=... lines=...
#   - one codepoint probe per scenario for the two "must say it out loud" labels
#     (U+7F3A 缺 in 失败但未记录原因, U+89E3 解 in 解析失败) - the probe is written as
#     an escape so this file stays pure ASCII.
#
# ★ Side effects: H:\brwork\last-task.json is renamed to last-task.json.tbak during the
#   run and restored at the end (also restored if the script exits early).
$out = 'H:\brwork\refresh-status-check.txt'
$guiLog = 'H:\brwork\logs\gui.log'
$lastTask = 'H:\brwork\last-task.json'
$lastTaskBak = 'H:\brwork\last-task.json.tbak'
$statusFile = 'H:\brwork\_t-status-test.json'
function Say($s) { $s | Add-Content -Path $out -Encoding UTF8 }
Set-Content -Path $out -Value ("STARTED " + (Get-Date -Format o)) -Encoding UTF8

Add-Type @'
using System;
using System.Runtime.InteropServices;
using System.Text;
public class RS {
  [DllImport("user32.dll")] public static extern bool EnumWindows(EnumCb cb, IntPtr l);
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern int GetClassNameW(IntPtr h, StringBuilder s, int n);
  [DllImport("user32.dll")] public static extern IntPtr GetDlgItem(IntPtr hDlg, int id);
  [DllImport("user32.dll")] public static extern IntPtr SendMessageW(IntPtr h, uint m, IntPtr w, StringBuilder l);
  [DllImport("user32.dll")] public static extern IntPtr SendMessageW(IntPtr h, uint m, IntPtr w, IntPtr l);
  [DllImport("user32.dll")] public static extern bool PostMessageW(IntPtr h, uint m, IntPtr w, IntPtr l);
  public delegate bool EnumCb(IntPtr h, IntPtr l);
  public const uint BM_CLICK = 0x00F5;
  public const uint WM_GETTEXT = 0x000D;
  public const uint WM_GETTEXTLENGTH = 0x000E;
  public static string Cls(IntPtr h) { StringBuilder sb = new StringBuilder(256); GetClassNameW(h, sb, 256); return sb.ToString(); }
  // ★ 用显式非托管缓冲区，不用 StringBuilder 封送：
  //   实测 StringBuilder 版在跨进程 WM_GETTEXT 上只读回 3 个字符
  //   （TEXT_LEN_RAW=218 而 $t.Length=3），读数那一侧先坏，判据必然失真。
  public static string Txt(IntPtr h) {
    int len = (int)SendMessageW(h, WM_GETTEXTLENGTH, IntPtr.Zero, IntPtr.Zero);
    if (len <= 0) return "";
    IntPtr buf = Marshal.AllocHGlobal((len + 8) * 2);
    try {
      SendMessageW(h, WM_GETTEXT, (IntPtr)(len + 8), buf);
      return Marshal.PtrToStringUni(buf) ?? "";
    } finally { Marshal.FreeHGlobal(buf); }
  }
}
'@

function Find-Win($cls) {
  $script:found = [IntPtr]::Zero
  $cb = [RS+EnumCb]{ param($h,$l) if ([RS]::Cls($h) -eq $cls) { $script:found = $h; return $false }; return $true }
  [void][RS]::EnumWindows($cb, [IntPtr]::Zero)
  return $script:found
}

function Write-Utf8($path, $text) {
  [System.IO.File]::WriteAllText($path, $text, (New-Object System.Text.UTF8Encoding($false)))
}

# --- last-task.json: back up the real one, restore it no matter how we exit ---------
$hadLast = Test-Path $lastTask
if ($hadLast) { Copy-Item $lastTask $lastTaskBak -Force }
try {
  $gui = Find-Win 'BackupRestoreNativeGui'
  if ($gui -eq [IntPtr]::Zero) { Say "GUI_NOT_FOUND"; exit 1 }
  $btn = [RS]::GetDlgItem($gui, 1004)
  if ($btn -eq [IntPtr]::Zero) { Say "ID_REFRESH_TASK_CONTROL_NULL"; exit 2 }
  $status = [RS]::GetDlgItem($gui, 1300)
  if ($status -eq [IntPtr]::Zero) { Say "ID_STATUS_CONTROL_NULL"; exit 3 }
  Say ("gui_hwnd=" + $gui.ToInt64() + " btn=1004 status=1300")

  $stamp = (Get-Date -Format 'yyyy-MM-ddTHH:mm:ssZ')
  $cases = @(
    @{
      name = 'S1_OK_WITH_CODE'
      statusJson = "{""taskId"":""0f7f2b1e-1111-2222-3333-444455556666"",""operation"":""backup"",""stage"":""failed"",""progress"":0,""updated"":""$stamp"",""errorCode"":1,""errorCodeText"":""BR-PE-039"",""error"":""DISM timeout TEST123""}"
      needCode = 'BR-PE-039'
      needText = 'TEST123'
      needCp = 0
      expectOutcome = 'ok'
      expectCode = 'BR-PE-039'
    },
    @{
      name = 'S2_OK_NO_REASON'
      statusJson = "{""taskId"":""0f7f2b1e-1111-2222-3333-444455556666"",""operation"":""backup"",""stage"":""failed"",""progress"":0,""updated"":""$stamp"",""errorCode"":2,""error"":""""}"
      needCode = ''
      needText = ''
      needCp = 0x7F3A
      expectOutcome = 'ok'
      expectCode = 'none'
    },
    @{
      name = 'S3_MISSING_FILE'
      statusJson = ''
      needCode = ''
      needText = ''
      needCp = 0
      expectOutcome = 'missing'
      expectCode = 'none'
    },
    @{
      name = 'S4_MALFORMED'
      statusJson = 'not json at all'
      needCode = ''
      needText = ''
      needCp = 0x89E3
      expectOutcome = 'malformed'
      expectCode = 'none'
    }
  )

  foreach ($c in $cases) {
    if ($c.statusJson -eq '') {
      if (Test-Path $statusFile) { Remove-Item $statusFile -Force }
    } else {
      Write-Utf8 $statusFile $c.statusJson
    }
    $lt = "{""taskId"":""0f7f2b1e-1111-2222-3333-444455556666"",""operation"":""backup"",""taskRoot"":""H:\\brwork\\tasks\\0f7f2b1e-1111-2222-3333-444455556666"",""statusJson"":""H:\\brwork\\_t-status-test.json"",""recoveryLog"":""H:\\brwork\\Recovery.log"",""prepareLog"":""H:\\brwork\\prepare.log"",""imagePath"":""H:\\BRImages\\test.wim""}"
    Write-Utf8 $lastTask $lt

    [void][RS]::PostMessageW($btn, [RS]::BM_CLICK, [IntPtr]::Zero, [IntPtr]::Zero)
    Start-Sleep -Milliseconds 1200
    $t = [RS]::Txt($status)
    $lenRaw = [RS]::SendMessageW($status, [RS]::WM_GETTEXTLENGTH, [IntPtr]::Zero, [IntPtr]::Zero)
    $lines = (($t -replace "`r", "") -split "`n").Count
    $dbg = ($t -replace "[^\x20-\x7e]", ".")
    Say ("  TEXT_LEN_RAW=" + $lenRaw.ToInt64() + " DBG=[" + $dbg + "]")
    Say ("CASE=" + $c.name + " text_len=" + $t.Length + " text_lines=" + $lines)
    if ($c.needCode -ne '') {
      Say ("  TEXT_HAS_CODE=" + [int]($t.Contains($c.needCode)))
      Say ("  TEXT_HAS_REASON=" + [int]($t.Contains($c.needText)))
    }
    if ($c.needCp -ne 0) {
      $probe = [string][char]$c.needCp
      Say ("  TEXT_HAS_LABEL_CP=" + [int]($t.Contains($probe)))
    }
    # Structured log line: the only ASCII-proof of which branch the GUI took.
    $tail = ''
    if (Test-Path $guiLog) { $tail = ((Get-Content $guiLog -Tail 4 -Encoding UTF8) -join ' | ') }
    # ★ 取**最后**一次匹配：gui.log 每次点击会追加 2 行以上，Tail 4 里混着
    #   上一个用例的行——取首个匹配会读到旧值（第一轮就栽在这：S3 显示 ok）。
    $ms = [regex]::Matches($tail, 'status_outcome=(\w+)')
    $logOutcome = $(if ($ms.Count -gt 0) { $ms[$ms.Count - 1].Groups[1].Value } else { 'none' })
    Say ("  LOG_OUTCOME=" + $logOutcome)
    $ms2 = [regex]::Matches($tail, 'code=(\S+?) ')
    $logCode = $(if ($ms2.Count -gt 0) { $ms2[$ms2.Count - 1].Groups[1].Value } else { 'none' })
    Say ("  LOG_CODE=" + $logCode)
    $ms3 = [regex]::Matches($tail, 'lines=(\d+)')
    $logLines = $(if ($ms3.Count -gt 0) { $ms3[$ms3.Count - 1].Groups[1].Value } else { 'none' })
    Say ("  LOG_LINES=" + $logLines)
    # 逐用例结论：冒烟只认这行 ASCII，不解析中文文案。
    $outcomeOk = ($logOutcome -eq $c.expectOutcome)
    $codeOk = ($logCode -eq $c.expectCode)
    $flagOk = $true
    if ($c.needCode -ne '') { $flagOk = $flagOk -and $t.Contains($c.needCode) -and $t.Contains($c.needText) }
    if ($c.needCp -ne 0) { $flagOk = $flagOk -and $t.Contains([string][char]$c.needCp) }
    $caseOk = ($outcomeOk -and $codeOk -and $flagOk)
    Say ("CASE_RESULT=" + $c.name + "=" + $(if ($caseOk) { 'PASS' } else { 'FAIL' }) +
         " outcome=" + $logOutcome + "/" + $c.expectOutcome +
         " code=" + $logCode + "/" + $c.expectCode +
         " flags=" + [int]$flagOk)
  }
} finally {
  # Restore the real last-task.json and remove the scratch status file.
  if ($hadLast -and (Test-Path $lastTaskBak)) { Copy-Item $lastTaskBak $lastTask -Force; Remove-Item $lastTaskBak -Force }
  if (Test-Path $statusFile) { Remove-Item $statusFile -Force }
}
Say ("RESTORED=" + [int](-not (Test-Path $lastTaskBak)))
Say "DONE"

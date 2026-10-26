# _t-probe-run.ps1 - ASCII only. Runs ELEVATED in session 1 (via run-gui-token.ps1).
#
# End-to-end acceptance for the GUI "Inspect" entry: switch to the Inspect tab,
# click "Create task", then verify:
#   - a NEW task directory appears under H:\brwork\tasks
#   - its status.json reports operation=probe
#   - BCD {bootmgr} still has NO bootsequence (--no-reboot must not arm anything)
#
# The GUI itself runs elevated, so ShellExecuteW("runas") does not raise a UAC
# prompt here and the whole flow can be driven unattended.
$out = 'H:\brwork\probe-run-check.txt'
function Say($s) { $s | Add-Content -Path $out -Encoding UTF8 }
Set-Content -Path $out -Value ("CHECK_START " + (Get-Date -Format o)) -Encoding UTF8

Add-Type @'
using System;
using System.Runtime.InteropServices;
using System.Text;
public class PR {
  [DllImport("user32.dll")] public static extern bool EnumWindows(EnumCb cb, IntPtr l);
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern int GetClassNameW(IntPtr h, StringBuilder s, int n);
  [DllImport("user32.dll")] public static extern IntPtr GetDlgItem(IntPtr hDlg, int id);
  [DllImport("user32.dll")] public static extern bool PostMessageW(IntPtr h, uint m, IntPtr w, IntPtr l);
  public delegate bool EnumCb(IntPtr h, IntPtr l);
  public const uint BM_CLICK = 0x00F5;
  public static string Cls(IntPtr h) { StringBuilder sb = new StringBuilder(256); GetClassNameW(h, sb, 256); return sb.ToString(); }
}
'@
function Find-Win($cls) {
  $script:found = [IntPtr]::Zero
  $cb = [PR+EnumCb]{ param($h,$l) if ([PR]::Cls($h) -eq $cls) { $script:found = $h; return $false }; return $true }
  [void][PR]::EnumWindows($cb, [IntPtr]::Zero)
  return $script:found
}

function Latest-Task {
  $d = Get-ChildItem H:\brwork\tasks -Directory -ErrorAction SilentlyContinue | Sort-Object CreationTime | Select-Object -Last 1
  if ($d) { return $d.Name } else { return "" }
}

$gui = Find-Win 'BackupRestoreNativeGui'
if ($gui -eq [IntPtr]::Zero) { Say "GUI_NOT_FOUND"; exit 1 }

$before = Latest-Task
Say ("task_before=" + $before)

[void][PR]::PostMessageW([PR]::GetDlgItem($gui, 1107), [PR]::BM_CLICK, [IntPtr]::Zero, [IntPtr]::Zero)
Start-Sleep -Milliseconds 800
[void][PR]::PostMessageW([PR]::GetDlgItem($gui, 1003), [PR]::BM_CLICK, [IntPtr]::Zero, [IntPtr]::Zero)
Say "create_task_clicked"
Start-Sleep -Seconds 60

$after = Latest-Task
Say ("task_after=" + $after)
if ($after -ne $before -and $after -ne "") {
  $sj = Join-Path "H:\brwork\tasks\$after" "status.json"
  if (Test-Path -LiteralPath $sj) {
    Say ("status_json=" + ((Get-Content -Raw -Encoding UTF8 $sj) -replace "`r`n"," " -replace "`n"," "))
  } else { Say "status_json=MISSING" }
  $tj = Join-Path "H:\brwork\tasks\$after" "task.json"
  if (Test-Path -LiteralPath $tj) {
    $raw = Get-Content -Raw -Encoding UTF8 $tj
    Say ("task_json_has_probe=" + [int]($raw -match '"probe"'))
    Say ("task_json_bytes=" + $raw.Length)
  } else { Say "task_json=MISSING" }
} else {
  Say "NO_NEW_TASK_CREATED"
}

$bs = cmd /c "bcdedit /enum {bootmgr} /v" | Select-String "bootsequence"
Say ("bootsequence_lines=" + ($bs | Measure-Object).Count)

[void][PR]::PostMessageW([PR]::GetDlgItem($gui, 1101), [PR]::BM_CLICK, [IntPtr]::Zero, [IntPtr]::Zero)
Say "CHECK_END"

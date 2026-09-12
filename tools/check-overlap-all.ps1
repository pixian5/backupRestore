# Enumerate ALL visible child controls and report pairwise overlaps.
# Runs in the interactive session (elevated) to see the real user-visible GUI.
Add-Type @"
using System;
using System.Runtime.InteropServices;
using System.Text;
public class Win32O {
    public delegate bool EnumProc(IntPtr hWnd, IntPtr lParam);
    public delegate bool EnumChildProc(IntPtr hWnd, IntPtr lParam);
    [DllImport("user32.dll")] public static extern bool EnumWindows(EnumProc cb, IntPtr lParam);
    [DllImport("user32.dll")] public static extern bool EnumChildWindows(IntPtr hParent, EnumChildProc cb, IntPtr lParam);
    [DllImport("user32.dll")] public static extern int GetWindowText(IntPtr hWnd, StringBuilder sb, int max);
    [DllImport("user32.dll")] public static extern int GetClassName(IntPtr hWnd, StringBuilder sb, int max);
    [DllImport("user32.dll")] public static extern int GetDlgCtrlID(IntPtr hWnd);
    [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr hWnd, out RECT rect);
    [DllImport("user32.dll", EntryPoint="GetWindowLongPtrW")] public static extern IntPtr GetWindowLongPtr(IntPtr hWnd, int nIndex);
    [DllImport("user32.dll")] public static extern IntPtr SendMessageW(IntPtr hWnd, uint msg, IntPtr wParam, IntPtr lParam);
    [DllImport("user32.dll")] public static extern IntPtr GetDlgItem(IntPtr hDlg, int id);
    public struct RECT { public int Left; public int Top; public int Right; public int Bottom; }
}
"@
$log = "C:\Users\Public\backupRestore-package\overlap-session1.log"
$GWL_STYLE = -16
$WS_VISIBLE = 0x10000000
$WM_COMMAND = 0x0111
$BM_CLICK = 0x00F5

function Log($msg) { Add-Content -Path $log -Value $msg -Encoding ascii }

$found = [IntPtr]::Zero
$cb = [Win32O+EnumProc]{ param($h,$l)
    $sb = New-Object System.Text.StringBuilder 512
    [void][Win32O]::GetWindowText($h, $sb, 512)
    if ($sb.ToString().StartsWith("BackupRestore - Rust GUI")) { $script:found = $h }
    return $true
}
[void][Win32O]::EnumWindows($cb, [IntPtr]::Zero)
if ($found -eq [IntPtr]::Zero) { "WINDOW NOT FOUND" | Out-File $log -Encoding ascii; exit 1 }
Log ("window=" + $found)

$global:ctrls = New-Object System.Collections.ArrayList
$cc = [Win32O+EnumChildProc]{ param($w,$l)
    $id = [Win32O]::GetDlgCtrlID($w)
    $cls = New-Object System.Text.StringBuilder 64
    [void][Win32O]::GetClassName($w, $cls, 64)
    $t = New-Object System.Text.StringBuilder 64
    [void][Win32O]::GetWindowText($w, $t, 64)
    $r = New-Object Win32O+RECT
    [void][Win32O]::GetWindowRect($w, [ref]$r)
    $style = [Win32O]::GetWindowLongPtr($w, $GWL_STYLE).ToInt64()
    $vis = ($style -band $WS_VISIBLE) -ne 0
    [void]$global:ctrls.Add([pscustomobject]@{ h=$w; id=$id; cls=$cls.ToString(); vis=$vis; L=$r.Left; T=$r.Top; R=$r.Right; B=$r.Bottom; txt=$t.ToString() })
    return $true
}
function Collect { $global:ctrls.Clear(); [void][Win32O]::EnumChildWindows($found, $cc, [IntPtr]::Zero) }
function Check-Overlaps($label) {
    Collect
    $vis = @($global:ctrls | Where-Object { $_.vis -and $_.cls -ne "ComboBox" })
    Log ("=== " + $label + " visible_noncombo=" + $vis.Count + " total=" + $global:ctrls.Count + " ===")
    $overlaps = 0
    for ($i = 0; $i -lt $vis.Count; $i++) {
        for ($j = $i + 1; $j -lt $vis.Count; $j++) {
            $a = $vis[$i]; $b = $vis[$j]
            $ovW = [Math]::Min($a.R, $b.R) - [Math]::Max($a.L, $b.L)
            $ovH = [Math]::Min($a.B, $b.B) - [Math]::Max($a.T, $b.T)
            if ($ovW -gt 4 -and $ovH -gt 4) {
                $area = $ovW * $ovH
                if ($area -gt 200) {
                    $overlaps++
                    Log ("OVERLAP: id=" + $a.id + "(" + $a.cls + ")[" + $a.txt + "] rect=(" + $a.L + "," + $a.T + ")-(" + $a.R + "," + $a.B + ")  x  id=" + $b.id + "(" + $b.cls + ")[" + $b.txt + "] rect=(" + $b.L + "," + $b.T + ")-(" + $b.R + "," + $b.B + ") area=" + $area)
                }
            }
        }
    }
    Log ("overlaps_found=" + $overlaps)
}

function Click-Tab($id) { [void][Win32O]::SendMessageW($found, $WM_COMMAND, [IntPtr][int64]$id, [IntPtr]::Zero) }
function Click-Radio($id) { $ctl = [Win32O]::GetDlgItem($found, $id); if ($ctl -ne [IntPtr]::Zero) { [void][Win32O]::SendMessageW($ctl, $BM_CLICK, [IntPtr]::Zero, [IntPtr]::Zero) } }

# fresh start: probe tab
Click-Tab 1100
Start-Sleep -Milliseconds 500
Check-Overlaps "probe"
# each tab
Click-Tab 1101
Start-Sleep -Milliseconds 500
Check-Overlaps "backup"
Click-Tab 1102
Start-Sleep -Milliseconds 500
Check-Overlaps "restore"
Click-Tab 1103
Start-Sleep -Milliseconds 500
Check-Overlaps "secondary"
Click-Tab 1104
Start-Sleep -Milliseconds 500
Check-Overlaps "pe"
# mode toggle then PE again
Click-Radio 1412
Click-Tab 1101
Click-Tab 1104
Click-Radio 1413
Start-Sleep -Milliseconds 500
Check-Overlaps "pe_after_toggles"
# stress
for ($i = 0; $i -lt 20; $i++) {
    Click-Tab 1103; Click-Tab 1104; Click-Radio 1412; Click-Radio 1413
    Click-Tab 1101; Click-Tab 1102; Click-Tab 1104
}
Start-Sleep -Seconds 1
Check-Overlaps "pe_after_stress20"
Log "DONE"

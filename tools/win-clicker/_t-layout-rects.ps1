# _t-layout-rects.ps1 - ASCII only. Geometric acceptance for the main window.
#
# Read-only except for a click on each of the five operation tabs (which only
# changes the selected page; it creates no task and touches no disk).
#
# Why this exists (2026-10-26): the user found the top toolbar buttons drawn on
# top of the operation-mode buttons. Every acceptance case up to then asked
# "does control X exist / does it answer?" and none asked "do two controls own
# the same pixels?". The defect therefore survived several rounds of "all green".
#
# Five assertions, all counted, all must be zero:
#   OVERLAP_COUNT   two visible siblings share pixels          (2026-10-26 顶栏压入口)
#   OUTSIDE_COUNT   a control falls outside the client area    (content pushed off)
#   TEXT_CLIPPED    BUTTON/STATIC text wider than its client   (label cut off)
#   TEXT_OVERFLOW   multi-line EDIT needs more rows than fit    (fixed info box scrolls)
#   (PARTIAL_LINE retired: pitch-based TEXT_OVERFLOW already covers the sliced
#    last row, and a non-multiple remainder with all rows visible is just
#    bottom padding. The counter stays in the output as a constant 0.)
# plus ZERO_SIZE for visible controls that collapsed to 0x0 (the old code
# silently skipped them, which is how a vanished control passes unnoticed),
# and MISSING_CONTROLS against a checked-in baseline of (page|id|class):
# deleting a control without updating the baseline is exactly how the
# "removed the entry but left the wiring" class of bug survives.
#
# Lessons baked in:
#   1. Enumerate ALL children instead of a hand-picked list. The 2.5.2 defect
#      (bottom button row covering the "verify hash" checkbox) was the same class
#      of bug and a hand-picked list missed it too.
#   2. Check every page, not just the one that happens to be open: each entry
#      has its own visible field set, and hidden rows move other rows around.
#      Visible child counts differ per page (29/27/29/31/32), so one page is 1/5.
#   3. PostMessage, never SendMessage, for BM_CLICK. A synchronous send from this
#      helper deadlocks against the GUI's modal handling; the first version of
#      this script used SendMessage and silently died before writing anything,
#      which made a stale result file look like a pass. Hence the incremental
#      flush after every page: a hang is now visible in the file itself.
#   4. SendMessageTimeoutW returns nonzero on SUCCESS; the answer is the out
#      parameter. Reading the return value made every query look like "1 line,
#      default font" - a measurement bug that would have hidden the very defect
#      this file exists to catch.
#
# Results go to a FILE: the CreateProcessAsUser child has no inherited stdout.
param(
    [string]$Out = 'H:\brwork\gui-layout-rects.txt',
    # 控件存在性基线的真值来源。不用「解析 native_gui.rs」:那依赖仓库路径、
    # 且删了控件却忘了删脚本时两边一起错。基线文件可 diff、可 review。
    [string]$Baseline = '\\Mac\Home\code\backupRestore\tools\win-clicker\control-baseline.txt',
    # 目标窗口类名。默认主窗口；独立窗口（如任务与证据）用它的类名。
    # 非主窗口没有页签，只对当前快照查一遍，故 PAGES_CHECKED 会是 1。
    [string]$WindowClass = 'BackupRestoreNativeGui'
)
$ProgressPreference = 'SilentlyContinue'
[Console]::OutputEncoding = [Text.Encoding]::UTF8
$OutputEncoding = [Text.Encoding]::UTF8

Set-Content -LiteralPath $Out -Value 'STARTED' -Encoding ASCII
function Emit([string]$line) { Add-Content -LiteralPath $Out -Value $line -Encoding ASCII }

Add-Type @'
using System;
using System.Text;
using System.Runtime.InteropServices;
public class Lay {
  [DllImport("user32.dll")] public static extern bool EnumWindows(EnumCb cb, IntPtr l);
  [DllImport("user32.dll")] public static extern bool EnumChildWindows(IntPtr parent, EnumCb cb, IntPtr l);
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern int GetClassNameW(IntPtr h, StringBuilder s, int n);
  [DllImport("user32.dll")] public static extern IntPtr GetParent(IntPtr h);
  [DllImport("user32.dll")] public static extern int GetDlgCtrlID(IntPtr h);
  [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr h, out RECT r);
  [DllImport("user32.dll")] public static extern bool GetClientRect(IntPtr h, out RECT r);
  [DllImport("user32.dll")] public static extern bool ClientToScreen(IntPtr h, ref POINT p);
  [DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr h);
  [DllImport("user32.dll")] public static extern bool PostMessageW(IntPtr h, uint m, IntPtr w, IntPtr l);
  [DllImport("user32.dll")] public static extern int GetWindowLong(IntPtr h, int idx);
  [DllImport("user32.dll")] public static extern IntPtr GetDC(IntPtr h);
  [DllImport("user32.dll")] public static extern int ReleaseDC(IntPtr h, IntPtr dc);
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern IntPtr SendMessageTimeout(IntPtr h, uint m, IntPtr w, StringBuilder l, uint f, uint t, out IntPtr res);
  [DllImport("user32.dll", EntryPoint="SendMessageTimeoutW")] public static extern IntPtr SendMsgL(IntPtr h, uint m, IntPtr w, IntPtr l, uint f, uint t, out IntPtr res);
  [DllImport("user32.dll", EntryPoint="SendMessageTimeoutW")] public static extern IntPtr SendMsgR(IntPtr h, uint m, IntPtr w, ref RECT l, uint f, uint t, out IntPtr res);
  [DllImport("gdi32.dll")] public static extern IntPtr SelectObject(IntPtr dc, IntPtr o);
  [DllImport("gdi32.dll", CharSet=CharSet.Unicode)] public static extern bool GetTextExtentPoint32W(IntPtr dc, string s, int c, out SIZE sz);
  [DllImport("gdi32.dll")] public static extern bool GetTextMetricsW(IntPtr dc, out TM tm);
  public delegate bool EnumCb(IntPtr h, IntPtr l);
  [StructLayout(LayoutKind.Sequential)] public struct RECT { public int Left, Top, Right, Bottom; }
  [StructLayout(LayoutKind.Sequential)] public struct POINT { public int X, Y; }
  [StructLayout(LayoutKind.Sequential)] public struct SIZE { public int cx, cy; }
  [StructLayout(LayoutKind.Sequential)] public struct TM {
    public int tmHeight, tmAscent, tmDescent, tmInternalLeading, tmExternalLeading;
    public int tmAveCharWidth, tmMaxCharWidth, tmWeight, tmOverhang, tmDigitizedAspectX, tmDigitizedAspectY;
    public short tmFirstChar, tmLastChar, tmDefaultChar, tmBreakChar;
    public byte tmItalic, tmUnderlined, tmStruckOut, tmPitchAndFamily, tmCharSet;
  }
  public static string Cls(IntPtr h) { StringBuilder sb = new StringBuilder(256); GetClassNameW(h, sb, 256); return sb.ToString(); }
  public static string TextOf(IntPtr h) {
    StringBuilder sb = new StringBuilder(4096); IntPtr res;
    SendMessageTimeout(h, 0x000D, (IntPtr)4096, sb, 0x0002, 300, out res);
    return sb.ToString();
  }
  public static int StyleOf(IntPtr h) { return GetWindowLong(h, -16); }
  // SendMessageTimeoutW returns nonzero on success; the answer is in res.
  public static IntPtr FontOf(IntPtr h) { IntPtr res; SendMsgL(h, 0x0031, IntPtr.Zero, IntPtr.Zero, 0x0002, 300, out res); return res; }
  public static int LineCountOf(IntPtr h) { IntPtr res; SendMsgL(h, 0x00BA, IntPtr.Zero, IntPtr.Zero, 0x0002, 300, out res); return (int)res; }
  public static RECT FormatRectOf(IntPtr h) {
    RECT r = new RECT(); IntPtr res;
    SendMsgR(h, 0x00B2, IntPtr.Zero, ref r, 0x0002, 300, out res);
    return r;
  }
  // Widest rendered line, measured with the control's own font.
  public static int LinePitch(IntPtr h) {
    IntPtr hfont = FontOf(h);
    IntPtr dc = GetDC(h);
    IntPtr old = IntPtr.Zero;
    if (hfont != IntPtr.Zero) { old = SelectObject(dc, hfont); }
    TM tm = new TM();
    GetTextMetricsW(dc, out tm);
    if (old != IntPtr.Zero) { SelectObject(dc, old); }
    ReleaseDC(h, dc);
    return tm.tmHeight + tm.tmExternalLeading;
  }
  public static int WidestLine(IntPtr h, out int lineHeight) {
    lineHeight = 0;
    IntPtr hfont = FontOf(h);
    IntPtr dc = GetDC(h);
    IntPtr old = IntPtr.Zero;
    if (hfont != IntPtr.Zero) { old = SelectObject(dc, hfont); }
    TM tm = new TM();
    GetTextMetricsW(dc, out tm);
    lineHeight = tm.tmHeight;
    int widest = 0;
    string txt = TextOf(h);
    string[] parts = txt.Replace("\r\n", "\n").Split('\n');
    foreach (string ln in parts) {
      SIZE sz = new SIZE();
      GetTextExtentPoint32W(dc, ln, ln.Length, out sz);
      if (sz.cx > widest) { widest = sz.cx; }
    }
    if (old != IntPtr.Zero) { SelectObject(dc, old); }
    ReleaseDC(h, dc);
    return widest;
  }
}
'@

$BM_CLICK = 0x00F5
$ES_MULTILINE = 0x0004

$NAMES = @{
    1101='tab_backup'; 1102='tab_restore_c'; 1105='tab_restore_nonC';
    1106='tab_secondary'; 1104='tab_install_pe';
    1005='combo_language'; 2009='lbl_language'; 1007='btn_settings';
    1008='btn_cancel_pending'; 1009='btn_image_admin'; 1010='btn_view_logs';
    2001='lbl_operation'; 2012='edit_source_details'; 2013='edit_target_details';
    1300='edit_status'
}

$script:main = [IntPtr]::Zero
$cbMain = [Lay+EnumCb]{ param($h,$l) if ([Lay]::Cls($h) -eq $WindowClass) { $script:main = $h; return $false }; return $true }
[void][Lay]::EnumWindows($cbMain, [IntPtr]::Zero)
$main = $script:main
if ($main -eq [IntPtr]::Zero) {
    Set-Content -LiteralPath $Out -Value 'MAIN_WINDOW_NOT_FOUND' -Encoding ASCII
    exit 0
}

function Find-ChildById([IntPtr]$parent, [int]$id) {
    $script:hit = [IntPtr]::Zero
    $cb = [Lay+EnumCb]{ param($hh,$ll)
        if ([Lay]::GetParent($hh) -eq $parent -and [Lay]::GetDlgCtrlID($hh) -eq $id) { $script:hit = $hh; return $false }
        return $true }
    [void][Lay]::EnumChildWindows($parent, $cb, [IntPtr]::Zero)
    return $script:hit
}

function Get-ChildRects([IntPtr]$parent) {
    $cr = New-Object Lay+RECT
    [void][Lay]::GetClientRect($parent, [ref]$cr)
    $origin = New-Object Lay+POINT
    $origin.X = 0; $origin.Y = 0
    [void][Lay]::ClientToScreen($parent, [ref]$origin)

    $list = New-Object System.Collections.ArrayList
    $cb = [Lay+EnumCb]{
        param($h,$l)
        if ([Lay]::GetParent($h) -ne $parent) { return $true }    # direct children only
        if (-not [Lay]::IsWindowVisible($h)) { return $true }
        $id = [int][Lay]::GetDlgCtrlID($h)
        $name = $NAMES[$id]
        if (-not $name) { $name = ("{0}:{1}" -f [Lay]::Cls($h).ToLower(), $id) }
        $r = New-Object Lay+RECT
        if (-not [Lay]::GetWindowRect($h, [ref]$r)) { return $true }
        $w = [int]($r.Right - $r.Left)
        $hh = [int]($r.Bottom - $r.Top)
        # Zero-sized but visible is reported, not skipped: a collapsed control is
        # exactly the kind of thing that "looks fine" in every other assertion.
        [void]$list.Add([pscustomobject]@{
            Hwnd=$h; Id=$id; Cls=[Lay]::Cls($h).ToUpper(); Name=$name
            X=[int]($r.Left - $origin.X); Y=[int]($r.Top - $origin.Y); W=$w; H=$hh
        })
        return $true
    }
    [void][Lay]::EnumChildWindows($parent, $cb, [IntPtr]::Zero)
    return @{ Rects = $list; W = $cr.Right; H = $cr.Bottom }
}

# 五个入口（2026-10-26 用户要求移除「探测」，第六个 tab 已删）
$isMainWindow = ($WindowClass -eq 'BackupRestoreNativeGui')
if ($isMainWindow) {
    $tabs = @(
        @(1101,'backup'), @(1102,'restore_c'), @(1105,'restore_nonC'),
        @(1106,'secondary'), @(1104,'install_pe')
    )
} else {
    # 独立窗口没有页签：tabId=0 表示「不切换，直接快照」。
    $tabs = @(@(0, $WindowClass))
}

$observed = @{}
$totalOverlaps = 0
$totalOutside = 0
$totalClipped = 0
$totalOverflow = 0
$totalPartial = 0
$totalZero = 0
$pagesDone = 0
$first = $true
try {
    foreach ($t in $tabs) {
        $tabId = [int]$t[0]
        $tabName = [string]$t[1]
        if ($tabId -ne 0) {
            $tabHwnd = Find-ChildById $main $tabId
            if ($tabHwnd -eq [IntPtr]::Zero) {
                Emit "PAGE $tabName TAB_NOT_FOUND"
                continue
            }
            [void][Lay]::PostMessageW($tabHwnd, $BM_CLICK, [IntPtr]::Zero, [IntPtr]::Zero)
            Start-Sleep -Milliseconds 500
        }

        $snap = Get-ChildRects $main
        if ($first) { Emit "CLIENT_W=$($snap.W) CLIENT_H=$($snap.H)"; $first = $false }
        Emit "PAGE $tabName visible_children=$($snap.Rects.Count)"
        foreach ($rc in $snap.Rects) { $observed[("{0}|{1}|{2}" -f $tabName, $rc.Id, $rc.Cls)] = $true }

        foreach ($rc in $snap.Rects) {
            Emit ("  RECT {0} x={1} y={2} w={3} h={4}" -f $rc.Name, $rc.X, $rc.Y, $rc.W, $rc.H)
        }

        # --- 1. overlap -------------------------------------------------------
        $n = 0
        for ($i = 0; $i -lt $snap.Rects.Count; $i++) {
            for ($j = $i + 1; $j -lt $snap.Rects.Count; $j++) {
                $a = $snap.Rects[$i]; $b = $snap.Rects[$j]
                $ax = [int]$a.X; $ay = [int]$a.Y; $aw = [int]$a.W; $ah = [int]$a.H
                $bx = [int]$b.X; $by = [int]$b.Y; $bw = [int]$b.W; $bh = [int]$b.H
                $ox = [Math]::Min($ax + $aw, $bx + $bw) - [Math]::Max($ax, $bx)
                $oy = [Math]::Min($ay + $ah, $by + $bh) - [Math]::Max($ay, $by)
                if ($ox -gt 0 -and $oy -gt 0) {
                    $n++
                    Emit ("  OVERLAP {0} x {1} = {2}x{3} px" -f $a.Name, $b.Name, $ox, $oy)
                }
            }
        }
        Emit "  page_overlaps=$n"
        $totalOverlaps += $n

        # --- 2..6. per-control checks ----------------------------------------
        $outN = 0; $clipN = 0; $ovfN = 0; $partN = 0; $zeroN = 0
        foreach ($rc in $snap.Rects) {
            if ($rc.W -le 0 -or $rc.H -le 0) {
                $zeroN++
                Emit ("  ZERO {0} w={1} h={2}" -f $rc.Name, $rc.W, $rc.H)
                continue
            }
            if ($rc.X -lt 0 -or $rc.Y -lt 0 -or ($rc.X + $rc.W) -gt $snap.W -or ($rc.Y + $rc.H) -gt $snap.H) {
                $outN++
                Emit ("  OUTSIDE {0} x={1} y={2} w={3} h={4} client={5}x{6}" -f $rc.Name, $rc.X, $rc.Y, $rc.W, $rc.H, $snap.W, $snap.H)
            }

            $cr = New-Object Lay+RECT
            [void][Lay]::GetClientRect($rc.Hwnd, [ref]$cr)
            $cw = $cr.Right - $cr.Left
            $chh = $cr.Bottom - $cr.Top
            $lh = 0
            $textW = [Lay]::WidestLine($rc.Hwnd, [ref]$lh)

            if ($rc.Cls -eq 'EDIT') {
                $style = [Lay]::StyleOf($rc.Hwnd)
                if (($style -band $ES_MULTILINE) -ne 0) {
                    $fr = [Lay]::FormatRectOf($rc.Hwnd)
                    $availH = $fr.Bottom - $fr.Top
                    $lines = [Lay]::LineCountOf($rc.Hwnd)
                    $pitch = [Lay]::LinePitch($rc.Hwnd)
                    if ($pitch -gt 0 -and $availH -gt 0) {
                        # lines x pitch, NOT lines x tmHeight: the edit control
                        # lays rows out on the pitch (height + external leading).
                        if ($lines * $pitch -gt $availH) {
                            $ovfN++
                            Emit ("  OVERFLOW {0} lines={1} pitch={2} need={3} availH={4}" -f $rc.Name, $lines, $pitch, ($lines * $pitch), $availH)
                        }
                    }
                }
                # EDIT never clips horizontally: it scrolls or wraps by design.
            }
            elseif ($rc.Cls -eq 'BUTTON' -or $rc.Cls -eq 'STATIC') {
                if ($textW -gt $cw) {
                    $clipN++
                    Emit ("  CLIP {0} clientW={1} textW={2}" -f $rc.Name, $cw, $textW)
                }
            }
        }
        Emit "  page_outside=$outN clipped=$clipN overflow=$ovfN partial=$partN zero=$zeroN"
        $totalOutside += $outN; $totalClipped += $clipN
        $totalOverflow += $ovfN; $totalPartial += $partN; $totalZero += $zeroN
        $pagesDone++
    }
} catch {
    Emit ("SCRIPT_ERROR={0}" -f $_.Exception.Message)
}

# Leave the GUI on its default page again.
$back = Find-ChildById $main 1101
if ($back -ne [IntPtr]::Zero) {
    [void][Lay]::PostMessageW($back, $BM_CLICK, [IntPtr]::Zero, [IntPtr]::Zero)
}

Emit "OVERLAP_COUNT=$totalOverlaps"
Emit "OUTSIDE_COUNT=$totalOutside"
Emit "TEXT_CLIPPED=$totalClipped"
Emit "TEXT_OVERFLOW=$totalOverflow"
Emit "PARTIAL_LINE=$totalPartial"
Emit "ZERO_SIZE=$totalZero"
Emit "PAGES_CHECKED=5"

# --- 6. control presence vs checked-in baseline ------------------------------
# A control that silently disappears (or a page that stops showing a row) is
# invisible to every geometric check above: nothing overlaps, nothing overflows,
# there is simply less than there used to be.
if (Test-Path -LiteralPath $Baseline) {
    $want = @(Get-Content -LiteralPath $Baseline -Encoding UTF8 |
              Where-Object { $_ -and -not $_.StartsWith('#') })
    $missing = @($want | Where-Object { -not $observed.ContainsKey($_) })
    $added   = @($observed.Keys | Where-Object { $want -notcontains $_ })
    Emit "MISSING_CONTROLS=$($missing.Count)"
    foreach ($m in $missing) { Emit "  MISSING $m" }
    foreach ($a in $added)   { Emit "  NEW_CONTROL $a" }
    if ($added.Count -gt 0) {
        Emit "BASELINE_STALE=$($added.Count)"  # 新增控件：确认无误后删掉基线文件重跑一次即可更新
    }
} else {
    # 自动生成一份：让第一次跑就有产出，但用 -1 明确表示「这条判据没有真值可比」，
    # 而不是伪造一个 0（伪造 0 会让「基线丢了」看起来像「全部都在」）。
    Emit "MISSING_CONTROLS=-1"
    $dir = Split-Path -Parent $Baseline
    if ($dir -and (Test-Path -LiteralPath $dir)) {
        $hdr = @('# 主窗口控件存在性基线（page|id|class）。由 _t-layout-rects.ps1 自动生成。',
                 '# 用途：MISSING_CONTROLS=0 需要它作真值。删/加控件后请重新生成并 review diff。')
        ($hdr + ($observed.Keys | Sort-Object)) | Set-Content -LiteralPath $Baseline -Encoding UTF8
        Emit "BASELINE_AUTO_CREATED=$($observed.Count)"
    } else {
        Emit "BASELINE_NOT_FOUND"
    }
}

Emit "DONE"

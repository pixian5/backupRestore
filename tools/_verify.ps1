$env:TEMP = "C:\Users\Public"
$env:TMP  = "C:\Users\Public"
try { Start-Transcript -LiteralPath "H:\brwork\_verify.log" -Force | Out-Null } catch { }
Add-Type @'
using System;
using System.Runtime.InteropServices;
using System.Text;
public static class V {
    [DllImport("user32.dll")] public static extern IntPtr GetDlgItem(IntPtr h, int id);
    [DllImport("user32.dll")] public static extern bool EnumWindows(EnumProc cb, IntPtr lParam);
    [DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr h);
    public delegate bool EnumProc(IntPtr hWnd, IntPtr lParam);
    [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern int GetWindowTextW(IntPtr h, StringBuilder sb, int max);
    [DllImport("user32.dll", EntryPoint="SendMessageW")] public static extern IntPtr SendM(IntPtr h, uint m, IntPtr w, IntPtr l);
    [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern IntPtr SendText(IntPtr h, uint m, IntPtr w, StringBuilder l);
    public static IntPtr FindByPrefix(string p) {
        IntPtr f = IntPtr.Zero;
        EnumWindows((h,l) => { if (!IsWindowVisible(h)) return true; StringBuilder sb = new StringBuilder(512); GetWindowTextW(h, sb, 512); if (sb.ToString().StartsWith(p)) { f = h; return false; } return true; }, IntPtr.Zero);
        return f;
    }
    public static string Txt(IntPtr c) {
        if (c == IntPtr.Zero) return "<null>";
        StringBuilder sb = new StringBuilder(2048);
        SendText(c, 0x000D, new IntPtr(2048), sb);
        return sb.ToString().Replace("\r"," ").Replace("\n"," | ");
    }
}
'@
$main = [V]::FindByPrefix("BackupRestore - Rust GUI")
if ($main -eq [IntPtr]::Zero) { Write-Output "WINDOW_NOT_FOUND" } else {
    Write-Output ("HWND=" + $main)
    $combo = [V]::GetDlgItem($main, 1202)
    $count = [V]::SendM($combo, 0x0146, [IntPtr]::Zero, [IntPtr]::Zero).ToInt32()
    $cur = [V]::SendM($combo, 0x0147, [IntPtr]::Zero, [IntPtr]::Zero).ToInt32()
    Write-Output ("SOURCE_COUNT=" + $count + " CURSEL=" + $cur)
    Write-Output ("SOURCE_TEXT='" + [V]::Txt($combo) + "'")
    Write-Output ("IMAGE1203='" + [V]::Txt([V]::GetDlgItem($main, 1203)) + "'")
    Write-Output ("DETAIL2012='" + [V]::Txt([V]::GetDlgItem($main, 2012)) + "'")
}
try { Stop-Transcript | Out-Null } catch { }

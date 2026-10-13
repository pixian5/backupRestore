$env:TEMP = "C:\Users\Public"
$env:TMP  = "C:\Users\Public"
try { Start-Transcript -LiteralPath "H:\brwork\_probe3.log" -Force | Out-Null } catch { }
Add-Type @'
using System;
using System.Runtime.InteropServices;
using System.Text;
public static class R {
    [DllImport("user32.dll")] public static extern IntPtr GetDlgItem(IntPtr h, int id);
    [DllImport("user32.dll")] public static extern bool EnumWindows(EnumProc cb, IntPtr lParam);
    [DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr h);
    public delegate bool EnumProc(IntPtr hWnd, IntPtr lParam);
    [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern int GetWindowTextW(IntPtr h, StringBuilder sb, int max);
    [DllImport("user32.dll", EntryPoint="SendMessageW")] public static extern IntPtr SendM(IntPtr h, uint m, IntPtr w, IntPtr l);
    [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern IntPtr SendText(IntPtr h, uint m, IntPtr w, StringBuilder l);
    public static IntPtr FindByPrefix(string prefix) {
        IntPtr found = IntPtr.Zero;
        EnumWindows((h, l) => {
            if (!IsWindowVisible(h)) return true;
            StringBuilder sb = new StringBuilder(512);
            GetWindowTextW(h, sb, 512);
            if (sb.ToString().StartsWith(prefix)) { found = h; return false; }
            return true;
        }, IntPtr.Zero);
        return found;
    }
    public static int ComboCount(IntPtr c) { return SendM(c, 0x014B, IntPtr.Zero, IntPtr.Zero).ToInt32(); }
    public static string ComboText(IntPtr c, int i) {
        int len = SendM(c, 0x0149, new IntPtr(i), IntPtr.Zero).ToInt32();
        if (len <= 0) return "";
        IntPtr b = Marshal.AllocHGlobal((len + 2) * 2);
        try { SendM(c, 0x0148, new IntPtr(i), b); return Marshal.PtrToStringUni(b, len); }
        finally { Marshal.FreeHGlobal(b); }
    }
    public static string Txt(IntPtr p, int id) {
        IntPtr c = GetDlgItem(p, id);
        if (c == IntPtr.Zero) return "<null>";
        StringBuilder sb = new StringBuilder(2048);
        SendText(c, 0x000D, new IntPtr(2048), sb);
        return sb.ToString().Replace("\r", " ").Replace("\n", " | ");
    }
}
'@
$main = [R]::FindByPrefix("BackupRestore - Rust GUI")
if ($main -eq [IntPtr]::Zero) { Write-Output "WINDOW_NOT_FOUND" } else {
    Write-Output ("HWND=" + $main)
    $combo = [R]::GetDlgItem($main, 1202)
    $n = [R]::ComboCount($combo)
    Write-Output ("SOURCE_COMBO_COUNT=" + $n)
    for ($i = 0; $i -lt $n; $i++) { Write-Output ("  item[" + $i + "]='" + [R]::ComboText($combo, $i) + "'") }
    Write-Output ("LOG1302='" + [R]::Txt($main, 1302) + "'")
    Write-Output ("IMG1203='" + [R]::Txt($main, 1203) + "'")
}
try { Stop-Transcript | Out-Null } catch { }

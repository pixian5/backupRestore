$env:TEMP = "C:\Users\Public"
$env:TMP  = "C:\Users\Public"
try { Start-Transcript -LiteralPath "H:\brwork\_probe-src.log" -Force | Out-Null } catch { }
Add-Type @'
using System;
using System.Runtime.InteropServices;
using System.Text;
public static class P {
    [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern int GetWindowText(IntPtr hWnd, StringBuilder sb, int max);
    [DllImport("user32.dll")] public static extern IntPtr GetDlgItem(IntPtr h, int id);
    [DllImport("user32.dll")] public static extern bool EnumWindows(EnumProc cb, IntPtr lParam);
    public delegate bool EnumProc(IntPtr hWnd, IntPtr lParam);
    [DllImport("user32.dll", EntryPoint="SendMessageW")] public static extern IntPtr SendM(IntPtr h, uint m, IntPtr w, IntPtr l);
    public static IntPtr FindByPrefix(string p) {
        IntPtr f = IntPtr.Zero;
        EnumWindows((h,l) => {
            StringBuilder sb = new StringBuilder(512);
            GetWindowText(h, sb, 512);
            if (sb.ToString().StartsWith(p)) { f = h; return false; }
            return true;
        }, IntPtr.Zero);
        return f;
    }
    public static int ComboCount(IntPtr combo) {
        return SendM(combo, 0x014B, IntPtr.Zero, IntPtr.Zero).ToInt32();
    }
    public static string ComboText(IntPtr combo, int idx) {
        int len = SendM(combo, 0x0149, new IntPtr(idx), IntPtr.Zero).ToInt32();
        if (len <= 0) { return ""; }
        IntPtr buf = Marshal.AllocHGlobal((len + 2) * 2);
        try {
            SendM(combo, 0x0148, new IntPtr(idx), buf);
            return Marshal.PtrToStringUni(buf, len);
        } finally { Marshal.FreeHGlobal(buf); }
    }
    public static string Text(IntPtr parent, int id) {
        IntPtr c = GetDlgItem(parent, id);
        if (c == IntPtr.Zero) { return "<null>"; }
        StringBuilder sb = new StringBuilder(1024);
        SendM(c, 0x000D, new IntPtr(1024), sb);
        return sb.ToString().Replace("`r"," ").Replace("`n"," | ");
    }
}
'@
$main = [P]::FindByPrefix("BackupRestore - Rust GUI")
if ($main -eq [IntPtr]::Zero) { Write-Output "WINDOW_NOT_FOUND"; } else {
    Write-Output ("hwnd=" + $main)
    $combo = [P]::GetDlgItem($main, 1202)
    Write-Output ("combo1202 handle=" + $combo)
    if ($combo -ne [IntPtr]::Zero) {
        $n = [P]::ComboCount($combo)
        Write-Output ("SOURCE_COMBO_COUNT=" + $n)
        for ($i = 0; $i -lt $n; $i++) {
            Write-Output ("  item[" + $i + "]='" + [P]::ComboText($combo, $i) + "'")
        }
    }
    Write-Output ("status1205='" + [P]::Text($main, 1205) + "'")
    Write-Output ("image1203='" + [P]::Text($main, 1203) + "'")
}
try { Stop-Transcript | Out-Null } catch { }

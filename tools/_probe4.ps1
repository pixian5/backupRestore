$env:TEMP = "C:\Users\Public"
$env:TMP  = "C:\Users\Public"
try { Start-Transcript -LiteralPath "H:\brwork\_probe4.log" -Force | Out-Null } catch { }
Add-Type @'
using System;
using System.Runtime.InteropServices;
using System.Text;
public static class S {
    [DllImport("user32.dll")] public static extern IntPtr GetDlgItem(IntPtr h, int id);
    [DllImport("user32.dll")] public static extern bool EnumWindows(EnumProc cb, IntPtr lParam);
    [DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr h);
    public delegate bool EnumProc(IntPtr hWnd, IntPtr lParam);
    [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern int GetWindowTextW(IntPtr h, StringBuilder sb, int max);
    [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern int GetClassNameW(IntPtr h, StringBuilder sb, int max);
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
    public static string Class(IntPtr h) {
        StringBuilder sb = new StringBuilder(128);
        GetClassNameW(h, sb, 128);
        return sb.ToString();
    }
    public static string Text(IntPtr h) {
        StringBuilder sb = new StringBuilder(1024);
        SendText(h, 0x000D, new IntPtr(1024), sb);
        return sb.ToString().Replace("\r", " ").Replace("\n", " | ");
    }
    public static int ComboCount(IntPtr c) { return SendM(c, 0x014B, IntPtr.Zero, IntPtr.Zero).ToInt32(); }
    public static string ComboText(IntPtr c, int i) {
        int len = SendM(c, 0x0149, new IntPtr(i), IntPtr.Zero).ToInt32();
        if (len <= 0) return "";
        IntPtr b = Marshal.AllocHGlobal((len + 2) * 2);
        try { SendM(c, 0x0148, new IntPtr(i), b); return Marshal.PtrToStringUni(b, len); }
        finally { Marshal.FreeHGlobal(b); }
    }
}
'@
$main = [S]::FindByPrefix("BackupRestore - Rust GUI")
if ($main -eq [IntPtr]::Zero) { Write-Output "WINDOW_NOT_FOUND" } else {
    Write-Output ("HWND=" + $main)
    foreach ($id in 1000..1450) {
        $c = [S]::GetDlgItem($main, $id)
        if ($c -eq [IntPtr]::Zero) { continue }
        $cls = [S]::Class($c)
        if ($cls -match "Combo") {
            $n = [S]::ComboCount($c)
            Write-Output ("ID=" + $id + " COMBO count=" + $n)
            for ($i = 0; $i -lt $n; $i++) {
                Write-Output ("    item[" + $i + "]='" + [S]::ComboText($c, $i) + "'")
            }
        } elseif ($cls -match "Edit") {
            Write-Output ("ID=" + $id + " EDIT text='" + [S]::Text($c) + "'")
        }
    }
    foreach ($id in 2000..2100) {
        $c = [S]::GetDlgItem($main, $id)
        if ($c -eq [IntPtr]::Zero) { continue }
        $cls = [S]::Class($c)
        if ($cls -match "Combo") {
            $n = [S]::ComboCount($c)
            Write-Output ("ID=" + $id + " COMBO count=" + $n)
            for ($i = 0; $i -lt $n; $i++) {
                Write-Output ("    item[" + $i + "]='" + [S]::ComboText($c, $i) + "'")
            }
        }
    }
}
try { Stop-Transcript | Out-Null } catch { }

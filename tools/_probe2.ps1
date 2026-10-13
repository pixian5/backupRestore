$env:TEMP = "C:\Users\Public"
$env:TMP  = "C:\Users\Public"
try { Start-Transcript -LiteralPath "H:\brwork\_probe2.log" -Force | Out-Null } catch { }
Add-Type @'
using System;
using System.Runtime.InteropServices;
using System.Text;
public static class Q {
    [DllImport("user32.dll")] public static extern IntPtr GetDlgItem(IntPtr h, int id);
    [DllImport("user32.dll", EntryPoint="SendMessageW")] public static extern IntPtr SendM(IntPtr h, uint m, IntPtr w, IntPtr l);
    [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern int GetWindowTextW(IntPtr h, StringBuilder sb, int max);
    [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern IntPtr SendText(IntPtr h, uint m, IntPtr w, StringBuilder l);
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
    public static string Text(IntPtr ctl) {
        if (ctl == IntPtr.Zero) { return "<null>"; }
        StringBuilder sb = new StringBuilder(1024);
        SendText(ctl, 0x000D, new IntPtr(1024), sb);
        return sb.ToString().Replace("\r"," ").Replace("\n"," | ");
    }
}
'@
$main = [IntPtr]263060
$sbTitle = New-Object System.Text.StringBuilder 512
[void][Q]::GetWindowTextW($main, $sbTitle, 512)
Write-Output ("TITLE='" + $sbTitle.ToString() + "'")
$combo = [Q]::GetDlgItem($main, 1202)
Write-Output ("combo1202=" + $combo)
$n = [Q]::ComboCount($combo)
Write-Output ("SOURCE_COMBO_COUNT=" + $n)
for ($i = 0; $i -lt $n; $i++) {
    Write-Output ("  item[" + $i + "]='" + [Q]::ComboText($combo, $i) + "'")
}
Write-Output ("status1205='" + [Q]::Text([Q]::GetDlgItem($main, 1205)) + "'")
Write-Output ("image1203='" + [Q]::Text([Q]::GetDlgItem($main, 1203)) + "'")
try { Stop-Transcript | Out-Null } catch { }

# br-win-list.ps1 —— 列出 VM 可见桌面上的窗口（标题 / hwnd / 位置矩形 / 是否前台）
#
# 用途：点击前先查坐标。坐标系是 VM 内部的「虚拟屏幕像素」，逐点对应 clicker 的 -X/-Y，
# 但注意 VM 若有 DPI 缩放，SetCursorPos 用的是物理像素（实测 1234 被裁到 1155），
# 所以本脚本同时输出屏幕宽高和 DPI，方便换算。
#
# 输出直接走 stdout（prlctl exec 会回传），无需中间文件。
# C# 代码块内保持 ASCII（PS -File 按 GBK 解码，UTF-8 中文注释会炸掉 C# 编译器）。

Add-Type -TypeDefinition @"
using System;
using System.Runtime.InteropServices;
using System.Text;
using System.Collections.Generic;
public class BrWin {
    [DllImport("user32.dll")] public static extern bool EnumWindows(EnumCb cb, IntPtr l);
    public delegate bool EnumCb(IntPtr h, IntPtr l);
    [DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr h);
    [DllImport("user32.dll")] public static extern int GetWindowText(IntPtr h, StringBuilder s, int n);
    [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr h, out RECT r);
    [DllImport("user32.dll")] public static extern IntPtr GetForegroundWindow();
    [DllImport("user32.dll")] public static extern int GetWindowTextLength(IntPtr h);
    [DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(IntPtr h, out uint pid);
    [StructLayout(LayoutKind.Sequential)] public struct RECT { public int L, T, R, B; }
    [DllImport("user32.dll")] public static extern int GetSystemMetrics(int n);
    public static string Title(IntPtr h) {
        int len = GetWindowTextLength(h);
        if (len <= 0) return "";
        StringBuilder sb = new StringBuilder(len + 2);
        GetWindowText(h, sb, len + 2);
        return sb.ToString();
    }
    public static List<string> List() {
        var res = new List<string>();
        IntPtr fg = GetForegroundWindow();
        EnumWindows(delegate(IntPtr h, IntPtr l) {
            if (!IsWindowVisible(h)) return true;
            string t = Title(h);
            if (t.Length == 0) return true;
            RECT r; GetWindowRect(h, out r);
            uint pid; GetWindowThreadProcessId(h, out pid);
            res.Add("hwnd=" + h.ToInt64() + " pid=" + pid + " rect=" + r.L + "," + r.T + "," + r.R + "," + r.B
                    + " fg=" + (h == fg ? 1 : 0) + " title=[" + t + "]");
            return true;
        }, IntPtr.Zero);
        return res;
    }
}
"@

Write-Output ("screen=" + [BrWin]::GetSystemMetrics(0) + "x" + [BrWin]::GetSystemMetrics(1))
Write-Output ("virtualScreen=" + [BrWin]::GetSystemMetrics(78) + "x" + [BrWin]::GetSystemMetrics(79))
foreach ($line in [BrWin]::List()) { Write-Output $line }
Write-Output "WINLIST_DONE"

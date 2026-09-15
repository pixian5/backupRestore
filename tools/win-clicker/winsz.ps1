# winsz.ps1 - report interactive-session screen bounds + GUI window rect (run in Session 1)
Add-Type -TypeDefinition @"
using System;
using System.Text;
using System.Runtime.InteropServices;
public class Wsz {
    [StructLayout(LayoutKind.Sequential)] public struct RECT { public int left; public int top; public int right; public int bottom; }
    [DllImport("user32.dll")] public static extern IntPtr GetForegroundWindow();
    [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr hWnd, out RECT r);
    [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern int GetWindowText(IntPtr h, StringBuilder s, int n);
}
"@
$fg = [Wsz]::GetForegroundWindow()
$r = New-Object Wsz+RECT
[Wsz]::GetWindowRect($fg, [ref]$r) | Out-Null
$sb = New-Object System.Text.StringBuilder 128
[Wsz]::GetWindowText($fg, $sb, 128) | Out-Null
$out = @()
$out += "fg=$fg title='$($sb.ToString())'"
$out += "win_rect=$($r.left),$($r.top),$($r.right),$($r.bottom)"
Add-Type -AssemblyName System.Windows.Forms
$out += "screen=$([System.Windows.Forms.Screen]::PrimaryScreen.Bounds)"
$out += "virt=$([System.Windows.Forms.SystemInformation]::VirtualScreen)"
$out += "dpi_scale=$([System.Windows.Forms.Screen]::PrimaryScreen.Bounds.Width / 100.0)"
$out | Out-File C:\Users\Public\backupRestore-package\winsz.txt -Encoding utf8

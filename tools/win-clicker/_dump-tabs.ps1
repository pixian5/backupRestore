$ErrorActionPreference = 'Continue'
try {
$sig = @'
using System;
using System.Text;
using System.Collections.Generic;
using System.Runtime.InteropServices;
using System.Threading;
public class CD4 {
  public delegate bool EnumProc(IntPtr h, IntPtr p);
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern bool EnumWindows(EnumProc cb, IntPtr p);
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern int GetWindowTextW(IntPtr h, StringBuilder s, int n);
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern int GetClassNameW(IntPtr h, StringBuilder s, int n);
  [DllImport("user32.dll")] public static extern bool EnumChildWindows(IntPtr h, EnumProc cb, IntPtr p);
  [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr h, out R r);
  [DllImport("user32.dll")] public static extern bool GetClientRect(IntPtr h, out R r);
  [DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr h);
  [DllImport("user32.dll")] public static extern bool ScreenToClient(IntPtr h, ref P p);
  [DllImport("user32.dll")] public static extern IntPtr SendMessage(IntPtr h, uint m, IntPtr w, IntPtr l);
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern IntPtr SendMessage(IntPtr h, uint m, IntPtr w, StringBuilder l);
  [DllImport("user32.dll")] public static extern IntPtr GetDlgItem(IntPtr h, int id);
  [DllImport("user32.dll")] public static extern int GetDlgCtrlID(IntPtr h);
  public struct R { public int l, t, r, b; }
  public struct P { public int X, Y; }
  const uint WM_GETTEXT = 0x000D;
  const uint WM_GETTEXTLENGTH = 0x000E;
  const uint BM_CLICK = 0x00F5;
  static IntPtr g_top;
  static List<string> g_out;
  static string Esc(string s) {
    var o = new StringBuilder();
    foreach (char c in s) {
      if (c == (char)10) o.Append("\\n");
      else if (c < 128) o.Append(c);
      else o.Append("\\u").Append(((int)c).ToString("x4"));
    }
    return o.ToString();
  }
  // 2026-10-24 修复：读别的进程的控件文本必须用 WM_GETTEXT 消息（GetWindowTextW
  // API 跨进程只返回创建时缓存的窗口名，编辑框创建时为空就恒为空）。详见
  // _dump-now.ps1 同处注释与 .test-artifacts/recon-38-lite-20261024/editprobe2.ps1。
  static string Txt(IntPtr h) {
    int n = (int)SendMessage(h, WM_GETTEXTLENGTH, IntPtr.Zero, IntPtr.Zero);
    if (n <= 0) return "";
    var sb = new StringBuilder(n + 1);
    SendMessage(h, WM_GETTEXT, (IntPtr)sb.Capacity, sb);
    return sb.ToString();
  }
  static bool Cb(IntPtr h, IntPtr p) {
    R r; GetWindowRect(h, out r);
    P tl; tl.X = r.l; tl.Y = r.t; ScreenToClient(g_top, ref tl);
    P br; br.X = r.r; br.Y = r.b; ScreenToClient(g_top, ref br);
    var sb = new StringBuilder(256); GetClassNameW(h, sb, 256);
    int id = 0;
    try { id = GetDlgCtrlID(h); } catch (Exception) { id = -1; }
    string t = Txt(h);
    string vis = IsWindowVisible(h) ? "1" : "0";
    if (vis == "0" && id < 0) return true;
    g_out.Add("C|" + id + "|" + sb.ToString() + "|" + tl.X + "," + tl.Y + "," + br.X + "," + br.Y + "|" + vis + "|" + Esc(t));
    return true;
  }
  static IntPtr g_main;
  static bool FindCb(IntPtr h, IntPtr p) {
    if (!IsWindowVisible(h)) return true;
    int n = (int)SendMessage(h, WM_GETTEXTLENGTH, IntPtr.Zero, IntPtr.Zero);
    if (n <= 0) return true;
    var sb = new StringBuilder(n + 2);
    SendMessage(h, WM_GETTEXT, (IntPtr)(n + 2), sb);
    if (sb.ToString().Contains("BackupRestore")) { g_main = h; return false; }
    return true;
  }
  static void DumpAll() {
    g_out.Add("BEGIN");
    EnumChildWindows(g_main, Cb, IntPtr.Zero);
    g_out.Add("END");
  }
  public static List<string> Run() {
    g_out = new List<string>();
    g_main = IntPtr.Zero;
    EnumWindows(FindCb, IntPtr.Zero);
    if (g_main == IntPtr.Zero) { g_out.Add("MAIN_NOT_FOUND"); return g_out; }
    g_top = g_main;
    int[] tabs = new int[] { 1101, 1102, 1105, 1106, 1104 };
    string[] names = new string[] { "backup", "restore_a", "restore_b", "create_secondary", "install_pe" };
    for (int i = 0; i < tabs.Length; i++) {
      IntPtr btn = GetDlgItem(g_main, tabs[i]);
      g_out.Add("TAB\t" + names[i] + "\tbtn=" + (btn == IntPtr.Zero ? "NULL" : "ok"));
      if (btn != IntPtr.Zero) {
        SendMessage(btn, BM_CLICK, IntPtr.Zero, IntPtr.Zero);
        Thread.Sleep(1200);
      }
      DumpAll();
    }
    g_out.Add("ALLDONE");
    return g_out;
  }
}
'@
Add-Type -TypeDefinition $sig
$res = [CD4]::Run()
foreach ($line in $res) { Write-Output $line }
} catch {
  Write-Output ("CAUGHT: " + $_.Exception.Message)
}

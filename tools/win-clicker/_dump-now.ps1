$ErrorActionPreference = 'Continue'
try {
$sig = @'
using System;
using System.Text;
using System.Collections.Generic;
using System.Runtime.InteropServices;
public class CD3 {
  public delegate bool EnumProc(IntPtr h, IntPtr p);
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern bool EnumWindows(EnumProc cb, IntPtr p);
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern int GetWindowTextW(IntPtr h, StringBuilder s, int n);
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern int GetClassNameW(IntPtr h, StringBuilder s, int n);
  [DllImport("user32.dll")] public static extern bool EnumChildWindows(IntPtr h, EnumProc cb, IntPtr p);
  [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr h, out R r);
  [DllImport("user32.dll")] public static extern bool GetClientRect(IntPtr h, out R r);
  [DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr h);
  [DllImport("user32.dll")] public static extern bool ClientToScreen(IntPtr h, ref P p);
  [DllImport("user32.dll")] public static extern bool ScreenToClient(IntPtr h, ref P p);
  [DllImport("user32.dll")] public static extern IntPtr SendMessage(IntPtr h, uint m, IntPtr w, IntPtr l);
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern IntPtr SendMessage(IntPtr h, uint m, IntPtr w, StringBuilder l);
  [DllImport("user32.dll")] public static extern int GetDlgCtrlID(IntPtr h);
  public struct R { public int l, t, r, b; }
  public struct P { public int X, Y; }
  const uint WM_GETTEXT = 0x000D;
  const uint WM_GETTEXTLENGTH = 0x000E;
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
  // 2026-10-24 修复：读别的进程的控件文本必须用 WM_GETTEXT 消息，不能用
  // GetWindowTextW API——后者跨进程只返回「创建时缓存」的窗口名：编辑框
  // 创建时为空就恒返回空、之后被程序改过（如 apply_language 换中文）就返回
  // 陈旧英文默认值。实测对照（editprobe2）：GetWindowTextW=[] 而
  // WM_GETTEXT=[Windows (T:) 20261024]。顶层窗口标题不受此影响，但统一走
  // 消息更稳。按钮同理安全。
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
    g_out.Add("ID=" + id + "|" + sb.ToString() + "|client=" + tl.X + "," + tl.Y + ".." + br.X + "," + br.Y + "|vis=" + (IsWindowVisible(h) ? 1 : 0) + "|text=" + Esc(t));
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
  public static List<string> Run() {
    g_out = new List<string>();
    g_main = IntPtr.Zero;
    EnumWindows(FindCb, IntPtr.Zero);
    if (g_main == IntPtr.Zero) { g_out.Add("MAIN_NOT_FOUND"); return g_out; }
    g_top = g_main;
    R wr; GetWindowRect(g_main, out wr);
    R cr; GetClientRect(g_main, out cr);
    P o; o.X = 0; o.Y = 0; ClientToScreen(g_main, ref o);
    g_out.Add("MAIN|hwnd=" + g_main.ToInt64() + "|winRect=" + wr.l + "," + wr.t + ".." + wr.r + "," + wr.b + "|clientSize=" + cr.l + "," + cr.t + ".." + cr.r + "," + cr.b + "|clientOrigin=" + o.X + "," + o.Y + "|title=" + Esc(Txt(g_main)));
    EnumChildWindows(g_main, Cb, IntPtr.Zero);
    g_out.Add("DUMP_DONE");
    return g_out;
  }
}
'@
Add-Type -TypeDefinition $sig
$res = [CD3]::Run()
foreach ($line in $res) { Write-Output $line }
} catch {
  Write-Output ("CAUGHT: " + $_.Exception.Message)
}

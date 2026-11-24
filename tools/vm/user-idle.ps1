# Ask the user's session: how long since the last keyboard/mouse input?
#
# Why not `quser`: for the local console session `quser` always prints "none" in the IDLE TIME column
# (measured 2026-11-24: identical readings right after resume and minutes later), so it cannot tell
# "somebody is typing" from "nobody touched it since boot".
#
# Two readings, because the idle timer restarts at zero on every reboot:
#   IDLE_MS                  ms since the last keyboard/mouse input
#   UPTIME_MS                ms since boot
#   LAST_INPUT_AFTER_BOOT_MS = UPTIME_MS - IDLE_MS: when (after boot) the last input happened.
# Right after a reboot "idle 70 s" may only mean "booted 70 s ago and nobody touched it".
# So: no input since boot -> nobody is using it; there was input and it was recent -> in use.
#
# NOTE: keep this file ASCII-only (PowerShell 5 reads BOM-less files as ANSI).
param([int]$ThresholdSeconds = 120)
Add-Type @"
using System;
using System.Runtime.InteropServices;
public static class IdleProbe {
    [StructLayout(LayoutKind.Sequential)]
    public struct LASTINPUTINFO { public uint cbSize; public uint dwTime; }
    [DllImport("user32.dll")] public static extern bool GetLastInputInfo(ref LASTINPUTINFO plii);
    [DllImport("kernel32.dll")] public static extern uint GetTickCount();
    public static uint IdleMilliseconds() {
        LASTINPUTINFO info = new LASTINPUTINFO();
        info.cbSize = (uint)Marshal.SizeOf(info);
        if (!GetLastInputInfo(ref info)) return uint.MaxValue;
        return GetTickCount() - info.dwTime;
    }
}
"@
$idle = [IdleProbe]::IdleMilliseconds()
if ($idle -eq [uint32]::MaxValue) { "IDLE_READ=FAILED"; "__END__"; exit 1 }
$uptime = [Environment]::TickCount
$sinceInput = $uptime - $idle
"SESSION=" + (Get-Process -Id $PID).SessionId
"IDLE_MS=$idle"
"IDLE_SECS=" + [int]($idle / 1000)
"UPTIME_MS=$uptime"
"LAST_INPUT_AFTER_BOOT_MS=$sinceInput"
# Verdict:
#   NO_INPUT_SINCE_BOOT  the last "input" falls within 15 s after boot -> that is the initial value,
#                        nobody has typed since boot;
#   IN_USE               there was input and the last one is more recent than the threshold;
#   IDLE                 otherwise.
if ($sinceInput -le 15000) { "IDLE_VERDICT=NO_INPUT_SINCE_BOOT" }
elseif ($idle -lt ($ThresholdSeconds * 1000)) { "IDLE_VERDICT=IN_USE" }
else { "IDLE_VERDICT=IDLE" }
"__END__"

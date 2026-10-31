# NOTE: keep this file ASCII-only.
#
# Resident ELEVATED helper: ONE console for the whole session.
#
# Why: the old design elevated a brand-new cmd.exe for EVERY probe
# (`Start-Process cmd.exe -Verb RunAs`). On Windows 11 the default terminal is
# Windows Terminal, so each console app is handed off to a host console pair
# (conhost.exe + OpenConsole.exe). When the process exits too fast the handoff
# fails and the pair is ORPHANED: its window stays on the desktop forever.
# Measured 2026-10-31: 828 orphan pairs accumulated, 432 of them inside a single
# hour of GUI-matrix work. The desktop was a wall of black windows.
#
# This helper starts once (elevated), hides its own console, then serves
# requests from a queue directory. Each request runs the target script with
# -NoNewWindow, i.e. it INHERITS this console instead of allocating a new one.
# No new console => nothing to leak.
#
# Protocol (all ASCII, no spaces in values):
#   <Root>\run\<id>.req    line 1 = script file name, line 2 = extra args
#   <Root>\run\<id>.out    script stdout (contains the script's own __END__)
#   <Root>\run\<id>.err    script stderr
#   <Root>\run\<id>.done   written after the request was consumed
#   <Root>\helper.pid      this process id (liveness marker)
#
# Stop it with: tools/vm/postflight.sh clean (or kill the pid in helper.pid).
param(
  [string]$Root = 'C:\Users\Public\pkg',
  [string]$Pkg  = 'H:\brwork\gui-check'
)

$ErrorActionPreference = 'Continue'

# Hide our own console window: otherwise it sits on top of the app under test
# and pollutes screenshots (shot.ps1) and window z-order.
$src = @'
using System;
using System.Runtime.InteropServices;
public class BrHelperConsole {
  [DllImport("kernel32.dll")] public static extern IntPtr GetConsoleWindow();
  [DllImport("user32.dll")] public static extern bool ShowWindow(IntPtr hWnd, int nCmdShow);
}
'@
try {
  Add-Type -TypeDefinition $src | Out-Null
  [void][BrHelperConsole]::ShowWindow([BrHelperConsole]::GetConsoleWindow(), 0)
} catch { }

$run = Join-Path $Root 'run'
New-Item -ItemType Directory -Force -Path $run | Out-Null

# Liveness marker written LAST-ish: runas.ps1 waits for this file, so if it
# exists the loop is already running and can accept requests.
Set-Content -LiteralPath (Join-Path $Root 'helper.pid') -Value $PID -Encoding ASCII
'HELPER_READY pid=' + $PID

while ($true) {
  $reqs = @(Get-ChildItem -LiteralPath $run -Filter '*.req' -ErrorAction SilentlyContinue | Sort-Object Name)
  foreach ($req in $reqs) {
    $id   = $req.BaseName
    $out  = Join-Path $run ($id + '.out')
    $err  = Join-Path $run ($id + '.err')
    $lines = @(Get-Content -LiteralPath $req.FullName -Encoding UTF8 -ErrorAction SilentlyContinue)
    $name = ''
    if ($lines.Count -ge 1) { $name  = ([string]$lines[0]).Trim() }
    $extra = ''
    if ($lines.Count -ge 2) { $extra = ([string]$lines[1]).Trim() }

    try {
      if ($name -eq '') { throw 'empty request (no script name)' }
      $script = Join-Path $Pkg $name
      if (-not (Test-Path -LiteralPath $script)) { throw ('script not found: ' + $script) }
      $argv = @('-NoProfile', '-ExecutionPolicy', 'Bypass', '-File', $script)
      if ($extra -ne '') { $argv += ($extra -split '\s+') }
      # -NoNewWindow is the whole point: the child reuses THIS console.
      $p = Start-Process -FilePath 'powershell.exe' -ArgumentList $argv -NoNewWindow -PassThru `
             -RedirectStandardOutput $out -RedirectStandardError $err
      # A hung probe must not brick the helper: bound it, then kill.
      if (-not $p.WaitForExit(120000)) {
        try { $p.Kill() } catch { }
        try { Add-Content -LiteralPath $out -Value ('HELPER_TIMEOUT name=' + $name) -ErrorAction SilentlyContinue } catch { }
      }
    } catch {
      try { Set-Content -LiteralPath $out -Value ('HELPER_EXEC_ERROR name=' + $name + ' msg=' + $_.Exception.Message) -Encoding UTF8 } catch { }
    }

    Remove-Item -LiteralPath $req.FullName -Force -ErrorAction SilentlyContinue
    try { Set-Content -LiteralPath (Join-Path $run ($id + '.done')) -Value 'OK' -Encoding ASCII } catch { }
  }
  Start-Sleep -Milliseconds 120
}

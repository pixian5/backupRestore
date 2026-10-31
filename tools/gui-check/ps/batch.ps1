# NOTE: keep this file ASCII-only.
#
# Run several judgement scripts inside ONE prlctl call.
#
# Why: every `prlctl exec --current-user powershell` leaks one orphan
# conhost/OpenConsole pair (measured 2026-11-01: +1 per call, elevated or not --
# the leaking process is the CLIENT, not the probe). A main-window combo used to
# make 7 such calls, an auxiliary window 5. Batching cuts that several-fold.
#
# Each child is started with -NoNewWindow so it INHERITS this process's console:
# no new console => nothing to leak.
#
# Protocol: one parameter per job, e.g.
#   -J1 "a.ps1|-Class X" -J2 "b.ps1|-FontSize 18 -Class X"
# (script name and extra args separated by |).
#
# ★ Do NOT encode the job list into ONE argument with a separator: prlctl exec
#   mangles it -- "@@" was dropped entirely and "~~" came out as a space, so
#   three jobs silently collapsed into one (measured 2026-11-01; cost ~1h).
#   Separate parameters survive intact.
#
# Output: each child's stdout (each ends with its own __END__), then stderr if
# any, then BATCH=<n> and a final __END__.
param(
  [string]$J1 = '', [string]$J2 = '', [string]$J3 = '',
  [string]$J4 = '', [string]$J5 = ''
)

$ErrorActionPreference = 'Continue'
$Pkg = 'H:\brwork\gui-check'

$specs = @($J1, $J2, $J3, $J4, $J5) | Where-Object { $_.Trim() -ne '' }
if ($specs.Count -eq 0) { 'BATCH_EMPTY'; '__END__'; exit 1 }

$stamp = [DateTime]::Now.ToString('yyyyMMddHHmmssfff') + '-' + ([Guid]::NewGuid().ToString('N').Substring(0, 6))
$tmpdir = Join-Path $env:TEMP ('br-batch-' + $stamp)
New-Item -ItemType Directory -Force -Path $tmpdir | Out-Null

$index = 0
foreach ($spec in $specs) {
  $index++
  $parts = $spec -split '\|', 2
  $name = $parts[0].Trim()
  $extra = ''
  if ($parts.Count -ge 2) { $extra = $parts[1].Trim() }
  $script = Join-Path $Pkg $name
  $out = Join-Path $tmpdir ("{0}.out" -f $index)
  $err = Join-Path $tmpdir ("{0}.err" -f $index)
  if (-not (Test-Path -LiteralPath $script)) {
    'BATCH_MISSING name=' + $name
    continue
  }
  $argv = @('-NoProfile', '-ExecutionPolicy', 'Bypass', '-File', $script)
  if ($extra -ne '') { $argv += ($extra -split '\s+') }
  try {
    # -NoNewWindow: inherit THIS console, so no orphan console host is created.
    $p = Start-Process -FilePath 'powershell.exe' -ArgumentList $argv -NoNewWindow -PassThru `
           -RedirectStandardOutput $out -RedirectStandardError $err
    if (-not $p.WaitForExit(120000)) {
      try { $p.Kill() } catch { }
      'BATCH_TIMEOUT name=' + $name
    }
  } catch {
    'BATCH_ERROR name=' + $name + ' msg=' + $_.Exception.Message
  }
  if (Test-Path -LiteralPath $out) { Get-Content -LiteralPath $out -Encoding UTF8 }
  if (Test-Path -LiteralPath $err) {
    $e = Get-Content -LiteralPath $err -Encoding UTF8
    if ($e) { $e }
  }
}

Remove-Item -LiteralPath $tmpdir -Recurse -Force -ErrorAction SilentlyContinue
'BATCH=' + $index
'__END__'

# NOTE: keep this file ASCII-only.
# Run a judgement script ELEVATED and return its stdout.
#
# Why this exists: the app runs elevated (launch.ps1 uses -Verb RunAs). A
# NON-elevated process cannot inject input into a higher-integrity window --
# UIPI drops SetCursorPos/mouse_event SILENTLY. The script still reported
# CLICK=OK with correct coordinates, yet not a single tab ever switched
# (proved by DIFF_COUNT=0 on the visible-control-id set). Verdicts that need to
# click MUST therefore run elevated.
#
# 2026-10-31 rewrite: elevation is done by ONE resident helper (helper.ps1)
# instead of a fresh `cmd.exe -Verb RunAs` per probe. The old way leaked an
# orphan conhost/OpenConsole pair on almost every call; 828 of them piled up.
# This script now only drops a request file and waits for the answer.
#
# Output contract is unchanged: the target script's own stdout (which ends with
# its own __END__), then a HELPER=... line, then __END__.
param([string]$Name = '', [string]$Extra = '')

$ErrorActionPreference = 'Continue'
if ($Name -eq '') { 'RUNAS_NO_NAME'; '__END__'; exit 1 }

$Root = 'C:\Users\Public\pkg'
$Pkg  = 'H:\brwork\gui-check'
$run  = Join-Path $Root 'run'
New-Item -ItemType Directory -Force -Path $run | Out-Null

function Get-HelperPid {
  $f = Join-Path $Root 'helper.pid'
  if (-not (Test-Path -LiteralPath $f)) { return 0 }
  $raw = (Get-Content -LiteralPath $f -ErrorAction SilentlyContinue | Select-Object -First 1)
  $n = 0
  if ([int]::TryParse(([string]$raw).Trim(), [ref]$n)) { return $n }
  return 0
}

function Test-HelperAlive {
  $h = Get-HelperPid
  if ($h -le 0) { return $false }
  return [bool](Get-Process -Id $h -ErrorAction SilentlyContinue)
}

# ---- make sure the helper is up (start it once, elevated) ----
$alive = Test-HelperAlive
if (-not $alive) {
  Remove-Item -LiteralPath (Join-Path $Root 'helper.pid') -ErrorAction SilentlyContinue
  $hp = Join-Path $Root 'helper.ps1'
  Copy-Item -LiteralPath (Join-Path $Pkg 'helper.ps1') -Destination $hp -Force -ErrorAction SilentlyContinue
  if (Test-Path -LiteralPath $hp) {
    $null = Start-Process -FilePath 'powershell.exe' -Verb RunAs -WindowStyle Hidden `
              -ArgumentList @('-NoProfile', '-ExecutionPolicy', 'Bypass', '-File', $hp) -PassThru
    for ($i = 0; $i -lt 60; $i++) {
      Start-Sleep -Milliseconds 250
      if (Test-HelperAlive) { $alive = $true; break }
    }
  }
}

if (-not $alive) {
  # ---- fallback: legacy per-call elevation. It leaks one console pair, but a
  # harness that cannot run at all is worse. Reported explicitly so the leak is
  # visible instead of silent.
  $dst = Join-Path $Root $Name
  Copy-Item -LiteralPath (Join-Path $Pkg $Name) -Destination $dst -Force
  $o2 = Join-Path $Root ('legacy-out-' + [DateTime]::Now.ToString('yyyyMMddHHmmssfff') + '.txt')
  $cmdline = "powershell -NoProfile -ExecutionPolicy Bypass -File `"$dst`" $Extra > `"$o2`" 2>&1"
  $null = Start-Process -FilePath 'cmd.exe' -ArgumentList '/c', $cmdline -Verb RunAs -PassThru
  for ($i = 0; $i -lt 240; $i++) {
    Start-Sleep -Milliseconds 500
    if (-not (Test-Path $o2)) { continue }
    if ((Get-Content -LiteralPath $o2 -Encoding UTF8) -match '__END__') { break }
  }
  if (Test-Path $o2) { Get-Content -LiteralPath $o2 -Encoding UTF8 } else { "RUNAS_NO_OUTPUT name=$Name" }
  'HELPER=fallback'
  '__END__'
  Remove-Item -LiteralPath $o2 -ErrorAction SilentlyContinue
  exit 0
}

# ---- hand the request to the helper and wait ----
$id  = [DateTime]::Now.ToString('yyyyMMddHHmmssfff') + '-' + ([Guid]::NewGuid().ToString('N').Substring(0, 6))
$req = Join-Path $run ($id + '.req')
$out = Join-Path $run ($id + '.out')
$err = Join-Path $run ($id + '.err')
Set-Content -LiteralPath $req -Value @($Name, $Extra) -Encoding UTF8

$ready = $false
# 150 ms poll: the helper answers in a few hundred ms, so a coarse poll would
# waste up to half a second per probe (the old per-call elevation cost ~1 s, and
# a matrix runs hundreds of probes).
for ($i = 0; $i -lt 800; $i++) {
  Start-Sleep -Milliseconds 150
  if (-not (Test-Path -LiteralPath $out)) { continue }
  $txt = Get-Content -LiteralPath $out -Encoding UTF8
  if ($txt -match '__END__') { $ready = $true; break }
}

if ($ready) {
  Get-Content -LiteralPath $out -Encoding UTF8
  # keep the old merged-stream behaviour: stderr used to land in the same file
  if (Test-Path -LiteralPath $err) {
    $e = Get-Content -LiteralPath $err -Encoding UTF8
    if ($e) { $e }
  }
  'HELPER=resident pid=' + (Get-HelperPid)
} else {
  # do NOT leave the request behind: a stale request would be executed later,
  # out of order, and could disturb the next measurement
  Remove-Item -LiteralPath $req -Force -ErrorAction SilentlyContinue
  "RUNAS_NO_OUTPUT name=$Name helper_alive=$(Test-HelperAlive)"
  'HELPER=resident_timeout pid=' + (Get-HelperPid)
}
'__END__'

Remove-Item -LiteralPath $out, $err, (Join-Path $run ($id + '.done')) -Force -ErrorAction SilentlyContinue

# NOTE: keep this file ASCII-only.
# Run a judgement script ELEVATED and return its stdout.
#
# Why this exists: the app runs elevated (launch.ps1 uses -Verb RunAs). A
# NON-elevated process cannot inject input into a higher-integrity window --
# UIPI drops SetCursorPos/mouse_event SILENTLY. The script still reported
# CLICK=OK with correct coordinates, yet not a single tab ever switched
# (proved by DIFF_COUNT=0 on the visible-control-id set). Verdicts that need to
# click MUST therefore run elevated.
param([string]$Name = '', [string]$Extra = '')
$ErrorActionPreference = 'Continue'
if ($Name -eq '') { 'RUNAS_NO_NAME'; '__END__'; exit 1 }
$dst = "C:\Users\Public\pkg\$Name"
Copy-Item -LiteralPath "H:\brwork\gui-check\$Name" -Destination $dst -Force
# ★ Unique file per invocation. A fixed name is a silent-corruption trap: a
# previous run that HUNG keeps the redirect handle open, so Remove-Item fails
# and the next run reads the STALE content while reporting a fresh result.
$out = "C:\Users\Public\pkg\runas-out-$([DateTime]::Now.ToString('yyyyMMddHHmmss')).txt"
$stamp = "C:\Users\Public\pkg\runas-start-$([DateTime]::Now.ToString('yyyyMMddHHmmss')).txt"
$cmdline = "powershell -NoProfile -ExecutionPolicy Bypass -File `"$dst`" $Extra > `"$out`" 2>&1"
$null = Start-Process -FilePath 'cmd.exe' -ArgumentList '/c', $cmdline -Verb RunAs -PassThru
# Wait for the COMPLETION marker, not merely for the file to appear: `>` creates
# the file immediately, so reading on first sight returns a half-written prefix.
for ($i = 0; $i -lt 80; $i++) {
  Start-Sleep -Milliseconds 500
  if (-not (Test-Path $out)) { continue }
  $txt = Get-Content $out -Encoding UTF8
  if ($txt -match '__END__') { break }
}
if (Test-Path $out) { Get-Content $out -Encoding UTF8 } else { "RUNAS_NO_OUTPUT name=$Name" }
'__END__'
Remove-Item $out -ErrorAction SilentlyContinue

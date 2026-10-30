# NOTE: keep this file ASCII-only.
# Read the LAST "tooltip audit:" line the app wrote at startup.
# Prints CANDIDATES=<n> UNTIPPED=<n> so the caller can tell
#   "every control is tipped" (candidates>0, untipped=0)
# apart from "the audit found no controls at all" (candidates=0 -> vacuous).
$ErrorActionPreference = 'Continue'
$log = if ($args.Count -ge 1 -and $args[0]) { $args[0] } else { 'H:\brwork\logs\gui.log' }
if (-not (Test-Path -LiteralPath $log)) { 'TIP_AUDIT=NO_LOG'; '__END__'; exit 1 }
$line = Get-Content -LiteralPath $log -Encoding UTF8 |
        Where-Object { $_ -match 'tooltip audit:' } |
        Select-Object -Last 1
if (-not $line) { 'TIP_AUDIT=NO_RECORD'; '__END__'; exit 1 }
$cand = if ($line -match 'total_candidates=(\d+)') { $matches[1] } else { 'NA' }
$unt  = if ($line -match 'untipped=(\d+)')        { $matches[1] } else { 'NA' }
"CANDIDATES=$cand UNTIPPED=$unt"
'__END__'

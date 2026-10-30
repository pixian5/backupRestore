# NOTE: keep this file ASCII-only (PowerShell 5 reads BOM-less files as ANSI).
# Kill any running GUI then relaunch the deployed exe elevated.
# Prints "PID=<n>" or "PID=NONE"; always ends with __END__ so the caller can tell
# "no process" apart from "the channel did not run anything".
$ErrorActionPreference = 'Continue'
$exe = if ($args.Count -ge 1 -and $args[0]) { $args[0] } else { 'H:\brwork\BackupRestore.exe' }
Get-Process -Name BackupRestore -ErrorAction SilentlyContinue | Stop-Process -Force -ErrorAction SilentlyContinue
Start-Sleep -Milliseconds 900
if (-not (Test-Path -LiteralPath $exe)) { "PID=MISSING exe=$exe"; '__END__'; exit 1 }
Start-Process -FilePath $exe -Verb RunAs
Start-Sleep -Seconds 4
$p = Get-Process -Name BackupRestore -ErrorAction SilentlyContinue
if ($p) { "PID=" + $p.Id } else { "PID=NONE" }
'__END__'

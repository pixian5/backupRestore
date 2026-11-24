# Start the GUI elevated WITHOUT stopping anything first.
# Why: tools/gui-check/ps/launch.ps1 stops every process named BackupRestore, and the
# command-line `prepare` process has the same image name. Using it here would make the
# race test measure "the harness killed the prepare" instead of the product behaviour.
# NOTE: keep this file ASCII-only (PowerShell 5 reads BOM-less files as ANSI).
$ErrorActionPreference = 'Continue'
$exe = 'H:\brwork\BackupRestore.exe'
$before = @(Get-Process -Name BackupRestore -ErrorAction SilentlyContinue | ForEach-Object { $_.Id })
Start-Process -FilePath $exe -Verb RunAs
Start-Sleep -Seconds 3
$after = @(Get-Process -Name BackupRestore -ErrorAction SilentlyContinue | ForEach-Object { $_.Id })
"PROCESSES_BEFORE=" + ($before -join ',')
"PROCESSES_AFTER=" + ($after -join ',')
"__END__"

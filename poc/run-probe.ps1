$ErrorActionPreference = 'Stop'
$log = 'C:\WinRE-PoC\app-run.log'
try {
    & 'C:\WinRE-PoC\app\BackupRestore.ps1' -Operation probe -NoReboot *>&1 |
        Tee-Object -FilePath $log -Append
    Set-Content 'C:\WinRE-PoC\app-run-success.txt' (Get-Date -Format o) -Encoding ascii
}
catch {
    $_ | Format-List * -Force | Out-File $log -Append
    Set-Content 'C:\WinRE-PoC\app-run-failed.txt' (Get-Date -Format o) -Encoding ascii
    exit 1
}

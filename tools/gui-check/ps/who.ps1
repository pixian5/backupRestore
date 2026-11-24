# Read-only: which session are the helper and the GUI processes in, and what is their command line.
[Console]::OutputEncoding=[Text.Encoding]::UTF8
"THIS_SESSION=$((Get-Process -Id $PID).SessionId) THIS_PID=$PID"
Get-CimInstance Win32_Process | Where-Object { $_.Name -in @('BackupRestore.exe','helper.ps1') -or ($_.Name -eq 'powershell.exe' -and $_.CommandLine -match 'helper\.ps1|runas\.ps1') } | ForEach-Object {
  $role = if ([string]$_.CommandLine -match 'helper\.ps1') { 'helper' } elseif ([string]$_.CommandLine -match 'runas\.ps1') { 'runas' } else { 'gui' }
  "ROLE={4} {0} pid={1} session={2} parent={3}" -f $_.Name, $_.ProcessId, $_.SessionId, $_.ParentProcessId, $role
}
"helper.pid file=" + ((Get-Content 'C:\Users\Public\pkg\helper.pid' -ErrorAction SilentlyContinue) -join '')
"__END__"

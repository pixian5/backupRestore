# Stop any resident elevated helper (whatever session it is in). The caller restarts it in the desktop session.
[Console]::OutputEncoding=[Text.Encoding]::UTF8
$n = 0
Get-CimInstance Win32_Process | Where-Object { $_.Name -eq 'powershell.exe' -and $_.CommandLine -match 'helper\.ps1' } | ForEach-Object {
  Stop-Process -Id $_.ProcessId -Force -ErrorAction SilentlyContinue; $n++
}
Remove-Item -LiteralPath 'C:\Users\Public\pkg\helper.pid' -Force -ErrorAction SilentlyContinue
"HELPERS_STOPPED=$n"
"__END__"

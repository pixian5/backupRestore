$events = Get-WinEvent -FilterHashtable @{LogName='System'; StartTime=(Get-Date).AddHours(-2)} -ErrorAction SilentlyContinue | Where-Object { $_.ProviderName -match 'boot|Winload|volmgr|Disk|Kernel-Power|storahci|stornvme|HAL.Initialized|BugCheck' }
$out = @()
foreach ($e in $events) {
    $line = "{0} | Id={1} | {2} | {3} | {4}" -f $e.TimeCreated.ToString("HH:mm:ss"), $e.Id, $e.LevelDisplayName, $e.ProviderName, (($e.Message -replace '\r?\n',' ').Substring(0,[Math]::Min(160,(($e.Message -replace '\r?\n',' ').Length))))
    $out += $line
}
if ($out.Count -eq 0) { $out = "no matching events in last 2h" }
Set-Content -Path 'C:\Users\Public\backupRestore-package\boot-log.txt' -Value $out
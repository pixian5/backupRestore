# Check C: free space + VSS/DISM task status (ASCII)
$out = 'C:\Users\Public\backupRestore-package\space-check.log'
$lines = New-Object System.Collections.ArrayList
$v = Get-Volume -DriveLetter C
[void]$lines.Add(('FREE_GB={0}' -f [math]::Round($v.SizeRemaining/1GB,1)))
[void]$lines.Add(('SIZE_GB={0}' -f [math]::Round($v.Size/1GB,1)))
foreach ($tn in 'VssClean','DismClean') {
    $t = Get-ScheduledTask -TaskName $tn -ErrorAction SilentlyContinue
    if ($t) {
        $i = $t | Get-ScheduledTaskInfo
        [void]$lines.Add(("TASK $tn : LastRun=$($i.LastRunTime) LastResult=$($i.LastTaskResult) Status=$($t.State)"))
    } else { [void]$lines.Add("TASK $tn : NOT_FOUND") }
}
$lines | Set-Content -Path $out -Encoding ascii
Write-Output 'DONE'

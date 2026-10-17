# br-diag-bcdrevert.ps1 -- 诊断 default 被回写的原因
$ErrorActionPreference = 'Continue'
"=== bootmgr full ==="
& bcdedit /enum {bootmgr} /v | ForEach-Object { $_ }
"=== firmware ==="
& bcdedit /enum firmware | ForEach-Object { $_ }
"=== boot events (System log) ==="
Get-WinEvent -FilterHashtable @{LogName='System'; ID=100,27,20,12,13,6005,6006,6008,41,1074,1076} -MaxEvents 25 -ErrorAction SilentlyContinue |
  Sort-Object TimeCreated | ForEach-Object {
    $_.TimeCreated.ToString("HH:mm:ss") + " id=" + $_.Id + " " + $_.ProviderName + " " + ($_.Message -replace "`r?`n"," ").Substring(0, [Math]::Min(90, $_.Message.Length))
  }
"=== scheduled tasks (non-microsoft) ==="
Get-ScheduledTask -ErrorAction SilentlyContinue | Where-Object { $_.TaskPath -notlike "\Microsoft\*" } | ForEach-Object { $_.TaskPath + $_.TaskName + " state=" + $_.State }
"=== services with BackupRestore ==="
Get-Service | Where-Object { $_.Name -like "*ackup*" -or $_.DisplayName -like "*ackup*" } | ForEach-Object { $_.Name + " " + $_.Status + " " + $_.StartType }
"=== last boot ==="
(Get-CimInstance Win32_OperatingSystem).LastBootUpTime.ToString("yyyy-MM-dd HH:mm:ss")

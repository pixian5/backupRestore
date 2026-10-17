# br-diag-boot2.ps1 -- 聚焦：启动时间线 + 计划任务 + 服务
$ErrorActionPreference = 'Continue'
"=== last boot ==="
(Get-CimInstance Win32_OperatingSystem).LastBootUpTime.ToString("yyyy-MM-dd HH:mm:ss")
"=== System log boot-related (last 2h) ==="
$since = (Get-Date).AddHours(-2)
Get-WinEvent -FilterHashtable @{LogName='System'; StartTime=$since} -ErrorAction SilentlyContinue |
  Where-Object { $_.Id -in 100,27,20,12,13,6005,6006,6008,41,1074,1076,6009,6013,7001,7000,7026,7031,7034,7040 } |
  Sort-Object TimeCreated | ForEach-Object {
    $_.TimeCreated.ToString("HH:mm:ss") + " id=" + $_.Id + " prov=" + $_.ProviderName
  }
"=== non-MS scheduled tasks ==="
Get-ScheduledTask -ErrorAction SilentlyContinue | Where-Object { $_.TaskPath -notlike "\Microsoft\*" } | ForEach-Object { $_.TaskPath + $_.TaskName + " state=" + $_.State }
"=== BackupRestore services ==="
Get-Service -ErrorAction SilentlyContinue | Where-Object { $_.Name -like "*ackup*" -or $_.DisplayName -like "*ackup*" } | ForEach-Object { $_.Name + " " + $_.Status + " " + $_.StartType }
"=== BCD file mtime ==="
(Get-Item "S:\EFI\Microsoft\Boot\BCD").LastWriteTime.ToString("yyyy-MM-dd HH:mm:ss")

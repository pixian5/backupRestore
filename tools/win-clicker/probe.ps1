# probe.ps1 - 诊断当前 GUI 实例 与 auto_install 状态
$p = Get-Process -Name BackupRestore -ErrorAction SilentlyContinue | Select-Object -First 1
if ($p) {
    Write-Output ("PROC Id={0} Start={1} Session={2}" -f $p.Id, $p.StartTime.ToString('yyyy-MM-dd HH:mm:ss'), $p.SessionId)
} else {
    Write-Output "PROC none"
}
$cfg = Get-Content C:\br-test.json -Raw -ErrorAction SilentlyContinue
Write-Output ("CFG=" + $cfg)
$log = Get-Content C:\Users\Public\backupRestore-package\logs\gui.log -Tail 20 -ErrorAction SilentlyContinue
Write-Output "=== LOG TAIL ==="
$log | ForEach-Object { Write-Output $_ }
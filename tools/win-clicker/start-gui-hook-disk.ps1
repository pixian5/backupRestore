# start-gui-hook-disk.ps1 - ASCII only (staged as GBK by br-s1.sh; keep it ASCII).
# Same as start-gui-hook.ps1 but for the PE HARD DISK mode: the target partition
# is formatted and the WIM is applied onto it (destructive on that partition).
param(
    [string]$Target = "T",
    [string]$Name = "BR-PE-DISK"
)
Write-Output ("TARGET=" + $Target + " NAME=" + $Name)
$json = '{"tab":"pe","mode":"disk","target_volume":"' + $Target + '","pe_name":"' + $Name + '","image":"H:\\BRImages\\pe-boot.wim","auto_install":true}'
[IO.File]::WriteAllText('C:\br-test.json', $json, (New-Object System.Text.UTF8Encoding($false)))
# read back: the hook silently falls back to defaults if the file is unreadable
$back = [IO.File]::ReadAllText('C:\br-test.json')
Write-Output ("JSON_WRITTEN=" + $json.Length + " RB=" + $back.Length)
Get-Process BackupRestore -ErrorAction SilentlyContinue | Stop-Process -Force
Start-Sleep -Seconds 2
Start-Process -FilePath 'H:\brwork\BackupRestore.exe' -ArgumentList '--test-hook' -WorkingDirectory 'H:\brwork'
Start-Sleep -Seconds 10
$p = Get-Process BackupRestore -ErrorAction SilentlyContinue | Select-Object -First 1
if ($p) {
    Write-Output ("GUI_PID=" + $p.Id + " SESSION=" + $p.SessionId)
} else {
    Write-Output "GUI_NOT_RUNNING"
}

# glog.ps1 - 打印 gui.log 最后 50 行
Get-Content C:\Users\Public\backupRestore-package\logs\gui.log -Tail 50 -ErrorAction SilentlyContinue | ForEach-Object { Write-Output $_ }
$s = New-Object -ComObject Shell.Application
$s.ShellExecute("powershell.exe", "-NoProfile -ExecutionPolicy Bypass -File C:\Users\Public\pkg\re-gui-run.ps1 -ImagePath F:\6.wim -StopBefore 0", "", "runas", 1)

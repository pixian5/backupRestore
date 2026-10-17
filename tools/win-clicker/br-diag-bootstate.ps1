# br-diag-bootstate.ps1 -- 诊断当前启动的是哪个系统、卷字母布局
$ErrorActionPreference = 'Continue'
$os = Get-CimInstance Win32_OperatingSystem
"bootuptime=" + $os.LastBootUpTime.ToString("yyyy-MM-dd HH:mm:ss")
"systemdrive=" + $os.SystemDrive
"systemroot=" + $env:SystemRoot
"computername=" + $env:COMPUTERNAME
"--- volumes ---"
Get-Volume | Where-Object { $_.DriveLetter } | ForEach-Object {
  "drive=" + $_.DriveLetter + " label=" + $_.FileSystemLabel + " sizeGiB=" + [math]::Round($_.Size/1GB,1) + " serial=" + $_.UniqueId
}
"--- boot entries ---"
& bcdedit /enum OSLOADER | ForEach-Object { $_ }

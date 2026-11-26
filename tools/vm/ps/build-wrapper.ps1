# Build + deploy inside the VM (the only release path). Everything goes to a per-run log with start / end markers.
# The caller (tools/vm/rebuild-deploy.sh) passes -Tag and reads the log back; it will not continue without the end marker.
# NOTE: keep this file ASCII-only (PowerShell 5 reads BOM-less files as ANSI).
param([string]$Tag = 'main')
$log = "H:\brwork\build-$Tag.log"
"TOOLS_BUILD_RUN_START tag=$Tag $(Get-Date -Format 'HH:mm:ss')" | Out-File -Encoding utf8 $log
& X:\windows\build-windows.ps1 -Architecture arm64 -Deploy -CargoTargetDir H:\br-target *>> $log
"TOOLS_BUILD_RUN_END tag=$Tag rc=$LASTEXITCODE $(Get-Date -Format 'HH:mm:ss')" | Out-File -Append -Encoding utf8 $log

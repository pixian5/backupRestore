# Build + deploy inside the VM (the only release path). Everything goes to one log with start / end markers.
# The caller (tools/vm/rebuild-deploy.sh) reads the log back and will not continue without the end marker.
# NOTE: keep this file ASCII-only (PowerShell 5 reads BOM-less files as ANSI).
$log = 'H:\brwork\build-vm-tools.log'
"TOOLS_BUILD_RUN_START $(Get-Date -Format 'HH:mm:ss')" | Out-File -Encoding utf8 $log
& X:\windows\build-windows.ps1 -Architecture arm64 -Deploy -CargoTargetDir H:\br-target *>> $log
"TOOLS_BUILD_RUN_END rc=$LASTEXITCODE $(Get-Date -Format 'HH:mm:ss')" | Out-File -Append -Encoding utf8 $log

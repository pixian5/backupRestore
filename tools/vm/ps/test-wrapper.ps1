# Run unit tests inside the VM on Windows (the half that a macOS `cargo test` never compiles).
# Launched detached by test-launcher.ps1; the caller reads the log back and waits for the end marker.
# param: -Tag <name>  (log file name),  -Filter <test name filter>  (empty = the whole workspace)
param([string]$Tag = 'main', [string]$Filter = '')
$log = "H:\brwork\test-$Tag.log"
"TOOLS_TEST_RUN_START tag=$Tag filter=$Filter $(Get-Date -Format 'HH:mm:ss')" | Out-File -Encoding utf8 $log
$env:CARGO_TARGET_DIR = 'H:\br-target'
$env:RUSTFLAGS = '-C target-feature=+crt-static'
$prevEap = $ErrorActionPreference
$ErrorActionPreference = 'Continue'
Set-Location X:\
$cargoArgs = @('test', '--workspace', '--target', 'aarch64-pc-windows-msvc', '--no-fail-fast')
if ($Filter -ne '') { $cargoArgs += $Filter }
& cargo @cargoArgs *>> $log
$rc = $LASTEXITCODE
$ErrorActionPreference = $prevEap
"TOOLS_TEST_RUN_END tag=$Tag rc=$rc $(Get-Date -Format 'HH:mm:ss')" | Out-File -Append -Encoding utf8 $log

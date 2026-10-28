# br-build-pe-2677.ps1 —— 用 2.6.77 包重出 PE 镜像（SkipIso）
# 由 br-s1.sh 提权拉到 Session 1 执行；stdout 落到 C:\Users\Public\pkg\br-build-pe-2677.out.log
# 判据不看本脚本的退出码，看：
#   1) build-result.txt 里的 SHA256（脚本自报，只说明跑完）
#   2) 之后另做只读挂载核对 Windows\System32\Recovery.exe 的 SHA-256（真判据）
$ErrorActionPreference = 'Stop'
$build = 'C:\br\build-backuprestore-pe.ps1'
& powershell.exe -NoProfile -ExecutionPolicy Bypass -File $build -SkipIso
$rc = $LASTEXITCODE
"BUILD_EXIT_CODE=$rc"
$result = 'C:\BackupRestorePE\build-result.txt'
if (Test-Path -LiteralPath $result) {
    "---- build-result.txt ----"
    Get-Content -LiteralPath $result
} else {
    "BUILD_RESULT_MISSING"
}
$pkg = 'C:\BackupRestorePE\package\BackupRestore-windows-arm64-v2.6.77'
"PKG_RECOVERY_SHA256=" + (Get-FileHash -LiteralPath "$pkg\Recovery.exe" -Algorithm SHA256).Hash.ToLowerInvariant()
"PKG_FRONTEND_SHA256=" + (Get-FileHash -LiteralPath "$pkg\BackupRestore.exe" -Algorithm SHA256).Hash.ToLowerInvariant()
"PKG_VCRUNTIME140_SHA256=" + (Get-FileHash -LiteralPath "$pkg\VCRUNTIME140.dll" -Algorithm SHA256).Hash.ToLowerInvariant()
"PKG_VCRUNTIME140_1_SHA256=" + (Get-FileHash -LiteralPath "$pkg\VCRUNTIME140_1.dll" -Algorithm SHA256).Hash.ToLowerInvariant()
"PKG_WINPESHL_BYTES=" + (Get-Item -LiteralPath "$pkg\winpe-winpeshl.ini").Length
$out = 'C:\BackupRestorePE\BackupRestorePE.wim'
if (Test-Path -LiteralPath $out) {
    "OUTPUT_BYTES=" + (Get-Item -LiteralPath $out).Length
    "OUTPUT_SHA256=" + (Get-FileHash -LiteralPath $out -Algorithm SHA256).Hash.ToLowerInvariant()
} else {
    "OUTPUT_MISSING"
}
"DONE_MARKER_2677"

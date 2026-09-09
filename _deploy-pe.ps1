$out = 'C:\Users\x\Desktop\BackupRestore\_deploy-log.txt'
$lines = @()
$lines += "=== deploy $(Get-Date -Format o) ==="
# 1. BCD current state (elevated)
$lines += '--- BCD enum all ---'
$lines += (bcdedit /enum 2>&1 | ForEach-Object { $_ })
# 2. Backup stock boot.wim, replace with custom PE desktop wim
$src = 'C:\BackupRestorePE\media\media\sources\boot.wim'
$dst = 'Q:\sources\boot.wim'
if (-not (Test-Path "$dst.stock")) {
    Copy-Item -LiteralPath $dst -Destination "$dst.stock" -Force
    $lines += "backed up stock boot.wim -> $dst.stock"
} else {
    $lines += 'stock backup already present'
}
$h1 = (Get-FileHash -LiteralPath $dst -Algorithm SHA256).Hash
$h2 = (Get-FileHash -LiteralPath $src -Algorithm SHA256).Hash
$lines += "Q current boot.wim sha256=$h1"
$lines += "new  boot.wim sha256=$h2"
if ($h1 -ne $h2) {
    Copy-Item -LiteralPath $src -Destination $dst -Force
    $lines += 'replaced Q:\sources\boot.wim with PE desktop wim'
}
$h3 = (Get-FileHash -LiteralPath $dst -Algorithm SHA256).Hash
$lines += "Q new boot.wim sha256=$h3"
Set-Content -LiteralPath $out -Value $lines -Encoding UTF8
Write-Output 'DEPLOY_WRITTEN'

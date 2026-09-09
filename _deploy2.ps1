$out = 'C:\Users\x\Desktop\BackupRestore\_deploy2-log.txt'
$lines = @()
$lines += "=== deploy2 $(Get-Date -Format o) ==="

function Invoke-Bcd([string]$Cmd) {
    $result = cmd /c $Cmd 2>&1
    $code = $LASTEXITCODE
    $lines += "> $Cmd"
    $lines += $result
    $lines += "exit=$code"
}

# 1. Copy PE media to Q:
$src = 'C:\BackupRestorePE\media\media'
$dst = 'Q:\'
$lines += '--- copy media to Q: ---'
robocopy $src $dst /E /NFL /NDL /NJH /NJS /NP | Out-Null
$lines += "robocopy exit=$LASTEXITCODE"
$lines += (Get-ChildItem Q:\sources -ErrorAction SilentlyContinue | ForEach-Object { "Q:\sources\$($_.Name) $($_.Length)" })
$lines += (Get-ChildItem Q:\boot -ErrorAction SilentlyContinue | ForEach-Object { "Q:\boot\$($_.Name) $($_.Length)" })
$lines += "Q:\bootmgr.efi exists: $(Test-Path Q:\bootmgr.efi)"

# 2. Remove stale StageBootRepaired entry
Invoke-Bcd "bcdedit /delete {547dddcc-ac5f-11f1-8d4a-cc37f74e6a64} /cleanup /f"

# 3. Create ramdiskoptions object if missing
$hasRd = cmd /c "bcdedit /enum {ramdiskoptions}" 2>&1 | Select-String -Quiet 'ramdiskoptions'
if (-not $hasRd) {
    Invoke-Bcd "bcdedit /create {ramdiskoptions} /d ""Ramdisk options"""
}
Invoke-Bcd "bcdedit /set {ramdiskoptions} ramdisksdidevice partition=Q:"
Invoke-Bcd "bcdedit /set {ramdiskoptions} ramdisksdipath \boot\boot.sdi"

# 4. Create PE osloader entry if missing
$hasPe = cmd /c "bcdedit /enum" 2>&1 | Select-String -Quiet '179ca179-ac73-11f1-8753-f4d0933357ef'
if (-not $hasPe) {
    Invoke-Bcd "bcdedit /create {179ca179-ac73-11f1-8753-f4d0933357ef} /d ""Windows PE Test"" /application osloader"
}
Invoke-Bcd "bcdedit /set {179ca179-ac73-11f1-8753-f4d0933357ef} device ramdisk=[Q:]\sources\boot.wim,{ramdiskoptions}"
Invoke-Bcd "bcdedit /set {179ca179-ac73-11f1-8753-f4d0933357ef} osdevice ramdisk=[Q:]\sources\boot.wim,{ramdiskoptions}"
Invoke-Bcd "bcdedit /set {179ca179-ac73-11f1-8753-f4d0933357ef} path \windows\system32\winload.efi"
Invoke-Bcd "bcdedit /set {179ca179-ac73-11f1-8753-f4d0933357ef} winpe yes"
Invoke-Bcd "bcdedit /set {179ca179-ac73-11f1-8753-f4d0933357ef} detecthal yes"

# 5. displayorder + default + timeout
Invoke-Bcd "bcdedit /displayorder {current} {179ca179-ac73-11f1-8753-f4d0933357ef} /addlast"
Invoke-Bcd "bcdedit /set {bootmgr} default {179ca179-ac73-11f1-8753-f4d0933357ef}"
Invoke-Bcd "bcdedit /timeout 5"

# 6. Final state
$lines += '--- BCD final ---'
$lines += (bcdedit /enum 2>&1 | ForEach-Object { $_ })
Set-Content -LiteralPath $out -Value $lines -Encoding UTF8
Write-Output 'DEPLOY2_DONE'

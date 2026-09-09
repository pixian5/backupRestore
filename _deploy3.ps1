$script:out = @()
$script:out += "=== deploy3 $(Get-Date -Format o) ==="

function Invoke-Bcd([string]$Cmd) {
    $result = cmd /c $Cmd 2>&1
    $code = $LASTEXITCODE
    $script:out += "> $Cmd"
    $script:out += $result
    $script:out += "exit=$code"
}

# ramdiskoptions
$hasRd = cmd /c "bcdedit /enum {ramdiskoptions}" 2>&1 | Select-String -Quiet 'ramdiskoptions'
$script:out += "ramdiskoptions exists: $hasRd"
if (-not $hasRd) {
    Invoke-Bcd "bcdedit /create {ramdiskoptions} /d ""Ramdisk options"""
}
Invoke-Bcd "bcdedit /set {ramdiskoptions} ramdisksdidevice partition=Q:"
Invoke-Bcd "bcdedit /set {ramdiskoptions} ramdisksdipath \boot\boot.sdi"

# PE osloader entry
$hasPe = cmd /c "bcdedit /enum" 2>&1 | Select-String -Pattern '标识符\s+{179ca179' -Quiet
$script:out += "PE entry exists: $hasPe"
if (-not $hasPe) {
    Invoke-Bcd "bcdedit /create {179ca179-ac73-11f1-8753-f4d0933357ef} /d ""Windows PE Test"" /application osloader"
}
Invoke-Bcd "bcdedit /set {179ca179-ac73-11f1-8753-f4d0933357ef} device ramdisk=[Q:]\sources\boot.wim,{ramdiskoptions}"
Invoke-Bcd "bcdedit /set {179ca179-ac73-11f1-8753-f4d0933357ef} osdevice ramdisk=[Q:]\sources\boot.wim,{ramdiskoptions}"
Invoke-Bcd "bcdedit /set {179ca179-ac73-11f1-8753-f4d0933357ef} path \windows\system32\winload.efi"
Invoke-Bcd "bcdedit /set {179ca179-ac73-11f1-8753-f4d0933357ef} systemroot \windows"
Invoke-Bcd "bcdedit /set {179ca179-ac73-11f1-8753-f4d0933357ef} nx OptIn"
Invoke-Bcd "bcdedit /set {179ca179-ac73-11f1-8753-f4d0933357ef} winpe yes"
Invoke-Bcd "bcdedit /set {179ca179-ac73-11f1-8753-f4d0933357ef} detecthal yes"

# displayorder + default + timeout (default = Windows, PE via menu)
Invoke-Bcd "bcdedit /displayorder {179ca179-ac73-11f1-8753-f4d0933357ef} /addlast"
Invoke-Bcd "bcdedit /set {bootmgr} default {current}"
Invoke-Bcd "bcdedit /timeout 5"

$script:out += '--- Q boot.wim hash verify ---'
try {
    $script:out += "Q hash: " + (Get-FileHash 'Q:\sources\boot.wim' -Algorithm SHA256).Hash
    $script:out += "src hash: " + (Get-FileHash 'C:\BackupRestorePE\media\media\sources\boot.wim' -Algorithm SHA256).Hash
} catch {
    $script:out += "hash verify error: $($_.Exception.Message)"
}

$script:out += '--- BCD final ---'
$script:out += (bcdedit /enum 2>&1 | ForEach-Object { $_ })
Set-Content -LiteralPath 'C:\Users\x\Desktop\BackupRestore\_deploy3-log.txt' -Value $script:out -Encoding UTF8
Write-Output 'DEPLOY3_DONE'

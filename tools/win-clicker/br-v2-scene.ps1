# br-v2-scene.ps1 - check readiness of the V2 test scene + WIM-internal ACL to prove
# whether the DISM-log ACL came from the restored image.
$ErrorActionPreference = 'Continue'
function P($n, $v) { Write-Output ("{0,-50}: {1}" -f $n, $v) }
function SEC($n) { Write-Output ("=" * 62); Write-Output "== $n" }

SEC "C: ROOT SENTINEL FILES (BR-*.txt)"
Get-ChildItem "C:\" -Filter "BR-*.txt" -Force -ErrorAction SilentlyContinue |
    ForEach-Object { P ("sentinel " + $_.Name) ($_.Length.ToString() + " bytes " + $_.LastWriteTime) }

SEC "C: VOLUME IDENTITY"
& cmd.exe /c "vol C:" 2>&1 | ForEach-Object { Write-Output ("  V| " + $_) }

SEC "BCD STATE"
& cmd.exe /c "bcdedit /enum {bootmgr}" 2>&1 | ForEach-Object { Write-Output ("  B| " + $_) }
Write-Output "  -- default target --"
& cmd.exe /c "bcdedit /enum {bootmgr} /v" 2>&1 | Select-String "default|displayorder" | ForEach-Object { Write-Output ("  D| " + $_) }

SEC "TEST HOOK CONFIG"
if (Test-Path -LiteralPath "C:\br-test.json") { P "br-test.json" (Get-Content "C:\br-test.json" -Raw) }
else { P "br-test.json" "MISSING" }

SEC "DISM DIR CREATION TIME (was it created by the V1 restore?)"
foreach ($d in @("C:\Windows\Logs", "C:\Windows\Logs\DISM", "C:\Windows\Logs\CBS", "C:\Windows\System32\Recovery")) {
    if (Test-Path -LiteralPath $d) {
        $i = Get-Item -LiteralPath $d -Force
        P $d ("created=" + $i.CreationTime.ToString("yyyy-MM-dd HH:mm:ss"))
    } else { P $d "MISSING" }
}
$dl = "C:\Windows\Logs\DISM\dism.log"
if (Test-Path -LiteralPath $dl) {
    $i = Get-Item -LiteralPath $dl -Force
    P "dism.log" ("created=" + $i.CreationTime.ToString("yyyy-MM-dd HH:mm:ss") + " mtime=" + $i.LastWriteTime.ToString("yyyy-MM-dd HH:mm:ss"))
}

SEC "WIM-INTERNAL ACL: Windows\\Logs\\DISM (mounted at H:\brwork\wimmnt)"
$mnt = "H:\brwork\wimmnt\Windows\Logs\DISM"
if (Test-Path -LiteralPath $mnt) {
    & cmd.exe /c ("icacls `"$mnt`" 2>&1") | ForEach-Object { Write-Output ("  W| " + $_) }
    Get-ChildItem -LiteralPath $mnt -Force -ErrorAction SilentlyContinue |
        ForEach-Object { P ("  wimfile " + $_.Name) ($_.Length + " bytes") }
} else { P $mnt "MISSING (mount not present?)" }

SEC "WIM MOUNT STATUS"
& cmd.exe /c "dism /Get-MountedImageInfo" 2>&1 | ForEach-Object { Write-Output ("  M| " + $_) }

SEC "GUI PROCESS RUNNING?"
$g = Get-Process BackupRestore -ErrorAction SilentlyContinue
if ($g) { $g | ForEach-Object { P ("pid " + $_.Id) ($_.Path + " session=" + $_.SessionId + " start=" + $_.StartTime.ToString("HH:mm:ss")) } }
else { P "BackupRestore" "NOT RUNNING" }

Write-Output "V2SCENE_DONE"
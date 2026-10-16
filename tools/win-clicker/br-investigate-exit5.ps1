# br-investigate-exit5.ps1 - pinpoint WHY dism /Apply-Image to a volume that already
# holds a full Windows refuses with Error 5. Two-phase experiment on the empty test
# volume F: (safe: F: is a blank NTFS test volume).
#
# Phase 1: apply c-full-20261014.wim index 2 to F:\            -> expect SUCCESS
# Phase 2: apply the SAME image to F:\ again (now non-empty)   -> expect Error 5
# Then dump the DISM log lines around the failure.
$ErrorActionPreference = 'Continue'
function P($n, $v) { Write-Output ("{0,-40}: {1}" -f $n, $v) }
function SEC($n) { Write-Output ("=" * 64); Write-Output "== $n" }

$wim = "H:\BRImages\c-full-20261014.wim"
$idx = 2
$dismLog = "C:\Windows\Logs\DISM\dism.log"

SEC "0 ENV"
P "whoami" (& whoami.exe)
P "F: free" ((Get-CimInstance Win32_LogicalDisk -Filter "DeviceID='F:'").FreeSpace)
P "wim" $wim
P "wim exists" (Test-Path -LiteralPath $wim)

SEC "PHASE 1: apply to EMPTY F: (expect success)"
$logBefore = (Get-Item -LiteralPath $dismLog).Length
$t0 = Get-Date
$out1 = & dism.exe /English /Apply-Image /ImageFile:$wim /Index:$idx /ApplyDir:F:\ 2>&1
$rc1 = $LASTEXITCODE
$dt1 = ((Get-Date) - $t0).TotalSeconds
P "phase1 exit" $rc1
P "phase1 secs" ([math]::Round($dt1,1))
$out1 | Select-Object -Last 8 | ForEach-Object { Write-Output ("  1| " + $_) }
# snapshot: what did F: get?
P "F:\Windows created" (Test-Path -LiteralPath "F:\Windows\System32\config\SYSTEM")

SEC "PHASE 2: apply again to NON-EMPTY F: (expect Error 5)"
$t0 = Get-Date
$out2 = & dism.exe /English /Apply-Image /ImageFile:$wim /Index:$idx /ApplyDir:F:\ 2>&1
$rc2 = $LASTEXITCODE
$dt2 = ((Get-Date) - $t0).TotalSeconds
P "phase2 exit" $rc2
P "phase2 secs" ([math]::Round($dt2,1))
$out2 | ForEach-Object { Write-Output ("  2| " + $_) }

SEC "DISM LOG: lines added by phase2"
$lines = Get-Content -LiteralPath $dismLog
Write-Output ("total lines now = " + $lines.Count)
# find the last two ApplyImage sections
$idxAll = @()
for ($i = 0; $i -lt $lines.Count; $i++) {
    if ($lines[$i] -match "ApplyImage|Apply-Image|CDismImageApply|Error|0x5|ACCESS_DENIED|Acl|denied|WIMApply|CreateDirectory|FileExists") { $idxAll += $i }
}
$start = [math]::Max(0, $lines.Count - 4000)
Write-Output "---- tail 250 (raw) ----"
$lines | Select-Object -Last 250 | ForEach-Object { Write-Output ("  M| " + $_) }

Write-Output "INVESTIGATE_DONE"
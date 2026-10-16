# br-acl-test.ps1 - DECISIVE TEST: is the elevated reagentc /info exit 9 caused by
# the ACL of C:\Windows\Logs\DISM (which denies Administrators write)?
#
# Runs in the elevated admin context. Sequence:
#   0) report dism.log state
#   1) run /info as-is                                       -> expect 9
#   2) grant Administrators full control on DISM dir+log, run /info -> observe
#   3) rename dism.log out of the way, run /info             -> observe
#   4) restore original state

$ErrorActionPreference = 'Continue'
function P($n, $v) { Write-Output ("{0,-46}: {1}" -f $n, $v) }
function SEC($n) { Write-Output ("=" * 62); Write-Output "== $n" }
$dl = "C:\Windows\Logs\DISM\dism.log"
$dd = "C:\Windows\Logs\DISM"

SEC "0 BASELINE"
P "whoami" (& whoami.exe)
P "whoami /groups integrity" (((& whoami.exe /groups | Select-String "Mandatory Level") -join "").Trim())
if (Test-Path -LiteralPath $dl) {
    $i = Get-Item -LiteralPath $dl
    P "dism.log" ("len=" + $i.Length + " mtime=" + $i.LastWriteTime.ToString("HH:mm:ss"))
}
try { $fs = [IO.File]::OpenWrite($dl); $fs.Close(); P "openWrite dism.log" "OK" }
catch { P "openWrite dism.log" ("DENIED: " + $_.Exception.Message) }

SEC "1 /info AS-IS"
& cmd.exe /c "reagentc /info 2>&1" | ForEach-Object { Write-Output ("  A| " + $_) }
P "exit" $LASTEXITCODE

SEC "2 GRANT Administrators F ON DISM DIR (recursive) THEN /info"
$r = & cmd.exe /c ("icacls " + $dd + " /grant *S-1-5-32-544:(OI)(CI)F /T 2>&1")
$r | ForEach-Object { Write-Output ("  G| " + $_) }
try { $fs = [IO.File]::OpenWrite($dl); $fs.Close(); P "openWrite after grant" "OK" }
catch { P "openWrite after grant" ("DENIED: " + $_.Exception.Message) }
& cmd.exe /c "reagentc /info 2>&1" | ForEach-Object { Write-Output ("  B| " + $_) }
P "exit" $LASTEXITCODE

SEC "3 RENAME dism.log THEN /info"
$bak = $dl + ".aclprobe"
if (Test-Path -LiteralPath $dl) {
    Move-Item -LiteralPath $dl -Destination $bak -Force -ErrorAction SilentlyContinue
    P "renamed" (Test-Path -LiteralPath $bak)
} else { P "dism.log" "already gone" }
& cmd.exe /c "reagentc /info 2>&1" | ForEach-Object { Write-Output ("  C| " + $_) }
P "exit" $LASTEXITCODE
if (Test-Path -LiteralPath $bak) { P "note" "dism.log.bak left in place (DISM will recreate a fresh one)" }

Write-Output "ACLTEST_DONE"
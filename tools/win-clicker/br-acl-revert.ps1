# br-acl-revert.ps1 - CONFIRM the root cause by reverting the ACL grant and re-testing.
# Run in the ELEVATED admin context. Expected: revert -> exit 9 again; re-grant -> exit 0.

$ErrorActionPreference = 'Continue'
function P($n, $v) { Write-Output ("{0,-46}: {1}" -f $n, $v) }
function SEC($n) { Write-Output ("=" * 62); Write-Output "== $n" }
$dd = "C:\Windows\Logs\DISM"
$dl = "$dd\dism.log"

SEC "1 ACL NOW"
& cmd.exe /c ("icacls " + $dd + " 2>&1") | ForEach-Object { Write-Output ("  1d| " + $_) }
& cmd.exe /c ("icacls " + $dl + " 2>&1") | ForEach-Object { Write-Output ("  1f| " + $_) }

SEC "2 REMOVE THE EXPLICIT Administrators GRANT (recursive)"
& cmd.exe /c ("icacls " + $dd + " /remove:g *S-1-5-32-544 /T 2>&1") | ForEach-Object { Write-Output ("  2| " + $_) }
& cmd.exe /c ("icacls " + $dd + " 2>&1") | ForEach-Object { Write-Output ("  2d| " + $_) }
& cmd.exe /c ("icacls " + $dl + " 2>&1") | ForEach-Object { Write-Output ("  2f| " + $_) }
try { $fs = [IO.File]::OpenWrite($dl); $fs.Close(); P "openWrite now" "OK" }
catch { P "openWrite now" ("DENIED: " + $_.Exception.Message) }

SEC "3 /info AFTER REVERT"
& cmd.exe /c "reagentc /info 2>&1" | ForEach-Object { Write-Output ("  R| " + $_) }
P "exit" $LASTEXITCODE

SEC "4 RE-GRANT Administrators (OI)(CI)F RECURSIVE"
& cmd.exe /c ("icacls " + $dd + " /grant *S-1-5-32-544:(OI)(CI)F /T 2>&1") | ForEach-Object { Write-Output ("  4| " + $_) }
& cmd.exe /c "reagentc /info 2>&1" | Select-Object -First 4 | ForEach-Object { Write-Output ("  G| " + $_) }
P "exit" $LASTEXITCODE

SEC "5 CAN ELEVATED ADMIN CREATE A CHILD UNDER C:\Windows\Logs ?"
$probe = "C:\Windows\Logs\br-probe-dir-" + $PID
try { New-Item -ItemType Directory -Path $probe -ErrorAction Stop | Out-Null; P "mkdir C:\Windows\Logs\<new>" "OK"; Remove-Item $probe -Force -Recurse }
catch { P "mkdir C:\Windows\Logs\<new>" ("DENIED: " + $_.Exception.Message) }

SEC "6 CAN ELEVATED ADMIN CREATE A CHILD UNDER C:\Windows\Logs\DISM ?"
$probe2 = "$dd\br-probe-dir-" + $PID
try { New-Item -ItemType Directory -Path $probe2 -ErrorAction Stop | Out-Null; P "mkdir C:\Windows\Logs\DISM\<new>" "OK"; Remove-Item $probe2 -Force -Recurse }
catch { P "mkdir C:\Windows\Logs\DISM\<new>" ("DENIED: " + $_.Exception.Message) }

Write-Output "ACLREVERT_DONE"
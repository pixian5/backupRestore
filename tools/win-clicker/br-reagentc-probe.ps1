# br-reagentc-probe.ps1 - run reagentc /info in the elevated user context and
# report the exit code, to reproduce the prepare-chain failure (exit 9).
$out = & reagentc.exe /info 2>&1
$rc = $LASTEXITCODE
Write-Output ("ELEVATED_RC=" + $rc)
Write-Output "---OUTPUT---"
Write-Output $out

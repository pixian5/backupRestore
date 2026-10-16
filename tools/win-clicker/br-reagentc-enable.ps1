# br-reagentc-enable.ps1 - run reagentc /enable in the elevated user context.
$diag = "C:\Users\Public\pkg\reagent-enable.log"
$out = & reagentc.exe /enable /logpath $diag 2>&1
$rc = $LASTEXITCODE
Write-Output ("ENABLE_RC=" + $rc)
Write-Output $out

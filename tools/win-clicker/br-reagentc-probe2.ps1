# br-reagentc-probe2.ps1 - user-context reagentc /info with a diagnostic log.
$diag = "C:\Users\Public\pkg\reagent-user.log"
$out = & reagentc.exe /info /logpath $diag 2>&1
$rc = $LASTEXITCODE
Write-Output ("USERCTX_RC=" + $rc)
Write-Output "---STDOUT/ERR---"
Write-Output $out

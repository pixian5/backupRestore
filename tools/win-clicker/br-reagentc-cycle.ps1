# br-reagentc-cycle.ps1 - full disable/enable cycle in the elevated user context.
$diag = "C:\Users\Public\pkg\reagent-cycle.log"
$d = & reagentc.exe /disable /logpath $diag 2>&1
$rcd = $LASTEXITCODE
Write-Output ("DISABLE_RC=" + $rcd)
$e = & reagentc.exe /enable /logpath $diag 2>&1
$rce = $LASTEXITCODE
Write-Output ("ENABLE_RC=" + $rce)
$i = & reagentc.exe /info /logpath $diag 2>&1
$rci = $LASTEXITCODE
Write-Output ("INFO_RC=" + $rci)

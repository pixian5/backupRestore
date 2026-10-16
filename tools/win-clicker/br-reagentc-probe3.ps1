# br-reagentc-probe3.ps1 - user-context /info with explicit working directory.
param([string]$Cwd = "C:\Windows\System32")
Set-Location -LiteralPath $Cwd
$diag = "C:\Users\Public\pkg\reagent-user3.log"
$out = & reagentc.exe /info /logpath $diag 2>&1
$rc = $LASTEXITCODE
Write-Output ("CWD=" + (Get-Location).Path)
Write-Output ("USERCTX_RC=" + $rc)

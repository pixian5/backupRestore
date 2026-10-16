# br-dism-probe.ps1 - run DISM Get-WimInfo on the registered Winre.wim in the
# current (elevated admin) context and report the exit code.
$wim = "C:\Recovery\WindowsRE\Winre.wim"
$out = & dism.exe /English /Get-WimInfo /WimFile:$wim 2>&1
$rc = $LASTEXITCODE
Write-Output ("ADMINCTX_DISM_RC=" + $rc)
Write-Output $out

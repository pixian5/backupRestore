# br-dism-online.ps1 - run an online DISM command in the elevated user context.
$out = & dism.exe /English /Online /Get-CurrentEdition 2>&1
$rc = $LASTEXITCODE
Write-Output ("ADMIN_ONLINE_DISM_RC=" + $rc)
Write-Output $out

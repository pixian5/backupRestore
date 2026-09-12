# Update BackupRestore.lnk target to versionless dir, remove BR-new.exe test artifact
$log = 'C:\Users\Public\backupRestore-package\fix-deploy.log'
$res = New-Object System.Collections.ArrayList

# 1) fix shortcut target
$lnk = $null
foreach ($d in 'C:\Users\x\Desktop','C:\Users\Public\Desktop') {
    $p = Join-Path $d 'BackupRestore.lnk'
    if (Test-Path $p) { $lnk = $p; break }
}
if ($lnk) {
    $sh = New-Object -ComObject WScript.Shell
    $sc = $sh.CreateShortcut($lnk)
    $old = $sc.TargetPath
    $new = 'C:\Users\Public\backupRestore-package\BackupRestore.exe'
    $sc.TargetPath = $new
    $sc.WorkingDirectory = 'C:\Users\Public\backupRestore-package'
    $sc.Save()
    [void]$res.Add("shortcut $lnk : $old -> $new")
} else {
    [void]$res.Add('NO_SHORTCUT')
}

# 2) remove test artifact exe
foreach ($f in @('BR-new.exe')) {
    $p = "C:\Users\Public\backupRestore-package\$f"
    if (Test-Path $p) { Remove-Item $p -Force; [void]$res.Add("removed $f") }
}

$res | Set-Content -Path $log -Encoding ascii
Write-Output 'DONE'

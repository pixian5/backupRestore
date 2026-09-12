# Dump BackupRestore desktop shortcuts (ASCII out)
$out = 'C:\Users\Public\backupRestore-package\shortcuts.log'
$res = New-Object System.Collections.ArrayList
$sh = New-Object -ComObject WScript.Shell
Get-ChildItem 'C:\Users\x\Desktop','C:\Users\Public\Desktop' -Filter *.lnk -ErrorAction SilentlyContinue | Where-Object { $_.Name -like '*BackupRestore*' } | ForEach-Object {
    $t = $sh.CreateShortcut($_.FullName).TargetPath
    [void]$res.Add("$($_.FullName)`t->`t$t")
}
if ($res.Count -eq 0) { [void]$res.Add('NO_BACKUPRESTORE_SHORTCUT_FOUND') }
$res | Set-Content -Path $out -Encoding ascii
Write-Output 'DONE'

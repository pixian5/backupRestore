# Inspect program location and E:/F:/H: contents (ASCII-safe)
$out = "C:\Users\Public\backupRestore-package\drives.log"
$lines = New-Object System.Collections.ArrayList

[void]$lines.Add("== PROGRAM ==")
Get-ChildItem -Path "C:\Users\Public" -Recurse -Filter "BackupRestore.exe" -ErrorAction SilentlyContinue | ForEach-Object {
    [void]$lines.Add("exe: $($_.FullName)  size=$($_.Length)  time=$($_.LastWriteTime.ToString('yyyy-MM-dd HH:mm'))")
}

[void]$lines.Add("== E: (BREFI) ==")
Get-ChildItem -Path "E:\" -Recurse -ErrorAction SilentlyContinue | Select-Object -First 40 | ForEach-Object {
    [void]$lines.Add("E: $($_.FullName)  dir=$($_.PSIsContainer)")
}

[void]$lines.Add("== F: (BRSource) ==")
Get-ChildItem -Path "F:\" -ErrorAction SilentlyContinue | Select-Object -First 40 | ForEach-Object {
    [void]$lines.Add("F: $($_.FullName)  dir=$($_.PSIsContainer)  size=$($_.Length)")
}

[void]$lines.Add("== H: (BRImages) ==")
Get-ChildItem -Path "H:\" -ErrorAction SilentlyContinue | Select-Object -First 40 | ForEach-Object {
    [void]$lines.Add("H: $($_.FullName)  dir=$($_.PSIsContainer)  size=$($_.Length)")
}

[void]$lines.Add("== G: ==")
Get-ChildItem -Path "G:\" -ErrorAction SilentlyContinue | Select-Object -First 20 | ForEach-Object {
    [void]$lines.Add("G: $($_.FullName)  dir=$($_.PSIsContainer)  size=$($_.Length)")
}

$lines | Set-Content -Path $out -Encoding ascii
Write-Output "WROTE"

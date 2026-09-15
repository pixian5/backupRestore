# archive-old-tasks.ps1 - 把 tasks\ 下除 keep 外的所有任务目录移到 _archive
$keep   = '04bf4a16-aff0-4055-ba23-612e05bf4f9f'
$root   = 'C:\Users\Public\backupRestore-package\tasks'
$arch   = 'C:\Users\Public\backupRestore-package\_archive'
if (-not (Test-Path $arch)) { New-Item -ItemType Directory -Path $arch -Force | Out-Null }
$moved = 0
$fail  = 0
foreach ($d in Get-ChildItem $root -Directory -ErrorAction SilentlyContinue) {
    if ($d.Name -eq $keep) { continue }
    try {
        Move-Item -LiteralPath $d.FullName -Destination $arch -Force -ErrorAction Stop
        $moved++
    } catch {
        $fail++
        Write-Output ("FAIL " + $d.Name + " :: " + $_.Exception.Message)
    }
}
Write-Output ("MOVED=" + $moved + " FAIL=" + $fail)
Write-Output "--- remaining in tasks ---"
Get-ChildItem $root -Directory | Select-Object -ExpandProperty Name
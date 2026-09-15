# tasks-status.ps1 - 列出所有任务目录的 stage 状态
$root = 'C:\Users\Public\backupRestore-package\tasks'
$dirs = Get-ChildItem $root -Directory -ErrorAction SilentlyContinue
Write-Output ("TOTAL=" + @($dirs).Count)
foreach ($d in $dirs) {
    $s = Join-Path $d.FullName 'status.json'
    $stage = '?'
    $op = '?'
    if (Test-Path $s) {
        $o = Get-Content $s -Raw -ErrorAction SilentlyContinue | ConvertFrom-Json -ErrorAction SilentlyContinue
        if ($o) { $stage = $o.stage; $op = $o.operation }
    }
    Write-Output ("{0} | {1} | {2} | {3}" -f $d.Name, $d.LastWriteTime.ToString('MM-dd HH:mm'), $op, $stage)
}
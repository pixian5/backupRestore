# task-latest.ps1 - 列出最近任务并输出其状态/日志
$root = 'C:\Users\Public\backupRestore-package\tasks'
$latest = @((Get-ChildItem $root -Directory -ErrorAction SilentlyContinue | Sort-Object LastWriteTime -Descending | Select-Object -First 2))
foreach ($t in $latest) {
    Write-Output ("== TASK {0}  {1} ==" -f $t.Name, $t.LastWriteTime.ToString('MM-dd HH:mm:ss'))
    Get-ChildItem $t.FullName | Select-Object -ExpandProperty Name | ForEach-Object { Write-Output ("   " + $_) }
    foreach ($f in @('status.json','task.json','prepare.log','Recovery.log')) {
        $p = Join-Path $t.FullName $f
        if (Test-Path $p) {
            Write-Output ("   --- {0} ---" -f $f)
            Get-Content $p -Tail 15 | ForEach-Object { Write-Output ("     " + $_) }
        }
    }
}
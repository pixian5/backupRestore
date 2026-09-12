# 清理 C:\Users\Public 下 BackupRestore 开发历史残留（保留 v12 当前部署）
# 删除：backupRestore-package、backupRestore-package-v2..v11、backupRestore-src、backupRestore-src-v2..v11
$log = 'C:\Users\Public\backupRestore-package-v12\cleanup.log'
$removed = New-Object System.Collections.ArrayList
$targets = @(
    'C:\Users\Public\backupRestore-package',
    'C:\Users\Public\backupRestore-package-v2',
    'C:\Users\Public\backupRestore-package-v3',
    'C:\Users\Public\backupRestore-package-v4',
    'C:\Users\Public\backupRestore-package-v5',
    'C:\Users\Public\backupRestore-package-v6',
    'C:\Users\Public\backupRestore-package-v7',
    'C:\Users\Public\backupRestore-package-v8',
    'C:\Users\Public\backupRestore-package-v9',
    'C:\Users\Public\backupRestore-package-v10',
    'C:\Users\Public\backupRestore-package-v11',
    'C:\Users\Public\backupRestore-src',
    'C:\Users\Public\backupRestore-src-v2',
    'C:\Users\Public\backupRestore-src-v3',
    'C:\Users\Public\backupRestore-src-v4',
    'C:\Users\Public\backupRestore-src-v5',
    'C:\Users\Public\backupRestore-src-v6',
    'C:\Users\Public\backupRestore-src-v7',
    'C:\Users\Public\backupRestore-src-v8',
    'C:\Users\Public\backupRestore-src-v9',
    'C:\Users\Public\backupRestore-src-v10',
    'C:\Users\Public\backupRestore-src-v11'
)
foreach ($t in $targets) {
    if (Test-Path $t) {
        Remove-Item -Path $t -Recurse -Force -ErrorAction SilentlyContinue
        if (Test-Path $t) { [void]$removed.Add("FAILED: $t") } else { [void]$removed.Add("removed: $t") }
    }
}
$removed | Set-Content -Path $log -Encoding ascii
Write-Output "DONE"

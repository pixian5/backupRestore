# br-apply-t-2677.ps1 -- 把 2.6.77 PE 镜像应用回 T:（恢复被我格式化空的 PE 分区）
# 背景：还原测试把 bootsequence 指向 T:，但当时 T: 刚被格式化成空 → PE 起不来 → VM 停机。
# 这里用 DISM 把镜像重新铺回去，让 BCD 里的 PE 启动项恢复可用。
$ErrorActionPreference = 'Continue'
dism.exe /English /Apply-Image /ImageFile:'C:\BackupRestorePE\BackupRestorePE.wim' /Index:1 /ApplyDir:'T:\'
"APPLY_EXIT=$LASTEXITCODE"
if (Test-Path -LiteralPath 'T:\Windows\System32\Recovery.exe') {
    "T_RECOVERY_SHA256=" + (Get-FileHash -LiteralPath 'T:\Windows\System32\Recovery.exe' -Algorithm SHA256).Hash.ToLowerInvariant()
    "T_RECOVERY_BYTES=" + (Get-Item -LiteralPath 'T:\Windows\System32\Recovery.exe').Length
} else {
    "T_RECOVERY_MISSING"
}
if (Test-Path -LiteralPath 'T:\Windows\System32\winpeshl.ini') {
    "T_WINPESHL_BYTES=" + (Get-Item -LiteralPath 'T:\Windows\System32\winpeshl.ini').Length
} else {
    "T_WINPESHL_MISSING"
}
"APPLY_DONE_2677"

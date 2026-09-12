# 重建 BackupRestore 测试盘分区（磁盘 1 = full-e2e / 磁盘 2 = blank-compare）
# 前提：Parallels 里已新建并附加对应虚拟磁盘（见 tools/create-test-disks.md）
# 用法（VM 内）：
#   powershell -ExecutionPolicy Bypass -File create-test-disks.ps1
# 磁盘 1（backuprestore-full-e2e SSD，24GB）：MSR + E:BREFI(FAT32 ESP) + F:BRSource + G: + H:BRImages
# 磁盘 2（backuprestore-blank-compare，20GB）：MSR + P: + Q:
$ErrorActionPreference = 'Stop'
$log = 'C:\Users\Public\backupRestore-package-v12\create-test-disks.log'
function Log($m) { Add-Content -Path $log -Value $m -Encoding ascii }

$EFI_GUID = '{c12a7328-f81f-11d2-ba4b-00a0c93ec93b}'
$MSR_GUID = '{e3c9e316-0b5c-4db8-817d-f92df00215ae}'
$DATA_GUID = '{ebd0a0a2-b9e5-4433-87c0-68b6b72699c7}'

function Init-TestDisk($namePattern, $layout) {
    $d = Get-Disk -ErrorAction SilentlyContinue | Where-Object { $_.FriendlyName -like $namePattern } | Select-Object -First 1
    if (-not $d) { Log "NOT FOUND: $namePattern"; return }
    Log ("== init disk $($d.Number) ($($d.FriendlyName))")
    $d | Clear-Disk -RemoveData -RemoveOEM -Confirm:$false
    $d | Initialize-Disk -PartitionStyle GPT
    foreach ($p in $layout) {
        $params = @{ DiskNumber = $d.Number; AssignDriveLetter = $true }
        if ($p.size) { $params.Size = $p.size }
        else { $params.UseMaximumSize = $true }
        if ($p.gpt) { $params.GptType = $p.gpt }
        $part = New-Partition @params
        if ($p.fs) {
            Format-Volume -Partition $part -FileSystem $p.fs -NewFileSystemLabel $p.label -Confirm:$false | Out-Null
        }
        Log ("  created: $($p.label) $($p.sizeText)")
    }
}

# 磁盘 1：MSR 16M + E:BREFI(FAT32,300M) + F:BRSource(NTFS,8G) + G:(NTFS,8G) + H:BRImages(NTFS,余量)
Init-TestDisk '*full-e2e*' @(
    @{ gpt = $MSR_GUID; size = 16MB; sizeText = 'MSR 16MB' },
    @{ gpt = $EFI_GUID; size = 300MB; fs = 'FAT32'; label = 'BREFI'; sizeText = 'E: BREFI 300MB' },
    @{ gpt = $DATA_GUID; size = 8GB; fs = 'NTFS'; label = 'BRSource'; sizeText = 'F: BRSource 8GB' },
    @{ gpt = $DATA_GUID; size = 8GB; fs = 'NTFS'; label = ''; sizeText = 'G: 8GB' },
    @{ gpt = $DATA_GUID; fs = 'NTFS'; label = 'BRImages'; sizeText = 'H: BRImages 余量' }
)

# 磁盘 2：MSR 16M + P:8G + Q:8G
Init-TestDisk '*blank-compare*' @(
    @{ gpt = $MSR_GUID; size = 16MB; sizeText = 'MSR 16MB' },
    @{ gpt = $DATA_GUID; size = 8GB; fs = 'NTFS'; label = ''; sizeText = 'P: 8GB' },
    @{ gpt = $DATA_GUID; size = 8GB; fs = 'NTFS'; label = ''; sizeText = 'Q: 8GB' }
)

Log 'DONE'

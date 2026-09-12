# Dump partition & volume layout (ASCII-safe output to file)
$out = "C:\Users\Public\backupRestore-package-v12\partitions.log"
$lines = New-Object System.Collections.ArrayList
[void]$lines.Add("== PARTITIONS ==")
Get-Partition | Sort-Object DiskNumber, PartitionNumber | ForEach-Object {
    $type = if ($_.Type -match "System") { "EFI-SYS" } elseif ($_.Type -match "Reserved") { "MSR" } elseif ($_.Type -match "Recovery") { "RECOVERY" } elseif ($_.Type -match "Primary") { "BASIC" } else { "OTHER" }
    $drv = if ($_.DriveLetter) { $_.DriveLetter + ":" } else { "-" }
    $gb = [math]::Round($_.Size / 1GB, 2)
    [void]$lines.Add("disk=$($_.DiskNumber) part=$($_.PartitionNumber) drv=$drv type=$type sizeGB=$gb")
}
[void]$lines.Add("== VOLUMES ==")
Get-Volume | Where-Object { $_.DriveLetter } | ForEach-Object {
    $gb = [math]::Round($_.Size / 1GB, 2)
    $fr = [math]::Round($_.SizeRemaining / 1GB, 2)
    $label = if ($_.FileSystemLabel) { $_.FileSystemLabel } else { "-" }
    [void]$lines.Add("drv=$($_.DriveLetter): label=$label fs=$($_.FileSystem) sizeGB=$gb freeGB=$fr")
}
[void]$lines.Add("== DISKS ==")
Get-Disk | ForEach-Object {
    [void]$lines.Add("disk=$($_.Number) name=$($_.FriendlyName) style=$($_.PartitionStyle) sizeGB=$([math]::Round($_.Size / 1GB, 2))")
}
$lines | Set-Content -Path $out -Encoding ascii
Write-Output "WROTE"

[CmdletBinding()]
param(
    [ValidateSet('probe', 'backup', 'restore', 'restore-existing', 'create-secondary')]
    [string]$Operation = 'probe',
    [ValidatePattern('^[A-Za-z]$')]
    [string]$TaskDrive = 'C',
    [ValidatePattern('^[A-Za-z]$')]
    [string]$SourceDrive = 'C',
    [ValidatePattern('^[A-Za-z]$')]
    [string]$ImageDrive = 'D',
    [ValidatePattern('^[A-Za-z]$')]
    [string]$TargetDrive = 'C',
    [string]$ImageRelativePath = 'BackupRestore\Windows.wim',
    [ValidateRange(1, 2147483647)]
    [int]$WimIndex = 1,
    [string]$BootMenuName = 'Windows Backup',
    [switch]$AllowDestructive,
    [switch]$NoReboot,
    [string]$RecoveryExe = ''
)

$ErrorActionPreference = 'Stop'
$root = 'C:\ProgramData\BackupRestore'
$logRoot = Join-Path $root 'logs'
$scriptRoot = Split-Path -Parent $MyInvocation.MyCommand.Path
$recoveryCmd = Join-Path $scriptRoot 'Recovery.cmd'
$recoveryLauncher = Join-Path $scriptRoot 'RecoveryLauncher.cmd'
$recoveryShell = Join-Path $scriptRoot 'winpeshl.ini'
$script:WindowsArchitecture = ''
$script:WindowsEdition = 'unknown'
$script:WindowsBuild = 'unknown'
$script:SecureBoot = 'unknown'
$reservedPartitionTypes = @(
    '{c12a7328-f81f-11d2-ba4b-00a0c93ec93b}'
    '{e3c9e316-0b5c-4db8-817d-f92df00215ae}'
    '{de94bba4-06d1-4d40-a16a-bfd50179d6ac}'
)

function Write-Log([string]$Message) {
    New-Item -ItemType Directory -Force -Path $logRoot | Out-Null
    "[{0}] {1}" -f (Get-Date -Format o), $Message | Tee-Object -FilePath (Join-Path $logRoot 'prepare.log') -Append
}

function Get-Sha256([string]$Path) {
    $algorithm = [System.Security.Cryptography.SHA256]::Create()
    $stream = [System.IO.File]::OpenRead($Path)
    try {
        return ([System.BitConverter]::ToString($algorithm.ComputeHash($stream))).Replace('-', '')
    } finally {
        $stream.Dispose()
        $algorithm.Dispose()
    }
}

function Assert-RelativeImagePath([string]$Value) {
    if ([string]::IsNullOrWhiteSpace($Value) -or $Value.Contains([char]0)) {
        throw 'ImageRelativePath must be a non-empty relative path.'
    }
    $normalized = $Value.Replace('/', '\')
    if ($normalized.StartsWith('\') -or $normalized -match '^[A-Za-z]:') {
        throw 'ImageRelativePath must not be rooted or contain a drive prefix.'
    }
    foreach ($part in ($normalized -split '\\')) {
        if ([string]::IsNullOrWhiteSpace($part) -or $part -eq '.' -or $part -eq '..') {
            throw 'ImageRelativePath contains an invalid path component.'
        }
    }
}

function Assert-BootMenuName([string]$Value) {
    if ($Value.Length -gt 256 -or $Value.IndexOfAny([char[]]"`r`n`t") -ge 0) {
        throw 'BootMenuName is too long or contains control characters.'
    }
}

function Require-Administrator {
    $identity = [Security.Principal.WindowsIdentity]::GetCurrent()
    $principal = [Security.Principal.WindowsPrincipal]::new($identity)
    if (-not $principal.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)) {
        throw 'Administrator elevation is required.'
    }
}

function Get-NativeWindowsArchitecture {
    $reported = @($env:PROCESSOR_ARCHITEW6432, $env:PROCESSOR_ARCHITECTURE) |
        Where-Object { -not [string]::IsNullOrWhiteSpace($_) } |
        ForEach-Object { $_.ToUpperInvariant() }
    if ($reported -contains 'ARM64') { return 'arm64' }
    if ($reported -contains 'AMD64') { return 'x64' }
    throw "Unsupported Windows architecture (reported $($reported -join ', ')). This package supports x64 and ARM64."
}

function Assert-PackageArchitecture {
    $manifestPath = Join-Path $scriptRoot 'build-manifest.json'
    if (-not (Test-Path $manifestPath)) { return }
    $manifest = Get-Content $manifestPath -Raw | ConvertFrom-Json
    if ($manifest.architecture -and "$($manifest.architecture)" -ne $script:WindowsArchitecture) {
        throw "This package is for $($manifest.architecture), but the current Windows OS is $script:WindowsArchitecture."
    }
}

function Get-VolumeIdentity([string]$Drive) {
    $volume = Get-Volume -DriveLetter $Drive -ErrorAction Stop
    $partition = Get-Partition -DriveLetter $Drive -ErrorAction Stop
    $disk = Get-Disk -Number $partition.DiskNumber -ErrorAction Stop
    if ($disk.PartitionStyle -ne 'GPT') { throw "Selected volume $Drive`:: disk is not GPT." }
    if ($disk.PSObject.Properties.Name -contains 'IsDynamic' -and $disk.IsDynamic) { throw "Selected volume $Drive`:: dynamic disks are unsupported." }
    [pscustomobject]@{
        Drive = "$Drive`:"
        VolumeGuid = $volume.UniqueId
        DiskGuid = $disk.UniqueId
        PartitionGuid = $partition.Guid
        PartitionNumber = $partition.PartitionNumber
        DiskNumber = $partition.DiskNumber
        PartitionOffset = $partition.Offset
        Size = $partition.Size
        FreeBytes = $volume.SizeRemaining
        Filesystem = $volume.FileSystem
        VolumeSerial = if ($volume.PSObject.Properties.Name -contains 'SerialNumber') { "$($volume.SerialNumber)" } else { '' }
        PartitionTypeGuid = "$($partition.GptType)"
        DriveLetter = $Drive
    }
}

function Assert-SystemEnvironment {
    $os = Get-CimInstance Win32_OperatingSystem
    if ($os.Caption -notmatch 'Windows 10|Windows 11') { throw "V1 only supports Windows 10/11: $($os.Caption)" }
    $script:WindowsArchitecture = Get-NativeWindowsArchitecture
    Assert-PackageArchitecture
    $script:WindowsEdition = "$($os.Caption)"
    $script:WindowsBuild = "$($os.BuildNumber)"
    $firmware = (Get-ComputerInfo -Property BiosFirmwareType).BiosFirmwareType
    if ("$firmware" -notmatch 'UEFI') { throw "V1 requires UEFI firmware (reported $firmware)." }
    try {
        $script:SecureBoot = if (Confirm-SecureBootUEFI -ErrorAction Stop) { 'on' } else { 'off' }
    } catch {
        $script:SecureBoot = 'unknown'
    }
    Write-Log "Secure Boot: $script:SecureBoot"
    $systemDisk = Get-Disk | Where-Object IsBoot -eq $true | Select-Object -First 1
    if (-not $systemDisk -or $systemDisk.PartitionStyle -ne 'GPT') { throw 'V1 requires the boot disk to use GPT.' }
    $reagent = reagentc.exe /info 2>&1 | Out-String
    # The status label is localized. A registered WinRE path is the stable
    # signal: disabled WinRE reports no Recovery\WindowsRE location.
    if ($reagent -notmatch '(?i)(GLOBALROOT|Recovery\\WindowsRE)') { throw 'Windows RE is disabled or unavailable. Enable WinRE before starting a task.' }
    $bitlocker = Get-Command Get-BitLockerVolume -ErrorAction SilentlyContinue
    if ($bitlocker) {
        $protected = Get-BitLockerVolume -MountPoint "$($SourceDrive):" -ErrorAction SilentlyContinue
        if ($protected -and "$($protected.ProtectionStatus)" -match 'On') {
            throw 'BitLocker protection is enabled on the source volume. Suspend/unlock it manually; V1 never changes BitLocker state.'
        }
    }
}

function Write-JsonAtomic([string]$Path, $Value) {
    $dir = Split-Path -Parent $Path
    New-Item -ItemType Directory -Force -Path $dir | Out-Null
    $tmp = "$Path.tmp"
    $Value | ConvertTo-Json -Depth 12 | Set-Content -Path $tmp -Encoding UTF8
    $stream = [System.IO.File]::Open($tmp, [System.IO.FileMode]::Open, [System.IO.FileAccess]::ReadWrite, [System.IO.FileShare]::None)
    $stream.Flush($true); $stream.Dispose()
    Move-Item -Force -Path $tmp -Destination $Path
}

function Convert-Identity($Identity) {
    [ordered]@{
        diskGuid = "$($Identity.DiskGuid)"
        partitionGuid = "$($Identity.PartitionGuid)"
        volumeGuid = "$($Identity.VolumeGuid)"
        partitionTypeGuid = "$($Identity.PartitionTypeGuid)"
        diskNumber = [int]$Identity.DiskNumber
        partitionNumber = [int]$Identity.PartitionNumber
        partitionOffset = [UInt64]$Identity.PartitionOffset
        partitionSize = [UInt64]$Identity.Size
        filesystem = "$($Identity.Filesystem)"
        volumeSerial = "$($Identity.VolumeSerial)"
        driveLetter = "$($Identity.DriveLetter)"
    }
}

function Get-RecoveryIdentity {
    $reagent = reagentc.exe /info 2>&1 | Out-String
    $partition = $null
    if ($reagent -match '(?i)harddisk(?<disk>\d+)\\partition(?<partition>\d+)') {
        $partition = Get-Partition -DiskNumber ([int]$matches.disk) -PartitionNumber ([int]$matches.partition) -ErrorAction SilentlyContinue
        if ($partition -and ($partition.GptType -ne '{de94bba4-06d1-4d40-a16a-bfd50179d6ac}' -or $partition.Size -le 500MB)) {
            $partition = $null
        }
    }
    if (-not $partition) {
        $partition = Get-Partition |
            Where-Object { $_.GptType -eq '{de94bba4-06d1-4d40-a16a-bfd50179d6ac}' -and $_.Size -gt 500MB } |
            Sort-Object Size -Descending |
            Select-Object -First 1
    }
    if (-not $partition) { throw 'The WinRE recovery partition was not found.' }
    $volume = $partition | Get-Volume
    [pscustomobject]@{ Partition = $partition; VolumeGuid = $volume.UniqueId }
}

function Get-EfiIdentity {
    $bootDisk = Get-Disk | Where-Object IsBoot -eq $true | Select-Object -First 1
    $partition = Get-Partition |
        Where-Object {
            $_.GptType -eq '{c12a7328-f81f-11d2-ba4b-00a0c93ec93b}' -and
            (-not $bootDisk -or $_.DiskNumber -eq $bootDisk.Number)
        } |
        Select-Object -First 1
    if (-not $partition) { throw 'The EFI system partition was not found.' }
    $volume = $partition | Get-Volume
    [pscustomobject]@{ Partition = $partition; VolumeGuid = $volume.UniqueId }
}

function Invoke-Native([string]$File, [string[]]$Arguments, [string]$LogFile) {
    Add-Content -LiteralPath $LogFile -Value "[native] starting $File $($Arguments -join ' ')" -Encoding UTF8
    $nativeArguments = @($Arguments)
    $nativeLogFile = $LogFile
    if ([System.IO.Path]::GetFileName($File) -ieq 'dism.exe' -and
        -not ($nativeArguments | Where-Object { $_ -match '(?i)^/LogPath:' })) {
        # DISM keeps its log handle open briefly after the image session closes.
        # Keep that handle separate from the task log we append below.
        $nativeLogFile = "$LogFile.dism.log"
        $nativeArguments += "/LogPath:$nativeLogFile"
    }
    $quotedArguments = foreach ($argument in $nativeArguments) {
        $value = [string]$argument
        if ($value -match '[\s"]') {
            $escaped = $value.Replace('"', '\"')
            if ($escaped.EndsWith('\')) { $escaped += '\' }
            '"' + $escaped + '"'
        } else {
            $value
        }
    }
    $startInfo = New-Object System.Diagnostics.ProcessStartInfo
    $startInfo.FileName = $File
    $startInfo.Arguments = $quotedArguments -join ' '
    $startInfo.UseShellExecute = $false
    $startInfo.CreateNoWindow = $true
    $process = New-Object System.Diagnostics.Process
    $process.StartInfo = $startInfo
    if (-not $process.Start()) { throw "Unable to start $File" }
    $process.WaitForExit()
    $exitCode = $process.ExitCode
    $process.Dispose()
    Add-Content -LiteralPath $LogFile -Value "[native] exited $File code=$exitCode" -Encoding UTF8
    if ($exitCode -ne 0) { throw "$File failed with exit code $exitCode" }
}

Require-Administrator
Assert-SystemEnvironment
Assert-RelativeImagePath $ImageRelativePath
Assert-BootMenuName $BootMenuName
if (-not (Test-Path $recoveryCmd) -or -not (Test-Path $recoveryLauncher) -or -not (Test-Path $recoveryShell)) { throw 'Recovery payload files are missing.' }
Write-Log "Preparing $Operation task"

$effectiveOperation = switch ($Operation) {
    'restore' { 'restore-existing' }
    default { $Operation }
}
$versionPath = Join-Path $scriptRoot 'VERSION'
if (-not (Test-Path $versionPath)) { $versionPath = Join-Path $scriptRoot '..\VERSION' }
$programVersion = (Get-Content $versionPath -ErrorAction SilentlyContinue | Select-Object -First 1).Trim()
if ([string]::IsNullOrWhiteSpace($programVersion)) { $programVersion = '0.0.0' }

$task = Get-VolumeIdentity $TaskDrive
$source = Get-VolumeIdentity $SourceDrive
$image = if ($Operation -eq 'probe') { $task } else { Get-VolumeIdentity $ImageDrive }
$target = Get-VolumeIdentity $TargetDrive
$recovery = Get-RecoveryIdentity
$efi = Get-EfiIdentity
if ($task.PartitionTypeGuid.ToLowerInvariant() -in $reservedPartitionTypes) {
    throw 'The task volume cannot be an EFI, MSR or Recovery partition.'
}
if ($image.PartitionTypeGuid.ToLowerInvariant() -in $reservedPartitionTypes) {
    throw 'The image volume cannot be an EFI, MSR or Recovery partition.'
}
if ($task.VolumeGuid -in @($recovery.VolumeGuid, $efi.VolumeGuid)) {
    throw 'The task volume cannot be the registered Recovery or EFI volume.'
}
if ($image.VolumeGuid -in @($recovery.VolumeGuid, $efi.VolumeGuid)) {
    throw 'The image volume cannot be the registered Recovery or EFI volume.'
}
$imagePath = Join-Path $image.Drive $ImageRelativePath
$minimumTargetSize = [UInt64]0
$sourceUsedBytes = [UInt64]($source.Size - $source.FreeBytes)
$reservedBytes = [UInt64](2GB)
if ($effectiveOperation -eq 'backup') { $minimumTargetSize = [UInt64]$source.Size }
if (-not (Test-Path (Join-Path $source.Drive 'Windows\System32\config\SYSTEM'))) {
    throw "Source volume $($source.Drive) does not contain an offline Windows SYSTEM hive."
}

if (Get-Command Get-BitLockerVolume -ErrorAction SilentlyContinue) {
    foreach ($drive in @($SourceDrive, $ImageDrive, $TargetDrive) | Select-Object -Unique) {
        $state = Get-BitLockerVolume -MountPoint "$drive`:" -ErrorAction SilentlyContinue
        if ($state -and "$($state.ProtectionStatus)" -match 'On') {
            throw "BitLocker protection is enabled on $drive`:. V1 will not alter or unlock it."
        }
    }
}

if ($effectiveOperation -in @('backup', 'restore-existing', 'create-secondary') -and $task.VolumeGuid -eq $source.VolumeGuid) {
    throw 'Backup and restore tasks must use a task volume different from the source volume.'
}
if ($effectiveOperation -in @('restore-existing', 'create-secondary')) {
    if (-not $AllowDestructive) { throw 'Restore requires -AllowDestructive.' }
    if ($effectiveOperation -eq 'create-secondary' -and [string]::IsNullOrWhiteSpace($BootMenuName)) { throw 'A boot menu name is required for create-secondary.' }
    if ($effectiveOperation -eq 'restore-existing' -and $target.VolumeGuid -ne $source.VolumeGuid) { throw 'restore-existing must target the currently selected Windows source volume.' }
    if ($image.VolumeGuid -eq $target.VolumeGuid) { throw 'The image volume must differ from the restore target.' }
    if ($task.VolumeGuid -eq $target.VolumeGuid) { throw 'The task volume must differ from the restore target.' }
    if ($target.PartitionTypeGuid -in @('{c12a7328-f81f-11d2-ba4b-00a0c93ec93b}', '{e3c9e316-0b5c-4db8-817d-f92df00215ae}', '{de94bba4-06d1-4d40-a16a-bfd50179d6ac}')) { throw 'EFI, MSR and Recovery partitions cannot be restore targets.' }
    if ($target.Filesystem -ne 'NTFS') { throw 'The restore target must be an NTFS partition.' }
    if (-not (Test-Path $imagePath)) { throw "WIM image does not exist: $imagePath" }
    Invoke-Native 'dism.exe' @('/Get-WimInfo', "/WimFile:$imagePath", "/Index:$WimIndex") (Join-Path $logRoot 'prepare.log')
    $metadataPath = Join-Path (Split-Path -Parent $imagePath) 'metadata.json'
    if (-not (Test-Path $metadataPath)) { throw "Backup metadata is missing: $metadataPath" }
    $metadata = Get-Content $metadataPath -Raw | ConvertFrom-Json
    $actualHash = Get-Sha256 $imagePath
    if ($metadata.imageSha256 -and $actualHash -ne $metadata.imageSha256) { throw 'WIM SHA-256 does not match metadata.' }
    $minimumTargetSize = [UInt64]$metadata.minimumTargetSize
    if ($minimumTargetSize -eq 0) { $minimumTargetSize = [UInt64]$metadata.source.partitionSize }
    if ($target.Size -lt $minimumTargetSize) { throw "Target partition is too small: $($target.Size) < $minimumTargetSize" }
}
if ($effectiveOperation -eq 'backup' -and $image.VolumeGuid -eq $source.VolumeGuid) {
    throw 'The image volume must differ from the captured source volume.'
}
if ($effectiveOperation -eq 'backup') {
    $requiredFree = [UInt64]($sourceUsedBytes + $reservedBytes)
    if ($image.FreeBytes -lt $requiredFree) { throw "Backup destination free space is insufficient: $($image.FreeBytes) < $requiredFree" }
}
if ($effectiveOperation -eq 'create-secondary' -and $target.VolumeGuid -eq $source.VolumeGuid) {
    throw 'The secondary Windows target must differ from the existing Windows source.'
}

if ($RecoveryExe -eq '') {
    $RecoveryExe = Join-Path $scriptRoot 'Recovery.exe'
}
if ($effectiveOperation -ne 'probe' -and -not (Test-Path $RecoveryExe)) {
    throw "Recovery.exe is required for real $effectiveOperation tasks. Build the ARM64 release binary and pass -RecoveryExe."
}

$recoveryMount = 'R:'
$existingRecoveryVolume = Get-Volume -DriveLetter R -ErrorAction SilentlyContinue
if ($existingRecoveryVolume -and $existingRecoveryVolume.UniqueId -ne $recovery.VolumeGuid) {
    throw "Drive R: is already assigned to a different volume; refusing to replace it."
}
if (-not $existingRecoveryVolume) {
    Add-PartitionAccessPath -DiskNumber $recovery.Partition.DiskNumber -PartitionNumber $recovery.Partition.PartitionNumber -AccessPath 'R:\'
}
$registeredWim = 'R:\Recovery\WindowsRE\Winre.wim'
if (-not (Test-Path $registeredWim)) { throw "Registered WinRE image missing: $registeredWim" }

$taskId = [guid]::NewGuid().Guid
$taskRootRelative = "BackupRestore\tasks\$taskId"
$taskRoot = "$($task.Drive)\$taskRootRelative"
$payload = Join-Path $taskRoot 'payload'
$original = Join-Path $taskRoot 'original'
$stage = Join-Path $taskRoot 'stage'
$mount = Join-Path $taskRoot 'mount'
$taskLog = Join-Path $taskRoot 'prepare.log'
New-Item -ItemType Directory -Force -Path $payload, $original, $stage, $mount | Out-Null

$bcdSnapshot = Join-Path $taskRoot 'bcd-before-export'
Invoke-Native 'bcdedit.exe' @('/export', $bcdSnapshot) $taskLog
$bcdHash = Get-Sha256 $bcdSnapshot
Write-Log "BCD snapshot exported: $taskId"

Copy-Item $registeredWim (Join-Path $original 'Winre.wim') -Force
$originalHash = Get-Sha256 (Join-Path $original 'Winre.wim')
Write-Log "Original WinRE copied: $taskId"
Copy-Item $recoveryCmd (Join-Path $payload 'Recovery.cmd') -Force
Copy-Item $recoveryLauncher (Join-Path $payload 'RecoveryLauncher.cmd') -Force
Copy-Item $recoveryShell (Join-Path $payload 'winpeshl.ini') -Force
if ($RecoveryExe -and (Test-Path $RecoveryExe)) { Copy-Item $RecoveryExe (Join-Path $payload 'Recovery.exe') -Force }
Get-ChildItem $scriptRoot -Filter '*.dll' -File -ErrorAction SilentlyContinue |
    ForEach-Object { Copy-Item $_.FullName (Join-Path $payload $_.Name) -Force }
Write-Log "Recovery payload copied: $taskId"

$allow = if ($AllowDestructive) { 'YES' } else { 'NO' }
$created = (Get-Date).ToUniversalTime().ToString('o')
@(
    "TASK_ID=$taskId"
    "OPERATION=$effectiveOperation"
    "TASK_VOLUME_GUID=$($task.VolumeGuid)"
    'TASK_MOUNT='
    "TASK_DISK_NUMBER=$($task.DiskNumber)"
    "TASK_PARTITION_NUMBER=$($task.PartitionNumber)"
    "TASK_DISK_GUID=$($task.DiskGuid)"
    "TASK_PARTITION_GUID=$($task.PartitionGuid)"
    "TASK_PARTITION_OFFSET=$($task.PartitionOffset)"
    "TASK_PARTITION_SIZE=$($task.Size)"
    "TASK_PARTITION_TYPE_GUID=$($task.PartitionTypeGuid)"
    "TASK_FILESYSTEM=$($task.Filesystem)"
    "TASK_VOLUME_SERIAL=$($task.VolumeSerial)"
    "TASK_ROOT_REL=$taskRootRelative"
    "RECOVERY_VOLUME_GUID=$($recovery.VolumeGuid)"
    'RECOVERY_MOUNT='
    "RECOVERY_DISK_NUMBER=$($recovery.Partition.DiskNumber)"
    "RECOVERY_PARTITION_NUMBER=$($recovery.Partition.PartitionNumber)"
    "SOURCE_VOLUME_GUID=$($source.VolumeGuid)"
    "SOURCE_DISK_GUID=$($source.DiskGuid)"
    "SOURCE_PARTITION_GUID=$($source.PartitionGuid)"
    "SOURCE_PARTITION_OFFSET=$($source.PartitionOffset)"
    "SOURCE_PARTITION_SIZE=$($source.Size)"
    "SOURCE_PARTITION_TYPE_GUID=$($source.PartitionTypeGuid)"
    "SOURCE_FILESYSTEM=$($source.Filesystem)"
    "SOURCE_VOLUME_SERIAL=$($source.VolumeSerial)"
    "SOURCE_USED_BYTES=$sourceUsedBytes"
    "RESERVED_BYTES=$reservedBytes"
    'SOURCE_MOUNT='
    "SOURCE_DISK_NUMBER=$($source.DiskNumber)"
    "SOURCE_PARTITION_NUMBER=$($source.PartitionNumber)"
    "IMAGE_VOLUME_GUID=$($image.VolumeGuid)"
    "IMAGE_DISK_GUID=$($image.DiskGuid)"
    "IMAGE_PARTITION_GUID=$($image.PartitionGuid)"
    "IMAGE_PARTITION_OFFSET=$($image.PartitionOffset)"
    "IMAGE_PARTITION_SIZE=$($image.Size)"
    "IMAGE_PARTITION_TYPE_GUID=$($image.PartitionTypeGuid)"
    "IMAGE_FILESYSTEM=$($image.Filesystem)"
    "IMAGE_VOLUME_SERIAL=$($image.VolumeSerial)"
    'IMAGE_MOUNT='
    "IMAGE_DISK_NUMBER=$($image.DiskNumber)"
    "IMAGE_PARTITION_NUMBER=$($image.PartitionNumber)"
    "TARGET_VOLUME_GUID=$($target.VolumeGuid)"
    "TARGET_DISK_GUID=$($target.DiskGuid)"
    "TARGET_PARTITION_GUID=$($target.PartitionGuid)"
    "TARGET_PARTITION_OFFSET=$($target.PartitionOffset)"
    "TARGET_PARTITION_SIZE=$($target.Size)"
    "TARGET_PARTITION_TYPE_GUID=$($target.PartitionTypeGuid)"
    "TARGET_FILESYSTEM=$($target.Filesystem)"
    "TARGET_VOLUME_SERIAL=$($target.VolumeSerial)"
    'TARGET_MOUNT='
    "TARGET_DISK_NUMBER=$($target.DiskNumber)"
    "TARGET_PARTITION_NUMBER=$($target.PartitionNumber)"
    "EFI_VOLUME_GUID=$($efi.VolumeGuid)"
    'EFI_MOUNT='
    "EFI_DISK_NUMBER=$($efi.Partition.DiskNumber)"
    "EFI_PARTITION_NUMBER=$($efi.Partition.PartitionNumber)"
    "IMAGE_RELATIVE_PATH=$ImageRelativePath"
    "IMAGE_SHA256=$(if (Test-Path $imagePath) { Get-Sha256 $imagePath } else { '' })"
    "MINIMUM_TARGET_SIZE=$minimumTargetSize"
    "WIM_INDEX=$WimIndex"
    "WINDOWS_ARCHITECTURE=$script:WindowsArchitecture"
    "WINDOWS_EDITION=$($script:WindowsEdition)"
    "WINDOWS_BUILD=$($script:WindowsBuild)"
    "SECURE_BOOT=$script:SecureBoot"
    "ALLOW_DESTRUCTIVE=$allow"
    "PROGRAM_VERSION=$programVersion"
    "TASK_CREATED=$created"
    "BOOT_MENU_NAME=$($BootMenuName.Trim())"
) | Set-Content (Join-Path $payload 'RecoveryTask.env') -Encoding ascii

$imageRelative = $ImageRelativePath.Replace('/', '\\')
$taskJsonPath = Join-Path $taskRoot 'task.json'
$taskJson = [ordered]@{
    taskId = $taskId
    version = 1
    operation = $effectiveOperation
    source = Convert-Identity $source
    taskVolume = Convert-Identity $task
    image = if ($effectiveOperation -in @('restore-existing', 'create-secondary')) { [ordered]@{
        volume = Convert-Identity $image
        relativePath = $imageRelative
        sha256 = if (Test-Path (Join-Path $image.Drive $imageRelative)) { (Get-Sha256 (Join-Path $image.Drive $imageRelative)).ToLowerInvariant() } else { '0' * 64 }
        sizeBytes = if (Test-Path (Join-Path $image.Drive $imageRelative)) { (Get-Item (Join-Path $image.Drive $imageRelative)).Length } else { 0 }
        index = $WimIndex
    } } else { $null }
    destination = if ($effectiveOperation -eq 'backup') { [ordered]@{ volume = Convert-Identity $image; relativePath = $imageRelative } } else { $null }
    target = if ($effectiveOperation -in @('restore-existing', 'create-secondary')) { [ordered]@{
        volume = Convert-Identity $target
        role = if ($effectiveOperation -eq 'create-secondary') { 'new-windows' } else { 'existing-windows' }
        bootMenuName = if ($effectiveOperation -eq 'create-secondary') { $BootMenuName.Trim() } else { $null }
        minimumSizeBytes = if ($minimumTargetSize -gt 0) { [UInt64]$minimumTargetSize } else { [UInt64]$target.Size }
    } } else { $null }
    bootPlan = [ordered]@{
        mode = if ($effectiveOperation -eq 'create-secondary') { 'add-secondary' } else { 'return-existing' }
        previousBcdSha256 = $bcdHash.ToLowerInvariant()
        menuName = if ($effectiveOperation -eq 'create-secondary') { $BootMenuName.Trim() } else { $null }
        bootSequenceRequested = $true
    }
    created = $created
    bootOnce = $true
    status = 'prepared'
}
Write-JsonAtomic $taskJsonPath $taskJson
Copy-Item $taskJsonPath (Join-Path $payload 'task.json') -Force
$validator = $RecoveryExe
if ([string]::IsNullOrWhiteSpace($validator)) {
    $candidate = Join-Path $scriptRoot 'BackupRestore.exe'
    if (Test-Path $candidate) { $validator = $candidate }
}
if (-not [string]::IsNullOrWhiteSpace($validator) -and (Test-Path $validator)) {
    # Use the same Rust schema validator that Recovery.exe will use. This
    # turns a PowerShell/schema drift into a preparation failure before any
    # WinRE image is modified.
    Invoke-Native $validator @('validate-task', $taskJsonPath) $taskLog
}
$launcherHash = Get-Sha256 (Join-Path $payload 'RecoveryLauncher.cmd')
$recoveryCmdHash = Get-Sha256 (Join-Path $payload 'Recovery.cmd')
$recoveryHash = if (Test-Path (Join-Path $payload 'Recovery.exe')) { Get-Sha256 (Join-Path $payload 'Recovery.exe') } else { $recoveryCmdHash }
$taskHash = Get-Sha256 (Join-Path $payload 'task.json')
@(
    "EXPECTED_LAUNCHER_SHA256=$launcherHash"
    "EXPECTED_RECOVERY_CMD_SHA256=$recoveryCmdHash"
    "EXPECTED_RECOVERY_SHA256=$recoveryHash"
    "EXPECTED_TASK_SHA256=$taskHash"
    "ORIGINAL_WINRE_SHA256=$originalHash"
) | Add-Content (Join-Path $payload 'RecoveryTask.env') -Encoding ascii
$recoveryTaskEnvHash = Get-Sha256 (Join-Path $payload 'RecoveryTask.env')

$stagedWim = Join-Path $stage 'Winre.wim'
$mounted = $false
try {
    Copy-Item $registeredWim $stagedWim -Force
    Invoke-Native 'dism.exe' @('/Mount-Image', "/ImageFile:$stagedWim", '/Index:1', "/MountDir:$mount") $taskLog
    $mounted = $true
    $mountSystem32 = Join-Path $mount 'Windows\System32'
    foreach ($name in @('Recovery.cmd', 'RecoveryLauncher.cmd', 'RecoveryTask.env', 'task.json', 'winpeshl.ini', 'Recovery.exe')) {
        Remove-Item (Join-Path $mountSystem32 $name) -Force -ErrorAction SilentlyContinue
    }
    Copy-Item (Join-Path $payload 'Recovery.cmd') (Join-Path $mountSystem32 'Recovery.cmd') -Force
    Copy-Item (Join-Path $payload 'RecoveryLauncher.cmd') (Join-Path $mountSystem32 'RecoveryLauncher.cmd') -Force
    Copy-Item (Join-Path $payload 'RecoveryTask.env') (Join-Path $mountSystem32 'RecoveryTask.env') -Force
    Copy-Item (Join-Path $payload 'task.json') (Join-Path $mountSystem32 'task.json') -Force
    Copy-Item (Join-Path $payload 'winpeshl.ini') (Join-Path $mountSystem32 'winpeshl.ini') -Force
    if (Test-Path (Join-Path $payload 'Recovery.exe')) {
        Copy-Item (Join-Path $payload 'Recovery.exe') (Join-Path $mountSystem32 'Recovery.exe') -Force
    }
    Get-ChildItem $payload -Filter '*.dll' -File -ErrorAction SilentlyContinue |
        ForEach-Object { Copy-Item $_.FullName (Join-Path $mountSystem32 $_.Name) -Force }
    Invoke-Native 'dism.exe' @('/Unmount-Image', "/MountDir:$mount", '/Commit') $taskLog
    $mounted = $false
    # DISM can leave its WIM service alive for a short period after returning.
    # Avoid enumerating that process from PowerShell 5.1 (which can itself hang
    # on a terminating wimserv); a bounded grace period is sufficient here.
    Start-Sleep -Seconds 5
    Write-Log "DISM image commit completed: $taskId"

    $stagedHash = Get-Sha256 $stagedWim
    if (-not $NoReboot) {
        Copy-Item $stagedWim $registeredWim -Force
        $registeredHash = Get-Sha256 $registeredWim
        if ($registeredHash -ne $stagedHash) { throw 'Registered WinRE hash differs from staged WinRE.' }
    }
    $manifest = [ordered]@{
        taskId = $taskId
        launcherSha256 = $launcherHash.ToLowerInvariant()
        recoverySha256 = $recoveryHash.ToLowerInvariant()
        taskSha256 = $taskHash.ToLowerInvariant()
        originalWinreSha256 = $originalHash.ToLowerInvariant()
        stagedWinreSha256 = $stagedHash.ToLowerInvariant()
        createdByVersion = $programVersion
        recoveryTaskEnvSha256 = $recoveryTaskEnvHash.ToLowerInvariant()
    }
    Write-JsonAtomic (Join-Path $taskRoot 'manifest.json') $manifest
    @(
        "task_id=$taskId"
        'stage=prepared'
        'progress=0'
        "operation=$effectiveOperation"
        "original_winre_sha256=$originalHash"
        "staged_winre_sha256=$stagedHash"
    ) | Set-Content (Join-Path $taskRoot 'status.env') -Encoding ascii
    $initialStatus = [ordered]@{
        taskId = $taskId
        operation = $effectiveOperation
        stage = 'prepared'
        progress = 0
        updated = $created
    }
    Write-JsonAtomic (Join-Path $taskRoot 'status.json') $initialStatus
    Write-JsonAtomic (Join-Path $root 'last-task.json') ([ordered]@{
        taskId = $taskId
        operation = $effectiveOperation
        taskRoot = $taskRoot
        statusJson = (Join-Path $taskRoot 'status.json')
        recoveryLog = (Join-Path $taskRoot 'Recovery.log')
        prepareLog = $taskLog
        imagePath = $imagePath
        metadataPath = (Join-Path (Split-Path -Parent $imagePath) 'metadata.json')
        created = $created
    })

    if ($NoReboot) {
        Write-Log "Task prepared without changing registered WinRE: $taskId"
        Write-Output "TASK_ID=$taskId"
        Write-Output "TASK_ROOT=$taskRoot"
        exit 0
    }
    Invoke-Native 'reagentc.exe' @('/boottore') $taskLog
    @(
        "task_id=$taskId"
        'stage=boot-requested'
        'progress=2'
        "operation=$effectiveOperation"
        "original_winre_sha256=$originalHash"
        "staged_winre_sha256=$stagedHash"
    ) | Set-Content (Join-Path $taskRoot 'status.env') -Encoding ascii
    Write-JsonAtomic (Join-Path $taskRoot 'status.json') ([ordered]@{
        taskId = $taskId
        operation = $effectiveOperation
        stage = 'boot-requested'
        progress = 2
        updated = (Get-Date).ToUniversalTime().ToString('o')
    })
    Write-Log "Task prepared: $taskId"
    shutdown.exe /r /t 0
} catch {
    $failureMessage = "Preparation failed: $($_.Exception.Message)"
    try { Write-Log $failureMessage } catch { Add-Content -LiteralPath (Join-Path $logRoot 'prepare-errors.log') -Value $failureMessage -Encoding UTF8 }
    if ($mounted) {
        try {
            Invoke-Native 'dism.exe' @('/Unmount-Image', "/MountDir:$mount", '/Discard') $taskLog
            $mounted = $false
        } catch {
            Write-Log "Preparation cleanup could not discard the mounted WinRE image: $($_.Exception.Message)"
        }
    }
    try {
        Copy-Item (Join-Path $original 'Winre.wim') $registeredWim -Force
        $restoredHash = Get-Sha256 $registeredWim
        if ($restoredHash -ne $originalHash) { throw 'WinRE restore hash differs from the original copy.' }
        Write-Log 'Preparation failed; original registered WinRE restored.'
    } catch {
        Write-Log "Preparation failed and automatic WinRE restore failed: $($_.Exception.Message)"
    }
    throw
}

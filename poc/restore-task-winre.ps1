[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)]
    [ValidatePattern('^[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}$')]
    [string]$TaskId,
    [string]$TaskRoot = 'C:\BackupRestore\tasks'
)

$ErrorActionPreference = 'Stop'

function Require-Administrator {
    $identity = [Security.Principal.WindowsIdentity]::GetCurrent()
    $principal = [Security.Principal.WindowsPrincipal]::new($identity)
    if (-not $principal.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)) {
        throw 'Administrator elevation is required.'
    }
}

function Get-Sha256([string]$Path) {
    (Get-FileHash -LiteralPath $Path -Algorithm SHA256).Hash.ToLowerInvariant()
}

function Read-EnvFile([string]$Path) {
    $values = @{}
    foreach ($line in Get-Content -LiteralPath $Path) {
        if ([string]::IsNullOrWhiteSpace($line) -or $line.StartsWith('#')) { continue }
        $pair = $line -split '=', 2
        if ($pair.Count -ne 2 -or [string]::IsNullOrWhiteSpace($pair[0])) {
            throw "Malformed RecoveryTask.env line: $line"
        }
        $values[$pair[0]] = $pair[1]
    }
    $values
}

Require-Administrator
$task = $TaskId.ToLowerInvariant()
$root = Join-Path $TaskRoot $task
$original = Join-Path $root 'original\Winre.wim'
$envPath = Join-Path $root 'payload\RecoveryTask.env'
$manifestPath = Join-Path $root 'manifest.json'
if (-not (Test-Path -LiteralPath $original)) { throw "Task original WinRE is missing: $original" }
if (-not (Test-Path -LiteralPath $envPath)) { throw "Task environment is missing: $envPath" }
if (-not (Test-Path -LiteralPath $manifestPath)) { throw "Task manifest is missing: $manifestPath" }

$values = Read-EnvFile $envPath
if ($values['TASK_ID'] -ne $task) { throw 'Task ID does not match RecoveryTask.env.' }
foreach ($key in @('RECOVERY_VOLUME_GUID', 'RECOVERY_DISK_NUMBER', 'RECOVERY_PARTITION_NUMBER', 'ORIGINAL_WINRE_SHA256')) {
    if ([string]::IsNullOrWhiteSpace($values[$key])) { throw "RecoveryTask.env is missing $key." }
}
$manifest = Get-Content -LiteralPath $manifestPath -Raw | ConvertFrom-Json
if ("$($manifest.taskId)".ToLowerInvariant() -ne $task) { throw 'Task ID does not match manifest.' }

$partition = Get-Partition -DiskNumber ([int]$values['RECOVERY_DISK_NUMBER']) -PartitionNumber ([int]$values['RECOVERY_PARTITION_NUMBER'])
if ("$($partition.GptType)".ToLowerInvariant() -ne '{de94bba4-06d1-4d40-a16a-bfd50179d6ac}') {
    throw 'Task recovery partition is not a Windows Recovery partition.'
}
$volume = $partition | Get-Volume
if ("$($volume.UniqueId)" -ne $values['RECOVERY_VOLUME_GUID']) {
    throw 'Task recovery volume identity differs from the current partition.'
}

$drive = "$($volume.DriveLetter)"
if ([string]::IsNullOrWhiteSpace($drive)) {
    $drive = 'R'
    $existing = Get-Volume -DriveLetter $drive -ErrorAction SilentlyContinue
    if ($existing -and "$($existing.UniqueId)" -ne $values['RECOVERY_VOLUME_GUID']) {
        throw 'R: is assigned to a different volume; refusing to replace it.'
    }
    if (-not $existing) {
        Add-PartitionAccessPath -DiskNumber $partition.DiskNumber -PartitionNumber $partition.PartitionNumber -AccessPath 'R:\'
    }
}
$registered = "$drive`:\Recovery\WindowsRE\Winre.wim"
if (-not (Test-Path -LiteralPath $registered)) { throw "Registered WinRE is missing: $registered" }

$originalHash = Get-Sha256 $original
if ($originalHash -ne $values['ORIGINAL_WINRE_SHA256'].ToLowerInvariant()) {
    throw 'Task original WinRE hash does not match RecoveryTask.env.'
}
if ($originalHash -ne "$($manifest.originalWinreSha256)".ToLowerInvariant()) {
    throw 'Task original WinRE hash does not match manifest.'
}
Copy-Item -LiteralPath $original -Destination $registered -Force
$registeredHash = Get-Sha256 $registered
if ($registeredHash -ne $originalHash) { throw 'Registered WinRE hash does not match task original after copy.' }

$log = Join-Path $root 'manual-winre-restore.log'
"[{0}] Original WinRE restored and verified from task {1}" -f (Get-Date -Format o), $task |
    Set-Content -LiteralPath $log -Encoding utf8
Write-Output "RESTORED_TASK=$task"
Write-Output "REGISTERED_WINRE_SHA256=$registeredHash"

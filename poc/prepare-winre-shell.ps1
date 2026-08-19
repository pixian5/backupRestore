$ErrorActionPreference = 'Stop'

$root = 'C:\WinRE-PoC'
$source = Join-Path $root 'source'
$stage = Join-Path $root 'stage'
$mount = Join-Path $root 'mount'
$original = Join-Path $root 'original\Winre.wim'
$log = Join-Path $root 'shell-prepare.log'

function Write-Log([string]$Message) {
    $Message | Tee-Object -FilePath $log -Append
}

function Invoke-Native([string]$File, [string[]]$Arguments) {
    Write-Log ("> {0} {1}" -f $File, ($Arguments -join ' '))
    & $File @Arguments 2>&1 | Tee-Object -FilePath $log -Append
    if ($LASTEXITCODE -ne 0) { throw "$File failed with exit code $LASTEXITCODE" }
}

New-Item -ItemType Directory -Force -Path $root, $source, $stage, $mount | Out-Null
Write-Log "WinRE shell PoC preparation started: $(Get-Date -Format o)"

$recoveryPartition = Get-Partition |
    Where-Object { $_.GptType -eq '{de94bba4-06d1-4d40-a16a-bfd50179d6ac}' -and $_.Size -gt 500MB } |
    Sort-Object Size -Descending |
    Select-Object -First 1
if (-not $recoveryPartition) { throw 'WinRE recovery partition was not found' }
if (-not (Get-Volume -DriveLetter R -ErrorAction SilentlyContinue)) {
    Add-PartitionAccessPath -DiskNumber $recoveryPartition.DiskNumber -PartitionNumber $recoveryPartition.PartitionNumber -AccessPath 'R:\'
}

$registered = 'R:\Recovery\WindowsRE\Winre.wim'
if (-not (Test-Path $registered)) { throw "Registered WinRE image not found: $registered" }
if (-not (Test-Path $original)) { throw "Original WinRE backup missing: $original" }

$staged = Join-Path $stage 'Winre.wim'
Copy-Item $original $staged -Force
if (Test-Path (Join-Path $mount 'Windows')) {
    Invoke-Native 'dism.exe' @('/Unmount-Image', "/MountDir:$mount", '/Discard')
}

Invoke-Native 'dism.exe' @('/Mount-Image', "/ImageFile:$staged", '/Index:1', "/MountDir:$mount")
$system32 = Join-Path $mount 'Windows\System32'
Copy-Item (Join-Path $source 'RecoveryPoC.cmd') (Join-Path $system32 'RecoveryPoC.cmd') -Force
Copy-Item (Join-Path $source 'winpeshl.ini') (Join-Path $system32 'winpeshl.ini') -Force
Get-Content (Join-Path $system32 'winpeshl.ini') | Tee-Object -FilePath $log -Append
Invoke-Native 'dism.exe' @('/Unmount-Image', "/MountDir:$mount", '/Commit')

Copy-Item $staged $registered -Force
$hash = (Get-FileHash $registered -Algorithm SHA256).Hash
Set-Content (Join-Path $root 'shell-staged-winre.sha256') $hash -Encoding ascii
Write-Log "Shell-staged WinRE SHA256: $hash"
Invoke-Native 'reagentc.exe' @('/boottore')
Write-Log 'Shell PoC ready. The next reboot must start RecoveryPoC.cmd before recenv.exe.'

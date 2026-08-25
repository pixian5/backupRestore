[CmdletBinding()]
param(
    [string]$Root = 'C:\BackupRestorePE',
    [string]$Package = ''
)

$ErrorActionPreference = 'Stop'
$log = Join-Path $Root 'build.log'
New-Item -ItemType Directory -Force -Path $Root | Out-Null
Set-Content -LiteralPath $log -Value "BackupRestorePE build started $(Get-Date -Format o)" -Encoding UTF8

function Write-Log([string]$Message) {
    Add-Content -LiteralPath $log -Value "[$(Get-Date -Format o)] $Message" -Encoding UTF8
}

function Invoke-Dism([string[]]$Arguments) {
    Write-Log ("dism.exe " + ($Arguments -join ' '))
    & dism.exe @Arguments 2>&1 | Tee-Object -FilePath $log -Append
    if ($LASTEXITCODE -ne 0) {
        throw "DISM failed with exit code $LASTEXITCODE"
    }
}

$mountActive = $false
try {
    if ([string]::IsNullOrWhiteSpace($Package)) {
        $packageRoots = @('C:\BackupRestorePE\package', 'C:\BackupRestoreBuild\package') |
            Where-Object { Test-Path -LiteralPath $_ }
        $Package = $packageRoots |
            ForEach-Object { Get-ChildItem -LiteralPath $_ -Directory -Filter 'BackupRestore-windows-arm64-*' } |
            Sort-Object LastWriteTime -Descending |
            Select-Object -First 1 -ExpandProperty FullName
    }
    if (-not (Test-Path (Join-Path $Package 'Recovery.exe'))) {
        throw "ARM64 package with Recovery.exe was not found: $Package"
    }

    $adkRoot = Join-Path ${env:ProgramFiles(x86)} 'Windows Kits\10\Assessment and Deployment Kit\Windows Preinstallation Environment\arm64'
    $base = Join-Path $adkRoot 'en-us\winpe.wim'
    if (-not (Test-Path $base)) { throw "ADK ARM64 winpe.wim was not found: $base" }

    $work = Join-Path $Root 'work'
    $mount = Join-Path $Root 'mount'
    $output = Join-Path $Root 'BackupRestorePE.wim'
    $baseCopy = Join-Path $work 'base.wim'

    if (Test-Path $mount) {
        try { Invoke-Dism @('/Unmount-Image', "/MountDir:$mount", '/Discard') } catch { Write-Log 'No previous committed mount was present.' }
        Remove-Item -LiteralPath $mount -Recurse -Force -ErrorAction SilentlyContinue
    }
    Remove-Item -LiteralPath $work -Recurse -Force -ErrorAction SilentlyContinue
    New-Item -ItemType Directory -Force -Path $work, $mount | Out-Null
    Copy-Item -LiteralPath $base -Destination $baseCopy -Force
    Remove-Item -LiteralPath $output -Force -ErrorAction SilentlyContinue

    Invoke-Dism @('/Mount-Image', "/ImageFile:$baseCopy", '/Index:1', "/MountDir:$mount")
    $mountActive = $true
    $system32 = Join-Path $mount 'Windows\System32'
    $payloadFiles = @(
        'BackupRestore.exe', 'Recovery.exe', 'VCRUNTIME140.dll', 'VCRUNTIME140_1.dll',
        'RecoveryLauncher.cmd', 'BackupRestore.cmd', 'winpeshl.ini'
    )
    foreach ($name in $payloadFiles) {
        $source = Join-Path $Package $name
        if (-not (Test-Path $source)) { throw "Package payload is missing: $source" }
        Copy-Item -LiteralPath $source -Destination (Join-Path $system32 $name) -Force
    }

    # The ADK base already carries its architecture-matched DISM provider
    # tree below System32\Dism. Verify it in place; do not mix host-version
    # provider DLLs into the image.
    foreach ($name in @('dism.exe', 'bcdboot.exe')) {
        if (-not (Test-Path (Join-Path $system32 $name))) {
            throw "Required WinPE tool is missing: $name"
        }
    }
    foreach ($name in @('DismCore.dll', 'WimProvider.dll')) {
        if (-not (Test-Path (Join-Path $system32 "Dism\$name"))) {
            throw "Required WinPE DISM provider is missing: $name"
        }
    }
    if (-not (Test-Path (Join-Path $system32 'DismApi.dll'))) {
        throw 'Required WinPE DISM API is missing: DismApi.dll'
    }
    Set-Content -LiteralPath (Join-Path $mount 'BackupRestorePE.txt') -Value @(
        'BackupRestorePE',
        'Architecture: ARM64',
        'Windows RE base: ADK arm64 winpe.wim',
        'DISM: Capture-Image and Apply-Image',
        'BCDBoot: bcdboot.exe',
        'Launcher: Windows\System32\winpeshl.ini -> RecoveryLauncher.cmd -> Recovery.exe'
    ) -Encoding UTF8

    Invoke-Dism @('/Unmount-Image', "/MountDir:$mount", '/Commit', '/CheckIntegrity')
    $mountActive = $false
    Invoke-Dism @('/Export-Image', "/SourceImageFile:$baseCopy", '/SourceIndex:1', "/DestinationImageFile:$output", '/Compress:max', '/CheckIntegrity')
    Invoke-Dism @('/Get-WimInfo', "/WimFile:$output")
    $verifyMount = Join-Path $Root 'verify-mount'
    New-Item -ItemType Directory -Force -Path $verifyMount | Out-Null
    Invoke-Dism @('/Mount-Image', "/ImageFile:$output", '/Index:1', "/MountDir:$verifyMount", '/ReadOnly')
    foreach ($relative in @(
        'Windows\System32\BackupRestore.exe', 'Windows\System32\Recovery.exe',
        'Windows\System32\RecoveryLauncher.cmd', 'Windows\System32\winpeshl.ini',
        'Windows\System32\dism.exe', 'Windows\System32\DismApi.dll', 'Windows\System32\Dism\DismCore.dll',
        'Windows\System32\Dism\WimProvider.dll', 'Windows\System32\bcdboot.exe',
        'Windows\System32\drivers\disk.sys', 'Windows\System32\drivers\ntfs.sys',
        'Windows\System32\drivers\stornvme.sys', 'Windows\System32\drivers\USBXHCI.SYS'
    )) {
        if (-not (Test-Path (Join-Path $verifyMount $relative))) {
            throw "WIM verification missing: $relative"
        }
    }
    Invoke-Dism @('/Unmount-Image', "/MountDir:$verifyMount", '/Discard')
    Remove-Item -LiteralPath $verifyMount -Recurse -Force -ErrorAction SilentlyContinue
    $hash = (Get-FileHash -LiteralPath $output -Algorithm SHA256).Hash.ToLowerInvariant()
    Write-Log "Output=$output"
    Write-Log "SHA256=$hash"
    if (Test-Path -LiteralPath 'Y:\') {
        Copy-Item -LiteralPath $output -Destination 'Y:\artifacts\BackupRestorePE.wim' -Force
        Write-Log 'Copied output to Y:\artifacts\BackupRestorePE.wim.'
    } else {
        Write-Log 'Y: is not visible in the elevated token; leaving output on C:.'
    }
    Set-Content -LiteralPath (Join-Path $Root 'build-result.txt') -Value @(
        "Output=$output",
        "SHA256=$hash",
        "Package=$Package",
        "Completed=$(Get-Date -Format o)"
    ) -Encoding UTF8
    Write-Log 'BackupRestorePE build completed successfully.'
}
catch {
    Write-Log ("ERROR: " + $_.Exception.Message)
    if ($mountActive) {
        try {
            Invoke-Dism @('/Unmount-Image', "/MountDir:$mount", '/Discard')
            $mountActive = $false
        } catch {
            Write-Log ("Could not discard failed mount: " + $_.Exception.Message)
        }
    }
    throw
}

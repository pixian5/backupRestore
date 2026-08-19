[CmdletBinding()]
param(
    [ValidateSet('x64', 'arm64', 'all')]
    [string]$Architecture = 'arm64',
    [string]$OutputRoot = ''
)

$ErrorActionPreference = 'Stop'
$repoRoot = Split-Path -Parent $PSScriptRoot
$version = (Get-Content (Join-Path $repoRoot 'VERSION') -Raw).Trim()
if ([string]::IsNullOrWhiteSpace($version)) { throw 'VERSION is empty.' }

$targets = [ordered]@{
    x64 = 'x86_64-pc-windows-msvc'
    arm64 = 'aarch64-pc-windows-msvc'
}
$architectures = if ($Architecture -eq 'all') { @('x64', 'arm64') } else { @($Architecture) }
$outputBase = if ([string]::IsNullOrWhiteSpace($OutputRoot)) {
    Join-Path $repoRoot 'artifacts\windows'
} else {
    $resolvedOutput = Resolve-Path -LiteralPath $OutputRoot -ErrorAction SilentlyContinue
    if ($resolvedOutput) { $resolvedOutput.Path } else { [System.IO.Path]::GetFullPath($OutputRoot) }
}
New-Item -ItemType Directory -Force -Path $outputBase | Out-Null

function Invoke-Cargo([string[]]$Arguments) {
    & cargo @Arguments
    if ($LASTEXITCODE -ne 0) { throw "cargo failed with exit code $LASTEXITCODE" }
}

foreach ($arch in $architectures) {
    $target = $targets[$arch]
    if ([string]::IsNullOrWhiteSpace($target)) { throw "Unknown architecture: $arch" }
    $installed = @(rustup target list --installed)
    if ($installed -notcontains $target) {
        throw "Rust target $target is not installed. Install it explicitly with 'rustup target add $target' before building; this script never downloads toolchains automatically."
    }

    Write-Host "Building $arch ($target) version $version"
    Invoke-Cargo @('build', '--release', '--locked', '--target', $target, '-p', 'backuprestore-cli')

    $package = Join-Path $outputBase "BackupRestore-windows-$arch-v$version"
    if (Test-Path $package) { Remove-Item -LiteralPath $package -Recurse -Force }
    New-Item -ItemType Directory -Force -Path $package | Out-Null
    $binary = Join-Path $repoRoot "target\$target\release\backuprestore-cli.exe"
    if (-not (Test-Path $binary)) { throw "Build output missing: $binary" }
    $binaryHash = (Get-FileHash $binary -Algorithm SHA256).Hash.ToLowerInvariant()

    Copy-Item $binary (Join-Path $package 'BackupRestore.exe')
    Copy-Item $binary (Join-Path $package 'Recovery.exe')
    foreach ($file in @(
        'BackupRestore.cmd',
        'BackupRestore.Gui.ps1',
        'BackupRestore.ps1',
        'Recovery.cmd',
        'RecoveryLauncher.cmd',
        'winpeshl.ini',
        '..\VERSION'
    )) {
        $source = Join-Path $PSScriptRoot $file
        if ($file -eq '..\VERSION') { $source = Join-Path $repoRoot 'VERSION' }
        Copy-Item $source (Join-Path $package (Split-Path -Leaf $source))
    }

    $manifest = [ordered]@{
        version = $version
        architecture = $arch
        rustTarget = $target
        nativeMachine = if ($arch -eq 'arm64') { 'ARM64' } else { 'AMD64' }
        binarySha256 = $binaryHash
        frontend = 'BackupRestore.exe'
        recovery = 'Recovery.exe'
        runtime = 'Windows 11 ARM64 development package'
        note = 'Architecture-specific Windows development package; x64 and ARM64 are separate binaries.'
    }
    $manifest | ConvertTo-Json -Depth 4 | Set-Content (Join-Path $package 'build-manifest.json') -Encoding UTF8
    Write-Host "Package: $package"
}

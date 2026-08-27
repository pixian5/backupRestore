[CmdletBinding()]
param(
    [ValidateSet('x64', 'arm64', 'all')]
    [string]$Architecture = 'arm64',
    [string]$OutputRoot = '',
    [string]$CargoTargetDir = ''
)

$ErrorActionPreference = 'Stop'
$repoRoot = Split-Path -Parent $PSScriptRoot
$version = (Get-Content (Join-Path $repoRoot 'VERSION') -Raw).Trim()
if ([string]::IsNullOrWhiteSpace($version)) { throw 'VERSION is empty.' }

function Get-PackageVersion([string]$manifest) {
    $match = Select-String -LiteralPath $manifest -Pattern '^version\s*=\s*"([^"]+)"\s*$' |
        Select-Object -First 1
    if (-not $match) { throw "Package version is missing: $manifest" }
    return $match.Matches[0].Groups[1].Value
}

# The package directory uses VERSION while the executable title uses
# CARGO_PKG_VERSION. Refuse to publish a misleading mixed-version package.
foreach ($manifest in @(
    (Join-Path $repoRoot 'crates\backuprestore-core\Cargo.toml'),
    (Join-Path $repoRoot 'crates\backuprestore-cli\Cargo.toml')
)) {
    $packageVersion = Get-PackageVersion $manifest
    if ($packageVersion -ne $version) {
        throw "VERSION ($version) does not match $manifest ($packageVersion). Synchronize release versions before building."
    }
}

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
$targetRoot = if (-not [string]::IsNullOrWhiteSpace($CargoTargetDir)) {
    [System.IO.Path]::GetFullPath($CargoTargetDir)
} elseif (-not [string]::IsNullOrWhiteSpace($env:CARGO_TARGET_DIR)) {
    [System.IO.Path]::GetFullPath($env:CARGO_TARGET_DIR)
} else {
    Join-Path $repoRoot 'target'
}
New-Item -ItemType Directory -Force -Path $targetRoot | Out-Null
if (-not [string]::IsNullOrWhiteSpace($CargoTargetDir)) {
    $env:CARGO_TARGET_DIR = $targetRoot
}

function Invoke-Cargo([string[]]$Arguments) {
    # prlctl/Guest Tools starts PowerShell in C:\. Always bind Cargo to this
    # package manifest so a documented build command works from any directory.
    if ($Arguments.Count -eq 0) { throw 'Cargo command is missing.' }
    $cargoArguments = @(
        $Arguments[0]
        '--manifest-path'
        (Join-Path $repoRoot 'Cargo.toml')
        $Arguments | Select-Object -Skip 1
    )
    & cargo @cargoArguments
    if ($LASTEXITCODE -ne 0) { throw "cargo failed with exit code $LASTEXITCODE" }
}

function Initialize-MsvcEnvironment([string]$arch) {
    $programFilesX86 = [Environment]::GetEnvironmentVariable('ProgramFiles(x86)')
    $vswhere = Join-Path $programFilesX86 'Microsoft Visual Studio\Installer\vswhere.exe'
    if (-not (Test-Path $vswhere)) {
        if (-not (Get-Command link.exe -ErrorAction SilentlyContinue)) {
            throw 'MSVC link.exe was not found. Install Visual Studio C++ Build Tools before building.'
        }
        return
    }
    $vsPath = & $vswhere -latest -products * -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 -property installationPath |
        Select-Object -First 1
    if ([string]::IsNullOrWhiteSpace($vsPath)) {
        throw 'A Visual Studio C++ Build Tools installation was not found.'
    }
    $vsDevCmd = Join-Path $vsPath 'Common7\Tools\VsDevCmd.bat'
    if (-not (Test-Path $vsDevCmd)) { throw "VsDevCmd.bat is missing: $vsDevCmd" }
    $targetArch = if ($arch -eq 'arm64') { 'arm64' } else { 'x64' }
    $hostArch = if ($env:PROCESSOR_ARCHITECTURE -match '(?i)ARM64') { 'arm64' } else { 'x64' }
    $commandLine = '"' + $vsDevCmd + '" -arch=' + $targetArch + ' -host_arch=' + $hostArch + ' >nul && set'
    $lines = & cmd.exe /d /s /c $commandLine
    $imported = 0
    foreach ($line in $lines) {
        if ($line -match '^([^=]+)=(.*)$') {
            [Environment]::SetEnvironmentVariable($matches[1], $matches[2], 'Process')
            $imported++
        }
    }
    if ($imported -eq 0 -or -not (Get-Command link.exe -ErrorAction SilentlyContinue)) {
        throw "Visual Studio developer environment could not be initialized for $targetArch."
    }
}

foreach ($arch in $architectures) {
    $target = $targets[$arch]
    if ([string]::IsNullOrWhiteSpace($target)) { throw "Unknown architecture: $arch" }
    Initialize-MsvcEnvironment $arch
    $installed = @(rustup target list --installed)
    if ($installed -notcontains $target) {
        throw "Rust target $target is not installed. Install it explicitly with 'rustup target add $target' before building; this script never downloads toolchains automatically."
    }

    Write-Host "Building $arch ($target) version $version"
    Invoke-Cargo @('build', '--release', '--locked', '--target', $target, '-p', 'backuprestore-cli')

    $package = Join-Path $outputBase "BackupRestore-windows-$arch-v$version"
    if (Test-Path $package) { Remove-Item -LiteralPath $package -Recurse -Force }
    New-Item -ItemType Directory -Force -Path $package | Out-Null
    $binary = Join-Path $targetRoot "$target\release\backuprestore-cli.exe"
    if (-not (Test-Path $binary)) { throw "Build output missing: $binary" }
    $binaryHash = (Get-FileHash $binary -Algorithm SHA256).Hash.ToLowerInvariant()

    Copy-Item $binary (Join-Path $package 'BackupRestore.exe')
    Copy-Item $binary (Join-Path $package 'Recovery.exe')
    $runtimeRoot = if ($arch -eq 'arm64') {
        Join-Path $env:WINDIR 'System32'
    } elseif ($env:PROCESSOR_ARCHITECTURE -match '(?i)ARM64') {
        Join-Path $env:WINDIR 'SysWOW64'
    } else {
        Join-Path $env:WINDIR 'System32'
    }
    foreach ($runtime in @('VCRUNTIME140.dll', 'VCRUNTIME140_1.dll')) {
        $runtimePath = Join-Path $runtimeRoot $runtime
        if (-not (Test-Path $runtimePath)) { throw "MSVC runtime is missing: $runtimePath" }
        Copy-Item $runtimePath (Join-Path $package $runtime)
    }
    foreach ($file in @(
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
        runtime = "Windows 10/11 $($arch.ToUpperInvariant()) development package"
        note = 'Architecture-specific Windows development package; x64 and ARM64 are separate binaries.'
    }
    $manifest | ConvertTo-Json -Depth 4 | Set-Content (Join-Path $package 'build-manifest.json') -Encoding UTF8
    Write-Host "Package: $package"
}

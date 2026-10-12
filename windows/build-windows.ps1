[CmdletBinding()]
param(
    [ValidateSet('x64', 'arm64', 'all')]
    [string]$Architecture = 'arm64',
    [string]$OutputRoot = '',
    [string]$CargoTargetDir = '',
    # 构建后直接把成品部署到客体运行目录，并逐个核对 SHA-256。
    # 2026-10-12 起构建主路径从 macOS 交叉编译改到 VM 内原生编译，
    # 因此把原先 build-win.sh --deploy 的能力补到这里，让 Windows 侧也能
    # 一条命令「构建 + 部署 + 核对」收工。
    [switch]$Deploy,
    [string]$DeployPackageDir = 'C:\Users\Public\backupRestore-package',
    [string]$DeployWorkDir = 'H:\brwork'
)

$ErrorActionPreference = 'Stop'
$repoRoot = Split-Path -Parent $PSScriptRoot
$version = (Get-Content (Join-Path $repoRoot 'VERSION') -Raw).Trim()
if ([string]::IsNullOrWhiteSpace($version)) { throw 'VERSION is empty.' }
if ($Deploy -and $Architecture -eq 'all') {
    throw '-Deploy 需要单一架构：all 会产出两个包，无法决定部署哪一个。'
}

function Assert-ShellContracts {
    # The WinRE contract is embedded in Recovery.exe at compile time. Keep the
    # repository copy only as a source/audit fixture; it is not a runtime file.
    $template = Join-Path $PSScriptRoot 'winre-winpeshl.ini'
    $expected = "[LaunchApps]`n%SYSTEMROOT%\System32\Recovery.exe,recover-env %SYSTEMROOT%\System32\RecoveryTask.env"
    if (-not (Test-Path -LiteralPath $template)) { throw "WinRE shell template is missing: $template" }
    $actual = ((Get-Content -LiteralPath $template -Raw) -replace "`r`n", "`n").Trim()
    if ($actual -ne $expected) {
        throw 'WinRE shell template must directly launch Recovery.exe recover-env.'
    }
    $peTemplate = Join-Path $PSScriptRoot 'winpe-winpeshl.ini'
    $peExpected = "[LaunchApps]`n%SYSTEMROOT%\System32\Recovery.exe,--pe-desktop"
    if (-not (Test-Path -LiteralPath $peTemplate)) { throw "WinPE shell template is missing: $peTemplate" }
    $peActual = ((Get-Content -LiteralPath $peTemplate -Raw) -replace "`r`n", "`n").Trim()
    if ($peActual -ne $peExpected) {
        throw 'WinPE shell template must launch Recovery.exe --pe-desktop.'
    }
}

Assert-ShellContracts

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
        'winpe-winpeshl.ini',
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
        winreShell = 'embedded in Recovery.exe; generated as task payload winpeshl.ini'
        winpeShellTemplate = 'winpe-winpeshl.ini'
        runtime = "Windows 10/11 $($arch.ToUpperInvariant()) development package"
        note = 'Architecture-specific Windows development package; x64 and ARM64 are separate binaries.'
    }
    $manifest | ConvertTo-Json -Depth 4 | Set-Content (Join-Path $package 'build-manifest.json') -Encoding UTF8
    Write-Host "Package: $package"

    if ($Deploy) {
        # 部署判据只有一个：客体上四个文件的 SHA-256 与刚构建的成品逐个相等。
        # 复制成功不算数——2026-09-30 踩过「只更新包目录、运行目录还是旧件」的坑，
        # 唯一症状是镜像元数据里的 programVersion 偏低，静态检查看不出来。
        Publish-Build -Binary $binary -PackageDir $DeployPackageDir -WorkDir $DeployWorkDir
    }
}

function Publish-Build {
    param(
        [Parameter(Mandatory = $true)][string]$Binary,
        [Parameter(Mandatory = $true)][string]$PackageDir,
        [Parameter(Mandatory = $true)][string]$WorkDir
    )
    $sourceHash = (Get-FileHash $Binary -Algorithm SHA256).Hash.ToLowerInvariant()
    Write-Host "Deploying to $PackageDir and $WorkDir (sha256 $sourceHash)"

    # 先停掉可能在跑的旧进程，否则文件被占用会导致复制半途而废。
    # 找不到进程是常态，不算失败。
    foreach ($name in @('BackupRestore.exe', 'Recovery.exe')) {
        Stop-Process -Name ([System.IO.Path]::GetFileNameWithoutExtension($name)) -Force -ErrorAction SilentlyContinue
    }
    # 句柄释放需要一点时间；立刻复制可能撞上"另一个程序正在使用此文件"。
    Start-Sleep -Milliseconds 500

    New-Item -ItemType Directory -Force -Path $PackageDir, $WorkDir | Out-Null

    # 清掉包目录里的历史启动件：它们已被 Recovery.exe 内嵌的 WinRE 契约取代，
    # 留着会让人误以为还有这条回退路径。
    foreach ($stale in @('RecoveryLauncher.cmd', 'winpeshl.ini', 'winpeshl-boot.cmd')) {
        $path = Join-Path $PackageDir $stale
        if (Test-Path -LiteralPath $path) { Remove-Item -LiteralPath $path -Force -ErrorAction SilentlyContinue }
    }

    # 四个目标文件：两个目录 × 两个名字（BackupRestore.exe / Recovery.exe）。
    # 少更新任何一个都会让 WinRE 载荷烘进旧代码。
    $targets = @(
        @{ Path = (Join-Path $PackageDir 'BackupRestore.exe'); Label = '包目录 BackupRestore.exe' },
        @{ Path = (Join-Path $PackageDir 'Recovery.exe'); Label = '包目录 Recovery.exe' },
        @{ Path = (Join-Path $WorkDir 'BackupRestore.exe'); Label = '运行目录 BackupRestore.exe' },
        @{ Path = (Join-Path $WorkDir 'Recovery.exe'); Label = '运行目录 Recovery.exe' }
    )
    foreach ($target in $targets) {
        Copy-Item $Binary $target.Path -Force
        if (-not (Test-Path -LiteralPath $target.Path)) {
            throw "部署失败：$($target.Label) 未生成于 $($target.Path)"
        }
    }
    # WinPE 的 winpeshl.ini 是仓库里的审计夹具，随包目录一起放，便于人工比对。
    Copy-Item (Join-Path $PSScriptRoot 'winpe-winpeshl.ini') (Join-Path $PackageDir 'winpe-winpeshl.ini') -Force

    foreach ($target in $targets) {
        $deployed = (Get-FileHash $target.Path -Algorithm SHA256).Hash.ToLowerInvariant()
        if ($deployed -ne $sourceHash) {
            throw "部署失败：$($target.Label) 的 SHA-256 与构建产物不一致（客体=$deployed 构建=$sourceHash）"
        }
        Write-Host ("  OK {0} => {1}" -f $target.Label, $deployed)
    }
    Write-Host "DEPLOYED SHA256=$sourceHash"
}

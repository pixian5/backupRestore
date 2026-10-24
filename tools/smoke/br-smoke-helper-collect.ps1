#Requires -Version 5.1
# Smoke helper: STEP 4 - collect post-restore state (verdict inputs).
#
# Executed from tools/smoke/br-smoke.sh straight off the host share:
#   powershell -NoProfile -EncodedCommand <PS_PRE + "$c=Get-Content -Raw -Encoding UTF8
#   '\\Mac\Home\...\br-smoke-helper-collect.ps1'; $Drive='T'; iex $c">
# (ExecutionPolicy is Restricted in the VM, so -File is rejected; see the
#  longer note in br-smoke-helper-seed.ps1.)
#
# Injected by the caller:
#   $Drive : target drive letter (default T)
#   $Pre   : baseline JSON written by the seed helper
#   $Out   : output JSON path

if (-not $Drive) { $Drive = 'T' }
if (-not $Pre) { $Pre = 'H:\brwork\smoke-pre.json' }
if (-not $Out) { $Out = 'H:\brwork\smoke-post.json' }

[IO.File]::WriteAllText('H:\brwork\smoke-collect-run.txt', 'STARTED')

$ErrorActionPreference = 'Continue'
$root = $Drive + ':\'
$post = @{}

# Get-Content -Raw keeps the UTF-8 BOM that Set-Content -Encoding UTF8 writes;
# PS 5.1 ConvertFrom-Json fails on it, so strip it before parsing.
$bom = [char]0xFEFF
$preText = (Get-Content $Pre -Raw).TrimStart($bom)

$post.serial = (Get-CimInstance Win32_LogicalDisk -Filter ("DeviceID='" + $Drive + ":'")).VolumeSerialNumber
$post.sentinel = if (Test-Path ($root + 'BR-BENTRY-SENTINEL.txt')) {
    [IO.File]::ReadAllText($root + 'BR-BENTRY-SENTINEL.txt')
}
else { '' }
$post.bigsha = if (Test-Path ($root + 'bigfile.bin')) {
    (Get-FileHash ($root + 'bigfile.bin') -Algorithm SHA256).Hash
}
else { '' }
$post.biglen = if (Test-Path ($root + 'bigfile.bin')) {
    (Get-Item ($root + 'bigfile.bin')).Length
}
else { 0 }

$post.br38 = @{
    marker = (Test-Path ($root + 'BR38Data\marker-pre.txt'))
    file1  = (Test-Path ($root + 'BR38Data\file1.txt'))
    file2  = (Test-Path ($root + 'BR38Data\file2.txt'))
    nested = (Test-Path ($root + 'BR38Data\sub\nested.txt'))
}
$post.br38complete = ($post.br38.marker -and $post.br38.file1 -and $post.br38.file2 -and $post.br38.nested)

# Stability verdict: EventID 1000 in the last 2 hours + WerFault processes.
$since = (Get-Date).AddHours(-2)
$post.evt1000 = @(Get-WinEvent -FilterHashtable @{LogName = 'Application'; Id = 1000; StartTime = $since } -ErrorAction SilentlyContinue).Count
$post.werfault = @(Get-Process WerFault -ErrorAction SilentlyContinue).Count

# Baseline (so the host can compare without a second file round-trip).
$preData = $preText | ConvertFrom-Json
$post.preSentinel = $preData.sentinel
$post.preSha = $preData.bigsha
$post.preLen = $preData.biglen
$post.preSerial = $preData.serial

$post | ConvertTo-Json -Compress | ForEach-Object {
    [IO.File]::WriteAllText($Out, $_, (New-Object Text.UTF8Encoding $false))
}
[IO.File]::AppendAllText('H:\brwork\smoke-collect-run.txt', 'DONE')

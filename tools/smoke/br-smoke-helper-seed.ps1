#Requires -Version 5.1
# Smoke helper: STEP 1 - seed the test volume (sentinel / BR38Data tree / 100MB payload).
#
# Executed from tools/smoke/br-smoke.sh, normally straight off the host share:
#   powershell -NoProfile -EncodedCommand <PS_PRE + "$c=Get-Content -Raw -Encoding UTF8
#   '\\Mac\Home\...\br-smoke-helper-seed.ps1'; $Drive='T'; $Out='...'; iex $c">
#
# Why "iex" and not "-File": the VM's ExecutionPolicy is Restricted, so script
# *files* cannot run (dot-invocation and -File are both rejected, and
# -ExecutionPolicy Bypass does not take effect through the prlctl exec channel).
# Invoke-Expression on in-memory text is unaffected by the policy.
# Why the share and not a transfer: the VM can read \\Mac\Home directly, so no
# copy step is needed at all - that also dodges the ~3.4KB command-line limit
# that a single -EncodedCommand carrying this script body would hit.
#
# All output files are written BOM-less (WriteAllText / UTF8Encoding($false)):
# PS 5.1's "Out-File -Encoding UTF8" prepends a BOM, which then shows up as
# U+FEFF on the host side and breaks line/field comparisons.
#
# Injected by the caller:
#   $Drive : target drive letter (default T)
#   $Out   : output JSON path (default H:\brwork\smoke-pre.json)

if (-not $Drive) { $Drive = 'T' }
if (-not $Out) { $Out = 'H:\brwork\smoke-pre.json' }

[IO.File]::WriteAllText('H:\brwork\smoke-seed-run.txt', 'STARTED')

$ErrorActionPreference = 'Stop'
try {
    $root = $Drive + ':\'

    # Defensive: file entities restored from a WIM can end up in a state where
    # [IO.File]::WriteAllText reports "access denied" while cmd echo,
    # Set-Content and an exclusive File.Open all succeed (chkdsk clean, ACL and
    # SDDL identical to a working file, no lock held; renaming does not help,
    # creating a fresh file does). Seeding therefore always removes first.
    Remove-Item ($root + 'BR-BENTRY-SENTINEL.txt') -Force -ErrorAction SilentlyContinue
    Remove-Item ($root + 'BR38Data') -Recurse -Force -ErrorAction SilentlyContinue
    Remove-Item ($root + 'bigfile.bin') -Force -ErrorAction SilentlyContinue

    $pre = @{}
    # Sentinel carries a timestamp so the verdict can compare exact text.
    $sent = 'BR-BENTRY-SENTINEL smoke-' + (Get-Date -Format 'yyyyMMdd-HHmmss')
    [IO.File]::WriteAllText($root + 'BR-BENTRY-SENTINEL.txt', $sent)
    $pre.sentinel = $sent

    # BR38Data tree (four verdict entries).
    New-Item -ItemType Directory -Force -Path ($root + 'BR38Data\sub') | Out-Null
    'marker-pre' | Set-Content ($root + 'BR38Data\marker-pre.txt')
    'file1-smoke' | Set-Content ($root + 'BR38Data\file1.txt')
    'file2-smoke' | Set-Content ($root + 'BR38Data\file2.txt')
    'nested-smoke' | Set-Content ($root + 'BR38Data\sub\nested.txt')

    # 100MB payload (fsutil fast create; SHA256 gives byte-level verdict).
    fsutil file createnew ($root + 'bigfile.bin') 104857600 | Out-Null
    $pre.bigsha = (Get-FileHash ($root + 'bigfile.bin') -Algorithm SHA256).Hash
    $pre.biglen = (Get-Item ($root + 'bigfile.bin')).Length
    $pre.serial = (Get-CimInstance Win32_LogicalDisk -Filter ("DeviceID='" + $Drive + ":'")).VolumeSerialNumber

    $pre | ConvertTo-Json -Compress | ForEach-Object {
        [IO.File]::WriteAllText($Out, $_, (New-Object Text.UTF8Encoding $false))
    }
    [IO.File]::AppendAllText('H:\brwork\smoke-seed-run.txt', 'DONE')
}
catch {
    [IO.File]::AppendAllText('H:\brwork\smoke-seed-run.txt',
        ('ERR: ' + $_.Exception.Message + ' @' + $_.InvocationInfo.ScriptLineNumber))
}

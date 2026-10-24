# br-acl-compare.ps1 -- E3 evaluation: ACL (SDDL) comparison, image vs restored volume.
#
# Usage (run in VM, SYSTEM channel, two passes + unmount):
#   powershell -ExecutionPolicy Bypass -File H:\brwork\br-acl-compare.ps1 -Mode image   -Wim H:\BRImages\br-2629-20261023.wim -Index 1
#   powershell -ExecutionPolicy Bypass -File H:\brwork\br-acl-compare.ps1 -Mode volume  -Volume F:
#   powershell -ExecutionPolicy Bypass -File H:\brwork\br-acl-compare.ps1 -Mode unmount
#
# Mode=image mounts the WIM read-only at H:\br-acl-mount and dumps per-path SDDL to JSON.
# Mode=volume reads the same paths from the target volume; unmount cleans up.
# Criterion: identical SDDL per path (owner + DACL). SACL is excluded on purpose:
# neither DISM nor wimlib restores SACLs, so it is a shared boundary, not a difference.

param(
    [ValidateSet('image','volume','unmount')][string]$Mode = 'image',
    [string]$Wim = 'H:\BRImages\br-2629-20261023.wim',
    [int]$Index = 1,
    [string]$Volume = 'F:',
    [string]$Mount = 'H:\br-acl-mount',
    [string]$Out = 'H:\br-acl-sddl.json'
)

$ErrorActionPreference = 'Stop'

# Six representative paths: volume root, Users, Program Files, WindowsApps
# (conditional ACEs + app package SIDs), System32 (TrustedInstaller-owned),
# and the SYSTEM hive file.
$Paths = @(
    '',
    'Users',
    'Program Files',
    'Program Files\WindowsApps',
    'Windows\System32',
    'Windows\System32\config\SYSTEM'
)

function Get-Sddl([string]$root, [string]$rel) {
    $p = if ($rel -eq '') { $root } else { Join-Path $root $rel }
    try {
        $acl = Get-Acl -LiteralPath $p
        return $acl.Sddl.ToString()
    } catch {
        return "ERROR: $($_.Exception.Message)"
    }
}

if ($Mode -eq 'unmount') {
    & dism.exe /English /Unmount-Image /MountDir:$Mount /Discard 2>&1 | Out-Null
    if (Test-Path -LiteralPath $Mount) { Remove-Item -LiteralPath $Mount -Force -ErrorAction SilentlyContinue }
    Write-Output "UNMOUNTED rc=$LASTEXITCODE"
    exit 0
}

$root = if ($Mode -eq 'image') {
    if (-not (Test-Path -LiteralPath $Mount)) { New-Item -ItemType Directory -Path $Mount -Force | Out-Null }
    $r = & dism.exe /English /Mount-Image /ImageFile:$Wim /Index:$Index /MountDir:$Mount /ReadOnly 2>&1
    if ($LASTEXITCODE -ne 0) { Write-Output ("MOUNT_FAILED: " + ($r -join ' | ')); exit 2 }
    $Mount
} else {
    $Volume
}

$result = [ordered]@{ mode = $Mode; root = $root; sddl = @{} }
foreach ($rel in $Paths) {
    $result.sddl[$rel] = Get-Sddl $root $rel
}
$json = $result | ConvertTo-Json -Depth 4
[IO.File]::WriteAllText($Out, $json, (New-Object Text.UTF8Encoding $false))
Write-Output "WROTE $Out"
foreach ($rel in $Paths) {
    $s = $result.sddl[$rel]
    $head = if ($s.Length -gt 72) { $s.Substring(0,72) + '...' } else { $s }
    $label = if ($rel -eq '') { '<root>' } else { $rel }
    Write-Output ("{0,-34} {1}" -f $label, $head)
}

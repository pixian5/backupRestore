$ErrorActionPreference = 'Stop'
# Create a normal OSLOADER BCD entry that boots the injected WinRE ramdisk directly.
$ro = 'e7e9964b-af5d-11f1-87bb-a75f11d145f3'   # existing Windows Recovery ramdisk options
$ramdisk = 'ramdisk=[R:]\Recovery\WindowsRE\Winre.wim,{' + $ro + '}'

$out = New-Object System.Collections.ArrayList
function RunBcd($argsList) { $o = & bcdedit.exe @argsList 2>&1; return ($o -join "`n") }

# create entry
$createOut = RunBcd @('/create','/d','BR-WinRE','/application','OSLOADER')
$out += "CREATE: $createOut"
$m = [regex]::Match($createOut, '\{([0-9a-fA-F-]+)\}')
if (-not $m.Success) { $out += 'NO-GUID'; Set-Content 'C:\Users\Public\backupRestore-package\bcd-winre.txt' -Value $out; exit 1 }
$g = $m.Groups[1].Value
$out += "GUID=$g"

$out += "dev: " + (RunBcd @('/set', ('{'+$g+'}'), 'device', $ramdisk))
$out += "osdev: " + (RunBcd @('/set', ('{'+$g+'}'), 'osdevice', $ramdisk))
$out += "path: " + (RunBcd @('/set', ('{'+$g+'}'), 'path', '\windows\system32\winload.efi'))
$out += "sysroot: " + (RunBcd @('/set', ('{'+$g+'}'), 'systemroot', '\Windows'))
$out += "winpe: " + (RunBcd @('/set', ('{'+$g+'}'), 'winpe', 'yes'))
$out += "maxmem: " + (RunBcd @('/set', ('{'+$g+'}'), 'ems', 'off'))

# add to boot manager display order (after current) and set as default for one-time boot
$out += "displayorder: " + (RunBcd @('/displayorder', ('{'+$g+'}'), '/addlast'))
$out += "bootsequence: " + (RunBcd @('/set', '{bootmgr}', 'bootsequence', ('{'+$g+'}')))

Set-Content 'C:\Users\Public\backupRestore-package\bcd-winre.txt' -Value $out
Write-Output $out
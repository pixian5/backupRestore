# probe-scope.ps1 - ASCII only. Minimal repro of $script: variables inside EnumWindows callback.
Add-Type @'
using System;
using System.Runtime.InteropServices;
public class SCP {
    [DllImport("user32.dll")] public static extern bool EnumWindows(EnumProc cb, IntPtr l);
    public delegate bool EnumProc(IntPtr h, IntPtr l);
}
'@

# Style A: $script: prefixed assignment
$script:listA = New-Object System.Collections.ArrayList
# Style B: plain top-level assignment (probe-window2 style, proven to work)
$listB = New-Object System.Collections.ArrayList
$count = 0

$proc = [SCP+EnumProc]{
    param($h, $l)
    $script:count = $script:count + 1
    try { [void]$script:listA.Add($h) } catch { Write-Output ("A-throw: " + $_.Exception.Message) }
    try { [void]$script:listB.Add($h) } catch { Write-Output ("B-throw: " + $_.Exception.Message) }
    return $true
}
[void][SCP]::EnumWindows($proc, [IntPtr]::Zero)
Write-Output ("count=" + $count)
Write-Output ("listA-count=" + $listA.Count)
Write-Output ("listB-count=" + $listB.Count)

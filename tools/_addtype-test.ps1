$out = New-Object System.Collections.ArrayList
try {
    Add-Type -TypeDefinition 'public class Z { public static int Q(){ return 42; } }' -Language CSharp
    [void]$out.Add("ADDTYPE_OK " + [Z]::Q())
} catch {
    [void]$out.Add("ADDTYPE_FAIL: " + $_.Exception.Message)
}
try {
    [void]$out.Add("TEMP=" + [System.Environment]::GetEnvironmentVariable('TEMP'))
} catch {
    [void]$out.Add("TEMP_FAIL: " + $_.Exception.Message)
}
[void]$out.Add("PSVER=" + $PSVersionTable.PSVersion.ToString())
$out | Out-File -LiteralPath 'H:\brwork\_at-out.txt' -Encoding ascii

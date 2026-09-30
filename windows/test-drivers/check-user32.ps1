Add-Type -TypeDefinition @"
using System;
using System.Runtime.InteropServices;
public class WinTest {
    [DllImport("kernel32.dll")] public static extern IntPtr GetProcAddress(IntPtr hModule, string procName);
    [DllImport("kernel32.dll")] public static extern IntPtr GetModuleHandle(string lpModuleName);
}
"@
$user32 = [WinTest]::GetModuleHandle("user32.dll")
Write-Output ("GetWindowLongPtrW=" + [WinTest]::GetProcAddress($user32, "GetWindowLongPtrW"))
Write-Output ("SetWindowLongPtrW=" + [WinTest]::GetProcAddress($user32, "SetWindowLongPtrW"))
Write-Output ("GetWindowLongW=" + [WinTest]::GetProcAddress($user32, "GetWindowLongW"))
Write-Output ("SetWindowLongW=" + [WinTest]::GetProcAddress($user32, "SetWindowLongW"))

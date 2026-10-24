# br-task-audit.ps1 -- ASCII-only summary of every task's status + whether a one-shot
# bootsequence is currently armed on {bootmgr}. Run before/after a reboot.
$out = "C:\Users\Public\task-audit.txt"
"=== H:\brwork\tasks ===" | Out-File $out -Encoding UTF8
Get-ChildItem 'H:\brwork\tasks' -Directory -ErrorAction SilentlyContinue | ForEach-Object {
    $p = Join-Path $_.FullName 'status.json'
    if (Test-Path $p) {
        try {
            $j = Get-Content $p -Raw -Encoding UTF8 | ConvertFrom-Json
            ("{0}  op={1}  stage={2}  progress={3}" -f $_.Name, $j.operation, $j.stage, $j.progress) | Out-File $out -Append -Encoding UTF8
        } catch { ("{0}  BAD_JSON" -f $_.Name) | Out-File $out -Append -Encoding UTF8 }
    } else { ("{0}  NO_STATUS" -f $_.Name) | Out-File $out -Append -Encoding UTF8 }
}
"=== H:\brwork\tasks ===" | Out-File $out -Append -Encoding UTF8
Get-ChildItem 'H:\brwork\tasks' -Directory -ErrorAction SilentlyContinue | ForEach-Object {
    $p = Join-Path $_.FullName 'status.json'
    if (Test-Path $p) {
        try {
            $j = Get-Content $p -Raw -Encoding UTF8 | ConvertFrom-Json
            ("{0}  op={1}  stage={2}  progress={3}" -f $_.Name, $j.operation, $j.stage, $j.progress) | Out-File $out -Append -Encoding UTF8
        } catch { ("{0}  BAD_JSON" -f $_.Name) | Out-File $out -Append -Encoding UTF8 }
    } else { ("{0}  NO_STATUS" -f $_.Name) | Out-File $out -Append -Encoding UTF8 }
}
$bs = (& bcdedit /enum '{bootmgr}') -join "`n"
if ($bs -match 'bootsequence') { "BOOTSEQUENCE: ARMED" | Out-File $out -Append -Encoding UTF8 } else { "BOOTSEQUENCE: none" | Out-File $out -Append -Encoding UTF8 }
"=== bcd object count ===" | Out-File $out -Append -Encoding UTF8
$all = (& bcdedit /enum all) -join "`n"
$n = ([regex]::Matches($all, '(?m)^[^\s-].*\r?\n-{5,}')).Count
("objects=" + $n) | Out-File $out -Append -Encoding UTF8
Copy-Item $out '\\Mac\backupRestore\.test-artifacts\e2e3\task-audit.txt' -Force
Write-Output "done"

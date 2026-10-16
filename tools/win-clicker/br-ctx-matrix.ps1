# br-ctx-matrix.ps1 - context matrix probe: why does reagentc /info fail with exit 9
# English-only output on purpose (avoids GBK decode issues across contexts).
# Read-only: no registration/enable/disable/setreimage calls.

$ErrorActionPreference = 'Continue'
function SEC($n) { Write-Output ("=" * 62); Write-Output "== $n" }
function P($n, $v) { Write-Output ("{0,-44}: {1}" -f $n, $v) }

SEC "CONTEXT"
P "timestamp" (Get-Date -Format "yyyy-MM-dd HH:mm:ss")
P "whoami" (& whoami.exe)
P "sessionId" (Get-Process -Id $PID).SessionId
P "pid" $PID
$il = ((& whoami.exe /groups | Select-String "Mandatory Level") -join " | ")
P "integrity" $il
P "isAdminRole" ([Security.Principal.WindowsPrincipal][Security.Principal.WindowsIdentity]::GetCurrent()).IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)
P "cwd" (Get-Location).Path
P "parentCmdline" ((Get-CimInstance Win32_Process -Filter "ProcessId=$PID").CommandLine)

SEC "VERSIONS"
$nb = (Get-ItemProperty "HKLM:\SOFTWARE\Microsoft\Windows NT\CurrentVersion")
P "OS registry build" ($nb.CurrentBuildNumber + "." + $nb.UBR + " " + $nb.ProductName)
foreach ($f in @(
    "C:\Windows\System32\reagentc.exe",
    "C:\Windows\System32\dism.exe",
    "C:\Windows\System32\cmd.exe",
    "C:\Windows\System32\kernel32.dll",
    "C:\Windows\System32\ntoskrnl.exe",
    "C:\Windows\System32\winre.wim",
    "C:\Windows\System32\Recovery\Winre.wim"
)) {
    if (Test-Path -LiteralPath $f) {
        P ("file " + $f) ((Get-Item -LiteralPath $f).VersionInfo.FileVersion)
    } else { P ("file " + $f) "MISSING" }
}

SEC "WINRE WIM FILES ON DISK"
foreach ($d in @("C:\Recovery\WindowsRE", "C:\Windows\System32\Recovery")) {
    if (Test-Path -LiteralPath $d) {
        Get-ChildItem -LiteralPath $d -Force | ForEach-Object {
            P ($d + "\" + $_.Name) ($_.Length.ToString() + " bytes  " + $_.LastWriteTime.ToString("yyyy-MM-dd HH:mm:ss") + "  attrs=" + $_.Attributes)
        }
    } else { P $d "MISSING" }
}

$wim = "C:\Recovery\WindowsRE\Winre.wim"
if (Test-Path -LiteralPath $wim) {
    try { $fs = [System.IO.File]::OpenRead($wim); $fs.Close(); P "openread $wim" "OK" }
    catch { P "openread $wim" ("FAIL: " + $_.Exception.Message) }
    try {
        $acl = Get-Acl -LiteralPath $wim
        P "acl $wim" (($acl.Access | ForEach-Object { $_.IdentityReference.ToString() + ":" + $_.FileSystemRights.ToString() + "/" + $_.AccessControlType }) -join "; ")
    } catch { P "acl" ("FAIL: " + $_.Exception.Message) }
}

SEC "SERVICING STATE"
$ti = Get-Service TrustedInstaller -ErrorAction SilentlyContinue
P "TrustedInstaller" ($ti.Status.ToString() + "/" + $ti.StartType.ToString())
P "TiWorker procs" (@(Get-Process TiWorker -ErrorAction SilentlyContinue).Count)
P "pending.xml exists" (Test-Path -LiteralPath "C:\Windows\WinSxS\pending.xml")
try {
    $cbs = Get-ItemProperty "HKLM:\SOFTWARE\Microsoft\Windows\CurrentVersion\Component Based Servicing" -ErrorAction Stop
    P "CBS RebootPending" $cbs.RebootPending
} catch { P "CBS RebootPending" "key/value absent" }

SEC "REAGENTC /info (plain, cwd=System32)"
Set-Location "C:\Windows\System32"
$out = & cmd.exe /c "reagentc /info 2>&1"
P "reagentc /info EXIT" $LASTEXITCODE
$out | ForEach-Object { Write-Output ("  R| " + $_) }

SEC "REAGENTC /info /logpath (fresh log)"
$lp = "C:\Users\Public\pkg\ctx-reagent-" + $PID + ".log"
Remove-Item -LiteralPath $lp -ErrorAction SilentlyContinue
$out2 = & cmd.exe /c ("reagentc /info /logpath " + $lp + " 2>&1")
P "reagentc /info /logpath EXIT" $LASTEXITCODE
if (Test-Path -LiteralPath $lp) {
    P "logpath size" ((Get-Item -LiteralPath $lp).Length.ToString() + " bytes")
    Get-Content -LiteralPath $lp -Tail 25 | ForEach-Object { Write-Output ("  L| " + $_) }
} else { P "logpath" "LOG NOT CREATED" }

SEC "DISM CLI /Get-WimInfo on registered wim"
$d1 = & cmd.exe /c ("dism /Get-WimInfo /WimFile:" + $wim + " /Index:1 2>&1")
P "dism get-wiminfo EXIT" $LASTEXITCODE
$d1 | Select-Object -First 14 | ForEach-Object { Write-Output ("  D| " + $_) }

SEC "DISM /Online /Get-CurrentEdition"
$d2 = & cmd.exe /c "dism /Online /Get-CurrentEdition 2>&1"
P "dism online EXIT" $LASTEXITCODE
$d2 | ForEach-Object { Write-Output ("  O| " + $_) }

SEC "DISM STORE LOG TAIL (C:\Windows\Logs\DISM\dism.log)"
$dl = "C:\Windows\Logs\DISM\dism.log"
if (Test-Path -LiteralPath $dl) {
    Get-Content -LiteralPath $dl -Tail 45 | ForEach-Object { Write-Output ("  M| " + $_) }
} else { P "dism.log" "MISSING" }

Write-Output "MATRIX_DONE"

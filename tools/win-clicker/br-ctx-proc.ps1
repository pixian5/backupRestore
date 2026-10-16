# br-ctx-proc.ps1 - probe #3: process inventory, file identity, V2 task logs
$ErrorActionPreference = 'Continue'
function P($n, $v) { Write-Output ("{0,-56}: {1}" -f $n, $v) }
function SEC($n) { Write-Output ("=" * 62); Write-Output "== $n" }

SEC "PROCESS INVENTORY"
$procs = Get-CimInstance Win32_Process |
    Where-Object { $_.Name -match "dism|reagent|TiWorker|BackupRestore|TrustedInstaller|wpeutil|conhost|TiWorker" -or $_.CreationDate -gt (Get-Date "2026-10-16 09:00") }
foreach ($p in ($procs | Sort-Object CreationDate)) {
    P ("pid=" + $p.ProcessId + " " + $p.Name + " @" + $p.CreationDate.ToString("HH:mm:ss")) ($p.CommandLine)
}

SEC "FILE IDENTITY: direct vs SysWOW64 vs UNC"
foreach ($f in @(
    "C:\Windows\System32\reagentc.exe",
    "C:\Windows\SysWOW64\reagentc.exe",
    "\\localhost\C$\Windows\System32\reagentc.exe"
)) {
    try {
        $i = Get-Item -LiteralPath $f
        P ("meta " + $f) ("len=" + $i.Length + " created=" + $i.CreationTime.ToString("MM-dd HH:mm:ss") + " modified=" + $i.LastWriteTime.ToString("MM-dd HH:mm:ss") + " ver=" + $i.VersionInfo.FileVersion)
        P ("hash " + $f) (Get-FileHash -LiteralPath $f -Algorithm SHA256).Hash
    } catch { P ("meta " + $f) ("FAIL: " + $_.Exception.Message) }
}
& cmd.exe /c "fsutil file queryfileid C:\Windows\System32\reagentc.exe" 2>&1
& cmd.exe /c "fsutil file queryfileid \\localhost\C$\Windows\System32\reagentc.exe" 2>&1

SEC "FILTER DRIVERS"
& cmd.exe /c "fltmc" 2>&1

SEC "DISM MOUNTED IMAGES"
& cmd.exe /c "dism /Get-MountedImageInfo" 2>&1

SEC "TODAY'S TASK DIRS (C:\Users\Public + H:\brwork)"
foreach ($root in @("C:\Users\Public", "H:\brwork")) {
    Get-ChildItem $root -Directory -ErrorAction SilentlyContinue |
        Where-Object { $_.LastWriteTime -gt (Get-Date "2026-10-16 09:00") } |
        ForEach-Object { P ("dir " + $_.FullName) ($_.LastWriteTime.ToString("HH:mm:ss")) }
}

SEC "COPY V2-ERA LOGS TO HOST SHARE"
$dst = "\\Mac\backupRestore\tools\win-clicker\_s1"
$targets = @()
foreach ($root in @("C:\Users\Public", "H:\brwork")) {
    $targets += Get-ChildItem $root -Recurse -Include "launcher-errors.log", "recovery.log" -ErrorAction SilentlyContinue |
        Where-Object { $_.LastWriteTime -gt (Get-Date "2026-10-16 09:30") }
}
foreach ($t in $targets) {
    $safe = ($t.FullName -replace '[\\/:]', '_')
    Copy-Item -LiteralPath $t.FullName -Destination (Join-Path $dst $safe) -Force -ErrorAction SilentlyContinue
    P "copied" ($t.FullName + " -> " + $safe)
}
Write-Output "PROBE3_DONE"

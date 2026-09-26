# br-lockers.ps1 - read-only identification of which process/service holds the
# exclusive lock on a file, using the Windows Restart Manager API (no downloads).
# ASCII only. Output: C:\Users\Public\pkg\lockers.txt (UTF-8 no BOM)
param([string]$Path = "T:\Mac disk")
$ErrorActionPreference = 'Continue'
$l = New-Object System.Collections.Generic.List[string]
function W($s) { [void]$l.Add([string]$s) }
function Flush {
  [System.IO.File]::WriteAllText("C:\Users\Public\pkg\lockers.txt", ($l -join "`r`n"), (New-Object System.Text.UTF8Encoding($false)))
}

W ("TIME=" + (Get-Date -Format "yyyy-MM-dd HH:mm:ss"))
W ("TARGET=" + $Path)
W ""

$cs = @'
using System;
using System.Runtime.InteropServices;
using System.Runtime.InteropServices.ComTypes;

public static class BrRM {
    [StructLayout(LayoutKind.Sequential)]
    public struct RM_UNIQUE_PROCESS {
        public int dwProcessId;
        public System.Runtime.InteropServices.ComTypes.FILETIME ProcessStartTime;
    }
    [StructLayout(LayoutKind.Sequential, CharSet = CharSet.Unicode)]
    public struct RM_PROCESS_INFO {
        public RM_UNIQUE_PROCESS Process;
        [MarshalAs(UnmanagedType.ByValTStr, SizeConst = 256)]
        public string strAppName;
        [MarshalAs(UnmanagedType.ByValTStr, SizeConst = 64)]
        public string strServiceShortName;
        public int ApplicationType;
        public uint AppStatus;
        public uint TSSessionId;
        [MarshalAs(UnmanagedType.Bool)] public bool bRestartable;
    }

    [DllImport("rstrtmgr.dll", CharSet = CharSet.Unicode)]
    public static extern int RmStartSession(out uint pSessionHandle, int dwSessionFlags, string strSessionKey);

    [DllImport("rstrtmgr.dll")]
    public static extern int RmEndSession(uint pSessionHandle);

    [DllImport("rstrtmgr.dll", CharSet = CharSet.Unicode)]
    public static extern int RmRegisterResources(uint pSessionHandle,
        uint nFiles, string[] rgsFilenames,
        uint nApplications, RM_UNIQUE_PROCESS[] rgApplications,
        uint nServices, string[] rgsServiceNames);

    [DllImport("rstrtmgr.dll")]
    public static extern int RmGetList(uint dwSessionHandle, out uint pnProcInfoNeeded,
        ref uint pnProcInfo, [In, Out] RM_PROCESS_INFO[] rgAffectedApps,
        ref uint lpdwRebootReasons);
}
'@

$added = $false
try {
  Add-Type -TypeDefinition $cs -ErrorAction Stop
  $added = $true
  W "ADDTYPE_OK"
} catch {
  W ("ADDTYPE_FAIL " + $_.Exception.GetType().Name + " :: " + $_.Exception.Message)
  if ($_.Exception.InnerException) { W ("  inner: " + $_.Exception.InnerException.Message) }
}

if ($added) {
  $key = [Guid]::NewGuid().ToString()
  $handle = [uint32]0
  $rc = -1
  try { $rc = [BrRM]::RmStartSession([ref]$handle, 0, $key) } catch { W ("RmStartSession EX " + $_.Exception.Message) }
  W ("RmStartSession rc=" + $rc + " handle=" + $handle)
  if ($rc -eq 0) {
    $files = New-Object 'System.String[]' 1
    $files[0] = $Path
    try { $rc = [BrRM]::RmRegisterResources($handle, [uint32]1, $files, [uint32]0, $null, [uint32]0, $null) }
    catch { W ("RmRegisterResources EX " + $_.Exception.Message) }
    W ("RmRegisterResources rc=" + $rc)

    $needed = [uint32]0; $count = [uint32]0; $reason = [uint32]0
    $arr = $null
    try {
      $rc = [BrRM]::RmGetList($handle, [ref]$needed, [ref]$count, $null, [ref]$reason)
    } catch { W ("RmGetList EX " + $_.Exception.Message) }
    W ("RmGetList(probe) rc=" + $rc + " needed=" + $needed)

    if ($needed -gt 0) {
      $arr = New-Object 'BrRM+RM_PROCESS_INFO[]' ([int]$needed)
      $count = $needed
      try { $rc = [BrRM]::RmGetList($handle, [ref]$needed, [ref]$count, $arr, [ref]$reason) }
      catch { W ("RmGetList2 EX " + $_.Exception.Message) }
      W ("RmGetList(fill) rc=" + $rc + " count=" + $count + " rebootReasons=" + $reason)
      W ""
      W "== LOCK HOLDERS =="
      for ($i = 0; $i -lt [int]$count; $i++) {
        $p = $arr[$i]
        $procPath = "?"
        try { $procPath = (Get-Process -Id $p.Process.dwProcessId -ErrorAction Stop).Path } catch {}
        W ("  app=[" + $p.strAppName + "] pid=" + $p.Process.dwProcessId + " svc=[" + $p.strServiceShortName + "] type=" + $p.ApplicationType + " session=" + $p.TSSessionId + " restartable=" + $p.bRestartable + " path=" + $procPath)
      }
    } else {
      W ""
      W "NO_LOCKER_REPORTED (Restart Manager located no process holding this file)"
    }
    try { [void][BrRM]::RmEndSession($handle) } catch {}
  }
}

W ""
W "== END =="
Flush
Write-Output "LOCKERS_DONE"
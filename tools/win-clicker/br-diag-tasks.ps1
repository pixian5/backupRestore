# br-diag-tasks.ps1 -- 检查 rbrun 计划任务定义与运行历史
$ErrorActionPreference = 'Continue'
foreach ($n in @("rbrun","rbrun2","rbrun3")) {
  "=== task: $n ==="
  $t = Get-ScheduledTask -TaskName $n -ErrorAction SilentlyContinue
  if (-not $t) { "NOT FOUND"; continue }
  "path=" + $t.TaskPath + " state=" + $t.State
  "--- triggers ---"
  $t.Triggers | ForEach-Object { $_.CimClass.CimClassName + " : " + ($_ | ConvertTo-Json -Compress -Depth 2) }
  "--- actions ---"
  $t.Actions | ForEach-Object { "exec=" + $_.Execute + " args=" + $_.Arguments }
  "--- principal ---"
  "user=" + $t.Principal.UserId + " runlevel=" + $t.Principal.RunLevel + " logon=" + $t.Principal.LogonType
  "--- info ---"
  $i = Get-ScheduledTaskInfo -TaskName $n -ErrorAction SilentlyContinue
  if ($i) { "lastRun=" + $i.LastRunTime + " lastResult=" + $i.LastTaskResult + " nextRun=" + $i.NextRunTime + "numMissed=" + $i.NumberOfMissedRuns }
}

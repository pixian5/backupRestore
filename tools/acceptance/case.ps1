param([ValidateSet('prepare','status','retry','resume','verify','collect')][string]$Action,[string]$Config)
$ErrorActionPreference='Stop'
$c=Get-Content -Encoding UTF8 -LiteralPath $Config -Raw | ConvertFrom-Json
function Partition($guid){
 $found=@(Get-Partition | Where-Object {$_.Guid -eq $guid})
 if($found.Count -ne 1 -or !$found[0].DriveLetter){throw "Partition missing or ambiguous: $guid"}
 return $found[0]
}
$w=Partition $c.workspaceGuid
$t=Partition $c.targetGuid
$r="$($w.DriveLetter):\$($c.rootRelative)"
$app="$r\app"
$exe="$app\BackupRestore.exe"
function Run($arguments,$name){
 $p=Start-Process $exe -ArgumentList $arguments -Wait -PassThru -RedirectStandardOutput "$r\$name.out" -RedirectStandardError "$r\$name.err"
 return $p.ExitCode
}
function TaskDir {
 $dirs=@(Get-ChildItem "$app\tasks" -Directory)
 if($dirs.Count -ne 1){throw 'Expected exactly one task'}
 return $dirs[0].FullName
}
if($Action -eq 'prepare'){
 if(Test-Path $r){throw 'Fresh case directory required'}
 if($t.Guid -eq (Get-Partition -DriveLetter ([char]$env:SystemDrive[0])).Guid){throw 'This runner is restricted to test volumes'}
 New-Item -ItemType Directory "$r\app","$r\images" | Out-Null
 $build='\\Mac\backupRestore\target\aarch64-pc-windows-msvc\release\BackupRestore.exe'
 foreach($name in @('BackupRestore.exe','Recovery.exe')){Copy-Item $build "$app\$name";if((Get-FileHash "$app\$name").Hash -ne $c.exeSha256){throw 'Build hash mismatch'}}
 if($c.systemImage){
  $i=Partition $c.imageGuid;$image="$($i.DriveLetter):\BRRE-217-20261008\system.wim";$source=$env:SystemDrive.TrimEnd(':')
  foreach($name in @('Recovery.log','Recovery-early.log','Recovery.reagent.log')){
   $old=Join-Path (Split-Path $image) $name
   if(Test-Path $old){Move-Item $old "$r\previous-$name"}
  }
 }
 else{Copy-Item "$($w.DriveLetter):\BRRE-213-20261005\images\test.wim" "$r\images\test.wim";$image="$r\images\test.wim";$source=[string]$t.DriveLetter}
 & bcdedit /enum all /v | Out-File -Encoding UTF8 "$r\bcd-before.txt"
 & reagentc /info | Out-File -Encoding UTF8 "$r\winre-before.txt"
 Get-FileHash "$env:SystemDrive\Recovery\WindowsRE\Winre.wim","$env:SystemDrive\Recovery\WindowsRE\boot.sdi" | ConvertTo-Json | Set-Content -Encoding UTF8 "$r\originals.json"
 [IO.File]::WriteAllText("$($t.DriveLetter):\br219-must-disappear.txt",'format acceptance')
 $operation=if($c.systemImage){'create-secondary'}else{'restore-existing'}
 $arguments=@('prepare','--operation',$operation,'--source-drive',$source,'--target-drive',[string]$t.DriveLetter,'--image-path',$image,'--verify-hash','--test-fault',$c.fault,'--allow-destructive')
 if(!$c.boot){$arguments+='--no-reboot'}
 $p=Start-Process $exe -ArgumentList $arguments -PassThru -RedirectStandardOutput "$r\prepare.out" -RedirectStandardError "$r\prepare.err"
 [pscustomobject]@{pid=$p.Id;root=$r;target=$t.Guid;build=$c.exeSha256} | ConvertTo-Json
 return
}
$dir=TaskDir
$task=Get-Content -Encoding UTF8 "$dir\task.json" -Raw | ConvertFrom-Json
if($task.target.volume.partitionGuid -ne $t.Guid){throw 'Task target identity differs'}
if($Action -eq 'retry'){
 $code=Run @('recover',$app,$task.taskId) ('retry-'+[DateTime]::Now.ToString('HHmmss'))
 [pscustomobject]@{exitCode=$code;status=(Get-Content -Encoding UTF8 "$dir\status.json" -Raw | ConvertFrom-Json)} | ConvertTo-Json -Depth 5
}elseif($Action -eq 'resume'){
 $p=Start-Process $exe -ArgumentList @('resume',$app,$task.taskId) -PassThru -RedirectStandardOutput "$r\resume.out" -RedirectStandardError "$r\resume.err"
 "RESUME_DISPATCHED $($p.Id)"
}elseif($Action -eq 'status'){
 Get-Content -Encoding UTF8 "$dir\status.json" -Raw
 Get-Process BackupRestore,Recovery,dism -ErrorAction SilentlyContinue | Select-Object Id,ProcessName
 Get-Content -Encoding UTF8 "$r\prepare.err" -Tail 4
 Get-ChildItem "$dir\fault-*.json" -ErrorAction SilentlyContinue | ForEach-Object {Get-Content -Encoding UTF8 $_.FullName -Raw}
 Get-Content -Encoding UTF8 "$dir\recovery.log" -Tail 5 -ErrorAction SilentlyContinue
}elseif($Action -eq 'verify'){
 $code=Run @('status',$app,$task.taskId) 'status-verified'
 if($code){throw 'Task status read failed'}
 $task=Get-Content -Encoding UTF8 "$dir\task.json" -Raw | ConvertFrom-Json
 if($task.status -ne 'success'){throw "Task not successful: $($task.status)"}
 if(Test-Path "$($t.DriveLetter):\br219-must-disappear.txt"){throw 'Format sentinel remains'}
 if(!$c.systemImage){
  $files=Get-Content -Encoding UTF8 "$($w.DriveLetter):\BRRE-213-20261005\before\files.json" -Raw | ConvertFrom-Json
  foreach($file in $files){if((Get-FileHash "$($t.DriveLetter):\$($file.path)").Hash -ne $file.sha256){throw 'Fixture hash mismatch'}}
  $before=[IO.File]::ReadAllText("$r\bcd-before.txt").Trim()
  $after=(& bcdedit /enum all /v | Out-String).Trim()
  if($before -cne $after){throw 'Primary BCD changed'}
 }
 $originals=Get-Content -Encoding UTF8 "$r\originals.json" -Raw | ConvertFrom-Json
 foreach($f in $originals){if((Get-FileHash $f.Path).Hash -ne $f.Hash){throw 'Primary recovery original changed'}}
 $info=(& reagentc /info | Out-String).Trim()
 if($info -cne [IO.File]::ReadAllText("$r\winre-before.txt").Trim()){throw 'Primary recovery registration changed'}
 if($c.boot -and (Test-Path "$($w.DriveLetter):\BackupRestoreRE") -and !$c.systemImage){throw 'Task staging remains'}
 $result=[pscustomobject]@{task=$task.taskId;status=$task.status;target=$t.Guid;fixturesVerified=$true;primaryRecoveryUnchanged=$true;build=$c.exeSha256}
 $result | ConvertTo-Json | Set-Content -Encoding UTF8 "$r\verified.json"
 $result | ConvertTo-Json
}
# 只收集本阶段的小型日志与状态，所有交付由宿主压缩。
if($Action -in @('collect','verify','status','retry')){
 foreach($file in Get-ChildItem $r -Recurse -File | Where-Object {$_.Extension -in @('.json','.log','.out','.err','.txt','.env') -and $_.Length -lt 5MB}){
  $dest=Join-Path "$($c.evidence)\guest" $file.FullName.Substring($r.Length+1)
  New-Item -ItemType Directory -Force (Split-Path $dest) | Out-Null
  Copy-Item -LiteralPath $file.FullName $dest -Force
 }
}

if($c.systemImage -and $Action -in @('collect','verify','status')){
 $i=Partition $c.imageGuid
 foreach($name in @('Recovery.log','Recovery-early.log','Recovery.reagent.log')){
  $path="$($i.DriveLetter):\BRRE-217-20261008\$name"
  if(Test-Path $path){Copy-Item $path "$($c.evidence)\$name" -Force}
 }
}

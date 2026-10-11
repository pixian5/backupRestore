param([ValidateSet('created','booted')][string]$Action,[string]$Config,[string]$Baseline)
$ErrorActionPreference='Stop'
$c=Get-Content -Encoding UTF8 $Config -Raw | ConvertFrom-Json
function Partition($guid){
 $p=@(Get-Partition | Where-Object {$_.Guid -eq $guid})
 if($p.Count -ne 1 -or !$p[0].DriveLetter){throw "Partition identity unavailable: $guid"}
 return $p[0]
}
function Blocks($text){
 $d=@{}
 foreach($b in ($text.Replace("`r`n","`n") -split "`n`n")){
  $m=[regex]::Match($b,'(?m)^(?:identifier|标识符)\s+(\{[0-9a-fA-F-]{36}\})')
  if($m.Success){$d[$m.Groups[1].Value]=$b.Trim()}
 }
 return $d
}
function Field($block,$key){
 $m=[regex]::Match($block,'(?m)^'+$key+'\s+([^\r\n]+)')
 if(!$m.Success){throw "Missing BCD field: $key"}
 return $m.Groups[1].Value.Trim()
}
$w=Partition $c.workspaceGuid;$target=Partition $c.targetGuid
$primary=Partition '{761230e8-107c-4396-8c37-82273720183d}'
$r="$($w.DriveLetter):\$($c.rootRelative)"
$td=@(Get-ChildItem "$r\app\tasks" -Directory)
if($td.Count -ne 1){throw 'Ambiguous task'}
$task=Get-Content -Encoding UTF8 "$($td[0].FullName)\task.json" -Raw | ConvertFrom-Json
if($task.status -ne 'success' -or $task.target.volume.partitionGuid -ne $target.Guid){throw 'Task not successfully restored to expected target'}
# 基线来自已验证的同一系统镜像；同时检查主系统和第二系统的关键文件。
foreach($f in (Get-Content -Encoding UTF8 "$Baseline\primary-before-secondary.json" -Raw | ConvertFrom-Json)){
 foreach($letter in @($primary.DriveLetter,$target.DriveLetter)){
  if((Get-FileHash ("${letter}:\"+$f.Path.Substring(3))).Hash -ne $f.Hash){throw 'System or recovery asset hash mismatch'}
 }
}
foreach($f in (Get-Content -Encoding UTF8 "$Baseline\fixture.json" -Raw | ConvertFrom-Json)){
 $path="$($target.DriveLetter):\$($f.path)"
 if((Get-Item $path).Length -ne $f.size -or (Get-FileHash $path).Hash -ne $f.sha256){throw 'Restored fixture mismatch'}
}
if(Test-Path "$($target.DriveLetter):\br219-must-disappear.txt"){throw 'Format sentinel remains'}
$bcd=(& bcdedit /enum all /v | Out-String)
if($LASTEXITCODE){throw 'BCD enumeration failed'}
$new=Blocks $bcd
$bootmgr='{9dea862c-5cdd-4e70-acc1-f32b344d4795}'
$loaders=@($new.Keys | Where-Object {$new[$_] -match "(?m)^device\s+partition=$($target.DriveLetter):\s*$" -and $new[$_] -match '(?im)^path\s+\\Windows\\system32\\winload.efi\s*$'})
if($loaders.Count -ne 1){throw 'Secondary loader ambiguous'}
$id=$loaders[0];$reid=Field $new[$id] 'recoverysequence'
$default=Field $new[$bootmgr] 'default'
if($reid -eq (Field $new[$default] 'recoverysequence')){throw 'Secondary recovery points to primary recovery'}
if(!(Field $new[$reid] 'device').StartsWith("ramdisk=[$($target.DriveLetter):]\Recovery\WindowsRE\Winre.wim,",[StringComparison]::OrdinalIgnoreCase)){throw 'Secondary recovery volume mismatch'}
$entry=Get-Content -Encoding UTF8 "$($td[0].FullName)\boot-entry.json" -Raw | ConvertFrom-Json
if($bcd.Contains($entry.loaderGuid) -or $bcd.Contains($entry.devoptsGuid) -or $bcd -match '(?m)^bootsequence\s'){throw 'Temporary boot objects or request remain'}
$os=Get-CimInstance Win32_OperatingSystem
$current=Partition (Get-Partition -DriveLetter ([char]$os.SystemDrive[0])).Guid
if($Action -eq 'created'){
 if($current.Guid -ne $primary.Guid){throw 'Creation verification requires primary Windows'}
 $old=Blocks ([IO.File]::ReadAllText("$r\bcd-before.txt"))
 $primaryId=Field $old[$bootmgr] 'default'
 $primaryRe=Field $old[$primaryId] 'recoverysequence'
 foreach($protected in @($primaryId,$primaryRe)){
  if($old[$protected] -cne $new[$protected]){throw 'Primary BCD object changed'}
 }
 foreach($key in @('default','timeout')){
  $beforeValue = Field $old[$bootmgr] $key
  $afterValue = Field $new[$bootmgr] $key
  if($beforeValue -ne $afterValue){throw "Primary Boot Manager $key changed: $beforeValue -> $afterValue"}
}
# resumeobject 在旧 BCD 中可能不存在；两边必须保持同样的存在性和值，不能用 Field 强制虚构。
$beforeResume = if($old[$bootmgr] -match '(?im)^resumeobject\s+') {Field $old[$bootmgr] 'resumeobject'} else {$null}
$afterResume = if($new[$bootmgr] -match '(?im)^resumeobject\s+') {Field $new[$bootmgr] 'resumeobject'} else {$null}
if($beforeResume -ne $afterResume){throw "Primary Boot Manager resumeobject changed: $beforeResume -> $afterResume"}
 function Order($block){
  $m=[regex]::Match($block,'(?ms)^displayorder\s+(.*?)(?=^\S|\z)')
  return @([regex]::Matches($m.Groups[1].Value,'\{[0-9a-fA-F-]{36}\}') | ForEach-Object {$_.Value})
 }
 $expected=@(Order $old[$bootmgr] | Where-Object {$new.ContainsKey($_)})
 if($expected -notcontains $id){$expected+=$id}
 if(((Order $new[$bootmgr]) -join ',') -cne ($expected -join ',')){throw 'Existing menu ordering changed'}
 $info=(& reagentc /info | Out-String)
 if($LASTEXITCODE -or $info.Trim() -cne [IO.File]::ReadAllText("$r\winre-before.txt").Trim()){throw 'Primary WinRE registration changed'}
}else{
 if($current.Guid -ne $target.Guid -or $os.SystemDrive -ne 'C:'){throw 'Secondary Windows did not boot as C:'}
 $active=(& bcdedit /enum '{current}' /v | Out-String)
 $info=(& reagentc /info | Out-String)
 if(!$active.Contains($id) -or $info -notmatch 'Enabled' -or !$info.Contains($reid.Trim('{}'))){throw 'Secondary boot or independent recovery registration mismatch'}
 if(@(Get-Service EventLog,Winmgmt,Schedule | Where-Object Status -ne 'Running').Count -or !(Get-Process explorer -ErrorAction SilentlyContinue)){throw 'Secondary services or desktop unavailable'}
}
$result=[pscustomobject]@{task=$task.taskId;stage=$task.status;target=$target.Guid;secondaryLoader=$id;secondaryRecovery=$reid;primaryLoader=$default;filesVerified=$true;independentRecovery=$true;currentPartition=$current.Guid;bootTime=$os.LastBootUpTime;action=$Action}
$result | ConvertTo-Json | Set-Content -Encoding UTF8 "$($c.evidence)\system-$Action.json"
$bcd | Out-File -Encoding UTF8 "$($c.evidence)\bcd-$Action.txt"
$info | Out-File -Encoding UTF8 "$($c.evidence)\winre-$Action.txt"
$result | ConvertTo-Json

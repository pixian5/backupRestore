param([Parameter(Mandatory=$true)][string]$Root)
$ErrorActionPreference='Stop'
$r=$Root;$app="$r\app";$dir=(Get-ChildItem "$app\tasks" -Directory).FullName
$t=Get-Content -Encoding UTF8 "$dir\task.json" -Raw|ConvertFrom-Json
$exe="$app\BackupRestore.exe";$results=@()
$payload=(Get-FileHash "$dir\payload\Recovery.exe").Hash
function Attempt($name){
 $p=Start-Process $exe -ArgumentList @('recover',$app,$t.taskId) -Wait -PassThru -RedirectStandardOutput "$r\$name.out" -RedirectStandardError "$r\$name.err"
 $status=Get-Content -Encoding UTF8 "$dir\status.json" -Raw|ConvertFrom-Json
 if($p.ExitCode -eq 0 -or $status.stage -ne 'target-erased' -or !$status.error){throw "Failure retention check failed $name"}
 if((Get-FileHash "$dir\payload\Recovery.exe").Hash -ne $payload){throw 'Recovery payload changed'}
 return [pscustomobject]@{case=$name;exit=$p.ExitCode;stage=$status.stage;error=$status.error;payloadPreserved=$true}
}
$image="$r\images\test.wim"
$results+=Attempt 'persistent-third'
Move-Item $image "$image.good"
try{$results+=Attempt 'image-missing'}finally{Move-Item "$image.good" $image}
$lock=[IO.File]::Open($image,[IO.FileMode]::Open,[IO.FileAccess]::ReadWrite,[IO.FileShare]::None)
try{$results+=Attempt 'image-read-sharing-error'}finally{$lock.Dispose()}
Copy-Item $image "$image.good"
try{
 $stream=[IO.File]::OpenWrite($image);$stream.Write([byte[]](0,0,0,0,0,0,0,0),0,8);$stream.Flush($true);$stream.Dispose()
 $results+=Attempt 'image-corrupt-same-size'
}finally{Copy-Item "$image.good" $image -Force;Remove-Item "$image.good"}
[IO.File]::WriteAllText("$dir\fault-released",'fault resolved')
$p=Start-Process $exe -ArgumentList @('recover',$app,$t.taskId) -Wait -PassThru -RedirectStandardOutput "$r\final.out" -RedirectStandardError "$r\final.err"
if($p.ExitCode){throw 'Same-task retry failed after fault release'}
$results | ConvertTo-Json -Depth 5 | Set-Content -Encoding UTF8 "$r\negative-verified.json"
$results | ConvertTo-Json -Depth 5

param([string]$Manifest,[string]$Evidence)
$ErrorActionPreference='Stop'
$workspace=@(Get-Partition | Where-Object {$_.Guid -eq '{0e68475f-4890-43f3-ab85-75e8339116ac}'})
if($workspace.Count -ne 1 -or !$workspace[0].DriveLetter){throw 'Workspace identity missing'}
$dest="$($workspace[0].DriveLetter):\BRRE-219-20261009\native-final"
New-Item -ItemType Directory -Force $dest | Out-Null
# 先复制到虚拟机本地卷，再串行运行默认安全测试；破坏性夹具仍默认忽略。
foreach($source in (Get-Content -Encoding UTF8 $Manifest -Raw | ConvertFrom-Json)){
 $name=Split-Path $source -Leaf
 $binary=Join-Path $dest $name
 Copy-Item -LiteralPath $source -Destination $binary -Force
 if((Get-FileHash $source).Hash -ne (Get-FileHash $binary).Hash){throw 'Test binary hash mismatch'}
 $p=Start-Process $binary -ArgumentList @('--test-threads=1') -Wait -PassThru -RedirectStandardOutput "$binary.out" -RedirectStandardError "$binary.err"
 Copy-Item "$binary.out","$binary.err" $Evidence -Force
 Get-Content "$binary.out" -Tail 3
 if($p.ExitCode){throw "Native tests failed: $name ($($p.ExitCode))"}
}

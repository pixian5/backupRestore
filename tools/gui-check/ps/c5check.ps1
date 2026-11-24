# Read-only: post-restore state of F:.
param([string]$Tag='x')
[Console]::OutputEncoding=[Text.Encoding]::UTF8
$v = (Get-CimInstance Win32_LogicalDisk -Filter "DeviceID='F:'").VolumeSerialNumber
$serial = if ($v -and $v.Length -eq 8) { $v.Substring(0,4) + '-' + $v.Substring(4) } else { [string]$v }
"CHECK tag=$Tag serial=$serial"
"CHECK sentinel_exists=$(Test-Path "F:\BR-SENTINEL-$Tag.txt")"
"CHECK brdata_exists=$(Test-Path 'F:\BRDATA')"
if (Test-Path 'F:\BRDATA') {
  "CHECK extra_exists=$(Test-Path 'F:\BRDATA\extra-not-in-image.txt')"
  $f1 = 'F:\BRDATA\sub\file1.txt'; "CHECK file1=$(if (Test-Path $f1) { (Get-Content $f1 -Raw).Trim() } else { 'ABSENT' })"
  $f2 = 'F:\BRDATA\file2.txt';     "CHECK file2=$(if (Test-Path $f2) { (Get-Content $f2 -Raw).Trim() } else { 'ABSENT' })"
  $p = 'F:\BRDATA\payload-100MB.bin'
  if (Test-Path $p) { "CHECK payload_len=$((Get-Item $p).Length) payload_sha256=$((Get-FileHash $p -Algorithm SHA256).Hash)" } else { "CHECK payload=ABSENT" }
  "CHECK brdata_file_count=$((Get-ChildItem 'F:\BRDATA' -Recurse -File | Measure-Object).Count)"
}
"__END__"

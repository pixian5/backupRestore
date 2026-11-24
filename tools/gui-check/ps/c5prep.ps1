# Before a data-restore segment on F: put a sentinel and an extra file (not in the image) and change file1.
param([string]$Tag='x')
[Console]::OutputEncoding=[Text.Encoding]::UTF8
[IO.File]::WriteAllText("F:\BR-SENTINEL-$Tag.txt", "sentinel $Tag")
[IO.File]::WriteAllText("F:\BRDATA\extra-not-in-image.txt", "extra $Tag")
[IO.File]::WriteAllText("F:\BRDATA\sub\file1.txt", "BRDATA file1 CHANGED $Tag")
"PREP tag=$Tag sentinel=$(Test-Path "F:\BR-SENTINEL-$Tag.txt") extra=$(Test-Path 'F:\BRDATA\extra-not-in-image.txt') file1=$((Get-Content 'F:\BRDATA\sub\file1.txt' -Raw).Trim())"
"__END__"

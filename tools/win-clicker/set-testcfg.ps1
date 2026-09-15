# set-testcfg.ps1 - write br-test.json for auto backup via test hook
$json = '{"tab":"backup","source_volume":"C","target_volume":"C","image":"E:\\br-cdrive-v1.wim","system_drive_choice":2,"auto_install":true}'
Set-Content -Path 'C:\br-test.json' -Value $json -Encoding ascii
Write-Output (Get-Content -Path 'C:\br-test.json' -Raw)

param([int]$Font = 14, [string]$Lang = 'zh')
# NOTE: keep this file ASCII-only (PowerShell 5 reads BOM-less files as ANSI).
$ErrorActionPreference = 'Continue'
$path = 'H:\brwork\BackupRestore.settings.ini'
$lines = @(
  '# BackupRestore program settings (auto-generated, safe to edit by hand)',
  'failure_threshold=3',
  'failure_action=auto-escape',
  'failure_pause_max_wait_secs=0',
  'dism_timeout_secs=0',
  ('font_size=' + $Font)
)
if ($Lang -eq 'en') { $lines += 'ui_language=en' }
($lines -join "`r`n") | Set-Content -LiteralPath $path -Encoding ASCII
$after = @(Get-Content -LiteralPath $path -Encoding ASCII)
'COMBO font=' + $Font + ' lang=' + $Lang + ' lines=' + $after.Count
'__END__'

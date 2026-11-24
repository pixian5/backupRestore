# Launch the test wrapper as a detached hidden process and return.
# -Filter is passed through only when it is not empty: an empty argument is dropped by command-line parsing and
# PowerShell then fails with "Missing an argument for parameter 'Filter'" -- the test never starts.
param([string]$Tag = 'main', [string]$Filter = '')
$args2 = @('-NoProfile','-ExecutionPolicy','Bypass','-File','\\Mac\backupRestore\tools\vm\ps\test-wrapper.ps1','-Tag',$Tag)
if ($Filter -ne '') { $args2 += @('-Filter', $Filter) }
$p = Start-Process powershell -WindowStyle Hidden -PassThru -ArgumentList $args2
"LAUNCHED pid=$($p.Id)"
"__END__"

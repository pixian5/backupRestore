# Launch the build wrapper as a detached hidden process (so it survives the exec session) and return.
param([string]$Tag = 'main')
$p = Start-Process powershell -WindowStyle Hidden -PassThru -ArgumentList @('-NoProfile','-ExecutionPolicy','Bypass','-File','\\Mac\backupRestore\tools\vm\ps\build-wrapper.ps1','-Tag',$Tag)
"LAUNCHED pid=$($p.Id) tag=$Tag"
"__END__"

# Copy the helper scripts the real-machine cases need into the directory the resident elevated helper runs from.
# One output line per file: STAGED <name> bytes=<n>. The caller (tools/vm/lib/vm.sh: stage_gui_helpers) compares
# every byte count with the file on its own side -- a copy that "succeeds" can still leave an old file behind.
# NOTE: keep this file ASCII-only (PowerShell 5 reads BOM-less files as ANSI).
$src = '\\Mac\backupRestore\tools\gui-check\ps'
$dst = 'H:\brwork\gui-check'
New-Item -ItemType Directory -Force -Path $dst | Out-Null
foreach ($n in $args) {
  Copy-Item -LiteralPath (Join-Path $src $n) -Destination (Join-Path $dst $n) -Force
  $item = Get-Item -LiteralPath (Join-Path $dst $n) -ErrorAction SilentlyContinue
  if ($item) { "STAGED $n bytes=" + $item.Length } else { "STAGED $n bytes=MISSING" }
}
'__END__'

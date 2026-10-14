# BCD orphan audit. Captures bcdedit output directly (no file encoding round-trip).
#   KEEP   - referenced by recoverysequence/resumeobject, or listed anywhere in {bootmgr}
#   ORPHAN - referenced by nothing
$ErrorActionPreference = 'SilentlyContinue'
[Console]::OutputEncoding = [Text.Encoding]::UTF8
$env:BCDEDIT_HIDE_UTF8 = '1'

$t = & bcdedit /enum osloader /v 2>&1
$bm = & bcdedit /enum '{bootmgr}' /v 2>&1

$ids = New-Object System.Collections.Specialized.OrderedDictionary
$refsA = @{}
$refsB = @{}
$cur = $null
foreach ($l in $t) {
  $s = [string]$l
  if ($s -match '(\{[0-9a-fA-F-]{36}\})\s*$' -and $s -notmatch '^\s') {
    $cur = $Matches[1].ToLowerInvariant(); $ids[$cur] = @{ dev = ''; desc = '' }; continue
  }
  if ($cur -eq $null) { continue }
  if ($s -match '^\s*device\s+(.+)$') { $ids[$cur]['dev'] = $Matches[1].Trim() }
  elseif ($s -match '^\s*description\s+(.+)$') { $ids[$cur]['desc'] = $Matches[1].Trim() }
  elseif ($s -match '(recoverysequence|resumeobject)\s+(\{[0-9a-fA-F-]{36}\})') { $refsA[$Matches[2].ToLowerInvariant()] = $true }
}
foreach ($l in $bm) {
  foreach ($m in [regex]::Matches([string]$l, '\{[0-9a-fA-F-]{36}\}')) { $refsB[$m.Value.ToLowerInvariant()] = $true }
}

Write-Output '=== OSLOADER CLASSIFICATION ==='
foreach ($k in $ids.Keys) {
  $tag = 'ORPHAN'
  if ($refsA.ContainsKey($k)) { $tag = 'KEEP-A' }
  elseif ($refsB.ContainsKey($k)) { $tag = 'KEEP-B' }
  $dev = $ids[$k]['dev']; if ($dev.Length -gt 44) { $dev = $dev.Substring(0, 44) }
  Write-Output ('{0,-6} {1}  {2,-44} {3}' -f $tag, $k, $dev, $ids[$k]['desc'])
}
Write-Output ''
Write-Output ('refsA=' + $refsA.Count + ' refsB=' + $refsB.Count)
Write-Output ('refsB set: ' + (($refsB.Keys | Sort-Object) -join ' '))

#!/bin/sh
set -eu

if [ -e windows/BackupRestore.ps1 ] || [ -e windows/Recovery.cmd ]; then
  echo "obsolete PowerShell or batch recovery runtime remains under windows/" >&2
  exit 1
fi

if rg -n -i -F -e 'powershell.exe' -e 'BackupRestore.ps1' \
  crates/backuprestore-cli/src crates/backuprestore-core/src windows/BackupRestore.cmd windows/RecoveryLauncher.cmd windows/winpeshl.ini; then
  echo "product runtime must not invoke PowerShell" >&2
  exit 1
fi

if rg -n -i -e 'Capture-Image|Apply-Image|bcdboot|format ' windows/RecoveryLauncher.cmd windows/winpeshl.ini; then
  echo "WinRE launcher must not contain recovery operations" >&2
  exit 1
fi

cargo fmt --all -- --check
cargo test --workspace --all-targets --offline
git diff --check
echo "runtime boundary audit passed"

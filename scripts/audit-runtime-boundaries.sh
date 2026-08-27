#!/bin/sh
set -eu

if [ -e windows/BackupRestore.ps1 ] || [ -e windows/Recovery.cmd ]; then
  echo "obsolete PowerShell or batch recovery runtime remains under windows/" >&2
  exit 1
fi

if find windows -maxdepth 1 -type f -name '*.cmd' -print | grep -q .; then
  echo "product batch wrapper remains under windows/" >&2
  exit 1
fi

if rg -n -i -F -e 'powershell.exe' -e 'BackupRestore.ps1' \
  crates/backuprestore-cli/src crates/backuprestore-core/src windows/winpeshl.ini; then
  echo "product runtime must not invoke PowerShell" >&2
  exit 1
fi

if rg -n -i -e 'cmd\.exe|\.cmd|powershell\.exe|Capture-Image|Apply-Image|bcdboot|format ' windows/winpeshl.ini; then
  echo "WinRE must launch the Rust recovery entry point directly" >&2
  exit 1
fi

if ! grep -Fx '%SYSTEMROOT%\System32\Recovery.exe,recover-env %SYSTEMROOT%\System32\RecoveryTask.env' windows/winpeshl.ini >/dev/null; then
  echo "WinRE entry point must directly invoke Recovery.exe recover-env" >&2
  exit 1
fi

cargo fmt --all -- --check
cargo test --workspace --all-targets --offline
git diff --check
echo "runtime boundary audit passed"

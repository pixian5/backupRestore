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

if [ -e windows/winpeshl.ini ] || [ -e windows/RecoveryLauncher.cmd ] || [ -e windows/winpeshl-boot.cmd ]; then
  echo "obsolete shared or batch WinRE shell assets remain under windows/" >&2
  exit 1
fi

if rg -n -i -F -e 'powershell.exe' -e 'BackupRestore.ps1' \
  crates/backuprestore-cli/src crates/backuprestore-core/src windows/winre-winpeshl.ini windows/winpe-winpeshl.ini; then
  echo "product runtime must not invoke PowerShell" >&2
  exit 1
fi

if rg -n -F -e 'relocate_to_image_volume' -e '"--relocated" =>' \
  crates/backuprestore-cli/src/windows_prepare.rs; then
  echo "restore must refuse a workspace target instead of auto-relocating" >&2
  exit 1
fi

if rg -n -i -e 'cmd\.exe|\.cmd|powershell\.exe|Capture-Image|Apply-Image|bcdboot|format ' windows/winre-winpeshl.ini; then
  echo "WinRE must launch the Rust recovery entry point directly" >&2
  exit 1
fi

if ! grep -Fx '%SYSTEMROOT%\System32\Recovery.exe,recover-env %SYSTEMROOT%\System32\RecoveryTask.env' windows/winre-winpeshl.ini >/dev/null; then
  echo "WinRE entry point must directly invoke Recovery.exe recover-env" >&2
  exit 1
fi

if ! grep -Fx '%SYSTEMROOT%\System32\Recovery.exe,--pe-desktop' windows/winpe-winpeshl.ini >/dev/null; then
  echo "WinPE entry point must directly invoke Recovery.exe --pe-desktop" >&2
  exit 1
fi

cargo fmt --all -- --check
cargo test --workspace --all-targets --offline
cargo clippy --workspace --all-targets --offline -- -D warnings
git diff --check
echo "runtime boundary audit passed"

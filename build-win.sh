#!/bin/bash
# BackupRestore macOS 交叉编译 Windows ARM64 一键构建脚本
# 用法：./build-win.sh [--deploy]
#   --deploy  构建后复制到 VM 部署目录（C:\Users\Public\backupRestore-package\BackupRestore.exe）
# 依赖（见 docs/backuprestore-pe.md 踩坑 23）：
#   - rustup target add aarch64-pc-windows-msvc（rust-std）
#   - ~/win-sdk-arm64/{um,ucrt,vc}/arm64（从 VM 复制的 Windows SDK + VC import libs）
set -e
cd "$(dirname "$0")"

RLLD=$(find ~/.rustup/toolchains/stable-aarch64-apple-darwin -name "rust-lld" -type f 2>/dev/null | head -1)
if [ -z "$RLLD" ]; then
  echo "找不到 rust-lld，请检查 rustup toolchain" >&2
  exit 1
fi
for d in /Users/x/win-sdk-arm64/um/arm64 /Users/x/win-sdk-arm64/ucrt/arm64 /Users/x/win-sdk-arm64/vc/arm64; do
  if [ ! -d "$d" ]; then
    echo "缺少 SDK libs 目录：$d（见 docs 踩坑 23）" >&2
    exit 1
  fi
done

echo ">> cargo build (aarch64-pc-windows-msvc)"
CARGO_TARGET_AARCH64_PC_WINDOWS_MSVC_LINKER="$RLLD" \
RUSTFLAGS="-C link-arg=/LIBPATH:/Users/x/win-sdk-arm64/um/arm64 -C link-arg=/LIBPATH:/Users/x/win-sdk-arm64/ucrt/arm64 -C link-arg=/LIBPATH:/Users/x/win-sdk-arm64/vc/arm64" \
cargo build --release --target aarch64-pc-windows-msvc -p backuprestore-cli

EXE=target/aarch64-pc-windows-msvc/release/backuprestore-cli.exe
cp "$EXE" target/aarch64-pc-windows-msvc/release/BackupRestore.exe
echo ">> 产物：$(ls -la target/aarch64-pc-windows-msvc/release/BackupRestore.exe | awk '{print $5}') 字节"

if [ "$1" = "--deploy" ]; then
  echo ">> 部署到 VM Windows 11"
  # Parallels mounts X: only in the interactive Windows session. Use the
  # elevated channel for process cleanup and the interactive channel for the
  # actual shared-folder copy. Never accept a stale executable as success.
  prlctl exec "Windows 11" cmd /d /c "taskkill /f /im BackupRestore.exe >nul 2>nul & taskkill /f /im Recovery.exe >nul 2>nul & if not exist C:\\Users\\Public\\backupRestore-package\\NUL mkdir C:\\Users\\Public\\backupRestore-package & del /f /q C:\\Users\\Public\\backupRestore-package\\RecoveryLauncher.cmd 2>nul & del /f /q C:\\Users\\Public\\backupRestore-package\\winpeshl.ini 2>nul & del /f /q C:\\Users\\Public\\backupRestore-package\\winpeshl-boot.cmd 2>nul"
  DEPLOY_OUTPUT=$(prlctl exec "Windows 11" --current-user cmd /d /c "copy /y X:\\target\\aarch64-pc-windows-msvc\\release\\BackupRestore.exe C:\\Users\\Public\\backupRestore-package\\BackupRestore.exe >nul && copy /y X:\\target\\aarch64-pc-windows-msvc\\release\\BackupRestore.exe C:\\Users\\Public\\backupRestore-package\\Recovery.exe >nul && copy /y X:\\windows\\winpe-winpeshl.ini C:\\Users\\Public\\backupRestore-package\\winpe-winpeshl.ini >nul && echo DEPLOYED")
  if ! echo "$DEPLOY_OUTPUT" | grep -q '^DEPLOYED'; then
    echo "部署失败：客体未返回 DEPLOYED 成功标记" >&2
    echo "$DEPLOY_OUTPUT" >&2
    exit 1
  fi
  HOST_SHA=$(shasum -a 256 target/aarch64-pc-windows-msvc/release/BackupRestore.exe | awk '{print $1}')
  GUEST_SHA=$(prlctl exec "Windows 11" powershell -NoProfile -Command "(Get-FileHash -Algorithm SHA256 'C:\\Users\\Public\\backupRestore-package\\BackupRestore.exe').Hash.ToLowerInvariant()" | tr -d '\r')
  RECOVERY_SHA=$(prlctl exec "Windows 11" powershell -NoProfile -Command "(Get-FileHash -Algorithm SHA256 'C:\\Users\\Public\\backupRestore-package\\Recovery.exe').Hash.ToLowerInvariant()" | tr -d '\r')
  if [ "$GUEST_SHA" != "$HOST_SHA" ] || [ "$RECOVERY_SHA" != "$HOST_SHA" ]; then
    echo "部署失败：客体可执行文件 SHA-256 与本机构建不一致" >&2
    exit 1
  fi
  echo ">> DEPLOYED SHA256=$HOST_SHA"
fi
echo ">> 完成"

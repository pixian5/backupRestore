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
  # Do not print DEPLOYED unless every file reached the guest. The executable
  # is always copied as both Rust entry points; the templates remain distinct.
  prlctl exec "Windows 11" cmd /d /c "chcp 65001 >nul & taskkill /f /im BackupRestore.exe >nul 2>nul & taskkill /f /im Recovery.exe >nul 2>nul & if not exist C:\\Users\\Public\\backupRestore-package\\NUL mkdir C:\\Users\\Public\\backupRestore-package & copy /y \\\\Mac\\backupRestore\\target\\aarch64-pc-windows-msvc\\release\\BackupRestore.exe C:\\Users\\Public\\backupRestore-package\\BackupRestore.exe >nul && copy /y \\\\Mac\\backupRestore\\target\\aarch64-pc-windows-msvc\\release\\BackupRestore.exe C:\\Users\\Public\\backupRestore-package\\Recovery.exe >nul && copy /y \\\\Mac\\backupRestore\\windows\\winre-winpeshl.ini C:\\Users\\Public\\backupRestore-package\\winre-winpeshl.ini >nul && copy /y \\\\Mac\\backupRestore\\windows\\winpe-winpeshl.ini C:\\Users\\Public\\backupRestore-package\\winpe-winpeshl.ini >nul && echo DEPLOYED"
fi
echo ">> 完成"

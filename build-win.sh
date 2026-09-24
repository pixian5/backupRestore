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
# +crt-static：静态链接 C 运行库（VCRUNTIME140 + UCRT）。
# 踩坑 25：动态链接时产物依赖 VCRUNTIME140.dll 与 api-ms-win-crt-*.dll，
# 在精简版 Windows（tiny11）、WinRE/PE 里这些 DLL 不存在，程序连进程都起不来
# （退出码 0xC0000135 / STATUS_DLL_NOT_FOUND）。静态链接后产物零外部 DLL 依赖，
# WinRE payload 也不必再搬运 VCRUNTIME140.dll。
CARGO_TARGET_AARCH64_PC_WINDOWS_MSVC_LINKER="$RLLD" \
RUSTFLAGS="-C target-feature=+crt-static -C link-arg=/LIBPATH:/Users/x/win-sdk-arm64/um/arm64 -C link-arg=/LIBPATH:/Users/x/win-sdk-arm64/ucrt/arm64 -C link-arg=/LIBPATH:/Users/x/win-sdk-arm64/vc/arm64" \
cargo build --release --target aarch64-pc-windows-msvc -p backuprestore-cli

EXE=target/aarch64-pc-windows-msvc/release/backuprestore-cli.exe
cp "$EXE" target/aarch64-pc-windows-msvc/release/BackupRestore.exe
echo ">> 产物：$(ls -la target/aarch64-pc-windows-msvc/release/BackupRestore.exe | awk '{print $5}') 字节"

# prlctl exec 会间歇性返回 `PrlJob_GetResult / PrlJob_GetRetCode: Invalid argument`
# 并以 255 退出，与客体命令本身是否成功无关（本机 GuestTools 报 state=outdated）。
# 脚本开了 set -e，这种抖动会让部署在复制完成、SHA 校验之前中断，留下半部署状态。
# 因此对 exec 统一重试；真正的成功判据仍然只有下面的 DEPLOYED 标记和 SHA-256 比对，
# 重试不会放过真实失败。
prl_exec_retry() {
  local attempts=3 delay=2 i=1 out err rc
  # stderr 单独收集：客体 stdout 会被调用方当数据用（DEPLOYED 标记、SHA-256），
  # 混入 prlctl 自己的诊断文本会让 SHA 比对以一条看不懂的消息失败。
  err=$(mktemp)
  while :; do
    out=$(prlctl exec "$@" 2>"$err"); rc=$?
    if [ "$rc" -eq 0 ] || ! grep -q 'Invalid argument' "$err"; then
      cat "$err" >&2
      rm -f "$err"
      printf '%s' "$out"
      return "$rc"
    fi
    if [ "$i" -ge "$attempts" ]; then
      echo "prlctl exec 连续 $attempts 次返回 Invalid argument（Parallels 通道抖动）" >&2
      cat "$err" >&2
      rm -f "$err"
      printf '%s' "$out"
      return "$rc"
    fi
    echo ">> prlctl exec 抖动，第 $i 次重试…" >&2
    i=$((i + 1))
    sleep "$delay"
  done
}

if [ "$1" = "--deploy" ]; then
  echo ">> 部署到 VM Windows 11"
  # Parallels mounts X: only in the interactive Windows session. Use the
  # elevated channel for process cleanup and the interactive channel for the
  # actual shared-folder copy. Never accept a stale executable as success.
  # 清理步骤的失败都是良性的（进程本来没在跑、文件本来不存在），
  # 用 exit /b 0 收尾，避免良性退出码在 set -e 下中断部署。
  prl_exec_retry "Windows 11" cmd /d /c "taskkill /f /im BackupRestore.exe >nul 2>nul & taskkill /f /im Recovery.exe >nul 2>nul & if not exist C:\\Users\\Public\\backupRestore-package\\NUL mkdir C:\\Users\\Public\\backupRestore-package & del /f /q C:\\Users\\Public\\backupRestore-package\\RecoveryLauncher.cmd 2>nul & del /f /q C:\\Users\\Public\\backupRestore-package\\winpeshl.ini 2>nul & del /f /q C:\\Users\\Public\\backupRestore-package\\winpeshl-boot.cmd 2>nul & exit /b 0"
  DEPLOY_OUTPUT=$(prl_exec_retry "Windows 11" --current-user cmd /d /c "copy /y X:\\target\\aarch64-pc-windows-msvc\\release\\BackupRestore.exe C:\\Users\\Public\\backupRestore-package\\BackupRestore.exe >nul && copy /y X:\\target\\aarch64-pc-windows-msvc\\release\\BackupRestore.exe C:\\Users\\Public\\backupRestore-package\\Recovery.exe >nul && copy /y X:\\windows\\winpe-winpeshl.ini C:\\Users\\Public\\backupRestore-package\\winpe-winpeshl.ini >nul && echo DEPLOYED")
  if ! echo "$DEPLOY_OUTPUT" | grep -q '^DEPLOYED'; then
    echo "部署失败：客体未返回 DEPLOYED 成功标记" >&2
    echo "$DEPLOY_OUTPUT" >&2
    exit 1
  fi
  HOST_SHA=$(shasum -a 256 target/aarch64-pc-windows-msvc/release/BackupRestore.exe | awk '{print $1}')
  GUEST_SHA=$(prl_exec_retry "Windows 11" powershell -NoProfile -Command "(Get-FileHash -Algorithm SHA256 'C:\\Users\\Public\\backupRestore-package\\BackupRestore.exe').Hash.ToLowerInvariant()" | tr -d '\r')
  RECOVERY_SHA=$(prl_exec_retry "Windows 11" powershell -NoProfile -Command "(Get-FileHash -Algorithm SHA256 'C:\\Users\\Public\\backupRestore-package\\Recovery.exe').Hash.ToLowerInvariant()" | tr -d '\r')
  if [ "$GUEST_SHA" != "$HOST_SHA" ] || [ "$RECOVERY_SHA" != "$HOST_SHA" ]; then
    echo "部署失败：客体可执行文件 SHA-256 与本机构建不一致" >&2
    exit 1
  fi
  echo ">> DEPLOYED SHA256=$HOST_SHA"
fi
echo ">> 完成"

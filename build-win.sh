#!/bin/bash
# BackupRestore macOS 交叉编译 Windows ARM64 一键构建脚本
# 用法：./build-win.sh [--deploy]
#   --deploy  构建后复制到 VM 部署目录（C:\Users\Public\backupRestore-package\BackupRestore.exe）
# 依赖（见 docs/20260825-2309-backuprestore-pe.md 踩坑 23）：
#   - rustup target add aarch64-pc-windows-msvc（rust-std）
#   - ~/win-sdk-arm64/{um,ucrt,vc}/arm64（从 VM 复制的 Windows SDK + VC import libs）
set -e
cd "$(dirname "$0")"

# rust-lld 位于 toolchain 的 lib/rustlib/<host>/bin 下（rustup 更新后从 bin/ 挪到了这里）。
# 用 sysroot 推导，避免硬编码 toolchain 版本号。
RLLD=$(rustc --print sysroot)/lib/rustlib/aarch64-apple-darwin/bin/rust-lld
if [ ! -f "$RLLD" ]; then
  RLLD=$(find ~/.rustup/toolchains -name "rust-lld" -type f 2>/dev/null | head -1)
fi
if [ -z "$RLLD" ] || [ ! -f "$RLLD" ]; then
  echo "找不到 rust-lld，请检查 rustup toolchain（需安装 rust-std）" >&2
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
    if out=$(prlctl exec "$@" 2>"$err"); then rc=0; else rc=$?; fi
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
  # 部署全程用 SYSTEM 通道（prlctl exec 不加 --current-user），不依赖 VM 是否有
  # 交互登录会话来跑命令。共享盘用 UNC 路径 \\Mac\backupRestore 而非盘符 X:，
  # 因为盘符是会话级的、SYSTEM 看不到；UNC 在 SYSTEM 下可读写。注意 Parallels
  # 共享文件夹虚拟通道本身只在客体有登录会话时才建立，所以共享盘能否用仍取决于
  # 是否登录（本机已登录，故可用）。真正的成功判据只有下面的 DEPLOYED 标记和
  # SHA-256 比对，重试不会放过真实失败。
  # 实机踩过的坑（2026-09-30）：部署只更新包目录，真正的运行目录 H:\brwork
  # 里的 Recovery.exe 还是旧件，于是 WinRE 载荷被烘进旧代码，唯一症状是镜像
  # 元数据里的 programVersion 偏低，静态检查看不出来。因此部署必须把两个目录、
  # 两个文件名（BackupRestore.exe / Recovery.exe）全部刷新并逐个比对哈希。
  # 清理步骤的失败都是良性的（进程本来没在跑、文件本来不存在），
  # 用 exit /b 0 收尾，避免良性退出码在 set -e 下中断部署。
  prl_exec_retry "Windows 11" cmd /d /c "taskkill /f /im BackupRestore.exe & taskkill /f /im Recovery.exe & if not exist C:\\Users\\Public\\backupRestore-package\\NUL mkdir C:\\Users\\Public\\backupRestore-package & if not exist H:\\brwork\\NUL mkdir H:\\brwork & del /f /q C:\\Users\\Public\\backupRestore-package\\RecoveryLauncher.cmd & del /f /q C:\\Users\\Public\\backupRestore-package\\winpeshl.ini & del /f /q C:\\Users\\Public\\backupRestore-package\\winpeshl-boot.cmd & exit /b 0"
  DEPLOY_OUTPUT=$(prl_exec_retry "Windows 11" cmd /d /c "copy /y \\\\Mac\\backupRestore\\target\\aarch64-pc-windows-msvc\\release\\BackupRestore.exe C:\\Users\\Public\\backupRestore-package\\BackupRestore.exe && copy /y \\\\Mac\\backupRestore\\target\\aarch64-pc-windows-msvc\\release\\BackupRestore.exe C:\\Users\\Public\\backupRestore-package\\Recovery.exe && copy /y \\\\Mac\\backupRestore\\windows\\winpe-winpeshl.ini C:\\Users\\Public\\backupRestore-package\\winpe-winpeshl.ini && copy /y \\\\Mac\\backupRestore\\target\\aarch64-pc-windows-msvc\\release\\BackupRestore.exe H:\\brwork\\BackupRestore.exe && copy /y \\\\Mac\\backupRestore\\target\\aarch64-pc-windows-msvc\\release\\BackupRestore.exe H:\\brwork\\Recovery.exe && echo DEPLOYED")
  if ! echo "$DEPLOY_OUTPUT" | grep -q '^DEPLOYED'; then
    echo "部署失败：客体未返回 DEPLOYED 成功标记" >&2
    echo "$DEPLOY_OUTPUT" >&2
    exit 1
  fi
  HOST_SHA=$(shasum -a 256 target/aarch64-pc-windows-msvc/release/BackupRestore.exe | awk '{print $1}')
  GUEST_SHA=$(prl_exec_retry "Windows 11" powershell -NoProfile -Command "(Get-FileHash -Algorithm SHA256 'C:\\Users\\Public\\backupRestore-package\\BackupRestore.exe').Hash.ToLowerInvariant()" | tr -d '\r')
  RECOVERY_SHA=$(prl_exec_retry "Windows 11" powershell -NoProfile -Command "(Get-FileHash -Algorithm SHA256 'C:\\Users\\Public\\backupRestore-package\\Recovery.exe').Hash.ToLowerInvariant()" | tr -d '\r')
  WORK_SHA=$(prl_exec_retry "Windows 11" powershell -NoProfile -Command "(Get-FileHash -Algorithm SHA256 'H:\\brwork\\BackupRestore.exe').Hash.ToLowerInvariant()" | tr -d '\r')
  WORK_RECOVERY_SHA=$(prl_exec_retry "Windows 11" powershell -NoProfile -Command "(Get-FileHash -Algorithm SHA256 'H:\\brwork\\Recovery.exe').Hash.ToLowerInvariant()" | tr -d '\r')
  for pair in "包目录 BackupRestore.exe:$GUEST_SHA" "包目录 Recovery.exe:$RECOVERY_SHA" "运行目录 BackupRestore.exe:$WORK_SHA" "运行目录 Recovery.exe:$WORK_RECOVERY_SHA"; do
    name=${pair%%:*}
    sha=${pair##*:}
    if [ "$sha" != "$HOST_SHA" ]; then
      echo "部署失败：$name 的 SHA-256 与本机构建不一致（客体=$sha 本机=$HOST_SHA）" >&2
      exit 1
    fi
  done
  echo ">> DEPLOYED SHA256=$HOST_SHA"
fi
echo ">> 完成"

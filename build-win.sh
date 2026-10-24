#!/bin/bash
# BackupRestore macOS 交叉编译 Windows ARM64 一键构建脚本
# 用法：./build-win.sh [--deploy]
#   --deploy  构建后部署到 VM 唯一目标 H:\brwork（BackupRestore.exe / Recovery.exe
#             / wimlib-imagex.exe / libwim-15.dll / winpe-winpeshl.ini 五件套）
#
# ⚠️ 部署目标唯一 = H:\brwork（2026-10-24 用户裁定「不再往 C: 部署 exe」）。
#    TaskStore 的根 = 可执行文件所在目录（GUI `TaskStore::new(&state.executable_dir)`、
#    CLI `current_exe().parent()`），任务目录随之落在 `<运行目录>\tasks`。
#    所以「任务存哪」由「从哪启动」决定：从 H:\brwork 起 → 任务在 H:\brwork\tasks。
#    C:\Users\Public\backupRestore-package 是历史部署位，自本版起**不再写入**：
#    备份 C: 会把该目录连旧 exe 一起打进镜像，A 入口还原 C: 后自启 GUI 会从那里
#    带回旧版二进制（2026-10-24 A 入口回归实测：自启实例 v2.6.31，而 H: 里是
#    v2.6.32）。脚本每次 --deploy 会顺带清掉该目录残留，让 C: 上零程序痕迹。

# ⚠️ 部署目标唯一约定（2026-10-13 用户裁定）：工作目录只有 H:\brwork。
#    旧工作区 F:\BR 已于 2026-10-13 退役并删除——F: 是镜像卷（标签 BRIMG），只放镜像。
#    不要再往 F:\BR（或 F: 上任何目录）部署程序：两套工作区会争抢机器上唯一那份
#    bootmgr 一次性启动槽位，造成交叉污染（典型症状：进 RE 后弹
#    "I/O error: 系统找不到指定的路径。 (os error 3)"，因为启动请求指向的任务目录
#    已被另一套工作区清掉）。客体侧脚本 windows/build-windows.ps1 的
#    -DeployWorkDir 默认值同样是 H:\brwork。
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
  # 元数据里的 programVersion 偏低，静态检查看不出来。因此部署必须把 H:\brwork
  # 里两个文件名（BackupRestore.exe / Recovery.exe）全部刷新并逐个比对哈希。
  # 清理步骤的失败都是良性的（进程本来没在跑、文件本来不存在），
  # 用 exit /b 0 收尾，避免良性退出码在 set -e 下中断部署。
  #
  # ⚠️ 2026-10-12 实测教训：不要把 mkdir/del/taskkill 串成一长串 `&` 命令。
  # 那条 400+ 字符的长链经 prlctl exec 传递时会**静默失败**——命令返回 0、
  # 不报任何错，但 mkdir 实际没执行，于是 H:\brwork 建不起来，后续 copy 失败，
  # 部署假失败。现场表现：脚本打印"部署到 VM"后什么输出都没有就结束。
  # 拆成独立短调用后每次都能确认结果；mkdir 不加 `|| true`，
  # 因为它失败必须让部署中止（后面 copy 到不存在的目录必然失败）。
  # 1) 停掉可能在跑的旧进程（找不到进程是常态，良性）
  prl_exec_retry "Windows 11" cmd /d /c "taskkill /f /im BackupRestore.exe & taskkill /f /im Recovery.exe & exit /b 0" || true
  # 2) 清掉历史部署位 C:\Users\Public\backupRestore-package（2026-10-24 用户裁定
  #    「不再往 C: 部署 exe」后该目录只会有残留；rmdir /s /q 对不存在的目录返回
  #    非零，属良性，用 exit /b 0 收尾）
  prl_exec_retry "Windows 11" cmd /d /c "rmdir /s /q C:\\Users\\Public\\backupRestore-package & exit /b 0" || true
  # 3) 建唯一目标目录（独立调用，确认真的建起来了）
  prl_exec_retry "Windows 11" cmd /d /c "if not exist H:\\brwork\\NUL mkdir H:\\brwork"
  # wimlib 官方 ARM64 二进制（任务 #110 覆盖式还原通道）：在线态按 executable_dir
  # 定位，故必须随 H:\brwork 部署；RE 态由 winre_payload 从 executable_dir
  # 暂存注入 Winre.wim 的 System32（同一来源，哈希校验在注入链）。
  # winpe-winpeshl.ini 是 PE 桌面壳模板（`--pe-desktop`），随部署位放一份便于人工比对。
  DEPLOY_OUTPUT=$(prl_exec_retry "Windows 11" cmd /d /c "copy /y \\\\Mac\\backupRestore\\target\\aarch64-pc-windows-msvc\\release\\BackupRestore.exe H:\\brwork\\BackupRestore.exe && copy /y \\\\Mac\\backupRestore\\target\\aarch64-pc-windows-msvc\\release\\BackupRestore.exe H:\\brwork\\Recovery.exe && copy /y \\\\Mac\\backupRestore\\windows\\winpe-winpeshl.ini H:\\brwork\\winpe-winpeshl.ini && copy /y \\\\Mac\\backupRestore\\windows\\wimlib-aarch64\\wimlib-imagex.exe H:\\brwork\\wimlib-imagex.exe && copy /y \\\\Mac\\backupRestore\\windows\\wimlib-aarch64\\libwim-15.dll H:\\brwork\\libwim-15.dll && echo DEPLOYED")
  if ! echo "$DEPLOY_OUTPUT" | grep -q '^DEPLOYED'; then
    echo "部署失败：客体未返回 DEPLOYED 成功标记" >&2
    echo "$DEPLOY_OUTPUT" >&2
    exit 1
  fi
  HOST_SHA=$(shasum -a 256 target/aarch64-pc-windows-msvc/release/BackupRestore.exe | awk '{print $1}')
  # wimlib 二进制基线哈希（仓库 windows/wimlib-aarch64/，任务 #110）
  WIMLIB_EXE_SHA=$(shasum -a 256 windows/wimlib-aarch64/wimlib-imagex.exe | awk '{print $1}')
  WIMLIB_DLL_SHA=$(shasum -a 256 windows/wimlib-aarch64/libwim-15.dll | awk '{print $1}')
  WORK_SHA=$(prl_exec_retry "Windows 11" powershell -NoProfile -Command "(Get-FileHash -Algorithm SHA256 'H:\\brwork\\BackupRestore.exe').Hash.ToLowerInvariant()" | tr -d '\r')
  WORK_RECOVERY_SHA=$(prl_exec_retry "Windows 11" powershell -NoProfile -Command "(Get-FileHash -Algorithm SHA256 'H:\\brwork\\Recovery.exe').Hash.ToLowerInvariant()" | tr -d '\r')
  WORK_WIMLIB_EXE_SHA=$(prl_exec_retry "Windows 11" powershell -NoProfile -Command "(Get-FileHash -Algorithm SHA256 'H:\\brwork\\wimlib-imagex.exe').Hash.ToLowerInvariant()" | tr -d '\r')
  WORK_WIMLIB_DLL_SHA=$(prl_exec_retry "Windows 11" powershell -NoProfile -Command "(Get-FileHash -Algorithm SHA256 'H:\\brwork\\libwim-15.dll').Hash.ToLowerInvariant()" | tr -d '\r')
  # 每项「名称:期望哈希:实际哈希」——期望值按项取（程序两份同哈希，wimlib 另有基线）。
  # ⚠️ 冒号是切分符（`name=${triple%%:*}` / `expected` / `sha` 三段靠它切），
  #    所以**标签里绝不能出现冒号**：2026-10-24 把标签写成「分发副本(C:包目录)」，
  #    被切成 `分发副本(C`，三段错位 → 八联校验全部误报「与基线不一致」。
  #    标签一律用「H-brwork」这类不含冒号的写法。
  for triple in "唯一部署位(H-brwork) BackupRestore.exe:$HOST_SHA:$WORK_SHA" "唯一部署位(H-brwork) Recovery.exe:$HOST_SHA:$WORK_RECOVERY_SHA" "唯一部署位(H-brwork) wimlib-imagex.exe:$WIMLIB_EXE_SHA:$WORK_WIMLIB_EXE_SHA" "唯一部署位(H-brwork) libwim-15.dll:$WIMLIB_DLL_SHA:$WORK_WIMLIB_DLL_SHA"; do
    name=${triple%%:*}
    rest=${triple#*:}
    expected=${rest%%:*}
    sha=${rest##*:}
    if [ "$sha" != "$expected" ]; then
      echo "部署失败：$name 的 SHA-256 与基线不一致（客体=$sha 基线=$expected）" >&2
      exit 1
    fi
  done
  echo ">> DEPLOYED SHA256=$HOST_SHA wimlib=$WIMLIB_EXE_SHA/$WIMLIB_DLL_SHA"
fi
echo ">> 完成"

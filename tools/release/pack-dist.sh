#!/bin/bash
# 打一个可分发的 Windows ARM64 交付包到 dist/。
#
# 用法：./tools/release/pack-dist.sh [版本号] [--local]
#   版本号省略时读仓库根的 VERSION。
#   --local  改用 macOS 交叉编译产物（**不是发布版**，仅供本地看包结构）。
#
# ## 为什么来源必须是 artifacts/windows/
#
# 用户 2026-10-15 拍板「编译完全走虚拟机内 Windows」：macOS 侧的 `./build-win.sh`
# 已废止为发布路径，产物「不得部署、不得作为发布版」（README §三）。所以本脚本
# 默认只认 `artifacts/windows/BackupRestore-windows-arm64-v<版本>/` —— 那是
# `windows/build-windows.ps1` 在虚拟机内原生编译的产物（`X:\artifacts\windows\`），
# 带 `build-manifest.json` 记着 binarySha256 与 nativeMachine。
#
# 拿 mac 产物打出来的包**能跑但不可信**：两边 CRC 表与优化都不同，且 mac 绿灯对
# `#[cfg(windows)]` 模块零证明力（2026-10-26 实测：cargo check/test 全绿，
# 交叉编译报两轮 E0308）。
#
# ## 为什么还要往里补文件
#
# `build-windows.ps1` 打的是「开发包」：exe×2 + VCRUNTIME×2 + winpe-winpeshl.ini
# + VERSION + manifest，**故意不含 wimlib 两个文件**（它们在 windows/wimlib-aarch64/，
# 由部署脚本单独拷）。分发时缺了 wimlib-imagex.exe / libwim-15.dll，覆盖式还原
# （`--apply` 走 wimlib 那一路）会直接失败，所以这里补齐并把文档一并放进去。
#
# 前置：先跑虚拟机内构建（本脚本**不编译**，避免出现第二条构建路径）：
#   prlctl exec "Windows 11" --current-user cmd /d /c \
#     'powershell -ExecutionPolicy Bypass -File X:\windows\build-windows.ps1 \
#       -Architecture arm64 -Deploy -CargoTargetDir H:\br-target'
set -euo pipefail
cd "$(dirname "$0")/../.."
ROOT=$(pwd)

LOCAL=0
VERSION=""
for arg in "$@"; do
  case "$arg" in
    --local) LOCAL=1 ;;
    -*) echo "未知参数：$arg" >&2; exit 2 ;;
    *) VERSION=$arg ;;
  esac
done
[ -n "$VERSION" ] || VERSION=$(tr -d ' \r\n' < VERSION)
if [ -z "$VERSION" ]; then
  echo "读不到版本号：$ROOT/VERSION 为空，且未在命令行指定" >&2
  exit 1
fi

if [ "$LOCAL" = "1" ]; then
  echo "!! --local：用 macOS 交叉编译产物，不是发布版（README §三明确禁止作为发布版）"
  EXE=target/aarch64-pc-windows-msvc/release/BackupRestore.exe
  SUFFIX=-local
  if [ ! -f "$EXE" ]; then
    echo "找不到 $EXE，先跑 ./build-win.sh" >&2
    exit 1
  fi
else
  ARTDIR=artifacts/windows/BackupRestore-windows-arm64-v$VERSION
  EXE=$ARTDIR/BackupRestore.exe
  SUFFIX=
  if [ ! -f "$EXE" ]; then
    echo "找不到 $EXE" >&2
    echo "这是虚拟机内原生构建的产物位（唯一发布路径）。先跑：" >&2
    echo "  prlctl exec \"Windows 11\" --current-user cmd /d /c 'powershell -ExecutionPolicy Bypass -File X:\\windows\\build-windows.ps1 -Architecture arm64 -Deploy -CargoTargetDir H:\\br-target'" >&2
    exit 1
  fi
fi

# 必带件：少一个就说明仓库/部署状态不对，宁可失败也不打残包。
for f in "$EXE" windows/winpe-winpeshl.ini windows/wimlib-aarch64/wimlib-imagex.exe windows/wimlib-aarch64/libwim-15.dll; do
  if [ ! -f "$f" ]; then
    echo "缺少交付件：$f" >&2
    exit 1
  fi
done

NAME=BackupRestore-$VERSION-windows-arm64$SUFFIX
STAGE=$(mktemp -d)
trap 'rm -rf "$STAGE"' EXIT
PKG=$STAGE/$NAME
mkdir -p "$PKG"

cp "$EXE" "$PKG/BackupRestore.exe"
# 恢复环境按 argv[0] 分派，Recovery.exe 是同一个二进制的第二个名字，不是冗余。
cp "$EXE" "$PKG/Recovery.exe"
cp windows/winpe-winpeshl.ini "$PKG/winpe-winpeshl.ini"
cp windows/wimlib-aarch64/wimlib-imagex.exe "$PKG/wimlib-imagex.exe"
cp windows/wimlib-aarch64/libwim-15.dll "$PKG/libwim-15.dll"
cp docs/USER-GUIDE.md "$PKG/USER-GUIDE.md"
cp docs/USER-GUIDE.en.md "$PKG/USER-GUIDE.en.md"

# 构建产物自带的 VCRUNTIME 与清单，原样带上（不是本脚本生成的，不重写）。
if [ "$LOCAL" = "0" ]; then
  for extra in VCRUNTIME140.dll VCRUNTIME140_1.dll VERSION build-manifest.json; do
    if [ -f "$ARTDIR/$extra" ]; then
      cp "$ARTDIR/$extra" "$PKG/$extra"
    fi
  done
fi

COMMIT=$(git rev-parse --short HEAD 2>/dev/null || echo unknown)
BUILT=$(date "+%Y-%m-%d %H:%M:%S")

# 包根放一份自述：拿到 zip 就知道这是什么、怎么跑、对应哪个 commit。
cat > "$PKG/README.txt" <<EOF
BackupRestore $VERSION — Windows ARM64
打包时间：$BUILT
对应提交：$COMMIT
二进制来源：$( [ "$LOCAL" = "1" ] && echo "macOS 交叉编译（非发布版，仅供本地验证）" || echo "虚拟机内原生构建（发布路径）" )

这是什么
  Windows ARM64 上的系统备份/还原工具：把整卷做成 WIM 镜像，配合自建的一次性
  启动项进恢复环境，把镜像写回目标卷。图形界面与命令行是同一个 exe。

怎么用
  1. 把本目录整体放到一个**不参与还原**的卷上（例：H:），不要放在还原目标卷里。
  2. 双击 BackupRestore.exe 打开图形界面；或在本目录用命令行：
       BackupRestore.exe --help
  3. 详细步骤见 USER-GUIDE.md（英文版 USER-GUIDE.en.md）。

文件分工
  BackupRestore.exe      主程序（图形界面 + 命令行）
  Recovery.exe           同一个二进制，恢复环境按 argv[0] 分派用的名字
  wimlib-imagex.exe      覆盖式还原通道（--apply 走 wimlib 时用）
  libwim-15.dll          wimlib 的依赖库，必须与 exe 同目录
  winpe-winpeshl.ini     PE 桌面壳模板（--pe-desktop 时随部署位放一份便于比对）
  VCRUNTIME140*.dll      构建产物自带的运行库（静态 CRT 下非必需，保留作兜底）
  build-manifest.json    原生构建清单：版本、目标三元组、binarySha256
  SHA256SUMS.txt         本包内每个文件的哈希，可自行核对

运行前提
  管理员权限；Windows 11 ARM64；x64 平台不受支持。
  还原目标卷上的数据会被覆盖，操作前请自行确认。

校验
      certutil -hashfile <文件名> SHA256
EOF

# 哈希清单：只列交付件，不含 README/SHA256SUMS 自身（否则自引用）。
(
  cd "$PKG"
  shasum -a 256 BackupRestore.exe Recovery.exe wimlib-imagex.exe libwim-15.dll \
    winpe-winpeshl.ini USER-GUIDE.md USER-GUIDE.en.md > SHA256SUMS.txt
  for extra in VCRUNTIME140.dll VCRUNTIME140_1.dll build-manifest.json; do
    [ -f "$extra" ] && shasum -a 256 "$extra" >> SHA256SUMS.txt
  done
  true
)

mkdir -p dist
OUT=$ROOT/dist/$NAME.zip
rm -f "$OUT"
# -X 去掉扩展属性，避免 macOS 的 __MACOSX 目录混进 Windows 交付包。
(cd "$STAGE" && zip -q -r -X "$OUT" "$NAME")

echo ">> 交付包：dist/$NAME.zip（$(ls -la "$OUT" | awk '{print $5}') 字节）"
unzip -l "$OUT" | sed -n '4,24p'
echo ">> 主程序 SHA256：$(shasum -a 256 "$PKG/BackupRestore.exe" | awk '{print $1}')"
echo ">> 对应提交：$COMMIT"

#!/bin/bash
# 交付包验收：把「每次发包手工核对的那几条」固化成一条命令。
#
# 用法：
#   ./tools/release/release-check.sh [版本号] [选项]
#     版本号       省略时读仓库根 VERSION
#     --no-vm      跳过「包内 exe == 实机部署 exe」这条（虚拟机关机时用）
#     --vm-name N  虚拟机名，默认 "Windows 11"
#     --baseline Z 额外与另一份交付包 zip 比条目结构（例：上一版）
#
# 退出码：0 = 全部通过（跳过的条目会明确列出）；1 = 有 FAIL；2 = 用法/前置错误。
#
# ## 为什么要有这个脚本
#
# 2026-10-26 一天里手工跑了四遍同样的核对：14 条目、shasum -c 10/10、包内 VERSION、
# 包内 exe SHA 对实机、手册 cmp、README 的对应提交。手工会漏——尤其在某次赶时间时。
# 而且「包里的 exe 是不是部署在跑的那个」是最关键的一条：它同时证明「产物→包」和
# 「产物→部署」两条拷贝都没串，手工核对时最容易被别的绿光掩盖。
#
# ## 判据设计原则
#
# 1. **只读**：解包到临时目录，绝不改动 dist/ 或 artifacts/。
# 2. **「读不到」与「判据不满足」分开**：VM 没开 / 文件缺失记 SKIP，不记 FAIL。
#    把通道抽风记成红判据，会训练人忽略红判据（2026-10-26 实测过）。
# 3. **每条都打印实测值**，不只是 PASS/FAIL：出问题时现场数据就在输出里。
set -uo pipefail
cd "$(dirname "$0")/../.."
ROOT=$(pwd)

VERSION=""
USE_VM=1
VM_NAME="Windows 11"
BASELINE=""
while [ $# -gt 0 ]; do
  case "$1" in
    --no-vm) USE_VM=0 ;;
    --vm-name) VM_NAME="${2:?--vm-name 需要参数}"; shift ;;
    --baseline) BASELINE="${2:?--baseline 需要参数}"; shift ;;
    -*) echo "未知参数：$1" >&2; exit 2 ;;
    *) VERSION=$1 ;;
  esac
  shift
done
[ -n "$VERSION" ] || VERSION=$(tr -d ' \r\n' < VERSION)
[ -n "$VERSION" ] || { echo "读不到版本号" >&2; exit 2; }

ZIP="dist/BackupRestore-$VERSION-windows-arm64.zip"
NAME="BackupRestore-$VERSION-windows-arm64"
ARTDIR="artifacts/windows/BackupRestore-windows-arm64-v$VERSION"

pass=0
fail=0
skip=0

ok()   { echo "  PASS  $1"; pass=$((pass+1)); }
bad()  { echo "  FAIL  $1"; fail=$((fail+1)); }
warn() { echo "  SKIP  $1"; skip=$((skip+1)); }

echo "== 交付包验收 $VERSION =="

# ---- 0. 前置 ---------------------------------------------------------------
if [ ! -f "$ZIP" ]; then
  echo "  FAIL  找不到 $ZIP —— 先跑 tools/release/pack-dist.sh" >&2
  exit 2
fi

STAGE=$(mktemp -d)
trap 'rm -rf "$STAGE"' EXIT
unzip -q "$ZIP" -d "$STAGE" || { echo "  FAIL  解包失败：$ZIP" >&2; exit 2; }
PKG="$STAGE/$NAME"
[ -d "$PKG" ] || { echo "  FAIL  包内没有 $NAME/ 目录（顶层目录名与版本不符）" >&2; exit 2; }

# ---- 1. 条目结构 -----------------------------------------------------------
files=$(unzip -Z1 "$ZIP" | grep -vc '/$' || true)
entries=$(unzip -Z1 "$ZIP" | wc -l | tr -d ' ')
if [ "$files" = "13" ] && [ "$entries" = "14" ]; then
  ok "条目结构 files=13 entries=14"
else
  bad "条目结构 files=$files entries=${entries}（期望 13 / 14）"
fi

# ---- 2. 哈希清单 -----------------------------------------------------------
sums_ok=$( cd "$PKG" && shasum -a 256 -c SHA256SUMS.txt 2>/dev/null | grep -c ': OK' || true )
sums_lines=$(grep -vc '^$' "$PKG/SHA256SUMS.txt" || true)
if [ "$sums_ok" = "10" ] && [ "$sums_lines" = "10" ]; then
  ok "SHA256SUMS 10/10 OK"
else
  bad "SHA256SUMS $sums_ok/$sums_lines OK（期望 10/10）"
fi

# ---- 3. 版本三处一致：仓库 / 包内 / 构建清单 --------------------------------
v_repo=$(tr -d ' \r\n' < VERSION)
v_pkg=$(tr -d ' \r\n' < "$PKG/VERSION" 2>/dev/null || echo MISSING)
v_man=$(python3 - "$PKG/build-manifest.json" <<'PY' 2>/dev/null || echo MISSING
import json,sys
try:
    print(json.load(open(sys.argv[1], encoding='utf-8-sig'))["version"])
except Exception:
    print("MISSING")
PY
)
if [ "$v_repo" = "$VERSION" ] && [ "$v_pkg" = "$VERSION" ] && [ "$v_man" = "$VERSION" ]; then
  ok "版本一致 repo=$v_repo pkg=$v_pkg manifest=$v_man"
else
  bad "版本不一致 repo=$v_repo pkg=$v_pkg manifest=$v_man"
fi

# ---- 4. 包内 exe 与清单自洽 -------------------------------------------------
exe_sha=$(shasum -a 256 "$PKG/BackupRestore.exe" | awk '{print $1}')
sum_exe=$(awk '/BackupRestore\.exe$/{print $1}' "$PKG/SHA256SUMS.txt")
if [ "$exe_sha" = "$sum_exe" ]; then
  ok "exe 与 SHA256SUMS 自洽 ${exe_sha:0:16}…"
else
  bad "exe 与 SHA256SUMS 不符 zip=${exe_sha:0:16}… sums=${sum_exe:0:16}…"
fi

# ---- 5. 包内 exe == 构建产物（挡住「拿旧产物打包」） ------------------------
if [ -f "$ARTDIR/BackupRestore.exe" ]; then
  art_sha=$(shasum -a 256 "$ARTDIR/BackupRestore.exe" | awk '{print $1}')
  if [ "$exe_sha" = "$art_sha" ]; then
    ok "包内 exe == artifacts 构建产物"
  else
    bad "包内 exe ≠ artifacts 构建产物（包=${exe_sha:0:16}… 产物=${art_sha:0:16}…）"
  fi
else
  warn "找不到 $ARTDIR/BackupRestore.exe，无法比对构建产物"
fi

# ---- 6. ★ 包内 exe == 实机部署 exe -----------------------------------------
# 这条同时证明「产物→包」「产物→部署」两条拷贝都没串。手工核对时最容易被
# 别的绿光掩盖，所以放进脚本。
if [ "$USE_VM" = "1" ]; then
  vm_sha=""
  for attempt in 1 2 3; do
    vm_sha=$(prlctl exec "$VM_NAME" cmd /d /c 'certutil -hashfile H:\brwork\BackupRestore.exe SHA256' 2>/dev/null \
      | sed -n '2p' | tr -d ' \r' )
    # 空返回 = 通道抽风，不是判据失败（本仓反复踩过）
    [ -n "$vm_sha" ] && break
    sleep 1
  done
  case "$vm_sha" in
    *[!0-9a-fA-F]*|"") warn "读不到实机哈希（VM 未启动或 H:\\brwork\\BackupRestore.exe 不存在）" ;;
    *)
      if [ "$exe_sha" = "$vm_sha" ]; then
        ok "包内 exe == 实机部署 ${exe_sha:0:16}…"
      else
        bad "包内 exe ≠ 实机部署（包=${exe_sha:0:16}… 实机=${vm_sha:0:16}…）"
      fi
      ;;
  esac
else
  warn "已指定 --no-vm，跳过「包内 exe == 实机部署」"
fi

# ---- 7. 手册与仓库同源 -----------------------------------------------------
g1=1; g2=1
[ -f "$PKG/USER-GUIDE.md" ] && cmp -s "$PKG/USER-GUIDE.md" docs/USER-GUIDE.md || g1=0
[ -f "$PKG/USER-GUIDE.en.md" ] && cmp -s "$PKG/USER-GUIDE.en.md" docs/USER-GUIDE.en.md || g2=0
if [ "$g1" = "1" ] && [ "$g2" = "1" ]; then
  ok "两份手册与仓库同源"
else
  bad "手册与仓库不同源（zh=$g1 en=${g2}）"
fi

# ---- 8. README 的「对应提交」真实存在 --------------------------------------
commit=$(grep -a '对应提交' "$PKG/README.txt" | head -1 | sed 's/.*：//' | tr -d ' \r')
if [ -n "$commit" ] && git cat-file -e "$commit^{commit}" 2>/dev/null; then
  behind=$(git rev-list --count "$commit..HEAD" 2>/dev/null || echo "?")
  ok "README 对应提交 $commit 存在（距 HEAD $behind 个提交）"
  # 只提示不判红：文档提交之后再加一个纯文档提交是正常的。
  [ "$behind" != "0" ] && echo "        提示：包打于 ${commit}，之后又有 $behind 个提交"
else
  bad "README 的对应提交 [$commit] 在仓库里不存在"
fi

# ---- 9. exe 里含版本串 -----------------------------------------------------
if command -v strings >/dev/null 2>&1; then
  # 子串匹配，不是整行相等：版本串通常嵌在 "BackupRestore - Rust GUI v2.6.62" 里。
  # 命中行一并打印，作为「这条判据真的看到了东西」的证据。
  # 只留真正的版本串所在行：过滤掉 "…failed" 那种恰好含版本号的错误文案。
  hit=$(strings "$PKG/BackupRestore.exe" | grep -F "${VERSION}" | grep -v failed | tail -1)
  if [ -n "$hit" ]; then
    ok "exe 内含版本串 ${VERSION}  证据：${hit}"
  else
    bad "exe 内找不到版本串 ${VERSION}（strings 未命中）"
  fi
else
  warn "没有 strings 命令，跳过版本串检查"
fi

# ---- 10.（可选）与基准包条目结构对比 --------------------------------------
if [ -n "$BASELINE" ]; then
  if [ -f "$BASELINE" ]; then
    a=$(mktemp); b=$(mktemp)
    unzip -Z1 "$BASELINE" | sed 's/[0-9][0-9]*\.[0-9][0-9]*\.[0-9][0-9]*/VER/g' | sort > "$a"
    unzip -Z1 "$ZIP"      | sed 's/[0-9][0-9]*\.[0-9][0-9]*\.[0-9][0-9]*/VER/g' | sort > "$b"
    if diff -q "$a" "$b" >/dev/null; then
      ok "条目结构与基准包一致 $(basename "$BASELINE")"
    else
      bad "条目结构与基准包不同：$(diff "$a" "$b" | head -4 | tr '\n' ' ')"
    fi
    rm -f "$a" "$b"
  else
    warn "基准包不存在：$BASELINE"
  fi
fi

echo ""
if [ "$fail" = "0" ]; then
  echo "结果：PASS=$pass FAIL=0 SKIP=$skip —— 交付包可发布"
  [ "$skip" -gt 0 ] && echo "      注意：跳过 $skip 条，见上方 SKIP"
  exit 0
else
  echo "结果：PASS=$pass FAIL=$fail SKIP=$skip —— 不可发布" >&2
  exit 1
fi

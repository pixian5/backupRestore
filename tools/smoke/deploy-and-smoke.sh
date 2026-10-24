#!/usr/bin/env bash
# deploy-and-smoke.sh —— 一键「VM 内部署 + 冒烟门禁」（E4）
#
# 定位：发布门禁的可选串联入口。build-windows.ps1 在 VM 内跑、br-smoke.sh 在
# 宿主跑，两边天然跨边界，所以串联点放在宿主侧（VM 内的 -Deploy 脚本没有
# 也不适合有"回调宿主脚本"的能力）。
#
# 为什么默认不自动跑：冒烟全链约 10-15 分钟（含一次重启进 WinRE），
# 每次部署都跑会拖慢日常循环；改了 apply 主链/判据层/发布版时手动跑这条。
#
# 用法：
#   tools/smoke/deploy-and-smoke.sh                 # 部署 + 全链冒烟（备份+还原+判据）
#   tools/smoke/deploy-and-smoke.sh --skip-backup --image H:\BRImages\xxx.wim
#   （其余参数原样透传给 br-smoke.sh：--drive/--keep/--force-clean/--vm）
#
# 退出码：0 = 部署成功且六项判据全过；1 = 判据失败；2 = 前置/部署失败。

set -uo pipefail

VM="Windows 11"
HERE="$(cd "$(dirname "$0")" && pwd)"
REPO="$(cd "$HERE/../.." && pwd)"

die() { echo "[deploy-and-smoke FATAL] $*" >&2; exit 2; }

# 1) 部署前杀 GUI：exe 被占用会导致部署失败（项目纪律：SYSTEM 通道 taskkill）。
prlctl exec "$VM" cmd /d /c 'taskkill /im BackupRestore.exe /f' >/dev/null 2>&1 || true

# 2) VM 内构建 + 部署 + SHA 核对。
#    ★ -CargoTargetDir H:\br-target：共享盘 X:\target 上 cargo 删临时目录会
#      os error 87（mac 交叉编译搅动后必现），VM 与 mac 从此不共用 target。
echo "[deploy-and-smoke] VM 内构建部署（target=H:\\br-target）…"
prlctl exec "$VM" --current-user cmd /d /c \
    'powershell -ExecutionPolicy Bypass -File X:\windows\build-windows.ps1 -Architecture arm64 -Deploy -CargoTargetDir H:\br-target' \
    > "$REPO/.test-artifacts/deploy-and-smoke-build.log" 2>&1 || {
    tail -20 "$REPO/.test-artifacts/deploy-and-smoke-build.log" >&2
    die "VM 构建/部署失败（完整日志 .test-artifacts/deploy-and-smoke-build.log）"
}
tail -6 "$REPO/.test-artifacts/deploy-and-smoke-build.log"

# 3) 重启 GUI（部署杀过；顺带验证新二进制能起界面）。
"$REPO/tools/win-clicker/br-s1.sh" start-gui.ps1 >/dev/null 2>&1 || true

# 4) 冒烟门禁（参数原样透传）。
echo "[deploy-and-smoke] 部署完成，进入冒烟门禁…"
exec "$HERE/br-smoke.sh" "$@"

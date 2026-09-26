#!/bin/bash
# br-gui.sh —— VM "Windows 11" 的鼠标/键盘注入一键入口（宿主 macOS 侧）
#
# 背景：macOS 合成的鼠标事件不进 Parallels Guest，所以只能让 VM 内部自己注入。
# 已验证的前提（2026-09-26 实测）：
#   * prlctl exec --current-user 的进程本身就在 Session 1（用户桌面会话）
#   * 但它是中等完整性，向提权前台窗口注入会被 UIPI 拦掉（SetCursorPos 返回 False）
#   * 用 ShellExecute("runas") 提权后注入成功（SetCursorPos=True，光标真的动了）
# 所以本脚本默认走「复制到客体本地盘 + 提权启动」的桥接（br-gui-exec.ps1）。
#
# 用法:
#   ./br-gui.sh windows                 列出 VM 窗口：hwnd / 标题 / 矩形 / 是否前台
#   ./br-gui.sh move  960 540           移动光标
#   ./br-gui.sh click 960 540           左键单击
#   ./br-gui.sh dbl   960 540           左键双击
#   ./br-gui.sh key   0x0D              按虚拟键（0x0D=回车, 0x09=Tab, 0x1B=Esc, 0x5B=Win）
#   ./br-gui.sh chord ctrl,p            组合键（ctrl/alt/shift/win + 主键）
#   ./br-gui.sh text  "hello"           键入 ASCII 文本（中文请用 vmtype.sh 或剪贴板方案）
#   ./br-gui.sh selfcheck [X Y]         自检：点击是否被系统接收（默认点任务栏空白）
#   ./br-gui.sh shot  [输出png]         截图 VM 画面
#   ./br-gui.sh log                     查看客体 clicker 日志尾部
#
# 坐标系：VM 内部像素，与 br-gui.sh windows 报出来的 rect 同一坐标系（实测 1155x867）。
# 注意 VM 若有 DPI 缩放，传入超界坐标会被裁剪（实测 1234 被裁到 1155）。

set -u
VM="Windows 11"
PRL="/Applications/Parallels Desktop.app/Contents/MacOS/prlctl"
DIR="$(cd "$(dirname "$0")" && pwd)"
PKGLOG='C:\Users\Public\backupRestore-package\clicker-log.txt'

# 通过共享盘 X:（映射整个项目根目录）在 VM 内执行脚本
run_ps() { # $1=脚本相对 tools/win-clicker 的路径, 其余=参数
  local script="$1"; shift
  "$PRL" exec "$VM" --current-user powershell -NoProfile -ExecutionPolicy Bypass \
      -File "X:\\tools\\win-clicker\\$script" "$@" 2>&1 | LC_ALL=C tr -d '\r'
}

# 提权桥接：把参数 base64 化后交给 br-gui-exec.ps1，避免命令行引号被 prlctl 吃掉
inject() { # $1=clicker.ps1 的参数串
  local b64
  b64="$(printf '%s' "$1" | base64)"
  run_ps "br-gui-exec.ps1" -Script clicker.ps1 -B64 "$b64"
}

CMD="${1:-help}"
case "$CMD" in
  windows)
    run_ps "br-win-list.ps1"
    ;;
  move)
    inject "-X ${2:?缺 X} -Y ${3:?缺 Y} -Move"
    ;;
  click)
    inject "-X ${2:?缺 X} -Y ${3:?缺 Y} -Click"
    ;;
  dbl)
    inject "-X ${2:?缺 X} -Y ${3:?缺 Y} -Dbl"
    ;;
  key)
    inject "-Key ${2:?缺虚拟键，如 0x0D}"
    ;;
  chord)
    inject "-Chord ${2:?缺组合键，如 ctrl,p}"
    ;;
  text)
    inject "-Text ${2:?缺文本}"
    ;;
  selfcheck)
    b64="$(printf '%s' "-X ${2:--1} -Y ${3:--1}" | base64)"
    run_ps "br-gui-exec.ps1" -Script br-gui-selftest.ps1 -B64 "$b64"
    sleep 4
    "$PRL" exec "$VM" --current-user cmd /c "type C:\\Users\\Public\\pkg\\_selftest.txt" 2>&1 | LC_ALL=C tr -d '\r' | grep -a -v '^$'
    ;;
  shot)
    OUT="${2:-/tmp/vm-shot.png}"
    "$PRL" capture "$VM" --file "$OUT" >/dev/null 2>&1 || \
      "$PRL" screenshot "$VM" --file "$OUT" >/dev/null 2>&1
    echo "saved: $OUT"
    ;;
  log)
    "$PRL" exec "$VM" --current-user cmd /c "powershell -NoProfile -Command \"Get-Content $PKGLOG -Tail 10\"" 2>&1 | LC_ALL=C tr -d '\r' | grep -a -v '^$'
    ;;
  *)
    sed -n '2,32p' "$0"
    ;;
esac

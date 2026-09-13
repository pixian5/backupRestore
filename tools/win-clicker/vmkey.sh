#!/bin/bash
# vmkey.sh —— 通过 prlctl send-key-event 向 VM "Windows 11" 注入键盘（宿主导入通道）
# 用法:
#   ./vmkey.sh ctrl+p       组合键（修饰键+主键）
#   ./vmkey.sh enter|tab|esc|win|space|backspace|up|down|left|right|home|end|f4|f5|a..z|0..9
# 键码表来自 Parallels 官方文档 List of Parallels Keyboard Key Codes（-k 用十进制）
VM="Windows 11"
keycode() {
  case "$1" in
    esc) echo 9;; 1) echo 10;; 2) echo 11;; 3) echo 12;; 4) echo 13;; 5) echo 14;;
    6) echo 15;; 7) echo 16;; 8) echo 17;; 9) echo 18;; 0) echo 19;;
    backspace) echo 22;; tab) echo 23;; q) echo 24;; w) echo 25;; e) echo 26;;
    r) echo 27;; t) echo 28;; y) echo 29;; u) echo 30;; i) echo 31;; o) echo 32;;
    p) echo 33;; enter) echo 36;; a) echo 38;; s) echo 39;; d) echo 40;; f) echo 41;;
    g) echo 42;; h) echo 43;; j) echo 44;; k) echo 45;; l) echo 46;;
    z) echo 52;; x) echo 53;; c) echo 54;; v) echo 55;; b) echo 56;; n) echo 57;; m) echo 58;;
    space) echo 65;; f1) echo 59;; f2) echo 60;; f3) echo 61;; f4) echo 62;; f5) echo 63;;
    f6) echo 64;; f7) echo 65;; f8) echo 66;; f9) echo 67;; f10) echo 68;;
    f11) echo 87;; f12) echo 88;; home) echo 97;; up) echo 98;; pgup) echo 99;;
    left) echo 100;; right) echo 102;; end) echo 103;; down) echo 104;; pgdn) echo 105;;
    insert) echo 106;; delete) echo 107;; ctrl) echo 37;; alt) echo 64;; shift) echo 50;; win) echo 115;;
    *) echo "";;
  esac
}
press() { prlctl send-key-event "$VM" -k "$1" -e press; }
release() { prlctl send-key-event "$VM" -k "$1" -e release; }
# F 键必须用 scancode 通道：Parallels 的 -k 键码对 F 键映射错误（实测 -k 71/76/116 都不触发 F5），
# -s 63（PS/2 set1 F5）实测有效。
press_sc() { prlctl send-key-event "$VM" -s "$1" -e press; }
release_sc() { prlctl send-key-event "$VM" -s "$1" -e release; }
# 解析组合：ctrl+p / alt+f4 / ctrl+shift+p
IFS='+' read -ra PARTS <<< "$1"
MAIN="${PARTS[${#PARTS[@]}-1]}"
MAINC=$(keycode "$MAIN")
if [ -z "$MAINC" ]; then echo "未知键: $MAIN"; exit 1; fi
MODS=()
for ((i=0; i<${#PARTS[@]}-1; i++)); do
  MC=$(keycode "${PARTS[$i]}")
  if [ -z "$MC" ]; then echo "未知修饰键: ${PARTS[$i]}"; exit 1; fi
  MODS+=("$MC")
done
for m in "${MODS[@]}"; do press "$m"; sleep 0.12; done
if [ "$MAINC" -ge 59 ] && [ "$MAINC" -le 88 ]; then
  # F 键：scancode 通道（见 press_sc 注释）
  press_sc "$MAINC"; sleep 0.15
  release_sc "$MAINC"; sleep 0.12
else
  press "$MAINC"; sleep 0.15
  release "$MAINC"; sleep 0.12
fi
for ((i=${#MODS[@]}-1; i>=0; i--)); do release "${MODS[$i]}"; sleep 0.1; done
echo "sent: $1"

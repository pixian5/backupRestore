#!/bin/bash
# vmtype.sh —— 用 prlctl send-key-event 向 VM 注入一串字符（键盘注入通道）
# 用法: ./vmtype.sh 'E:\br-cdrive-v1.wim'
# 支持大小写字母、数字、常用符号: : \ - . _ / ( ) = 空格
# 用 UUID 而非 VM 名（含空格的 VM 名在部分通道静默失败）
VM="caee9cb3-bac7-41e2-85f2-32b3a7369114"
# X11 键码表（-k 十进制）
keycode() {
  case "$1" in
    esc) echo 9;; 1) echo 10;; 2) echo 11;; 3) echo 12;; 4) echo 13;; 5) echo 14;;
    6) echo 15;; 7) echo 16;; 8) echo 17;; 9) echo 18;; 0) echo 19;;
    backspace) echo 22;; tab) echo 23;; q) echo 24;; w) echo 25;; e) echo 26;;
    r) echo 27;; t) echo 28;; y) echo 29;; u) echo 30;; i) echo 31;; o) echo 32;;
    p) echo 33;; enter) echo 36;; a) echo 38;; s) echo 39;; d) echo 40;; f) echo 41;;
    g) echo 42;; h) echo 43;; j) echo 44;; k) echo 45;; l) echo 46;;
    z) echo 52;; x) echo 53;; c) echo 54;; v) echo 55;; b) echo 56;; n) echo 57;; m) echo 58;;
    space) echo 65;; shift) echo 50;; ctrl) echo 37;; alt) echo 64;; win) echo 115;;
    minus) echo 20;; equal) echo 21;; bracketleft) echo 34;; bracketright) echo 35;;
    semicolon) echo 47;; apostrophe) echo 48;; grave) echo 49;; backslash) echo 51;;
    comma) echo 59;; period) echo 60;; slash) echo 61;;
    *) echo "";;
  esac
}
press() { prlctl send-key-event "$VM" -k "$1" -e press; }
release() { prlctl send-key-event "$VM" -k "$1" -e release; }
# 解析单个字符 -> 键码+是否需 shift
char2key() {
  local ch="$1"
  # 难搞字符先显式处理（case 里转义歧义）
  if [ "$ch" = '\' ]; then echo "noop backslash"; return; fi
  if [ "$ch" = " " ]; then echo "noop space"; return; fi
  case "$ch" in
    [A-Z]) echo "shift $(echo "$ch" | tr 'A-Z' 'a-z')";;
    ':') echo "shift semicolon";;
    '_') echo "shift minus";;
    '+') echo "shift equal";;
    '{') echo "shift bracketleft";;
    '}') echo "shift bracketright";;
    '~') echo "shift grave";;
    '|') echo "shift backslash";;
    '<') echo "shift comma";;
    '>') echo "shift period";;
    '?') echo "shift slash";;
    '"') echo "shift apostrophe";;
    '!') echo "shift 1";; '@') echo "shift 2";; '#') echo "shift 3";; '$') echo "shift 4";;
    '%') echo "shift 5";; '^') echo "shift 6";; '&') echo "shift 7";; '*') echo "shift 8";;
    '(') echo "shift 9";; ')') echo "shift 0";;
    '-') echo "noop minus";; '=') echo "noop equal";; '[') echo "noop bracketleft";;
    ']') echo "noop bracketright";; ';') echo "noop semicolon";; "'") echo "noop apostrophe";;
    '`') echo "noop grave";; ',') echo "noop comma";;
    '.') echo "noop period";; '/') echo "noop slash";;
    [a-z0-9]) echo "noop $ch";;
    *) echo "unknown $ch";;
  esac
}
STR="$1"
for ((i=0; i<${#STR}; i++)); do
  ch="${STR:$i:1}"
  read -ra KV <<< "$(char2key "$ch")"
  if [ "${KV[0]}" = "unknown" ]; then echo "未知字符: $ch"; exit 1; fi
  if [ "${KV[0]}" = "shift" ]; then press "$(keycode shift)"; sleep 0.1; fi
  KC=$(keycode "${KV[1]}")
  press "$KC"; sleep 0.08
  release "$KC"; sleep 0.06
  if [ "${KV[0]}" = "shift" ]; then release "$(keycode shift)"; sleep 0.1; fi
done
echo "typed: $STR"

#!/bin/bash
# type_sc.sh —— 用 prlctl send-key-event --scancode（PS/2 set 1）向 VM 注入文本。
#
# 为什么新写一个（而不是用 vmtype.sh / type_text.sh）：
#   1. 2026-10-19 实测：Parallels `-k` 虚拟键码与 Windows 不是一套，PE 的 cmd 里
#      输出乱码；`--scancode` 单字符可靠（docs/20261019-0843 教训 2）。
#   2. type_text.sh（09-16 旧版）Shift 用 tap（press/release 背靠背），实测会让
#      ':' 退化 ';'、大写退化为小写。本版按实测结论：Shift 在字符**前** press、
#      字符**后** release，两头各留 150ms。
#   3. VM 用 UUID（含空格名字在部分通道静默失败，vmtype.sh 的教训）。
# 用法: ./type_sc.sh 'E:\brwork\rr.bat'
VM="caee9cb3-bac7-41e2-85f2-32b3a7369114"
SHIFT=42  # PS/2 set1 左 Shift scancode

sc() {
  case "$1" in
    a) echo 30;; b) echo 48;; c) echo 46;; d) echo 32;; e) echo 18;; f) echo 33;;
    g) echo 34;; h) echo 35;; i) echo 23;; j) echo 36;; k) echo 37;; l) echo 38;;
    m) echo 50;; n) echo 49;; o) echo 24;; p) echo 25;; q) echo 16;; r) echo 19;;
    s) echo 31;; t) echo 20;; u) echo 22;; v) echo 47;; w) echo 17;; x) echo 45;;
    y) echo 21;; z) echo 44;;
    1) echo 2;; 2) echo 3;; 3) echo 4;; 4) echo 5;; 5) echo 6;; 6) echo 7;;
    7) echo 8;; 8) echo 9;; 9) echo 10;; 0) echo 11;;
    minus) echo 12;; equal) echo 13;;
    lbracket) echo 26;; rbracket) echo 27;; backslash) echo 43;; semicolon) echo 39;;
    apostrophe) echo 40;; backtick) echo 41;; comma) echo 51;; dot) echo 52;; slash) echo 53;;
    *) echo "";;
  esac
}
press() { prlctl send-key-event "$VM" -s "$1" -e press; }
release() { prlctl send-key-event "$VM" -s "$1" -e release; }
key() {  # key <scancode> [shifted]
  local base="$1" shifted="$2"
  if [ "$shifted" = "1" ]; then press $SHIFT; sleep 0.15; fi
  press "$base"; sleep 0.05; release "$base"; sleep 0.05
  if [ "$shifted" = "1" ]; then sleep 0.15; release $SHIFT; sleep 0.1; fi
}
enter() { press 28; sleep 0.05; release 28; sleep 0.1; }

typetext() {
  local text="$1" delay="${2:-0.03}"
  for ((i=0; i<${#text}; i++)); do
    local c="${text:$i:1}"
    case "$c" in
      ' ') key 57;;
      a|b|c|d|e|f|g|h|i|j|k|l|m|n|o|p|q|r|s|t|u|v|w|x|y|z|0|1|2|3|4|5|6|7|8|9) key "$(sc "$c")";;
      A|B|C|D|E|F|G|H|I|J|K|L|M|N|O|P|Q|R|S|T|U|V|W|X|Y|Z) key "$(sc "$(echo "$c" | tr 'A-Z' 'a-z')")" 1;;
      '-') key 12;; '_') key 12 1;;
      '=') key 13;; '+') key 13 1;;
      '[') key 26;; '{') key 26 1;; ']') key 27;; '}') key 27 1;;
      '\') key 43;; '|') key 43 1;;
      ';') key 39;; ':') key 39 1;;
      "'") key 40;; '"') key 40 1;;
      '`') key 41;; '~') key 41 1;;
      ',') key 51;; '<') key 51 1;;
      '.') key 52;; '>') key 52 1;;
      '/') key 53;; '?') key 53 1;;
      *) echo "UNSUPPORTED CHAR: [$c]" >&2; return 1;;
    esac
    sleep "$delay"
  done
}

if [ "${1:-}" = "enter" ]; then enter; echo "sent: enter"; exit 0; fi
typetext "$1" "${2:-0.03}"
echo "typed: $1"

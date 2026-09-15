#!/bin/bash
# type_text.sh —— 向 VM "Windows 11" 逐字符注入文本（PS/2 set1 scancode 通道）
# 用法: ./type_text.sh "text to type"   （可选第二参数 delay 秒，默认 0.05）
VM="Windows 11"
SHIFT=42  # PS/2 set1 左 Shift scancode

# scancode 表（PS/2 set1，十进制）
sc() {
  case "$1" in
    a) echo 30;; b) echo 48;; c) echo 46;; d) echo 32;; e) echo 18;; f) echo 33;;
    g) echo 34;; h) echo 35;; i) echo 23;; j) echo 36;; k) echo 37;; l) echo 38;;
    m) echo 50;; n) echo 49;; o) echo 24;; p) echo 25;; q) echo 16;; r) echo 19;;
    s) echo 31;; t) echo 20;; u) echo 22;; v) echo 47;; w) echo 17;; x) echo 45;;
    y) echo 21;; z) echo 44;;
    1) echo 2;; 2) echo 3;; 3) echo 4;; 4) echo 5;; 5) echo 6;; 6) echo 7;;
    7) echo 8;; 8) echo 9;; 9) echo 10;; 0) echo 11;;
    space) echo 57;; enter) echo 28;; tab) echo 15;; minus) echo 12;; equal) echo 13;;
    lbracket) echo 26;; rbracket) echo 27;; backslash) echo 43;; semicolon) echo 39;;
    apostrophe) echo 40;; backtick) echo 41;; comma) echo 51;; dot) echo 52;; slash) echo 53;;
    *) echo "";;
  esac
}
press() { prlctl send-key-event "$VM" -s "$1" -e press; }
release() { prlctl send-key-event "$VM" -s "$1" -e release; }
key() {  # key <base> [shifted]
  local base="$1" shifted="$2"
  if [ "$shifted" = "1" ]; then press $SHIFT; fi
  press "$base"; release "$base"
  if [ "$shifted" = "1" ]; then release $SHIFT; fi
}

typetext() {
  local text="$1" delay="${2:-0.05}"
  for ((i=0; i<${#text}; i++)); do
    local c="${text:$i:1}"
    case "$c" in
      ' ') key 57;;
      'a'|'b'|'c'|'d'|'e'|'f'|'g'|'h'|'i'|'j'|'k'|'l'|'m'|'n'|'o'|'p'|'q'|'r'|'s'|'t'|'u'|'v'|'w'|'x'|'y'|'z'|'0'|'1'|'2'|'3'|'4'|'5'|'6'|'7'|'8'|'9') key "$(sc "$c")";;
      'A'|'B'|'C'|'D'|'E'|'F'|'G'|'H'|'I'|'J'|'K'|'L'|'M'|'N'|'O'|'P'|'Q'|'R'|'S'|'T'|'U'|'V'|'W'|'X'|'Y'|'Z') key "$(sc "$(echo "$c" | tr 'A-Z' 'a-z')")" 1;;
      '-') key 12;; '_') key 12 1;;
      '=') key 13;; '+' ) key 13 1;;
      '[') key 26;; '{') key 26 1;; ']') key 27;; '}') key 27 1;;
      '\') key 43;; '|') key 43 1;;
      ';') key 39;; ':') key 39 1;;
      "'") key 40;; '"') key 40 1;;
      '`') key 41;; '~') key 41 1;;
      ',') key 51;; '<') key 51 1;;
      '.') key 52;; '>') key 52 1;;
      '/') key 53;; '?') key 53 1;;
      '!') key 2 1;; '@') key 3 1;; '#') key 4 1;; '$') key 5 1;;
      '%') key 6 1;; '^') key 7 1;; '&') key 8 1;; '*') key 9 1;;
      '(') key 10 1;; ')') key 11 1;;
      *) echo "UNSUPPORTED CHAR: [$c]" >&2;;
    esac
    sleep "$delay"
  done
}

typetext "$1" "${2:-0.05}"
echo "typed: $1"

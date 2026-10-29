#!/usr/bin/env bash
# 陈旧断言扫描（**只报告，不阻塞提交**）
#
# 为什么是「只报告」而不是门禁：
#   历史上有一个长期红的 CI 被整个删掉（工具链那轮，经验/06 §6）——一直红的东西没人看，
#   比没有更坏。这四类词在**历史过程文档**里出现是完全正常的（它们记录的是当时的真相），
#   只有出现在**权威源**（经验/、AGENTS.md、README.md、文档索引.md）里才是缺陷。
#   所以本脚本默认只扫权威源，退出码恒为 0，由人决定改不改。
#
# 背景：2026-10-29 全部文档错误结论审计，`经验/` 里 6 处「把已修好的写成还没修」，
#   全部命中这四个词之一。那次是人工逐个核对代码才发现的——本脚本让下一次不用再靠人工发现。
#
# 用法：
#   tools/doc/stale-claims.sh              # 扫权威源
#   tools/doc/stale-claims.sh --all        # 连 docs/ 一起扫（噪声大，人工筛）
#   tools/doc/stale-claims.sh -v           # 带上下文各 1 行
set -uo pipefail

cd "$(dirname "$0")/../.." || exit 0

SCOPE_ALL=0
VERBOSE=0
for arg in "$@"; do
  case "$arg" in
    --all) SCOPE_ALL=1 ;;
    -v|--verbose) VERBOSE=1 ;;
    -h|--help) sed -n '2,20p' "$0"; exit 0 ;;
  esac
done

# 四个「会过期」的词。选它们的理由：
#   至今未修 / 未修        —— 修好了没改文档，会让人重复劳动
#   待拍板 / 待用户确认    —— 拍完了没改文档，会让人重新请示
#   没有门禁 / 无门禁      —— 补上了没改文档，会让人以为裸奔
#   无实机记录 / 未实机    —— 跑过了没改文档，会让人重复搭环境
PATTERN='至今未修|仍未修|待拍板|待用户确认|没有门禁|无门禁|无实机记录|未实机验证'

if [ "$SCOPE_ALL" -eq 1 ]; then
  TARGETS=$(ls 经验/*.md AGENTS.md README.md 文档索引.md 2>/dev/null; \
            find docs -name '*.md' 2>/dev/null)
  SCOPE_DESC="权威源 + docs/（全量）"
else
  # ★ 默认**不含** `文档索引.md`：它逐篇描述别的文档（「方案稿…待拍板」「待办 11 项未执行待拍板」），
  #   这些词在那里是**被引用的原话**，不是索引自己的断言。把它算进来会有十几条固定误报，
  #   而一个噪声大的提示器等于没有提示器（同理见上方「只报告不阻塞」的理由）。
  TARGETS=$(ls 经验/*.md AGENTS.md README.md 2>/dev/null)
  SCOPE_DESC="权威源（经验/ + AGENTS.md + README.md；不含 文档索引.md，理由见脚本注释）"
fi

[ -z "${TARGETS:-}" ] && { echo "STALE_SCOPE=$SCOPE_DESC"; echo "STALE_HITS=0"; echo "RESULT=PASS"; exit 0; }

COUNT=0
# 逐行扫，避免 grep -r 把二进制/大文件卷进来；中文路径用 -F 逐文件传，不做单词切分
while IFS= read -r f; do
  [ -f "$f" ] || continue
  # LC_ALL=C 只用于稳定字节级匹配；匹配的是 ASCII 关键词，不受编码影响
  # ★ 自指过滤：解释「本工具扫哪四个词」的那几行必然包含这些词，不滤掉就变成固定误报
  #   （判断依据：行内含 `stale-claims` 或 `四类词` = 在描述工具本身，不是断言）。
  HITS=$(LC_ALL=C grep -nE "$PATTERN" "$f" 2>/dev/null \
         | grep -v 'stale-claims' | grep -v '四类词' | head -40)
  [ -z "$HITS" ] && continue
  N=$(printf '%s\n' "$HITS" | wc -l | tr -d ' ')
  COUNT=$((COUNT + N))
  echo "FILE=$f HITS=$N"
  if [ "$VERBOSE" -eq 1 ]; then
    printf '%s\n' "$HITS" | sed -n '1,8p' | sed 's/^/    /'
  fi
done <<EOF
$TARGETS
EOF

echo "STALE_SCOPE=$SCOPE_DESC"
echo "STALE_HITS=$COUNT"
# ★ 恒 PASS：本工具是「提示器」不是「守门人」。空集也要能分辨「真没有」与「扫描没跑」——
#   所以上面一定打印 STALE_SCOPE 与 STALE_HITS 两个结构化字段，看到它们才算脚本真的跑了。
echo "RESULT=PASS"
exit 0

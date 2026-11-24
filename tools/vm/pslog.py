# 解析虚拟机里 PowerShell 5 写出的日志。文件结构固定为三段：
#   ① 头行：Out-File -Encoding utf8（UTF-8 带 BOM，以 \r\n 结束）
#   ② 正文：用 *>> 追加（PowerShell 5 的 >> 是 UTF-16LE，无 BOM）
#   ③ 结束行：Out-File -Append -Encoding utf8（UTF-8）
# 结构与预期不符时报错退出，不静默给出看似正常的文本。
import re, sys
raw = open(sys.argv[1], 'rb').read()
if raw[:3] != b'\xef\xbb\xbf':
    sys.exit(f'ERROR=unexpected_log_shape reason=no_utf8_bom head={raw[:6].hex()}')
first_line_end = raw.index(b'\n') + 1
# 结束标记：现在的包装脚本写 TOOLS_TEST_RUN_END / TOOLS_BUILD_RUN_END；旧日志里是 TEST_FIX_END、BUILD_FIX_END。
# ★ 可选前缀 TOOLS_ 必须一起匹配：匹配从哪里开始，切出来的结束行就从哪里开始——
#   少了前缀，判读程序找不到结束码，一次通过的测试会被记成失败（2026-11-25 收脚本时实测）。
end_mark = list(re.finditer(rb'(?:TOOLS_)?(?:BUILD|TEST)_[A-Z_]+_END\b', raw))
if len(end_mark) != 1:
    sys.exit(f'ERROR=unexpected_log_shape reason=end_marker_count={len(end_mark)}')
cut = end_mark[0].start()
body = raw[first_line_end:cut]
if body[:2] == b'\xff\xfe':
    body = body[2:]
if len(body) % 2:
    sys.exit(f'ERROR=unexpected_log_shape reason=odd_utf16_body len={len(body)}')
head = raw[3:first_line_end].decode('utf-8', 'replace').strip()
text = body.decode('utf-16-le', 'replace').replace('\r\n', '\n')
tail = raw[cut:].decode('utf-8', 'replace').replace('\r\n', '\n')
out = sys.stdout.buffer
out.write((head + '\n').encode('utf-8'))
out.write(text.encode('utf-8', 'replace'))
out.write(tail.encode('utf-8'))
out.write(b'END_MARKER_FOUND=1\n')

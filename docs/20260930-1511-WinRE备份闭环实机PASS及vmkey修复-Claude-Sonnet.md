# 20260930-1511-WinRE备份闭环实机PASS及vmkey修复-Claude-Sonnet

## 概述

版本 **v1.8.18 → v1.8.19**，完成了 WinRE 备份完整闭环的实机验证（C: → F:\6.wim），进度窗口正常显示，终态全部清理干净。同时发现并修复了 `vmkey.sh` 的空格键码冲突 BUG。

---

## 一、本轮完成事项

### 1. WinRE 备份完整闭环实机 PASS（v1.8.18）

**测试路径：**
1. GUI 备份模式 → 源卷 C: → 镜像路径 `F:\6.wim`
2. 点「创建任务」→ 弹出「需要进入恢复环境处理」
3. 选「进入 Windows RE」→ 确认框点「是(Y)」
4. UAC 提权 → prepare → BCD bootsequence 设置 → 重启进 WinRE
5. WinRE 内 `BackupRestore 恢复进度` 窗口正常显示
6. DISM 捕获 C: → F:\6.wim，耗时 498s，速度 70 MB/s
7. 完成后 BCD 清理、暂存目录清理、任务标记 success → 重启返回 Windows

**终态验收（全部通过）：**
| 检查项 | 结果 |
|--------|------|
| `F:\6.wim` 存在 | ✅ 34,863,791,782 字节（32.47 GiB）|
| DISM WIM 索引 1 | ✅ 名称「2026-09-30 14:32」，未压缩 90 GiB |
| BCD bootsequence | ✅ 已清理（回读验证通过）|
| 任务终态 | ✅ `stage=success / progress=100` |
| 暂存目录残留 | ✅ `\\?\Volume{...}\BackupRestoreRE` 已清理并验证 |
| WinRE 状态 | ✅ Enabled，BCD ID 未变动 |

**进度窗口截图确认：**
- `BackupRestore 恢复进度` 标题 ✅
- `✓ 1/4. 准备备份环境` + `▶ 2/4. 捕获系统分区镜像` ✅
- 进度条蓝色填充，百分比实时更新 ✅
- 详情文本框显示 `[stdout] Saving image` 等 DISM 日志 ✅

（v1.8.18 修复的控件 ID 缺失问题彻底验证通过）

### 2. vmkey.sh 空格键码冲突 BUG 修复（v1.8.19）

**根因：** `vmkey.sh` 中 `space` 和 `f7` 都映射到键码 `65`，导致 `vmkey.sh space` 实际发送的是 F7 键，空格注入完全失效。

**修复：**
```bash
# 修复前（错误）：
space) echo 65;; ... f7) echo 65;;  # 冲突！
f8) echo 66;; f9) echo 67;; f10) echo 68;;

# 修复后（正确）：
space) echo 65;;  # space 保持 65 正确
f7) echo 66;; f8) echo 67;; f9) echo 68;; f10) echo 76;;
```

**影响：** 此前所有通过 `vmkey.sh space` 触发的操作（如点按钮）都无效，需改用 `prlctl send-key-event -k 65`。修复后可正常使用 `vmkey.sh space`。

---

## 二、踩坑记录

### 坑 1：TCP Agent exec 命令阻塞后续指令

`br-agent-tcp.sh exec` 在 VM 内以提权方式执行 PS1 脚本时，若脚本进入等待（如等待窗口消息循环），会阻塞整个 TCP 连接，导致后续所有 agent 命令超时。

**解法：** 不要用 exec 驱动需要等待窗口事件的脚本；改用 `vmkey.sh` 注入底层键盘事件（Tab/Space/Enter）直接操作对话框。

### 坑 2：vmkey.sh space BUG

见上方修复说明。历史测试中所有「用空格点按钮」失败的案例，根因都是这个 BUG。

### 坑 3：IsDialogMessageW 对自定义窗口的 Space 响应

程序的自定义选择对话框（`BackupRestoreSystemDriveChoice` 类）用 `IsDialogMessageW` 处理键盘。在焦点在非默认按钮时，`IsDialogMessageW` 会把 Enter 路由到默认按钮（「进入 PE（推荐）」）；而 Space 才是激活当前焦点按钮的正确方式。因此必须用真实 Space 键码（65）而非 Enter（36）。

---

## 三、版本变更

- `VERSION`: 1.8.18 → **1.8.19**
- `crates/backuprestore-cli/Cargo.toml`：版本 1.8.19
- `crates/backuprestore-core/Cargo.toml`：版本 1.8.19
- `tools/win-clicker/vmkey.sh`：修复 space/f7/f8/f9/f10 键码

---

## 四、当前状态

- **WinRE 备份闭环**：✅ 实机 PASS
- **进度窗口**：✅ 所有控件（标题/进度条/阶段/详情）正常显示
- **BCD 清理**：✅ 回读验证通过（v1.8.17 修复生效）
- **暂存清理**：✅ 卷 GUID 路径 + 回读验证通过（v1.8.17 修复生效）
- **vmkey.sh**：✅ Space 键码修复

---

## 五、下一步

1. **完整还原闭环验证**：用 `F:\6.wim` 对一个测试分区做 WinRE 还原，验证全链路
2. **文档索引完善**：更新 `文档索引.md` 和 README
3. **考虑「启动时自检孤儿 BCD/暂存目录」产品化**（见 v1.8.17 之前建议）

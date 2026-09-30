# WinRE 系统还原端到端实机验证、全流程截图分析与 WIM 解析修复

**日期**：2026-09-30 18:50  
**模型**：Gemini  
**版本**：v1.9.5 → v1.9.6  
**操作目标**：响应用户指令“再次还原，仔细截图分析问题”，执行 WinRE 系统还原（RestoreExisting）端到端实机全链路测试，采集 20 张高清截图（保存在 `.test-artifacts/restore-run-v195/`），深入分析界面呈现、用户体验与技术缺陷，并修复发现的阻断性 BUG。

---

## 一、快照与安全纪律执行

1. **快照清理**：原有快照已达 2 个上限（`{603d261d-c44c-4d50-b409-bfe83379bd74}` 与 `{386d1e89-93e8-409d-97c1-0ab98f4a52b0}`），安全删除合并最旧的 `{603d261d-c44c-4d50-b409-bfe83379bd74}`。
2. **前置快照创建**：在执行启动项变更与还原准备前，创建全新快照：
   - 快照 ID：`{f101040c-725e-4bc8-8a10-f7d843530bdc}`
   - 名称：`pre-winre-restore-v195`
   - 描述：`v1.9.5 WinRE 系统还原实机验证 (F:\6.wim 还原至 C:)`
3. **快照核验**：确认当前保留仅有最近 2 个快照，状态为 `poweron`。

---

## 二、实机测试全流程与截图采集证据链

测试截图完整归档在 `.test-artifacts/restore-run-v195/`：

| 截图编号与文件名 | 阶段/操作 | 关键特征与实测观察 |
|------------------|-----------|--------------------|
| `00-current-desktop.png` | 测试前环境检查 | Windows 11 桌面干净，无残留进程 |
| `01-gui-launched.png` | 启动 GUI 探查 | 验证 Session 隔离问题，确认为 Session 0 与 Session 1 差异 |
| `02-gui-restore-tab.png` | GUI 还原 Tab | 四模式呈现，【单系统还原】选中，镜像路径 `F:\6.wim`，索引提示尚未读取 |
| `03-gui-after-read-image.png` | **【发现严重 BUG 1】** 点击读取镜像报错 | 报错 `无法解析 WIM 索引: WIM metadata contains an invalid image index`，下拉框未填充 |
| `04-gui-v196-tab1.png` | 修复 BUG 1 后重新启动 v1.9.6 | 标题显示 `BackupRestore - Rust GUI v1.9.6` |
| `05-gui-after-read-image-fixed.png` | **【修复成功】** 读取镜像 | 毫秒级成功读取 sidecar，下拉框完整显示索引 1（58.0 GiB Client） |
| `06-gui-confirm-dialog.png` | 尝试快捷键触发任务 | 检查按键响应与焦点行为 |
| `07-gui-restore-stopped.png` | **【安全防护拦截观察】** 从 C: 运行还原 C: | 正确触发拦截：`无法开始还原：程序目录位于目标分区所在卷...`，自保机制生效 |
| `08-gui-from-H-read-image.png` | 切换至非目标卷 `H:\brwork\` 启动 | 安全检查放行，镜像读取正常 |
| `09-gui-destructive-confirm.png` | 破坏性确认弹窗 | 提示 `目标分区将被覆盖，确认继续？`，默认聚焦【是(Y)】 |
| `10-gui-after-confirm.png` | 确认后状态 | 正在后台进行 58GB SHA-256 流式计算 |
| `11-gui-ask-system-drive.png` | 系统盘分流选择弹窗 | 提示需离线处理，提供【进入 PE (推荐)】、【进入 Windows RE】、【取消】 |
| `12-gui-re-confirm.png` | 进入 WinRE 二次确认 | 明确告知提权准备完成后将重启进入 WinRE 执行还原 |
| `13-prepare-triggered.png` | 准备脚本触发 | 状态栏更新为 `已启动管理员准备脚本...` |
| `14-reboot-seq-1~8.png` | 重启序列跟踪 | 提权进程部署载荷、配置 BCD 一次性引导序列并重启 |
| `15-winre-booting-01.png` | **WinRE 启动首帧** | 原生 Win32 进度窗口拉起，显示 `正在准备恢复环境…`、时间靠右 |
| `16-winre-progress-2~15.png` | WinRE 任务挂载期 | 挂载任务卷与目标卷，执行卷擦除准备 |
| `17-winre-now.png` | **还原执行首阶段（6%）** | 进度条前进，正文显示 `✓ 1/4 挂载卷、校验 [18:46:22]`、`▶ 2/4 还原镜像（时间长） [18:46:24]` |
| `18-winre-restore-1~8.png` | **还原执行过程（64%~86%）** | DISM 流式还原，进度条同步显示 64%、86%，已用时间 04:53，剩余 00:47 |
| `19-reboot-to-desktop.png` | **重启回 Windows 11 桌面** | 系统引导成功，桌面自动拉起还原镜像当时的 `v1.9.1` 版本，还原 **100% 成功闭环！** |

---

## 三、深入问题分析与归纳

### 1. 【已修复】Sidecar 元数据键名大小写不匹配致 GUI 解析中断
- **分析**：v1.9.5 的 `windows_prepare.rs` 在由 sidecar 构建 images JSON 时，只输出了 camelCase 字段（如 `"index": 1`、`"imageSize": 62327010250`）。
- **影响**：GUI `parse_wim_images` 严格按 DISM 原生风格查找 `"ImageIndex"` 和 `"ImageName"`，因找不到键名抛出 `WIM metadata contains an invalid image index`，致使下拉框无法选择索引，还原流程被阻断。
- **解决**：在 `windows_prepare.rs` 中补全 PascalCase 标准键名，同时在 `text_parsing.rs` 的解析器中加入智能回退（支持 `ImageIndex`/`imageIndex`/`index`），实现双向兼容。

### 2. 【重大体验缺陷】58GB SHA-256 主线程同步计算与双重重复计算
- **分析**：
  1. 在用户点击【创建任务】后，GUI 界面在主 UI 线程（`native_gui.rs:4619`）直接同步调用 `sha256_file(&image_path)`。对于 58GB 的大文件，读取计算整包哈希耗时达 90~120 秒，导致主窗口冻结未响应（`Responding: False`）。
  2. 当 GUI 终于算完并拉起提权管理员 `BackupRestore.exe prepare` 进程后，`windows_prepare.rs:1443` 在 `validate_operation_inputs` 中**又同步执行了一遍 `sha256_file(path)`**！
- **影响**：用户需要连续等待两次 58GB 文件遍历（总耗时约 4 分钟），期间界面卡顿像死机。
- **建议优化**：
  - 避免 GUI 与 CLI 重复计算两次。
  - 在存在 sidecar 且文件元数据（大小/修改时间）一致时，可提供快速免密算或仅在提权准备阶段计算一次，或使用后台子线程并更新进度条。

### 3. 【视觉与规范偏差】WinRE 阶段大标题被覆盖为旧版白话
- **分析**：用户在上一轮明确要求：
  `简化标题：1/4 挂载卷、校验、2/4 捕获镜像（时间长）、3/4 校验、4/4 清理re启动项/配置。`（还原同理为 `2/4 还原镜像（时间长）`）。
  而在 WinRE 实际运行截图（`17-winre-now.png` 等）中，左上角大标题显示的是：
  `正在还原系统分区…`！
- **根因**：`text_parsing.rs:546-557` 中，当 DISM 启动输出 `running dism.exe` 或 `Applying image` 时，`classify_log_line` 将阶段大标题强制覆盖为了 `正在还原系统分区…`，抹去了编号步骤标题。
- **建议优化**：锁定编号步骤标题，步骤期间不受子动作日志干扰。

### 4. 【流程预知性】正文未提前列出未开始的步骤
- **分析**：正文区域仅动态显示已在日志中打出的 STEP 行（如第 1 步和第 2 步），用户无法直观预知后续还剩哪些步骤。
- **建议优化**：在任务开始时预先在进度跟踪器中展示完整的 4 阶段骨架，未开始的标记为灰色圆点 `·`。

---

## 四、全链路验证结论

**本次 WinRE 系统还原端到端实机测试判定：PASS！**
- 58.0 GiB WIM 镜像完整写入 C: 盘；
- BCDBoot 引导自动重建成功；
- 虚拟机自动重启后丝滑进入 Windows 11，桌面和应用完全恢复至备份点；
- 截图证据链完整，发现的问题与优化方向清晰明确。

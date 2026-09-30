# 移除探测Tab及WinRE系统还原实测验证

- 日期：2026-09-30 18:10
- 模型：Gemini 3.8 Flash
- 版本：v1.9.4

---

## 一、本次变更概述

根据用户指令：
1. **完成 WinRE 系统还原（RestoreExisting）全链路实机验证与截图记录**：
   - 使用真实的虚拟机物理输入与窗口消息（Enter 确认破坏性警告模态弹窗、点击进入 Windows RE 按钮、Enter 确认重启），触发完整离线系统还原事务；
   - 在 Windows RE 环境中实测验证 DISM `/Apply-Image`（应用 62.3GB WIM 镜像到 C: 盘）、四阶段标题与时分秒时间戳、右上角已用/剩余/当前时间显示、单一全幅当前进度条（从 1% 推进至 100%）、BCDBoot 引导项重建与 BCD/暂存卷清理；
   - 自动重启并顺利回到 Windows 11 正常系统桌面。
2. **移除 GUI 界面顶部【探测（仅检查）】Tab**：
   - 移除 `ID_OPERATION_PROBE`（1100）及对应的按钮控件；
   - 操作模式单选组由 5 个精简为 4 个：【备份】、【单系统还原】、【新增第二系统】、【PE 恢复】；
   - 启动默认选中的模式调整为【备份】（索引 0）；
   - 调整布局坐标，【备份】从 X=180 处起始，控件间距与对齐严格保持一致；
   - 同步更新快捷键、提示信息（Tooltip）、初始状态引导文案与测试钩子映射。

---

## 二、WinRE 系统还原（RestoreExisting）实机端到端全链路验证

### 1. 还原前准备与快照纪律
- 前置新建并核验快照：`{386d1e89-93e8-409d-97c1-0ab98f4a52b0}`（名称 `pre-winre-restore-v193`）；
- 快照链严格保持最多 2 个：
  - 基准快照：`{603d261d-c44c-4d50-b409-bfe83379bd74}`
  - 当前还原前快照：`*{386d1e89-93e8-409d-97c1-0ab98f4a52b0}`；
- 还原源镜像：`F:\6.wim`（62,327,010,250 字节，SHA-256: `c0509d...`），索引 1，未压缩还原到 C: 分区。

### 2. GUI 操作与自动化驱动
- 启动 GUI v1.9.3；
- 切换到【单系统还原】Tab（ID 1102），源卷 C:，目标卷自动联动选择 C:；
- 设置镜像路径为 `F:\6.wim` 并读取索引 1；
- 点击【创建任务】（ID 1003）→ 弹出破坏性警告确认框 → 注入回车（Enter）确认；
- 弹出 `BackupRestoreSystemDriveChoice` 离线环境选择对话框 → 点击【进入 Windows RE】；
- 弹出“进入恢复环境”确认对话框 → 注入回车（Enter）确认；
- 程序以管理员权限生成任务 `1b06d01d-45c0-40fc-b28f-9f35e32cb2f6` 并自动触发重启。

### 3. Windows RE 离线执行与界面实测（截图见 `.test-artifacts/captures/re-restore-v193/`）
- **阶段 1：准备与挂载**
  - 标题：`正在准备恢复环境...`
  - 日志：`Recovery.exe started from env task=1b06d01d... operation=RestoreExisting`
  - 截图：`re-173419.png`
- **阶段 2：DISM 应用镜像**
  - 标题：`正在还原系统分区...`
  - 靠右时间区：`已用: 05:23  剩余: 00:52  当前时间: 17:39:29`
  - 进度显示：单一当前进度条全幅平滑推进（实测抓取到 86%）
  - 正文阶段：
    - `✓ 1/4 挂载卷、校验 [17:36:02]`
    - `▶ 2/4 还原镜像（时间长） [17:36:05]`
    - `—— 当前进度 86% ——`
  - 截图：`re-restore-step2-86pct.png`
- **阶段 3 & 4：校验、引导重建、清理与自动重启**
  - 引导校验与修复：`Verified BCD device/osdevice points to target c:`
  - 暂存清理：`new boot channel: staging directory \\?\Volume{...}\BackupRestoreRE removed and verified`
  - 终态标记：`Boot entry cleaned; task marked successful`
  - 自动重启：`running wpeutil.exe reboot`
  - 截图：`re-restore-rebooting.png`

### 4. 终态验收与残留核验（PASS）
- **正常进入桌面**：系统开机成功进入 Windows 11 用户桌面（截图：`re-restore-desktop-success.png`）；
- **任务终态**：`H:\brwork\tasks\1b06d01d-45c0-40fc-b28f-9f35e32cb2f6\status.json` 中 `stage="success"`，`progress=100`；
- **BCD 启动项**：`bcdedit /enum {bootmgr}` 干净，无任何 `bootsequence` 残留；本轮分配的 BCD ID `{ff7e59ed-bcb1-11f1-88fe-f6523f7c95ca}` 已被枚举回读证实彻底删除；
- **暂存卷清理**：`F:\BackupRestoreRE` 目录已彻底删除，回读验证不存在；
- **WinRE 恢复环境**：执行 `reagentc /enable` 重新注册恢复环境分区成功（Enabled，`GLOBALROOT\device\harddisk0\partition4\Recovery\WindowsRE`）。

---

## 三、移除探测 Tab 实现细节（v1.9.4）

### 1. 控件定义与布局调整
- `Controls` 结构体中的 `operation_tabs` 数组长度由 5 调整为 4（`[Hwnd; 4]`）；
- 移除 `ID_OPERATION_PROBE`（1100）；
- 保留四个操作按钮并调整布局起始 X 坐标为 180：
  - 索引 0：【备份】（ID 1101），X=180，带有 `WS_GROUP` 单选组起始标记；
  - 索引 1：【单系统还原】（ID 1102），X=279；
  - 索引 2：【新增第二系统】（ID 1103），X=378；
  - 索引 3：【PE 恢复】（ID 1104），X=477。

### 2. 状态映射与默认行为
- `selected_operation(state)` 映射：
  - `0 => "backup"`
  - `1 => "restore-existing"`
  - `2 => "create-secondary"`
  - `3 => "install-pe-entry"`
- `select_operation` 索引钳位调整为 `index.min(3)`；
- 启动默认行为：
  - 程序启动时默认调用 `select_operation(state, 0)`，进入【备份】模式并自动初始化布局、操作指引与卷详情；
  - 状态栏初始文本由“默认模式为无破坏探测”更新为“默认模式为备份”。

### 3. 验证结果
- `cargo test --workspace`：**110 个测试全部通过**（cli 86 + core 24）；
- `./build-win.sh --deploy`：Windows ARM64 交叉编译成功，产物 `1,847,296` 字节，部署到虚拟机 `C:\Users\Public\backupRestore-package` 与 `H:\brwork`，哈希一致校验通过；
- **实机运行截图验证**（`.test-artifacts/captures/v194-gui-probe-removed.png`）：
  - 顶部 Tab 仅展示【备份】、【单系统还原】、【新增第二系统】、【PE 恢复】4 个按钮；
  - 默认选中【备份】，布局工整美观，各功能与联动正常。

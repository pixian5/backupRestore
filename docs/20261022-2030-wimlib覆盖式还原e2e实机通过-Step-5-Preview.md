# wimlib 覆盖式还原 e2e 实机通过（任务 #110 收口，2026-10-22 20:30）

> 基线：commit `adfb298`（v2.6.25 代码集成）+ `windows/build-windows.ps1` 的 wimlib 部署块（e2e 前打的补丁，本轮随文档一并提交）。
> 结论：**#110 实机验收六条判据全部通过 + 三件套零新增 + 环境已恢复原系统启动**。`wimlib --no-acls` 覆盖式还原通道端到端可用。

## 一、验收执行链（一条龙，prepare→resume 间隙 <1s）

| 步 | 命令 / 动作 | 结果 |
|---|---|---|
| 1 部署 | VM 内 `& X:\windows\build-windows.ps1 -Architecture arm64 -Deploy` | VM 原生构建 v2.6.25，八联 SHA 全对 |
| 2 prepare | `prepare create-secondary --source-drive C --target-drive F --image-path H:\BRImages\br-2619-cur.wim --skip-format --allow-destructive --no-reboot --boot-menu-name BR-e2e-110` | RC=0，任务 `ab95a79a-090a-43a9-8eb3-4f0ff922c3ec` |
| 3 闸门落盘 | 读 task.json | `"formatTarget": false` + `"overlayNoAcl": true`（serde camelCase） |
| 4 resume | 武装一次性 WinRE 启动 → 重启 | RC=0，进 WinRE；进度窗 2/4 出现 wimlib 输出格式 `Extracting file data: 29/68 GiB (43%)→97%`，约 8 分钟，宿主磁盘零增长 |
| 5 断言 | 哨兵 / 序列号 / SYSTEM / status | 全过（见下） |
| 6 三件套 | EventID 1000 / WER Queue / Archive | 零新增（见下） |

部署哈希（e2e 后复核仍在位）：

| 文件 | SHA-256 |
|---|---|
| VM 构建 BackupRestore.exe（包目录 + `H:\brwork` 两处一致） | `87ad04d6ecf1a35f94cf3053dc06b22589dea3c66a6777bd9672e861c78bfc03` |
| `H:\brwork\wimlib-imagex.exe` = 仓库基线 | `fddb8c21…d8f908`（`libwim-15.dll` = `b34549c6…c992`） |

> ⚠️ 文档口径勘误：**`1423beba…091d` 是 macOS 交叉编译产物哈希（2,869,248 B），不是 VM 部署件**。按 2026-10-15 用户拍板「构建完全走 VM 内 Windows（唯一发布路径）」，实机部署一律走 VM 内 `build-windows.ps1 -Deploy`；macOS 侧 `build-win.sh --deploy` 虽存在且已含 wimlib 八联校验，但它部署的是交叉编译产物，**不得用于实机验收/发布**。

## 二、六条判据逐条证据（判据口径：`经验/05` §10）

1. **status.json**：`stage=success` / `progress=100`（`updated: 2026-10-22T11:43:07Z`）—— `.test-artifacts/e2e-assert-out.txt`
2. **日志铁证**：`H:\BRImages\Recovery.log` 第 **2,162,589** 行
   `wimlib 开始（--no-acls 覆盖式应用）：X:\windows\System32\wimlib-imagex.exe apply E:\BRImages\br-2619-cur.wim 1 F:\ --no-acls`；
   本会话（第 2,159,488 行起，11:34:33）**零** DISM `/Apply-Image` 行——全文最后一条 DISM 是 02:48:35 的历史行（`.test-artifacts/e2e-logcheck2-out.txt`）
3. **覆盖式语义**（与格式化路径判据相反）：哨兵文件**存活**（镜像外文件保留，`SENTINEL=PRESENT`）+ hosts 标记**被镜像覆写**（`HOSTS_MARKER=GONE_OVERWRITTEN`）+ 目标卷序列号**不变**（`D47D-8E54`，`--skip-format` 不格式化）—— `.test-artifacts/e2e-assert2-out.txt`
4. **apply 真发生**：目标卷 `Windows\System32\config\SYSTEM` 存在（`C_SYSTEM=PRESENT`）
5. **启动 + 三件套**：还原系统认领 C: 进桌面（截图在案）；EventID 1000 **零新增**（最新事件 2026-09-30 08:44，全为镜像自带历史）；WER Queue 空、Archive 仅 2 条 `NonCritical_Update` —— `.test-artifacts/e2e-threepiece-out.txt`
6. **RE 载荷注入链通**：WinRE 内 `Windows\System32\wimlib-imagex.exe` 哈希 = 仓库基线 `fddb8c21…d8f908`（payload 目录同时含 `libwim-15.dll`/`Recovery.exe`/`RecoveryTask.env`/`task.json`/`winpeshl.ini`）

## 三、环境恢复（e2e 后已完成）

- **回滚材料**：`bcdedit /export` 落共享盘 `.test-artifacts/bcd-rollback-e2e110.bcd` + `H:\brwork\` 副本（503,808 B）
- **默认项改回原系统**：`bcdedit /default {077a3904-cdc4-11f1-b813-9ac3f0b4f6c2}`（原系统条目，device=E:）→ 重启验证：
  - **C: = `BCD4-EAAB`（原系统正常启动，自启 GUI 为部署的 v2.6.25）**
  - F: = `D47D-8E54`（e2e 还原系统完整保留，字母漂移回 F:）
  - H: = `8297-CCB6` 工作卷完好
- 证据：`.test-artifacts/e2e-bcd-after-default.txt`（default 已指 {default} 即 {077a3904}）、`e2e-vol-after-boot.txt`、`e2e-capt-after-bcdrestore.png`

## 四、验收覆盖边界（诚实声明）

- ✅ F: 小规模 e2e 全链（WinRE 内 wimlib apply 真跑通，含 payload 注入、BCD 一次性启动、恢复后注册）
- ✅ 三件套（哨兵法 + 序列号 + EventID/WerFault）
- ⚠️ 「含原生 MSIX 保护目录的卷」场景：本 VM 的 MSIX 保护已被历史镜像回滚洗掉（POC `20261022-1427` 已证实），无法构造；wimlib `--no-acls` 结构性跳过 SD 还原，对该场景免疫，残余风险低
- 复跑确认非偶发：**未做**（可选，待用户拍板）

## 五、本轮新发现（不阻塞验收）

1. **镜像「带回旧状态」效应再确认**：还原后的系统自启 GUI 是镜像内置的 **v2.6.19**（包目录被镜像带回拍摄时点）——与 #38 轮记录一致；已补部署 v2.6.25 的是原系统侧。不影响验收判定（RE 载荷由注入链提供，版本正确）
2. CBS `CrossDeviceResume.exe` 弹「参数错误」对话框：Windows 自身组件行为，与本程序无关
3. `pe_safety.rs` 旧硬拒文案成死代码（三个调用点只取 `is_some()`，文案被优化器 GC）——功能无损，待清理
4. **prlctl 通道坑（新）**：PowerShell 5.1 的 `-Command` 遇 `&&` 直接 ParserError（PS7 才支持）；且 `-Command` 参数里的引号在 argv 传递时被剥离，格式串内的 `|` 会变管道符——**VM 内执行统一走 `cmd /d /c '...'` 直调或 .cmd 文件**
5. 宿主空间：镜像展开 68GiB + 薄配 `.hds` 删文件不还宿主空间 → 停 VM 优雅停机触发 TRIM 回收（102→68.8GiB），宿主空闲 70→112GiB

## 六、遗留与待拍板

- **门禁缺陷 `/ImageFile:`→`/WimFile:`**（下一大块；与 e2e 的互斥前提已解除）
- 测试卷清理：删 BR-e2e-110 启动项 + `format F:` 释放 350GB（待用户拍板）
- 32GB `br-2619-noRE.wim` 副本删除（e2e 后已符合删除条件，待确认）
- 是否复跑一次确认非偶发

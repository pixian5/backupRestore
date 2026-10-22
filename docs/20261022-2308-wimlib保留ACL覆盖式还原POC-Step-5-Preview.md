# wimlib「保留 ACL」覆盖式还原 POC：自带提权覆盖 SD 还原的实验证据

- 时间：2026-10-22 23:02–23:55（tiny WIM 机制实验 + 全量真实镜像两轮）
- 模型：Step-5-Preview
- 起因：用户问「wimlib 完全替换 DISM 是否可行、是否完美升级」→ 评估中提出唯一值得 POC 的改进方向：**覆盖式还原去掉 `--no-acls` 能否保留镜像 ACL**（比当前「权限全丢」更保真）→ 用户指示「做试验」→ 补测后再定。
- **结论先行**：**可以保留，且全量真实场景验证通过**。特权禁用上下文（金丝雀 icacls exit 5 证实 SYSTEM 被拒）下，wimlib **不加 `--no-acls`** 对 TrustedInstaller owner 的保护目录**完整还原 SD**（tiny WIM：SDDL 逐字节一致；全量 71GiB：标尺目录/文件 SD 与 C: 镜像参考**逐字节一致**，含 `WIN://SYSAPPID` 条件 ACE + app package SID + TI FA 的原生 MSIX ACL），exit 0、日志零警告、卷序列号不变。**wimlib 自带提权同时覆盖写入与 SD 还原两条路径**——DISM 失败的本质是 RE 里 wimgapi 不提权。`--no-acls` 是「双保险 + 行为确定」而非功能必需。

---

## 1. 实验设计

复用 10-22 POC（`docs/20261022-1427`）验证过的三件套方法：**特权禁用上下文 + 合成限制性目录 + 金丝雀 icacls**。

- **牺牲卷**：F:（6A2D-BA03，清理后的空卷）
- **tiny WIM**：`C:\Users\Public\pkg\acl-poc.wim`（36 字节两文件 + 5 目录，wimlib capture 默认带 ACL；镜像 SD = owner `BUILTIN\Administrators` + DACL `BA(FA)/CO(FA)/SY(FA)/IU/SU/S-1-5-3`，非 protected）
- **合成限制性目录**：`F:\Program Files\WindowsApps\PocAclPkg`（按镜像内真实路径预置；SDDL 同 E1：owner=TrustedInstaller SID、protected、DACL 仅 TI(FA)+RC(RX)、**无任何 SYSTEM ACE**）
- **特权禁用**：PowerShell P/Invoke `AdjustTokenPrivileges` 禁用 SeRestorePrivilege + SeTakeOwnershipPrivilege；wimlib 以 `Start-Process` 子进程启动（继承禁用态）；金丝雀 = 同上下文 `icacls /setowner NT AUTHORITY\SYSTEM` 必须 exit≠0
- **执行通道**：SYSTEM（`prlctl exec` 不加 `--current-user`；UNC 路径读写共享盘）

## 2. 实验矩阵与结果

| # | 上下文 | wimlib 参数 | exit | 文件落盘 | 目录 SD 事后 |
| --- | --- | --- | --- | --- | --- |
| **T1** | 特权禁用父进程（金丝雀 icacls **exit 5** = 被拒） | **不加 `--no-acls`** | **0** | file.txt=19B 内容正确、sub\nested.txt=17B 内容正确 | **完整还原为镜像 SD**：owner `NT SERVICE\TrustedInstaller`→`BUILTIN\Administrators`、protected True→False、SDDL 与镜像**逐字节一致** |
| **T3** | 同 T1 | 加 `--no-acls` | 0 | 同上（修正取证后证实） | 保持 TI 预设不动（owner=TI、protected=True） |

- T3 首轮取证曾报「文件 MISSING」——**取证伪影**：取证时父进程仍在禁用态，SYSTEM 列不了无 SYSTEM ACE 的目录，`Test-Path` 假阴。临时授予 SYSTEM 读权限后证实文件在位、内容正确（与 10-22 E4 一致）。**教训复犯**：10-22 poc-final-proof.ps1 已处理过该坑（temporarily grant SYSTEM read），本轮新脚本漏改。
- T0 确认：wimlib capture 默认包含 ACL（`--no-acls` 才是显式跳过）。

## 3. 核心发现

### G1：wimlib 自带提权覆盖 SD 还原（本实验最主要结论）

禁用 SeRestore/SeTakeOwnership 的父进程下：
- 同上下文金丝雀 icacls 改 owner **被拒（exit 5）**——复现了 DISM `RestoreDirMetadata 0x80070005` 的等价被拒条件；
- wimlib 子进程却把 TI owner 的保护目录 **owner 改写 + DACL 整体替换 + protected 标志清除**，一步到位。

结合 10-22 E5（同禁用上下文下写入成功），wimlib 的「自行获取 SeRestore/SeBackup」**同时覆盖写入与 SD 还原两条路径**。DISM 失败的本质是「RE/PE 上下文 wimgapi 提权不生效」（F1），而 wimlib 在 RE 里自己提权——这才是覆盖式通道成立的第一性原因，`--no-acls` 只是在此之上的额外保险。

### G2：`--no-acls` 的价值重定位

当前证据下 `--no-acls` 不是覆盖式场景的**功能必需**，而是：
1. **双保险**：不依赖「wimlib 提权必然成功」这一前提；
2. **行为确定性**：SD 一律不动，语义简单、可预期；
3. 已按此完成 e2e 全链验证 + GUI 确认框文案体系。

### G3：未测边界（tiny 轮提出，全量轮已覆盖 3/4）

| 边界 | tiny 轮 | 全量轮（23:29–23:55） |
| --- | --- | --- |
| app-package SID ACE（原生 MSIX ACL 特征成分） | 未测（合成 SDDL 无此成分） | ✅ **已覆盖**：C: 同路径真实 MSIX SDDL 作参考，含 `WIN://SYSAPPID Contains "Clipchamp..."` 条件 ACE + 两条 S-1-15-3-* app package SID + TI FA——还原后与参考**逐字节一致** |
| SD 还原失败时的行为（跳过 or 整体失败） | 未测（SYSTEM+提权下无可改动的 SD，本 VM 内不可构造） | ⚠️ **仍不可测**（同因）；但全量 634,091 条目零失败 + 日志零 warning，失败路径未出现的证据充分 |
| 全量尺度（71GiB / 634,091 条目） | 未测（36B tiny WIM） | ✅ **已覆盖**：两遍全量 apply 均 exit 0（各约 8 分钟），序列号不变，树完整（WindowsApps 31,749 + System32 103,864 文件） |
| 文件级 SD | 未测（只测目录） | ✅ **已覆盖**：标尺文件（AppxManifest.xml）SD 还原后与 C: 参考逐字节一致 |

仍剩一道边界 = **RE 内真实场景**（本实验在完整 Windows 里用特权禁用模拟 RE 最坏上下文；RE 内 wimlib 同为 SYSTEM、提权机制相同，但未做实机）——这已属 e2e 范畴而非 POC 范畴。

## 3.5 全量实验详情（23:29–23:55）

- **pass1（复验 E3 基线）**：`wimlib-imagex apply H:\BRImages\br-38sys-20261022.wim 1 F:\ --no-acls` → exit 0，634,091/634,091 条目，约 8 分钟。F: 标尺 SD = 裸默认 ACL（O:BA + BA/CO/SY/IU/SU/S-1-5-3）——**证实 `--no-acls` 全量下 SD 一律不还原**。
- **pass2（核心）**：同一镜像、同目标卷、**不加 `--no-acls`**、特权禁用父进程（金丝雀 icacls exit 5）→ exit 0、约 8 分钟、日志零 warning/error。
- **标尺设计**：`F:\Program Files\WindowsApps\Clipchamp.Clipchamp_4.5.10920.0_arm64__yxz26nhyzhsrt`（目录）及其 `AppxManifest.xml`（文件），pass1 后置为 TI owner + protected + 无 SYSTEM ACE 态；参考 = C: 同路径 SD（C: 为同镜像 DISM 还原产物，SD 忠于镜像——**这是本 VM 可获得的「镜像保真 SD」活样本**，含原生 MSIX ACL 的条件 ACE 与 app package SID）。
- **结果**：标尺目录/文件 SD 还原后与 C: 参考**逐字节一致**（owner=SY、DACL 全量还原、protected 标志按镜像）；F: 序列号 `6A2D-BA03` 两遍不变；卷树完整可用。
- **旁证**：pass1/pass2 日志 grep warn/error/fail/denied/skip **零命中**。

## 4. 产品含义（待用户拍板，不动代码）

- 「覆盖式 + 保留 ACL」**原理成立且全量验证通过**：`wimlib_args` 去掉 `--no-acls` 即可让镜像 ACL/owner 完整还原（含原生 MSIX 的条件 ACE / app package SID / TI FA），GUI 确认框的代价文案可从「目录权限不还原」改为「个别目录权限可能不还原」（且全量证据下连这个代价都未出现）。
- 产品化路径：改 `wimlib_args` + 改 GUI 文案 + 重走 e2e（WinRE 内真实场景）+ 三件套。剩余风险仅「RE 内实机」一道（提权机制与完整 Windows 相同，预期通过）。
- **维持现状（`--no-acls`）的理由仍在**：行为完全确定（不依赖提权必然成功）、已按此完成 e2e 全链验证 + GUI 文案体系。本次 POC 的价值：**证明了演进空间存在且风险低**——若将来要提升覆盖式还原的保真度，实验基线已备好。

## 5. 环境终态与证据清单

- F: = `6A2D-BA03`（含 pass2「全量覆盖 + ACL 完整还原」的完整 Windows 树，可随时再格式化）；C: 未受影响；wimlib 二进制仍在 `C:\Users\Public\pkg\`。
- 证据（`.test-artifacts/acl-poc-20261022/`）：
  - tiny 轮：`t0-stage.cmd/txt`（capture + 镜像 SD 记录）、`t1-main.ps1`、`t1-run.cmd/txt`、`t1-result.txt`（T1/T3 + 金丝雀 + 逐字节 SDDL 比对）、`t3-fix.ps1/txt`（T3 修正取证 + 清理）
  - 全量轮：`full-pass1.cmd/txt`（--no-acls 全量基线，PASS1_EXIT=0）、`full-pass2.ps1`、`full-pass2.cmd`、`full-pass2-run.txt`（预设+金丝雀+取证）、`full-pass2-stdout.txt`/`full-pass2-stderr.txt`（wimlib 原始输出，零警告）、`full-pass2-result.txt`（SDDL == C: 参考 判定）

# wimlib「保留 ACL」覆盖式还原 POC：自带提权覆盖 SD 还原的实验证据

- 时间：2026-10-22 23:02–23:12
- 模型：Step-5-Preview
- 起因：用户问「wimlib 完全替换 DISM 是否可行、是否完美升级」→ 评估中提出唯一值得 POC 的改进方向：**覆盖式还原去掉 `--no-acls` 能否保留镜像 ACL**（比当前「权限全丢」更保真）→ 用户指示「做试验」。
- **结论先行**：在模拟 RE 最坏上下文（禁用 SeRestore/SeTakeOwnership 的父进程，金丝雀 icacls exit 5 证实 SYSTEM 被拒）下，wimlib **不加 `--no-acls`** 对 TrustedInstaller owner 的保护目录**完整还原了 SD**（owner 改写 + DACL 全换 + protected 清除，与镜像逐字节一致），文件写入正确，exit 0。即 **wimlib 自带提权覆盖 SD 还原，不只能写文件**；`--no-acls` 在当前证据下是「双保险」而非「必需」。

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

### G3：未测边界（产品化前必须知晓）

1. **app-package SID ACE**：原生 MSIX 专用 ACL 的特征成分，本 VM 的原生 ACL 已被「镜像回滚效应」洗掉（F2），合成 SDDL 中无此成分——**未测**；
2. **SD 还原失败时的行为**：跳过并继续 or 整体失败——SYSTEM + 提权下构造不出「改不动的 SD」，**本 VM 内不可测**；
3. **全量尺度**：tiny WIM（36B）→ 71GiB 全量的行为一致性未测（机制相同，风险低但非零）；
4. **文件级 SD**：只测了目录；文件 owner/ACL 还原同走 SeRestore 路径，未单独测。

## 4. 产品含义（待用户拍板，不动代码）

- 「覆盖式 + 保留 ACL」在原理上成立：`wimlib_args` 去掉 `--no-acls` 即可让镜像 ACL/owner 完整还原，GUI 确认框的代价文案可从「目录权限不还原」改为「个别目录权限可能不还原」。
- 但产品化需重新走 e2e 级验证（含哨兵/序列号/三件套），并承担 G3 的未测边界风险；且 RE 内真实场景的「失败路径行为」仍不可测。
- **维持现状（`--no-acls`）的理由**：行为确定 + 已全链验证 + 双保险。本次 POC 的价值主要是**知识资产**：明白了覆盖式通道为什么成立、`--no-acls` 为什么只是保险，为将来可能的「保留 ACL」演进提供实验基线。

## 5. 环境终态与证据清单

- F: = `6A2D-BA03`（测试树已清理，卷未动）；C: 未受影响；wimlib 二进制仍在 `C:\Users\Public\pkg\`。
- 证据（`.test-artifacts/acl-poc-20261022/`）：`t0-stage.cmd`/`t0-stage.txt`（capture + 镜像 SD 记录）、`t1-main.ps1`（T1/T3 主实验）、`t1-run.cmd`/`t1-run.txt`（执行入口）、`t1-result.txt`（T1/T3 结果，含金丝雀与逐字节 SDDL 比对）、`t3-fix.ps1`/`t3-fix.txt`（T3 修正取证 + F: 清理）。

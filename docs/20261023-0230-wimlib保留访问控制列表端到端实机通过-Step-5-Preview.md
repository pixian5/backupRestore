# wimlib「保留访问控制列表」端到端实机通过（v2.6.27 收尾，2026-10-23 02:30）

> 基线：commit `2c8b588`（v2.6.27 代码：`wimlib_args` 去掉 `--no-acls`）+ `f992626`（文档全称化补遗）。
> 结论：**恢复环境内真实链路端到端通过，访问控制列表判据 8/8 逐字节一致**。wimlib 覆盖式应用
> **不带 `--no-acls`** 时，镜像的属主/访问控制列表/protected 标志在恢复环境里完整还原，
> 与 `C:` 参考（同镜像保真样本）逐字节相同，含原生微软商店应用的条件 ACE 与 app package SID。
> 六条常规判据（三项判据）全部通过，环境已恢复原系统启动、测试卷已格式化。

## 一、验收执行链

| 步 | 命令 / 动作 | 结果 |
|---|---|---|
| 1 部署 | 虚拟机内 `build-windows.ps1 -Architecture arm64 -CargoTargetDir C:\BackupRestoreBuild\target -Deploy` | 增量构建 32.22 秒，八联 SHA-256 全对，部署哈希 `3ef72e73…5b82` |
| 2 前置 | 哨兵 `F:\BR-ACL-227-SENTINEL.txt` + `F:\Windows\System32\drivers\etc\hosts` 追加覆写标记 + 采集 `C:`/`F:` 样本安全描述符 | 哨兵与标记落位，18 条安全描述符参考入库 |
| 3 prepare | `prepare --operation create-secondary --source-drive C --target-drive F --image-path H:\BRImages\br-2619-cur.wim --skip-format --allow-destructive --no-reboot --boot-menu-name BR-acl-227` | 退出码 0，任务 `356f0b55-dee2-4fd0-8bf3-190f63946fc4`，门禁挂载约 60 秒 |
| 4 闸门核对 | 读 `task.json` | `"formatTarget": false` + `"overlayNoAcl": true`（字段名沿袭历史，语义已是「保留访问控制列表的覆盖式通道」） |
| 5 resume | `resume H:\brwork 356f0b55-…` | 退出码 0，一次性启动武装并重启 |
| 6 恢复环境内应用 | 进度窗 2/4，wimlib 展开 633,699 个文件 | 02:04:04 开始，02:15:47 完成（约 11 分 43 秒），68 GiB |
| 7 断言 | 哨兵 / 覆写标记 / 序列号 / 状态 / 日志 / 三项判据 / 访问控制列表 | 全过（见下） |
| 8 环境恢复 | 默认项回原系统 → 删 `BR-acl-227` → 格式化 `F:` | 全部完成（见 §四） |

## 二、访问控制列表判据（本轮新增，v2.6.27 的核心）

**判据口径**：还原后 `F:` 的样本对象安全描述符（属主 + 访问控制列表 + protected 标志）与
`C:` 同路径参考**逐字节一致**；同时与还原前 `F:`（概念验证 pass2 的全量还原树）逐字节一致。

| 样本路径 | 属主 | protected | 还原前 vs 还原后 | C: 参考 vs 还原后 |
|---|---|---|---|---|
| `Program Files\WindowsApps` | TrustedInstaller | True | 一致 | 一致 |
| `Program Files\WindowsApps\Clipchamp.Clipchamp_4.5.10920.0_arm64__yxz26nhyzhsrt` | SYSTEM | False | 一致 | 一致 |
| …\`AppxManifest.xml`（文件） | SYSTEM | False | 一致 | 一致 |
| `Windows\System32\config\SYSTEM`（文件） | SYSTEM | False | 一致 | 一致 |
| `Windows\System32\drivers\etc\hosts`（文件，本轮被镜像覆写） | SYSTEM | False | 一致 | 一致 |
| `Windows\SystemApps` | SYSTEM | False | 一致 | 一致 |
| `Windows\servicing` | TrustedInstaller | True | 一致 | 一致 |
| `Windows\registration` | Administrators | True | 一致 | 一致 |

- **8/8 逐字节一致，0 条不同**（`.test-artifacts/e2e-sd-227-ref.txt` 与 `e2e-sd-227-after.txt` 全量比对）。
- 样本含原生微软商店应用特征成分：`WIN://SYSAPPID Contains "Clipchamp.Clipchamp_…"` 条件 ACE、
  两条 `S-1-15-3-*` app package SID、TrustedInstaller 完全控制 ACE。
- **反证基线**：概念验证 pass1 已证实 `--no-acls` 全量应用后样本安全描述符退化为裸默认
  （`O:BA` + Administrators/CO/SYSTEM/交互用户/服务 的默认 ACE 集）。本轮结果与之一致性排除——
  若 `--no-acls` 仍生效，8 条样本不可能保持镜像原值。

## 三、三项判据逐条证据

1. **`status.json`**：`stage=success` / `progress=100`（`updated: 2026-10-22T18:16:07Z`）—— `e2e-assert-227.txt`
2. **日志铁证**：`H:\BRImages\Recovery.log` 第 **2,203,313** 行
   `[2026-10-22T18:04:04] wimlib 开始（覆盖式应用，还原安全描述符）：X:\windows\System32\wimlib-imagex.exe apply E:\BRImages\br-2619-cur.wim 1 F:\`
   —— **不带 `--no-acls`**；第 2,203,319 行 `wimlib 结果：退出码 0`。
   对照上一轮（第 2,162,589 行，11:34:35）末尾带 `--no-acls`，两版行为差异在日志里可直接区分。
   **本会话（18:00 之后）零 `dism.exe /Apply-Image` 行**；最后一条 DISM 应用是 02:48:35 的历史行。
3. **覆盖式语义**：哨兵**存活**（`SENTINEL=PRESENT`）+ hosts 覆写标记**消失**
   （`HOSTS_MARKER=GONE_OVERWRITTEN`）+ 目标卷序列号**不变**（`6A2D-BA03`）+ 目标卷
   `Windows\System32\config\SYSTEM` 存在（`C_SYSTEM=PRESENT`）。
4. **三项判据**：事件标识符 1000 最近 3 条**全部为 2026-09-30 历史事件**（零新增）；
   Windows 错误报告 Queue **空**；Archive **2 条**（与上一轮一致，无新增）。
5. **启动**：还原系统正常启动进桌面（图形用户界面标题 `BackupRestore - Rust GUI v2.6.19`，
   即镜像内置版本；源卷显示「磁盘 2/分区 2、350.0 GiB」= 还原卷认领 `C:`）—— `e2e-227-08-desktop.png`
6. **恢复环境载荷与自建条目清理**：任务载荷目录 `H:\BackupRestoreRE\356f0b55-…` 已删（父目录一并消失）、
   一次性启动已撤（`bootsequence` 空）、任务自建条目 `{cde63412}`/`{cde63411}` 零残留。

## 四、环境恢复（已执行）

| 项 | 结果 |
|---|---|
| 默认启动项 | `bcdedit /default {077a3904-…}`（原系统）→ 重启验证原系统正常启动，`C:` = `BCD4-EAAB` |
| 还原系统启动项 | `{9d575070-…}`（`BR-acl-227`）已 `bcdedit /delete … /f`，回读零命中 |
| 测试卷 | `F:` 已快速格式化（卷标 `BRTEST`，序列号 `BE7F-3EC3`，349.9 GB 可用） |
| 恢复环境载荷 | `H:\BackupRestoreRE\` 已由任务收尾删除 |
| BCD 现状 | `displayorder` = 原系统 + 预安装环境条目 `{177f3ea3-…}`，与测试前一致 |

## 五、本轮踩坑与口径更正

1. **`prepare` 子命令式写法不被接受**：`prepare create-secondary …` 报
   `unknown prepare option: create-secondary`；解析器只认 `--operation create-secondary`。
   上一轮文档表格里的子命令式写法是简化记录，照抄会失败。
2. **`resume` 第一个参数是工作区根目录**，不是任务目录：`resume H:\brwork <task-id>`。
   误传 `H:\brwork\tasks\<id>` 会让 `TaskStore` 去找
   `H:\brwork\tasks\<id>\tasks\<id>\task.json`，报 `I/O error: 系统找不到指定的路径 (os error 3)`。
3. **`bcdedit` 的 `{current}` / `{default}` 是显示别名，不是存储损坏**：当默认项等于「本次引导的条目」时，
   `bcdedit /enum {bootmgr}` 会把 `default`/`displayorder` 显示成 `{current}`；显式设默认项后又会显示成
   `{default}`。任务准备期创建自建条目后即可观察到该现象，功能无影响（重启进原系统已实证）。
4. **任务目录的 `recovery.log` 只有 resume 三行**：恢复环境内的主日志落在镜像卷
   `H:\BRImages\Recovery.log`（会话日志跨重启存活的镜像），查 wimlib 行要去那里。
5. **还原系统自启图形用户界面会弹「持续跑的还原任务」对话框**（镜像内置的旧备份任务，停在 Capturing）：
   必须选「否(N)」——选「是」会重启去续跑那个历史任务。本轮已用按键通道按 N 关闭。
6. **`H:\brwork\Recovery.log` 被运行中的图形用户界面占用**：断言脚本用 `findstr` 打开会报「无法打开」，
   复制也会失败；以 `H:\BRImages\Recovery.log` 为准。

## 六、验收覆盖边界（诚实声明）

- ✅ 恢复环境内真实链路（一次性启动 → 恢复环境 → wimlib 不带 `--no-acls` 覆盖式应用 → 引导修复 → 重启）
- ✅ 访问控制列表判据（8 个样本对象、目录与文件、含原生微软商店应用条件 ACE）
- ✅ 三项判据（哨兵 / 序列号 / 事件标识符与错误报告）
- ⚠️ 「安全描述符还原失败时的行为」仍不可构造（本虚拟机内 wimlib 提权恒成功；全量 633,699 条目零警告，
  失败路径未出现的证据充分）
- ⚠️ 复跑确认非偶发：未做（上一轮同类链已跑过多次；本轮为单次通过）

## 七、证据清单（`.test-artifacts/`）

| 文件 | 内容 |
|---|---|
| `e2e-sd-227-ref.txt` | 还原前 `C:`/`F:` 18 条样本安全描述符参考 |
| `e2e-sd-227-after.txt` | 还原后 18 条样本安全描述符 |
| `e2e-assert-227.txt` | 序列号、哨兵、覆写标记、状态、事件、错误报告、BCD、任务状态 |
| `e2e-brimglog-227.log` | `H:\BRImages\Recovery.log` 全量副本（2,241,023 行） |
| `e2e-prep-227.txt` / `e2e-task-227.json` / `e2e-bootentry-227.json` | 准备输出、任务闸门、启动项记录 |
| `e2e-resume-227b.txt` | resume 输出与 BCD 全量快照 |
| `e2e-227-01…10-*.png` | 恢复环境进度（20%→95%）、启动、还原系统桌面、原系统桌面截图 |
| `e2e-cleanup-227.txt` | 删启动项、格式化 `F:` 的回读结果 |

# ✅ WinRE 完整备份闭环 PASS（v1.8.14 实机，真实系统卷 C:）

**时间**：2026-09-30 约 09:44（任务 `status.json` 落盘 `stage=success`）
**部署版本**：v1.8.14（SHA-256 `71a157d0…abab06`，实机跑的就是这份，与客体 `H:\brwork\BackupRestore.exe` 一致）。
**树内版本**：v1.8.15（本轮未重新部署）；v1.8.15 仅新增「载荷 `Recovery.exe` 来源」修正——
优先用正在运行的可执行文件当载荷来源，避免「改了 `BackupRestore.exe` 却没改 PE 里跑的 `Recovery.exe`」的旧坑（详见正文第四节）。
本 RE 测试的结论（RE 分支完整链路可跑通）不受该修正影响。
**触发**：用户指令「先完整测试 RE」——跑**真实重启进入 WinRE** 的完整备份链路，而非只验按钮。

---

## 一、目标与前置

- 验证 **RE 分支（GUI 选择 `choice=2` → 「进入恢复环境」）** 在「新 PE 式 BCD 通道」下的完整闭环：
  真实重启进 WinRE → 离线 DISM 捕获 → 修复/收尾 → 回正常 Windows。
- **为什么用 backup 而不是 restore**：真实 C: 69.7GB 已用，backup 是只读源、零破坏；镜像落 `F:\brimg\re-full-backup.wim`（F: 374GB 空闲）。
- **前置硬约束（AGENTS.md）**：触发任何 BCD/重启变更前建并核验 Parallels 快照。
  - 快照名 `before-RE-full-backup-test-20260930`，ID `{3fd66dce-b215-4dfd-83ed-fa2e82edd5b9}`。
- **强制走真实对话框**：`C:\br-test.json` 只给 `tab/ source_volume/ image`，**不含** `system_drive_choice` / `auto_install`，
  因此 `--test-hook` 会强制弹出「系统盘选择」对话框 + 确认框，用真实 `WM_COMMAND` 注入驱动（非 `--test-hook` 预置）。

---

## 二、实机驱动链路（全部真实注入，非预置）

```
GUI 主界面
 └─ 点「创建任务」（真实 WM_COMMAND 注入，1003）
     └─ ask_system_drive_handler 弹「系统盘选择」对话框
         └─ 注入 choice=2（进入 Windows RE）→ 2022
             └─ 弹确认框（标题「进入恢复环境」/ MB_YESNO）→ 注入 6（Yes）
                 └─ prepare 链：build prepare 命令 → ShellExecute "runas"（42）
                     └─ VM 真实重启 → 进 WinRE（prlctl exec 报 "Unable to open new session" 即 RE 无 Tools，确认已进 RE）
                         └─ WinRE 自动拉起 Recovery.exe → DISM 捕获 C:
                             └─ 修复/收尾 → wpeutil reboot → 回正常 Windows
```

---

## 三、终态核验（逐项实机取证）

| 核验项 | 结果 | 取证 |
|---|---|---|
| 任务状态 | ✅ `stage=success` / `progress=100` | `H:\brwork\tasks\247a7169-…\status.json`：`"operation":"backup","stage":"success","progress":100,"updated":"2026-09-30T01:44:45Z"` |
| 镜像可用 | ✅ 索引 1 可读 | `dism /Get-WimInfo`：`名称=2026-09-30 09:27`、`大小=87,254,275,408 字节`（87GB 未压缩；磁盘 33.4GB fast 压缩） |
| BCD 一次性启动 | ✅ 已清 | `bcdedit /enum {bootmgr}`：**无 `bootsequence` 字段**，`default={current}`、`displayorder={current}` |
| 注册 WinRE | ✅ 未动 / Enabled | `reagentc /info`：`Windows RE 状态: Enabled`，位置 `harddisk0\partition4\Recovery\WindowsRE`，标识符 `f530b9e0-bc2c-11f1-88f0-d32b69265400` |
| **本轮自建 BCD 条目** | ✅ 已删 | 本轮 armed loader = `{29662687-bc6f-11f1-88f7-eb79500f628e}`（见 `prepare.log`），`bcdedit /enum all` **查无此 GUID** → `disarm()` 正确清理 |
| 回桌面 | ✅ `SYSTEMROOT=C:\Windows` | 监控脚本 `WINDOWS_UP` 状态确认 |

**结论**：RE 分支完整链路（含真实重启）在「新 PE 式 BCD 通道」下 **PASS**；`disarm()` 终态清理（清 bootsequence + 删自建条目 + 删暂存 WIM）对本轮任务生效。

---

## 四、重要发现：存在一个 PRE-EXISTING 孤儿 BCD 条目（非本轮）

`bcdedit /enum all` 中仍有一对**历史残留** BCD 对象：

```
标识符   {76ead7a9-bc37-11f1-88f2-ccec9ea9df4b}
device   ramdisk=[F:]\BackupRestoreRE\Winre.wim,{76ead7a8-bc37-11f1-88f2-ccec9ea9df4b}
description   BackupRestore task RE
标识符   {76ead7a8-bc37-11f1-88f2-ccec9ea9df4b}
description   BackupRestore task RE device options
```

判定为**更早测试轮次的遗留**，理由：
1. 本轮 `prepare.log` 武装的是 `{29662687-…}`（与 `boot-entry.json` 一致），而残留是 `{76ead7a9-…}`——**不是同一 GUID**。
2. 本轮 GUID `{29662687}` 在 BCD 中已**完全消失**，证明本轮 `disarm()` 工作正常。
3. 残留条目**不在** `bootsequence` / `displayorder` 中，不会被动引导，对启动无影响。

附带：BCD 里还有若干历史 WinRE ramdisk 孤儿条目（`{265d7bf0}`→Y:、`{b69adf69}`→C:、`{eeca24dc}`→F:、`{f530b9e0}`→C: 为当前注册项），
均为此前各轮次累积，非本轮产生。

**处置建议**：列为「下一步待实现」的清理项——用 `bcdedit /delete` 移除明确属于本项目的 `{76ead7a9}/{76ead7a8}` 一对（描述含 "BackupRestore task RE"），
其余 WinRE 孤儿条目先不动（可能是合法/历史注册）。`F:\BackupRestoreRE` 目录已空，可一并移除。
**本轮不擅自删 BCD**（用户只要求写文档），且删除前必须新建快照。

---

## 五、与既有验证的关系

- 此前「新 PE 式通道第一次完整备份闭环」（`20260930-003000-…`）用的是 **T: 测试卷（实占 54MB，镜像 258KB）**，且是**轻量**验证。
- 本轮是**真实系统卷 C:（87GB 未压缩）** 的 RE 备份闭环，且走**真实重启 + 真实 GUI 对话框注入**，是产品主场景的更接近验收形态。
- RE 分支此前只在「按钮级」被实机复验（`20260930-085630-…`，未验点「是」后完整链路）；本轮把「点『是』→ 真实重启进 RE → 备份 → 回 Windows」这一整段补齐。

---

## 六、证据位置（不入库，`.test-artifacts/` / 客体 `H:\brwork`）

- 任务目录：`H:\brwork\tasks\247a7169-2c10-49a8-b1d6-a78b6333a936\`（含 `status.json` / `prepare.log` / `boot-entry.json` / `bcd-before-*` 快照）
- 镜像：`F:\brimg\re-full-backup.wim`（33.4GB）
- BCD 诊断快照（本轮抓取）：`C:\diag\bootmgr.txt` / `all.txt` / `reagentc.txt` / `wim.txt`（已落到客体，未拉回宿主）
- Parallels 快照：`{3fd66dce-b215-4dfd-83ed-fa2e82edd5b9}`（基线，BCD/重启变更前建）

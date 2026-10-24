# 步 0 清尾 + E1 用户手册（v2.6.50→部署 11ac5890…，2026-10-25 05:35，Step-5-Preview）

> 承接 [`20261025-0505-下一步开发方向`](./20261025-0505-下一步开发方向-Step-5-Preview.md) 的推荐路线：用户拍板「按推荐路线四步推进（E1 先行）」+「菜单遗留项删除」。
> 本轮 = 步 0 三项清尾 + E1 用户手册与实机走查。HEAD `a526bee`，全部已 push。

---

## 1. 启动菜单遗留项删除（用户拍板「删除」）

- 目标：`Windows Backup-202610250107`（v2.6.44 实机 10-25 01:07 产生的测试启动项）。
- 回滚材料先落盘（`.test-artifacts/bcd-menu-cleanup-20261025/`：bootmgr/osloader 的 before/after 全量 `/v` 输出）。
- 按 GUID 精确删：`{446ace41-cfce-11f1-a501-b5edaeceab66}`（device=F:）。**删除前核实它不在 displayorder 里**
  （displayorder 只有默认系统一项，删它不动启动菜单）。
- 核验：osloader 的 winload.efi 条目 **7 → 6**；`{bootmgr}` 的 `default`/`resumeobject`/`displayorder`
  三行逐字不变（`{af5ee73f…}`）；重启后正常进 Windows、`default` 仍指原系统；重启前后截图留证。
- 教训复用：`bcdedit` 不带 `/v` 时 `default {current}` 是**当前运行条目的显示别名**，别误判成 default 被改。

## 2. `online` 补 `--ui-language`（步 0 小项，commit 13172ce）

- 语义与 `prepare --ui-language` 的**区别**：prepare 那个只写进任务 env 给恢复环境用；online 在 Windows 里直接跑，
  旗标切**本进程**语言。优先级：**显式旗标 > env `BACKUPRESTORE_LANG` > ini `ui_language` > 中文**。
- 只认 `zh*`/`en*` 前缀，其余明确拒绝（文案与 prepare 一致）。
- usage 一直没列 `online` 命令——本轮补上完整命令行（含全部可选旗标）。
- **实机验证**：`online --operation backup --source-drive T: --image-path H:\BRImages\lang-test-20261025.wim --ui-language en --json`
  → `success:true`，日志整段英文（`Log location:` / `STEP 1/4 Validating …`，6 处英文标记行）。

## 3. 实机撞出并修复：online 备份不传 `--compress` 必失败（commit a526bee）

- **现象**：上面那条命令第一次跑（当时还没修 compress）失败：`[BR-CORE-034] compression must be fast or none; max compression is not supported`。
- **根因**：`online_cli` 把 `--compress` 空串原样下传，core 的 `canonical_compression("")` 直接判死；
  而 GUI 口径是「下拉不选 = fast」（`compression_from_ui_index(None) => "fast"`）。**可选旗标变成了必填**。
- **为什么一直没暴露**：冒烟脚本显式传 `--compress fast`。
- **修法**：解析后空值归一 `fast`（usage 标注 default fast）。★ 与 v2.6.44「配置读了但没接线」同类：
  **默认值断链只有真实命令行会踩到**——脚本永远显式传参。
- 修复后同命令 `success:true`。单测 247+45、交叉编译通过、VM 内部署重验。

## 4. VM 构建目标目录改 `H:\br-target`（构建链路变更，后续照此执行）

- **现象**：`build-windows.ps1 -Deploy` 在 `X:\target`（= 共享文件夹 9p 挂载）上
  `failed to remove temporary directory: 参数错误 (os error 87)`，重试仍失败（换着 crate 报）。
- **诱因**：macOS 侧跑了一次 release 交叉编译（`./build-win.sh`），把 `target/` 里宿主机构建脚本产物搅动，
  VM 侧 cargo 重建时在共享盘上删临时目录触发 87。
- **处置**：VM 构建统一加 `-CargoTargetDir H:\br-target`（虚拟机本地盘），与 mac 侧 target 彻底隔离。
  全量 21.61s 完成，部署 SHA `11ac5890…`（BackupRestore.exe/Recovery.exe 一致，wimlib 两件不变）。
- **新约定**：以后 VM 内部署一律 `powershell -ExecutionPolicy Bypass -File X:\windows\build-windows.ps1 -Architecture arm64 -Deploy -CargoTargetDir H:\br-target`；
  mac 侧 `./build-win.sh` 只做类型检查，不再与 VM 共用 target。

## 5. E1 用户手册落库 + T: 走查验收

- 新建 [`docs/USER-GUIDE.md`](./USER-GUIDE.md)（活文档，与 ERROR-CODES.md 同属命名例外的常青文件；
  先中文）。界面描写以运行中的 GUI v2.6.50 截图为准（五个入口：备份 / 还原系统到 C 盘 / 还原数据到非 C 盘 /
  新增非 C 盘第二系统 / 安装 PE；顶栏：取消待重启任务… / 设置… / 语言）。
- **走查验收（按手册命令，T: 5G 测试卷）**：
  1. 备份：`online --operation backup --source-drive T: --image-path H:\BRImages\lang-test-20261025.wim --ui-language en --json` → success；
  2. 放哨兵 `T:\BR-0531.txt` + 记录序列号 `BC9C-BF66`；
  3. 还原：`online --operation restore-existing --source-drive T: --target-drive T: --image-path … --index 1 --format-target --json` → success；
  4. **哨兵消失 + 序列号 `BC9C-BF66` → `0E74-C8BA`**（格式化+应用真实发生的双铁证）；`dir T:\` 空。
- 任务日志目录 `BackupRestore-backup-*` / `BackupRestore-restore-*`（B6 命名分叉在真实 CLI 链上再次印证）。

## 6. 现场状态

GUI 已从 H:\brwork 重启（v2.6.50 新部署）；default 指原系统；T: 空卷；证据与过程产物
（`.test-artifacts/bcd-menu-cleanup-20261025/`、`H:\BRImages\lang-test-20261025.wim`）按「证据全留」保留。

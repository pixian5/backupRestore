# 纯 WinPE 走 create-secondary 被产品按设计拒绝（2.4.2 实测）

> 一句话结论：**纯 WinPE 镜像（Edition=WindowsPE）走 `prepare --operation create-secondary`
> 会在准备阶段的"镜像完整性预检"被直接拒绝，并明确提示改用「PE 恢复」功能。
> 零副作用：未格式化任何卷、未写引导、未建任务、未改 BCD。**
> 这正是产品把"PE 安装"与"第二系统部署"两条路分开设计的结果——纯 PE 的正确入口是
> 「PE 恢复」（GUI tab 4 / 桌面快捷方式），不是 create-secondary。

## 1. 复现

- 镜像：`pe-assets/pe-boot.wim`（ARM64 纯 WinPE，`wimlib-imagex info` 显示
  `Name: Microsoft Windows PE (arm64)`、`Edition ID: WindowsPE`、`Installation Type: WindowsPE`、
  `Build: 28000`，367,395,379 字节）。该镜像能在 ARM64 Parallels VM 上启动（架构匹配）。
- 构建：macOS 交叉编译 `./build-win.sh --deploy`，产物 `BackupRestore.exe` SHA-256
  `68e217cba7258552468873097db125bae32bc081c59d0ad60f467ec66ecb5966`，已核对四份二进制一致。
- 命令（完整自动化，未加 `--no-reboot`）：

```
H:\brwork\BackupRestore.exe prepare --operation create-secondary ^
  --source-drive C --target-drive P ^
  --image-path <pe.wim> --boot-menu-name 纯PE测试 --allow-destructive
```

### 结果 A：镜像放在系统卷 C:\BR\pe-boot.wim

```
error: invalid task: 镜像存放位置不能位于承载 Windows 恢复环境（WinRE）的分区。请更换镜像保存卷。
```

角色校验（validate_volume_roles）拒绝：C: 同时承载 `C:\Recovery\WindowsRE`（WinRE 宿主），
因此 C: 不能作镜像卷。

### 结果 B：镜像放在数据卷 F:\BR\pe-boot.wim（用户真实场景）

```
error: invalid task: 镜像"Microsoft Windows PE (arm64)"是 Windows PE，不是完整 Windows；
要安装 PE 请改用「PE 恢复」功能。本次未格式化任何卷、未写入引导。
```

完整性预检（wim_full_windows_probe）拒绝：纯 PE 没有 `reagentc.exe` 与 `SYSTEM` hive，
且镜像名含 "Microsoft Windows PE"，被名称启发式命中。

## 2. 判定逻辑（源码位置）

`crates/backuprestore-cli/src/windows_prepare.rs`：

- `wim_full_windows_probe`（:1359）：**擦除前**的镜像完整性预检。背景是
  2026-10-11 实验发现纯 PE 走 create-secondary 会在"目标卷已格式化、镜像已应用"之后
  才死在离线 WinRE 注册前置检查（缺 `reagentc.exe`/`SYSTEM` hive），目标白毁；
  这里把同样判据前移，避免破坏性副作用。
- `wim_mount_markers`（:1398）：只读 `/ReadOnly` 挂载镜像，探测 `reagentc.exe` + `SYSTEM` hive。
  - 挂载成功且缺标记 → `Ok(false)` → 拒（"镜像不是完整 Windows…请改用 PE 恢复"）。
  - 挂载成功且标记齐全 → `Ok(true)` → 放行（视为完整 Windows）。
  - 挂载失败（如本机附 VHD 时 DISM 报 87，探测不可用）→ 降级为**镜像名启发式**：
    名称含 "windows pe" / "winpe" 即拒，否则放行（恢复阶段离线注册仍是硬兜底）。

本次命中"挂载失败→名称启发式"分支（VM 上 DISM 挂载探测在该环境报错），
"Microsoft Windows PE" 匹配 "windows pe" → 拒。两条分支对纯 PE 结论一致：**拒**。

## 3. 设计意图：两条路径分开

- **create-secondary（新增第二系统）** = 部署一份**完整 Windows** 到目标卷（捕获当前系统 /
  应用镜像 / `bcdboot` 建启动项）。它要求镜像带 `reagentc.exe`+`SYSTEM` hive，即完整 Windows。
- **PE 恢复（GUI tab 4 / 桌面快捷方式「打开 PE 恢复」）** = 安装一个自包含的 PE 恢复环境，
  作为开机**可选**启动项「Windows PE (BackupRestore)」（RAM disk 或硬盘启动两种模式），
  **不改当前 Windows 默认启动**。这才是为纯 PE 设计的路径（`native_gui.rs` 多处文案确认）。
- **CLI 当前没有 pe-recovery 操作**：`usage()` 仅列
  `probe | backup | restore-existing | create-secondary`。纯 PE 安装目前只能走 GUI。

所以用户此前的设想"直接创建第二系统、选 PE 的 WIM、直接当 RE 硬盘版"在产品里不成立：
create-secondary 接收的是**完整 Windows 镜像**，纯 PE 被路由到 PE 恢复。

## 4. 与 WinRE 的关键对比（别混淆）

- **WinRE.wim**（Edition=WindowsRE，带恢复标记）能通过完整性预检 → 走 create-secondary →
  但暂存卷 = 镜像卷；若镜像在数据卷，则 bootmgr 解析不了数据卷盘符 → 一次性启动被丢弃、回落 C:
  （这是 2026-10-12 已证实的"新启动通道盘符解析回归"bug，详见
  `docs/20261012-1935-新启动通道盘符解析回归根因与修复方向-Step-5-Preview.md`）。
- **纯 WinPE**（Edition=WindowsPE）在更前面就被完整性预检拒绝，**根本到不了启动那一步**。

一句话：对纯 PE，"RE 硬盘版"的正确做法是 PE 恢复；create-secondary 不是它的入口，
强行喂纯 PE 只会被拒。

## 5. 验证无副作用（已核验）

- `H:\brwork\tasks` 仍为 `NO_TASKS_DIR`：准备在 `Task::new` 之前就失败，未建任务目录。
- `bcdedit /enum {bootmgr} /v` 的 `bootsequence` 为空：未武装一次性启动、未建 BCD 对象。
- 测试产物已清理：`C:\BR\pe-boot.wim`、`F:\BR\pe-boot.wim`（VM 侧）、
  `target/pe-boot.wim`（宿主侧）、临时 HTTP 服务（pid 70710）均已删除/停止。

## 6. 下一步

- 若用户想验证"纯 PE 当 RE 硬盘版"的真实效果，应测 **PE 恢复**功能（GUI tab 4），
  而非 create-secondary。该路径会新增「Windows PE (BackupRestore)」启动项并支持 RAM disk / 硬盘启动。
- 当前 CLI 缺 pe-recovery 命令；若需命令行安装纯 PE，要补一个 `--operation pe-recovery`
  （或等价子命令），对齐 GUI 既有安装逻辑。
- 暂不修改源码（本轮仅测试观察），版本保持 2.4.2。

# v1.7.8 VM 实机验证：部分通过 + 发现一个真实 BUG

> 日期：2026-09-28
> 结论：**v1.7.8 常规备份周期 VM 端到端通过（2 个完整任务）**；但**续跑修复路径（`ensure_registered_is_payload`）在实机上失败**，
> 报错 `volume has no disk number`——这是一个真实 BUG，没有这次 VM 验证就会上线。
> 相关：[20260928-074516-registered-winre-policy-and-resume-design.md](20260928-074516-registered-winre-policy-and-resume-design.md)

## 一、已验证通过的部分（真实证据）

| 项 | 证据 |
|---|---|
| v1.7.8 部署 | `H:\brwork\BackupRestore.exe` SHA256 = `2b1529db…`（与宿主构建一致） |
| 任务1 完整周期 | `469922e5`（源 T: ≈0.5GB）：prepare→WinRE 自动拉起→DISM 捕获 368MB→回 Windows，`stage=success` |
| 任务2 完整周期（加大数据） | `9bffc7a1`（T: 灌 3.5GB 随机数据后重跑）：捕获 4.06GB 镜像 `E:\brimg\v178-cut.wim`，`stage=success`，全程 ~2 分钟 |
| 终态注册位还原 | 两个任务结束后 `C:\Recovery\WindowsRE\Winre.wim` 哈希均 = `1060a552…`（original），非侵入不变量 C 成立 |
| manifest 正确性 | `createdByVersion: "1.7.8"`；original=`1060a552`、staged=`2b1529db`（注入后不同，符合预期） |

## 二、发现的真实 BUG（本次验证的最大价值）

### 现象
用 `--test-fault power-loss-window` 制造 durable 中断态（`stage=boot-requested`、未重启）后，
resume 链在 Windows 侧触发了**两次**（08:58 GUI 自启 + 09:02），但**修复路径失败**：

```
[00:58:15] Detected durable interrupted task after normal Windows startup; resuming task 6571753e … stage=BootRequested
[00:58:16] Pending boot recovery abandoned: registered WinRE could not host the payload:
           invalid task: volume has no disk number
[01:02:32] Pending boot recovery abandoned: （同样报错，第二次）
```

### 定位
报错文本 `volume has no disk number` 来自 `windows_prepare.rs` 的卷身份解析
（`.ok_or_else(|| err("volume has no disk number"))`）。resume 路径新增的
`ensure_registered_is_payload` → `ensure_volume_mounted(recovery, 'R', log)` 在解析
RECOVERY 卷身份时，`identity.disk_number` 为 None。

### 为什么 prepare 没炸、resume 炸了
prepare 路径部署载荷用的是**另一条**挂载/身份推导代码（对 RECOVERY 卷不做 disk_number 强制）；
而 resume 修复路径复用了 `ensure_volume_mounted`，它**强制要求 disk_number**。
本 VM 的 WinRE 注册在 `harddisk0\partition4`（C: 所在盘的恢复分区，环境里 RECOVERY 身份无独立 disk number
可解析）→ resume 路径一进来就 `ok_or_else` 硬失败。

### 影响
- **旧缺口依旧存在**：捕获阶段断电 → resume 仍然起不来（正是本次要修的场景）。
- 且比旧缺口更糟：resume 失败还会**烧掉每 stage 一次的重试额度**（claim 已成功、release 只在
  boottore/shutdown 失败时调用）→ 第二次中断直接进人工介入分支。

### 修复方向（待做）
1. resume 修复路径**不要复用 `ensure_volume_mounted` 的 disk_number 强制语义**；
   RECOVERY 卷在本项目环境里挂在注册卷所在盘（`harddisk0`），可退化为
   `if let Some(letter) = identity.drive_letter { return Ok(letter) }` 的既有短路 +
   失败时回退到「按 volume GUID 枚举盘符」而非 disk/partition 编号。
2. 或者：resume 侧不重新解析身份，直接复用 prepare 已写盘的 `mount/` 产物（prepare 已挂载过一次）。
3. 修复后必须补的回归测试：RECOVERY env 无 disk number 时 `ensure_registered_is_payload` 不得失败。

## 三、测试环境事实（复跑必读）

- `--test-fault power-loss-window`：prepare 在 durable `boot-requested` 后**停住不重启**（不发 shutdown）。
- VM 磁盘极快：T:（5.4GB 盘、灌 3.5GB 随机数据）完整备份周期仅 ~2 分钟 → **靠轮询截图抢断电窗口不可行**。
- 本 VM 的 WinRE 注册位 = `harddisk0\partition4`（C: 盘内恢复分区），RECOVERY env 身份无独立 disk number
  → 这正是触发上述 BUG 的环境条件。
- 翻转模拟已验证可行：`copy original\Winre.wim → C:\Recovery\WindowsRE\Winre.wim` 后注册位=`1060a552`、
  stage 载荷 `2b1529db` 完好——修复路径的前置状态可以无损构造。
- `prlctl exec` 走 SYSTEM 通道可以无 GUI 触发 resume（resume 在窗口创建前返回），
  但**复杂命令（变量/管道/引号嵌套）会被执行环境改写**，必须用最朴素的 ASCII 单命令。

## 四、遗留清单

1. **修复 `volume has no disk number` BUG**（最高优先，见第二节修复方向）→ 重编译 → 重跑本验证。
2. resume 修复成功后的完整闭环（修复→boottore→重启→WinRE 自动续跑→捕获→终态还原）仍未实机走通。
3. 测试残留清理：`T:\fill.bin`（3.5GB）、任务 `6571753e`（boot-requested 挂起态）、
   `E:\brimg\v178-resume.wim`/`v178-cut.wim`。

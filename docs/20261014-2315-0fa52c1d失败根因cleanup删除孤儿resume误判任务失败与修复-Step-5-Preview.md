# 0fa52c1d 失败根因定位与修复：cleanup 删除孤儿 resume 误判整任务失败（附今晚救援接手复盘）

> 模型：Step-5-Preview｜2026-10-14 23:15｜版本 v2.5.6 → **v2.5.7**｜前置：`docs/20261014-2244-项目交接说明-VM卡自动修复与救援全程-GLM-5.md`
>
> 一句话：**任务 0fa52c1d 的实质工作 100% 成功了**（F: 第二系统已装好、引导项正确、
> 盘符绑定已做），却在最后一步「删除一个已不存在的孤儿 resume object」上因代码用 `?`
> 把失败当致命错误 → 整任务判失败 →「保留 boot entry 与载荷」→ 下次启动 bootsequence
> 指向失效项报 `0xc000000e` → 触发今晚整场人工救援。

---

## 1. 接手时的状态与处置

| 阶段 | 操作 | 结果 |
| --- | --- | --- |
| 接手时 | 交接文档记录「VM 卡在正在准备自动修复」 | 实测画面**已变成 PE 命令提示符** `X:\Windows\System32>`（状态已自行变化） |
| 取硬证据 | `prlctl capture` 截图（`prlctl` 的截图动作名是 **`capture`**，不是 `screenshot`） | 拿到画面 |
| PE 内验 BCD | `vmtype.sh` 逐键输入 `bcdedit /enum {bootmgr} /v` → **截图核对命令完整** → 回车 | `default={12e3701c…}`（主系统）、**无 bootsequence** ✅ |
| 重启前检查 | `prlctl list -i` | Boot order = **hdd0**、无光驱设备 → 重启必走硬盘 BCD |
| 回系统 | `wpeutil reboot`（同样先截图核对）→ 每 10 秒截图 + md5 变化检测 | 第 8 帧起画面稳定 = **已进主系统桌面**（BackupRestore GUI v2.5.6 在跑） |
| 系统内硬验证 | `bcdedit /enum {bootmgr} /v` | `default={12e3701c…}`、无 bootsequence —— 与 PE 内一致 ✅ |

**没有用到快照回滚**（交接文档 §3 的第 3 步），VM 自己恢复了主系统。

---

## 2. 失败根因（证据链）

### 2.1 任务终态

```json
// H:\brwork\tasks\0fa52c1d-40d2-411b-9b9f-8a03f130c264\status.json
{ "operation": "create-secondary", "stage": "boot-repaired", "progress": 90,
  "errorCode": 1, "error": "invalid task: bcdedit.exe failed with exit code: 1" }
```

`stage=boot-repaired` 说明已到 STEP 3/4（引导修复阶段），且**已经写入了 BootRepaired 状态**。

### 2.2 主日志（`H:\BRImages\Recovery.log`）显示**两个完整周期，两次都卡在同一处**

**周期 1（14:26:41 ~ 14:26:57）**：

```
14:26:42  STEP 1/4 挂载卷、校验
14:26:44  reg.exe … MountedDevices /v \DosDevices\C: /t REG_BINARY /d 444D494F3A49443A663775F0A4300E41944F38D139113634
14:26:44  第二系统离线盘符已绑定并核验：C: -> {f0753766-30a4-410e-944f-38d139113634}；原配置备份 …\BackupRestore-secondary-original   ✅
14:26:46  bcdboot.exe F:\Windows /s Z:\ /f UEFI /v /d /addlast /description 2nd-1014-v2                       ✅
14:26:52  WinRE existing registration reconnected to repaired system loader                                    ✅
14:26:57  Recovery retry required at BootRepaired: invalid task: bcdedit.exe failed with exit code: 1          ❌
```

**周期 2（14:30:00 ~ 14:30:14）**：重试，步骤完全相同（含 repair、bcdboot、WinRE 重连全部再次成功），
最后仍卡在同一处。周期 2 的尾部（完整 8 步）：

```
14:30:12  WinRE registration verified: Enabled; target location, recoverysequence, ramdisk and original hashes match  ✅
14:30:12  /set {be683a7b…} description 2nd-1014-v2    → Secondary Windows loader {be683a7b…} named 2nd-1014-v2      ✅
14:30:13  /set {bootmgr} default {12e3701c…}                                                                        ✅
14:30:13  /deletevalue {bootmgr} resumeobject                                                                       ✅
14:30:13  /displayorder {12e3701c…} {152dcf4e…} {be683a7b…}                                                         ✅
14:30:14  running bcdedit.exe /delete {2ad660f2-c7ce-11f1-b35f-9cc8e18b08a2} /f                                     ❌ exit 1
14:30:14  Recovery retry required at BootRepaired; boot entry and payload retained
```

### 2.3 逐步判定

| 步骤 | 状态 |
| --- | --- |
| diskpart 格式化 F: | ✅（第一轮、14:26 之前完成，因 stage 已是 BootRepaired，重试跳过） |
| DISM Apply-Image → F: | ✅ |
| **secondary_registry::repair**（第二系统盘符绑定） | ✅ `C: -> {f0753766-30a4-410e-944f-38d139113634}`（该 GUID 就是 F: 的分区 GUID） |
| bcdboot `/d /addlast /description` | ✅ |
| WinRE 注册重连 + 校验 | ✅ `verified: Enabled; recoverysequence, ramdisk and original hashes match` |
| 菜单命名 | ✅ `{be683a7b…}` = `2nd-1014-v2` |
| `{bootmgr}` default 恢复 | ✅ |
| displayorder 写入 | ✅ |
| **清理孤儿 resume object** | ❌ **唯一失败点** |

### 2.4 失败命令对应的源码

`crates/backuprestore-cli/src/main.rs:3458`（函数 `cleanup_orphaned_secondary_resume_objects`）：

```rust
run_logged("bcdedit.exe", &["/delete", &guid, "/f"], log)?;   // ← 缺陷：把「删不掉」当致命
```

**为什么 exit 1**：被删的 `{2ad660f2-c7ce-11f1-b35f-9cc8e18b08a2}` **已经不在活动 BCD store 里**
（它属于早前一轮的 F: 条目对，已被 bcdboot / 早前步骤移除或从未持久化）。
`bcdedit /delete <不存在的 GUID>` 返回 exit 1（"指定的项未找到"）。

**语义错位**：这个函数的目标是「**确保**孤儿 resume object 不存在」。
既然它本来就不存在 → **目的已达成**，绝不该判失败。

### 2.5 项目里已有正确模式（本次修复对齐它）

`crates/backuprestore-cli/src/boot_entry.rs::remove_entry_objects`：

```rust
for guid in [&loader, &devopts] {
    if let Err(error) = bcd(&["/delete", guid, "/f"]) {
        // 已删除的对象再次清理可能报错；是否成功以枚举回读为准。
        crate::append_log(log, &format!("new boot channel: delete {guid}: {error}"))?;
    }
}
let remaining = bcd(&["/enum", "all", "/v"])?.to_ascii_lowercase();     // ← 回读验证才是判据
if remaining.contains(&loader…) || remaining.contains(&devopts…) { return Err(…); }
```

**删除容错 + 枚举回读验证**才是这个项目认可的写法。同文件的 `preserve_primary_boot_manager`
也已谨慎处理（`resumeobject` 快照有条件判断，不会删不存在的值）。

---

## 3. 修复（v2.5.7）

`main.rs::cleanup_orphaned_secondary_resume_objects`：删除失败改为**记录日志并继续**，
并保留「是否真的不存在」的确认语义（失败原因已写入日志）：

```rust
match run_logged("bcdedit.exe", &["/delete", &guid, "/f"], log) {
    Ok(()) => append_log(log, &format!("Removed orphan resume object {guid} for replaced loader {loader}"))?,
    Err(error) => append_log(log, &format!(
        "Orphan resume object {guid} for replaced loader {loader} is already absent (deletion skipped): {error}"))?,
}
```

- 版本三处同步：`VERSION` + 两个 `Cargo.toml` → **2.5.7**
- 构建部署：`./build-win.sh --deploy` → 产物 2,541,568 B，SHA256 `491bff5eec8a48ad9a8818d21b79544faefc5c523e298e6f0c59de11b918e9a7`
- 部署校验：宿主与客体哈希一致；二进制内含 `2.5.7` 且不含 `2.5.6` ✅

---

## 4. 副作用与遗留（本次未动，需用户决策）

### 4.1 BCD 残留条目（`bcdedit /enum osloader /v` 实测）

| GUID | 内容 | 判断 |
| --- | --- | --- |
| `{12e3701c-c361-11f1…}` | device/osdevice = `partition=C:`，desc `Windows 11` | ✅ **主系统**（default 指向它） |
| `{be683a7b-c7db-11f1…}` | device/osdevice = `partition=F:`，desc `2nd-1014-v2` | ✅ **F: 第二系统**（本次任务成果） |
| `{152dcf4e-c674-11f1…}` | **device/osdevice = unknown**，desc `Professional 10.0.26200` | ⚠️ **孤儿**（却仍在 displayorder 里；选中必然失败） |
| `{1b8096b0-c7d8-11f1…}` | ramdisk `H:\BackupRestoreRE\0fa52c1d-…\Winre.wim`，desc `BackupRestore task RE` | ⚠️ **失败任务的一次性条目残留** |
| `{6c70c110/6c70c112-c6f8-11f1…}` | ramdisk `[F:]\devpathtest\Winre.wim`，desc `SVTestA loader` | ⚠️ 测试残留（devpathtest） |
| 若干 `ramdisk=[unknown]\Recovery\WindowsRE\Winre.wim` | desc `Windows Recovery Environment` | 历史 WinRE ramdisk 条目（设备 unknown） |

⚠️ **不要按"看着多了"清理**（项目历史教训）：每个系统必须有自己的 WinRE 注册对象，
删错会砸掉休眠恢复入口。清理前逐个确认归属。

### 4.2 载荷与任务目录

- `H:\BackupRestoreRE\0fa52c1d-40d2-411b-9b9f-8a03f130c264\`（载荷 WIM，因"retained"保留）
- `H:\brwork\tasks\` 下 14 个任务目录（多数为已终结的历史任务）

### 4.3 为什么这次失败会升级成 `0xc000000e`

任务判失败 → 「boot entry and payload retained」→ **bootsequence 未被清除**，仍指向
任务自己的 RE 条目 `{1b8096b0…}`（ramdisk 指向镜像卷上的载荷 WIM）。该一次性启动项
在后续的重启中无法正确解析设备 → `0xc000000e`。人工在 PE 里
`bcdedit /deletevalue {bootmgr} bootsequence` 才解开。

**修复本次缺陷后，这条链路不会再有触发点**（不再有"实质成功却判失败"的情形）。

---

## 5. 教训（值得固化的判定规则）

1. **「让它不存在」类操作的失败语义要反过来看**：`bcdedit /delete`、`/deletevalue`、
   `Remove-Item -ErrorAction SilentlyContinue` —— 目标已不存在 = **目的达成**
   （除非能证明"不存在"本身是异常）。凡是清理/收尾动作，都不该用 `?` 把"已经不在"当致命。
2. **这类 bug 的现场特征**（可用于快速识别）：
   - 任务在**最终阶段**失败，但日志里所有实质步骤都是 ✅；
   - 报错命令是 `/delete`、`/deletevalue` 一类**清理命令**；
   - 注释里已存在同类问题的记录（本次 `preserve_primary_boot_manager` 的注释就描述过
     一个"每次重试都会同样失败"的 resumeobject 失配问题）——**同类问题要一次找齐**。
3. **PE 里能拿到硬证据就别猜**：本次在 PE 命令行直接 `bcdedit /enum {bootmgr} /v` 拿到
   default/bootsequence 实况，比回系统后再验证快得多。
4. **逐键输入铁律继续有效**：`vmtype.sh` 打完命令 → **截图核对完整** → 才回车（本次两步都照做，零失误）。

---

## 6. 下一步

1. **验证 F: 第二系统能启动到桌面**（今晚失败任务的最终目标；素材：菜单项 `2nd-1014-v2`、
   F: 完整 Windows、盘符绑定与 WinRE 注册均已就绪）。建议用一次性启动：
   `bcdedit /bootsequence {be683a7b-c7db-11f1-9425-9b0fdf705de1}`（启动成功会自动清除）。
2. 决策清理项（§4.1/§4.2），需用户确认归属后再动。
3. 交接文档 §4 其余待办（阶段 1c/4/5、P3/P4/P5、安装 PE 端到端）。

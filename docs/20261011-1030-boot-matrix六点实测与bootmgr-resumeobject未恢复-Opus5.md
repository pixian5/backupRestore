# boot-matrix 六点实测与 `{bootmgr}.resumeobject` 未恢复（2.2.4，Opus 5 / Fable 5）

- 时间：2026-10-11 03:16 – 10:18
- 产物：`BackupRestore.exe` SHA-256 `bef5ccd77df2821f…`（2.2.4，与 cleanup 矩阵同构建）
- 场景：`acceptance:boot-matrix:hold` + `--boot --system-image`，操作 `create-secondary`，
  目标第二系统卷 `{fd0a50cf-735d-4ed5-bb6d-a4ab0d4eff47}`（E:），源镜像 `F:\BRRE-217-20261008\system.wim`（35.6 GB）
- 任务 ID：`826d8b91-324f-4e8a-9086-4e0f481fa904`
- 证据：`.test-artifacts/re224-power-20261011/boot-matrix/`（各检查点 JPEG/PNG、`power-cut-*.json`、
  `guest/`、`Recovery.log`、`bcd-created.txt`、`winre-created.txt`、`system-created.json`、`verify-101602.txt`）

## 一、六个检查点全部真实断电并恢复

| 检查点 | 代码位置 | marker 时刻 | 断电时刻 | 断电前确认的关键日志 |
|---|---|---|---|---|
| bcdboot-before | main.rs（BCDBoot 调用前） | 03:24:10 | 03:42 | 第二系统离线盘符绑定核验 `C:`→`{fd0a50cf}`；原 SYSTEM 配置备份 |
| bcdboot-running | 同上（真实 bcdboot 跑 10 ms 后挂起） | 03:44:26 | 03:48 | `BCDBOOT_PROCESS_SUSPENDED pid=1652` |
| bcdboot-after | 同上（BCDBoot 完成后） | 03:50:17 | 09:49 | `已成功创建启动文件`、BFSVC 设置 `{default}` |
| registration-register | winre_registration.rs:193 | 09:54:36 | 10:03 | `reagentc /setreimage` 操作成功，目录设为 harddisk3\partition2 |
| registration-enable | winre_registration.rs:195 | 10:08:44 | 10:10 | `reagentc /enable /osguid {ab6d9baa…}` 操作成功 |
| registration-reconnect | winre_registration.rs:198 | 10:11:19 | 10:12 | `bcdedit /store Z:\EFI\…\BCD /set {08257b46…} recoveryenabled Yes`、`WinRE existing registration reconnected to repaired system loader` |

断电后恢复方式：前五次由 `run.py resume`（每次先建并核验新快照）或 GUI 自启动自动续跑；
**最后一次（reconnect）断电后自动续跑直接把任务走完**，10:14:34 写 `success` 并 `wpeutil reboot`。

## 二、核心安全断言（从客体 `F:\BRRE-217-20261008\Recovery.log`，20160 行）

- **FORMAT_COUNT=1**：全程只有一条 `DiskPart 成功格式化该卷`；`diskpart-format.txt` 仅一段脚本（`select disk 3 / select partition 2 / format fs=ntfs quick`）
- **APPLY_COUNT=1**：全程只有一段 `Applying image`——**35.6 GB 系统镜像在六次断电中只应用了一次**
- 阶段跳过机制实证（8 次进入 WinRE 的 STEP 序列）：

  | 轮次 | STEP 序列 | 结束于 |
  |---|---|---|
  | 1（19:19:53Z） | 1/4 挂载 → **2/4 还原镜像** → 3/4 校验 | bcdboot-before hold |
  | 2–7 | 1/4 挂载 → 3/4 校验（**2/4 全部跳过**） | 其余五个检查点 / 无 |
  | 7 末（02:14:33Z） | → **4/4 清理** | success |

- `SUSPEND_COUNT=1`（仅 bcdboot-running 检查点挂起过真实 bcdboot 进程）
- 终态清理：`one-shot bootsequence cleared` → `BCD objects removed and verified` →
  staging `\\?\Volume{f0753766…}\BackupRestoreRE\826d8b91…` removed and verified → `Boot entry cleaned; task marked successful`

## 三、验收脚本结论

`run.py verify`（`verify-101602.txt`）：`status=success`、`primaryRecoveryUnchanged=true`、`systemVerificationRequired=true`。

`system.ps1 -Action created`（`system-created.json`，RC 0，基线 `re219-reliability-20261009/system-baseline` 同镜像）：

```json
{"task":"826d8b91-…","stage":"success","target":"{fd0a50cf-…}",
 "secondaryLoader":"{78fecd93-c519-11f1-9112-c28d261e863e}",
 "secondaryRecovery":"{af1ecd39-c518-11f1-8ac2-9a808d18a8a6}",
 "primaryLoader":"{12e3701c-c361-11f1-90a2-c222b5348ef7}",
 "filesVerified":true,"independentRecovery":true,
 "currentPartition":"{761230e8-…}","action":"created"}
```

脚本内部已通过的断言：主/次系统关键文件哈希与基线一致、fixture 哈希一致、格式化哨兵消失、
第二系统 loader 唯一且 `device partition=E:`、独立恢复指向 `ramdisk=[E:]\Recovery\WindowsRE\Winre.wim`
（非主系统恢复）、主系统 BCD 对象未变、`default`/`timeout` 未变、原菜单顺序保留并仅追加一项、
主系统 `reagentc /info` 未变、本任务临时 loader/devopts 不在终态 BCD、`bootsequence` 为空。

值得注意：bcdboot 以 `/addlast` **跑了 6 次**（每次续跑重做，属幂等步骤），
但 `system.ps1` 的菜单顺序断言通过且第二系统 loader 唯一——**6 次 `/addlast` 没有堆积重复菜单项**。

## 四、新发现（两处，均建议收口，待用户决定）

### 发现 1：`{bootmgr}` 的 `resumeobject` 被 BCDBoot 改写且产品未恢复

`bcd-before.txt` 与 `bcd-created.txt` 的 Windows Boot Manager 块逐字段比对：

| 字段 | before | created | 判定 |
|---|---|---|---|
| `default` | `{12e3701c}` | `{12e3701c}` | 未变 ✅ |
| `displayorder` 首项 | `{12e3701c}` | `{12e3701c}` | 未变 ✅ |
| `displayorder` 次项 | `{bce8e6b3}`（旧第二系统） | `{78fecd93}`（新第二系统） | 预期替换 ✅ |
| `timeout` | `10` | `10` | 未变 ✅ |
| **`resumeobject`** | **`{bce8e6b2}`** | **`{78fecd92}`** | **被改写 ⚠️** |

`{78fecd92}` 是本次新建的 `Windows Resume Application`：`device partition=E:`、
`path \Windows\system32\winresume.efi`、`filepath \hiberfil.sys`——即 `{bootmgr}` 的休眠恢复对象
指向了第二系统卷。

原因定位：`main.rs` 的 `preserve_primary_boot_manager` 在 BCDBoot 之后只恢复两个字段
（`BcdBootManagerState` 结构体也只有 `default` 与 `display_order`），未捕获/未恢复 `resumeobject`。
代码注释已意识到"BCDBoot can replace Boot Manager's default with the new loader"，但同类影响的
`resumeobject` 没有纳入。

影响评估（不夸大）：
- 主系统 loader `{12e3701c}` **自己的** `resumeobject {12e3701b}` 仍指向 `partition=C:`、`filedevice partition=C:`，**完好**；
- **不是本轮回归**：before 的 `{bce8e6b2}` 同样是 E: 分区的 resume 对象，说明 2.2.3 创建第二系统时就已被改写；
- 验收脚本 `system.ps1` 只断言 `default`/`timeout`，**漏检 `resumeobject`**，这是历轮未暴露的直接原因。

建议：`BcdBootManagerState` 增加 `resume_object` 字段并在 `preserve_primary_boot_manager` 中一并恢复；
同时给 `system.ps1` 的 `created` 分支加上 `resumeobject` 断言，否则修了也测不出来。

### 发现 2：替换第二系统时旧 resume 对象成为孤儿

旧第二系统 loader `{bce8e6b3}` 在本轮被正确删除（终态 BCD 中已不存在），
但其配套 `{bce8e6b2}`（`Windows Resume Application`, `device partition=E:`）**仍留在 BCD 中**。

累积效果（当前测试机终态 BCD 实测）：**30+ 个孤儿 `Windows Resume Application`**
（C:/E: 各卷混杂）、**10+ 个孤儿 `Windows Recovery Environment`**（C:/E:/F:/Y:），
以及 `PE`、`MyCustomPE`、`device unknown` 的历史残留对象。

说明：这些主要是历次测试累积，但根因是"创建/替换第二系统时只删 loader、不删其配套 resume 对象"。
生产环境反复重建第二系统会同样累积。建议删除第二系统 loader 时顺带删除它引用的 `resumeobject` 对象
（需先确认该对象未被其它 loader 引用）。

## 五、过程记录

- 快照链（每次启动配置变更前均先建并核验，最多保留 2 个）：`{c4e639ba}`（03:16 prepare）→
  `{504fb2d5}`（09:51 resume）→ `{c2e133c9}`（10:06 resume）→ `{c39d95e3}`（10:06）
- 03:42 的 `bcdboot-before` 断电是接续操作：该 hold 自 03:24 起已停住约 18 分钟
  （本会话另一进程实例启动后未继续推进），确认画面与 marker 后接手断电。
- `bcdboot-after` 断电后出现 Windows 启动菜单（两项 `Windows 11`，分别位于卷 4 与卷 13，
  默认项 10 秒倒计时）——证明第二系统菜单项此时已写入且可见。
- 09:49 之后的四次断电客体均正常回到 Windows（未触发自动修复，与 cleanup 矩阵 cp4/cp5 不同）。
- 判据经验（重要）：**WinRE 进度窗口的步骤标签（1/4~4/4）不是进度判据**——三个 registration
  检查点全部位于 `STEP 3/4 校验` 内部，画面看起来"一直停在 3/4"。真实判据是
  `ACCEPTANCE_CHECKPOINT` 行、marker 文件数量与 `status.json` 的 stage/progress。

## 六、未执行项

- `system.ps1 -Action booted`（实际启动第二系统并核验 C: 盘符/服务/桌面）：本轮未做，
  2.2.3 已单独验证过同类链路；如需在新构建下复验，需另建快照并实际切换启动项。
- T: 数据卷夹具哈希补验（cleanup 矩阵遗留，见 20261011-0330 文档 §五）。

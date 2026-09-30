# v1.8.1：方案 A 门槛通过（E1/E2/E3）+ ESP 日志迁出根目录 + 合理清理快照

- 日期：2026-09-29
- 版本：v1.8.1（1,779,200 B，SHA-256 `605c32a0b10851e0e74388709266ae1f8dbfe804568f390e52339091760f1f16`）
- 证据：`.test-artifacts/elev-channel/`（未入库）

## 一、方案 A 能否实施？——能，前置门槛已过

原方案把「卷 GUID 路径、零临时盘符」列为首选，但用一条未验证的风险挡住：
**卷路径打开时 `device` 显示 `partition=\Device\HarddiskVolume2`，盘符方式显示 `partition=S:`，
若写操作也归一化，可能改写 ESP 的设备引用。**

本轮在用户提权会话里用一个 bat 把它测完了：

| 实验 | 结果 | 证据 |
|---|---|---|
| E3 读路径字节等价 | ✅ | 两条路径 `copy` 出的 BCD 副本 SHA-256 完全相同：`976951da628b944543c64adbc97fa410d4159470eff6b09833b95381a66de961` |
| E1 写路径归一化 | ✅ **不归一化** | 两条路径各 `set {bootmgr} default <当前值>` 后，`/enum` 输出 `fc /b` **逐字节无差异** |
| E2 四个写动词 | ✅ | `/set`、`/create`、`/delete` 均成功；`/deletevalue` 在元素存在时成功（本次例子取了个不存在的元素，报"数据类型无法识别"是我构造不当） |
| E4 卷路径枚举 | ✅ | `[System.IO.Directory]::GetFiles('\\?\Volume{GUID}\')` 在 SYSTEM 会话直接列出 ESP 全部文件 |

### 一度像风险信号的"写后哈希不同"

`set` 之后两条路径落盘的文件哈希不同（`6c1be26f…` vs `58354cbd…`）。`fc /b` 显示差异
**全部在偏移 `0x30` 起的固件描述/路径缓存区**：盘符路径会写回一段含
`S:\EFI\Microsoft\Boot\BCD` 的可读缓存，卷路径写一段不可读缓存。属显示层缓存，不是语义字段
——判据是同一时刻两条路径 `/enum` 输出逐字节一致。

### `device` 两种显示的真相

同一次运行里 `S:` 挂着时，**两种路径都显示 `partition=S:`**；`S:` 不挂时才显示
`partition=\Device\HarddiskVolume2`。原文档所谓"卷路径方式显示 HarddiskVolume2"只是
"当时恰好没挂盘符"的观察结果，**不代表卷路径会改写语义**。

**结论：方案 A 不再被阻塞，可以实施。**

## 二、开发日志为什么在 ESP 根目录（本次查清）

ESP 根原有 38 个 `.txt`/`.log`（约 93 KB，09-11~09-26）。逐个 grep 源码后确认：

- **产品代码写的**：`pe-drive.txt`、`diag1-4.txt`、`verify-bcd-enum.txt`、`bcdboot-*.txt`、
  `backup-out.txt` 等——全是 `native_gui.rs` 里 `run_cmd_to_file(...) > S:\xxx.txt` 形式的
  取证输出，散落在 PE 桌面各动作路径上。
- **我这一侧写的**：`bcd-all.txt`、`pe-enum.txt` 等探针输出。
- **必须留在根的**：只有 `pe-task.txt` / `.done` / `pe-task-result.txt` 三个——PE 启动最早期
  按固定路径读它们，是自动执行链的约定，改目录整条链断掉。

原因一句话：**当初图省事，所有 `mountvol S: /S` 之后的中间输出都直接甩在 `S:\` 根，
没有区分"控制通道"和"取证日志"，也没建子目录。** ESP 是引导分区、容量百 MB 级，
根目录还混着 `EFI\`，堆日志会拖慢固件枚举。

## 三、v1.8.1 改动

1. `text_parsing::esp_log_path(name)`：控制通道 3 个文件留在 `S:\` 根，其余全部落到
   `S:\BackupRestore\logs\`，子目录不存在则顺带 `create_dir_all`。
   47 处调用点改为走它。函数放 `text_parsing`（macOS 也编译）而非 `native_gui`
   （`#[cfg(windows)]`）——延续 v1.7.12 的教训。
2. 新增单测 `esp_log_path_keeps_control_channel_at_the_root`，锁住这个约定。
3. **清理 ESP 根既有的 38 个残留**：用 verbatim 卷路径 `\\?\Volume{GUID}\`
   `[System.IO.File]::Delete` 逐个删（`del` 不接受 verbatim 路径，而挂盘符又会触发自动播放，
   等于一边治一边造）。删完复核：ESP 根只剩 `pe-exit-guid.txt` + `EFI\` +
   `System Volume Information`，`BCD_OK=True`，注册位与 `{bootmgr}` 未受影响。

## 四、快照清理（用户授权"可以合理删除"）

7 → 2：删掉中间 5 个诊断快照（v1.7.12 / v1.7.13 / v1.7.14 / E1 / 主路径各一个）——
它们对应的修复都已提交推送、验证结论已写进文档，不再是有效回退点。保留：

- `{e9b6419a-7008-4ac3-8d3a-5bb7b68eb9f6}`：链根，无父、无记录文件，删它要重挂整条链，不动。
- `{113744a0-b197-470f-b9ac-330703e5084c}`：当前状态回退点。

宿主可用空间 152 GiB → 180 GiB。

## 五、下一步

方案 A 现在可以开工了，改动点见本文档方案 A 一节（`snapshot_raw_bcd`、
`rollback_boot_request`、`efi_identity`、GUI 两处、PE 三处）。做完预期
**一次完整准备任务的临时盘符挂载降到 0 次**，自动播放弹窗与"不可访问"两条路径一起消失。

# 进入 WinRE 备份 T:（5G 数据卷）端到端验证（2026-10-14 02:00，v2.4.7）

> 模型：Step-5-Preview｜触发：用户要求"进入 RE 备份 T:（5G 数据卷）"
> 任务：`e8d27926-b079-466a-950a-0bd31ede9ef2`｜镜像：`H:\BRImages\re-channel-t-backup.wim`

## 0. 结论先行

**成功，且全程无人工干预**：`prepare` 就绪后自动重启 → 一次性 bootsequence 被消费 → WinRE 内
`Recovery.exe` 用 DISM 捕获 T: → 清理 BCD → `wpeutil reboot` 回桌面。从 prepare 到回桌面约
**4 分钟**（DISM 注入 WinRE 约 3 分钟 + RE 内执行约 20 秒）。

| 验收项 | 结果 |
| --- | --- |
| 镜像产物 | `re-channel-t-backup.wim` **16,779,593 B**，sha256 `fc1bfb39…252c75` |
| 副档 | `re-channel-t-backup.wim.index-1.metadata.json`（1,051 B，version 3） |
| WIM 索引 | DISM 直查：`Index : 1 / Name : T-Drive-RE-Channel / Size : 16,777,397 bytes` |
| 捕获命令 | `/Capture-Image /CaptureDir:D:\ /Name:T-Drive-RE-Channel /Compress:fast /CheckIntegrity`（D:\ = RE 内 T:） |
| 任务终态 | `status.json`：`stage=success progress=100`；日志 `Boot entry cleaned; task marked successful` |
| BCD 现场恢复 | 与备份前基线逐字一致（见 §4） |
| 载荷暂存清理 | `H:\BackupRestoreRE\` 已空 |
| T: 源卷 | 完好（`BRRE213testdata` 四项俱在），备份只读源 |

## 1. 通道选择：为什么必须走 `prepare`（重要，避免再犯"悄悄用命令行"的错）

- **GUI 对数据卷只有在线路径**：`native_gui.rs:5077`「非当前活动系统（数据盘 / 未运行的
  第二系统）：在线直接执行，不重启」。界面没有任何"强制离线"开关；只有当源/目标是**当前
  活动系统盘**时才会弹 `ask_system_drive_handler` 让你选 PE/WinRE。
- **程序内进 RE 备份的唯一入口就是 `prepare` 离线通道**——这正是 GUI 备份 C: 时内部调的
  同一套（`windows_prepare::prepare`）。本次由 CLI 面触发，但**实际捕获执行者是 WinRE 里
  的 `Recovery.exe`**（`manifest.json` 记录 `recoverySha256=1b39a1e2…`，与部署二进制一致），
  不是助手手工跑 DISM。
- 复用命令（ASCII，`prlctl exec` 下直接可跑）：

```bat
H:\brwork\BackupRestore.exe prepare --operation backup --source-drive T ^
  --image-path H:\BRImages\re-channel-t-backup.wim --image-name T-Drive-RE-Channel
```

## 2. 完整时间线（UTC /  GMT+8）

| 时间 | 事件 | 证据 |
| --- | --- | --- |
| 17:56:53 / 01:56:53 | DISM 向 WinRE 注入完成（100%） | `prepare.log` |
| 17:56:54 | 载荷暂存到镜像卷 `H:\BackupRestoreRE\e8d27926…\Winre.wim`，sha256 `3cbe983a…`；`reagentc /info` 只读核对（Enabled，`harddisk0\partition4\Recovery\WindowsRE`） | `prepare.log` |
| 17:56:55 | BCD 条目建成：loader `{79348359-c72f-11f1-897a-bb640e02250f}`、devopts `{79348358-…}`，回读 `device=ramdisk=[H:]\BackupRestoreRE\e8d27926…\Winre.wim,{79348358-…}` | `prepare.log` + `boot-entry.json` |
| 17:57:02 | 一次性 bootsequence 武装；H:（载荷卷）与 S:（ESP/BCD）卷缓存强制落盘；`shutdown.exe /r /t 0` | `prepare.log` |
| 17:57:14 / 01:57:14 | WinRE 起来，wpeinit 完成 | `wpeinit.log` |
| 17:57:18 | 挂载三角：RECOVERY 经 C:（GUID `761230e8…`，disk0/part4）、SOURCE T: 经 **D:**（GUID `950bd694…`，disk0/part5）、IMAGE H: 经 **E:**（GUID `0e68475f…`，disk1/part2） | `Recovery-early.log` |
| 17:57:19 | DISM 捕获成功，1 秒，16,779,593 B；副档写入 | `H:\BRImages\Recovery.log` |
| 17:57:21 | boot entry 清理、任务标记成功、`wpeutil.exe reboot` | 同上 |
| ~17:57:40 | 回桌面（Windows），prlctl exec 恢复可用 | 实测 |

## 3. 证据文件位置

| 文件 | 内容 |
| --- | --- |
| `H:\brwork\tasks\e8d27926…\prepare.log` | 注入/暂存/建条目/武装/落盘/重启全序列 |
| `H:\brwork\tasks\e8d27926…\Recovery-early.log` | RE 内挂载（盘符重排再次印证：T:→D:、H:→E:） |
| `H:\brwork\tasks\e8d27926…\wpeinit.log` / `WinPE-init.log` | WinRE 初始化 |
| **`H:\BRImages\Recovery.log`** | **主日志（与镜像并排）**：DISM 捕获、副档、BCD 清理、`wpeutil reboot`。注意它是**追加式**，含 10-13 旧 run（16:28）记录 |
| `H:\brwork\tasks\e8d27926…\boot-entry.json` | loader/devopts GUID、载荷卷身份（disk1/part2，volumeGuid `0e68475f…`，序列号 8297CCB6）、载荷 sha256 |
| `H:\brwork\tasks\e8d27926…\manifest.json` | recovery/task/original/staged 四份 sha256 + `createdByVersion=2.4.7` |
| `H:\brwork\tasks\e8d27926…\status.json` | 终态 success/100 |
| `C:\BackupRestore-Recovery-early.log` | 历史累积日志（含 9 月旧任务），本 run 只追加了一行 TASK_ID 头；**排障时别把它当唯一来源**，主日志在镜像同目录 |

## 4. BCD 现场恢复核对（改引导类操作的回滚基线纪律）

备份前抄下的基线 vs 恢复后实测，**逐字一致**：

- `default = {12e3701c-c361-11f1-90a2-c222b5348ef7}` ✓
- `displayorder` 首项 `{152dcf4e-c674-11f1-895e-8d900a1bd081}` ✓
- `toolsdisplayorder = {b2721d73-1db4-4c62-bf78-c548a880142d}` ✓，`timeout 10` ✓
- 无 `bootsequence`（一次性请求已消费并清除）、无 `resumeobject` ✓
- 自建 ramdisk 条目 `{79348359…}`/`{79348358…}` 已随 boot entry 清理移除 ✓
- `H:\BackupRestoreRE\` 空（载荷删净）✓

## 5. 观察与备注

1. **副档显示名与 WIM 实际名不一致（cosmetic）**：副档/`wim-info` 的 `name` 字段是
   `Windows Backup (index 1)`，而 DISM 实际捕获名是 `T-Drive-RE-Channel`（`--image-name`
   确实生效，DISM `/Get-WimInfo` 直查为证）。界面列表显示用的是副档字段，不影响功能，
   若要对齐可查副档写入处取 DISM 回显名。
2. **三条备份通道产物大小对比**（同为 T: 源）：CLI/RE 16,779,583B（10-13）、GUI 在线
   16,779,577B（10-14 00:57）、本次 RE 16,779,593B——几十字节差异来自元数据/压缩块，
   内容等价（capturedUsedBytes 均 39,694,336）。
3. **本次未遇 10-12 的"载荷被外部删除"复发**：`prepare` 落盘到 reboot 之间载荷哈希
   三次一致（staged/readback/refresh），回桌面后暂存目录已清。
4. RE 窗口只有约 20 秒，宿主侧截图来不及也不必——日志证据链（wpeinit + mount + DISM +
   wpeutil reboot）比截图更强。

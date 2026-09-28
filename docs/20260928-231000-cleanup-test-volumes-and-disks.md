# 测试卷/旧备份清理 + 真实还原被 WinRE 防护拦截记录

时间：2026-09-28 23:10-23:30

## 一、清理内容（用户指令：删旧备份、删不用的测试虚拟卷）

| 对象 | 处置 | 释放 |
|---|---|---|
| Parallels 残留快照 ×3（v179-acceptance-pre-c-backup、pre-freedisksync-off/on-20260928） | 删除 | ~22GB |
| hdd1 = br.hdd（E: 测试镜像盘 262GB） | snapshot 删净后 `--device-del hdd1`（**必须停机**）+ 删孤儿文件 | 宿主 34GB |
| E:\BackupRestore、E:\BR-* 全部旧测试目录 | 随盘删除 | - |
| P:（BRSource，被还原测试写成完整 Windows 副本 + DoS 缓存 13GB） | 格式化 | 64GB |
| T:（BRTEST174 测试数据 data1-4.bin、petest） | 格式化 | 5GB |
| H:\BR-V173-FULL-E2E.wim.partial + H:\brwork\tasks 下 10 个旧任务目录（各含 0.7GB stage Winre.wim） | 删除 | ~10GB |

**保留**：`H:\brwork`（程序目录）、`F:\c-real.wim`（31.1GB 真实 C: 备份产物，还原验收必需）、任务 `c2a5ebe3`（成功备份记录）。
清理后 VM 内：E: 消失、盘符无重排（H/P/T/F 原样）、宿主可用 105GB→127GB。

## 二、br.hdd 删除的坑（复用必读）

1. `prlctl set --device-del hdd1` **运行中直接拒绝**；停机后仍失败 →
2. 真因：**磁盘被快照引用**（报错会列出快照名，但 `snapshot-list` 的名字列和 ID 列不对齐，按名字 grep 提取 ID 会拿到空值）→ 直接从 `snapshot-list` 抄 SNAPSHOT_ID，**从叶到根**逐个 `snapshot-delete`；
3. 快照清零后 `--device-del` 成功；br.hdd 在 `/Users/x/Parallels/br.hdd`（pvm 包外），需手动 `rm -rf`；
4. 删 E: 盘后 Windows 盘符未重排（F:/H:/P:/T: 保持原字母）。

## 三、真实还原（rb3）被产品防护拦截 —— 待设计

命令 `prepare --operation restore-existing --source-drive C --target-drive C --image-path F:\c-real.wim --allow-destructive` 返回：

> invalid task: **还原目标分区承载当前 Windows 恢复环境（WinRE）：格式化会删除 Winre.wim 及用于回滚的原件副本，系统将失去恢复环境且无法自动回滚。请先把 WinRE 迁出该分区，或选择其它还原目标。**

这是合法防护（C: partition4 注册着 WinRE，最坏 F3 场景）。真实 C: 还原需要「迁出→还原→重建注册」流程或显式豁免开关，下一步设计（结合已定型的注册 WinRE 状态机：还原后 `ensure_registered_is_payload`/reagentc 重建路径均已具备）。

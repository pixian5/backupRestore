# entry-fields 失败补偿实机验收（问题 11 收尾）

版本 **2.2.4**，开发测试版。日期：2026-10-11 02:10；模型：Opus 5（1M 上下文）。

本轮在实机运行 `boot_entry::fault_tests::missing_payload_conflict_and_cleanup_retry`
破坏性夹具（快照 `{b6641819-91c0-4bab-aeaf-4506ff3f0cb7}` 保护），**六段全部通过**：

```text
FAULT_PASS missing-payload: BCD unchanged; original retained
FAULT_PASS corrupt-payload: BCD unchanged; original retained
FAULT_PASS boot-request-conflict: foreign request preserved
FAULT_PASS cleanup-compensation-failure: retry succeeded; baseline BCD restored
FAULT_PASS entry-field-failure: residue recorded with exact GUIDs
FAULT_PASS entry-field-compensation: residue objects removed; baseline restored
test result: ok. 1 passed（161 秒）
```

终态核验：`bootsequence` 空、BCD 无 `BackupRestore task RE` 残留对象、staging 目录已清。

## 夹具暴露并修复的三个自身缺陷

该夹具 2.2.1 写成后**从未在实机真正跑通过**——`boot_entry` 是 `#[cfg(windows)]`，
本机不编译；此前各轮验收也没显式执行它。本轮首次实跑，连续暴露三个设计错误：

1. **复用分支短路注入点**：前段成功的 `create_entry` 在同目录留下 `boot-entry.json`，
   末段注入 `entry-fields:error` 时命中"复用已有条目"直接返回 Ok，注入点从未执行。
   修复：注入前删簿记与本任务 staging 载荷，强制走全新创建路径。

2. **断言语义写错两处**：
   - 先误以为"注入失败发生在建对象之前，不应留下条目"——实际注入点在 `/copy` 之后，
     KEEP 保留的对象**继承模板全部字段**（device 指向 C: 注册位）。"从未写成功"的正确
     证据是字段仍指向模板路径而非本任务载荷路径（`BackupRestoreRE\<任务ID>`）。
   - 再误用英文 `identifier` 判断对象存在——SYSTEM 通道下 bcdedit 输出**中文本地化**
     （"标识符"），断言必然失败。改为语言无关的 GUID 包含判断。

3. **结尾未清理 KEEP 对象**：清了 residue 簿记文件但没删那两个 BCD 诊断对象，终态
   `assert_eq!(baseline, ...)` 必失败。补上用与产品补偿同一 `remove_entry_objects`
   按精确 GUID 删除。

这三处都是**夹具**缺陷，产品代码（KEEP 分支、残留簿记、统一补偿）行为全部正确。

## 实机确认的产品行为

- `create_entry` 字段写入块注入错误后：4 次退避重试（2/6/15/30 秒，实机日志逐条可见），
  全部失败才 KEEP，并把两个 GUID 落入 `boot-entry-residue.json`（与任务 ID 绑定）；
- 半成品对象的 device/osdevice **从未被改写**成指向本任务载荷——机器不会误启动进它；
- 残留簿记可被补偿按精确 GUID 消费，删除后 BCD 逐字节回基线。

## 检查

本机 111+43=154 项、双侧严格 Clippy 0 告警、fmt 通过、ARM64 构建通过
（2,277,888 字节，SHA-256 前缀 `bef5ccd7`）。版本 2.2.4（仅版本号变更）。

证据：`.test-artifacts/re223-windows-20261011/ef14.out`、`ef14-residue.json`、`boot-fault.log`。

## 下一步剩余

ESP 多候选场景、新载荷路径真实断电矩阵重跑（cleanup/boot 两矩阵）、GUI 三路弹窗真实鼠标交互。

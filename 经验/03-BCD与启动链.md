# 03 BCD 与启动链

> 启动链是本项目最危险区域：改错机器起不来。所有条目前先看 `07-墓碑与争议清单.md`。

## 1. ramdisk 规格形态（历史最大误判，已定论）

- **真变量是 BCD 里 ramdisk 规格的写法形态，不是"卷"**：
  - 盘符形式 `ramdisk=[F:]\…\Winre.wim,{devopts}` 在数据卷被 bootmgr 解析失败——**静默丢弃一次性 bootsequence → 回落 default，不弹错、看着像没重启**；
  - 设备路径形式 `ramdisk=[\Device\HarddiskVolumeN]\…,{devopts}` 在系统卷与数据卷**都能引导**。
  —— 20261014-0003（单变量矩阵 A/B/C 定论）[✅]
- `bcdedit /enum` **把两种形态都规范化显示成 `ramdisk=[X:]`**——存储层差异回读不可见，这是历史上误判"根因是卷"的直接原因 —— 20261014-0003/2115 [✅]
- 代码落点：`boot_entry.rs` `dos_device_of()` 用 `QueryDosDeviceW` 转设备路径，**失败即拒绝写盘符形式**（不静默退化）；返回值必须以首个 NUL 截断（否则报 `nul byte found in provided data`）；写入点写 `ramdisk=[\Device\HarddiskVolumeN]<卷内路径>,{devopts}` + `ramdisksdidevice partition=\Device\HarddiskVolumeN`，回读期望值仍用盘符形式（恰好一致）—— 20261014-0003/2115 [✅ 源码]
- **载荷留在镜像卷**（`windows_prepare.rs` `re_staging.volume = image_volume`）；2.4.5「搬去 C:」已回退（污染 C:，且 `restore-existing` 格式化目标会抹掉自己的续跑载荷和 boot.sdi）—— 20261014-0003/2356 [✅]
- ramdisk 形态细节：`]` **紧跟卷闭合、结尾没有第二个 `]`**（`ramdisk=[F:]\path,{opts}`）；`{ramdiskoptions}`（`{ae5534e0-…}`）与真实 WinRE 条目用的设备选项对象不是一回事，且 `/enum all` 不列出它 —— 20260929-1300/20261011-2350 [✅/⚠️]
- RAMDISK osloader 权威参数：`ramdisksdidevice partition=X:` + `ramdisksdipath \目录\boot.sdi`（**必须 boot.sdi，误设 boot.wim 直接起不来**）+ `winpe yes` + `detecthal yes` + `systemroot \windows` + `nx OptIn`；`0xc0000225` 根因常是缺 `systemroot`/`nx` —— 20260825-2309/20260909-2145 [✅]

## 2. 盘符绑定（第二系统的灵魂）

- 每套 Windows 有自己的 `HKLM\SYSTEM\MountedDevices`；同一物理分区在两套系统里可叫不同字母，"两个都自认 C:"不冲突 —— 20261014-0945 [✅]
- GPT 盘 blob = 字符串 `DMIO:ID:` + **分区 GUID 小端 16 字节**（`to_bytes_le()`）；**MBR 盘（12 字节 blob）走不了这条链**——`gpt_mapping()` 只支持 GPT，`repair()` 会因 GUID 解析失败报错。与"UEFI+GPT"支持边界一致，但接手方要知道这是硬边界 —— 20261014-0945+README [✅]
- 判定"镜像自认哪个盘符"的权威依据 = **离线 SOFTWARE hive 的 `SystemRoot`**，不是物理位置 —— 20261014-0945 [✅]
- `secondary_registry::repair()` 四步：读 `SystemRoot` 得期望字母 → 算 `DMIO:ID:<GUID小端>` → 改离线 SYSTEM hive（删指向该分区的其他字母，写 `\DosDevices\C:`）→ 回读该分区恰好出现一次。改前备份 SYSTEM/SOFTWARE+事务日志族到 `config\BackupRestore-secondary-original\` 并验 sha256；**拒绝修改当前运行系统的离线配置** —— 20261014-0945 [✅]
- **离线路径在 bcdboot 前调用（main.rs），在线路径曾漏调**（2026-10-12 在线还原第二系统崩溃循环根因），修复已落地；两侧调用点都要有 —— 20261012-1435/1505 [✅]
- 不调 repair() 的后果：新系统 `C:` 仍指原物理分区 → LogonUI/dwm 每 3-5 秒交替崩溃 + 7 个 WerFault + KERNELBASE 0x8000ffff；权威判据 = `MsMpEng.exe` 从 `C:\ProgramData\...` 加载（在第二系统上却指着旧 C:) —— 20261012-1405/1425 [✅]
- 失效时的判真：离线 SYSTEM hive 的 `\DosDevices\C:` 值尾部 = `DMIO:ID:` + 目标分区 GUID（`444D494F3A49443A` = ASCII `DMIO:ID:`）—— 20261012-1505/20261015-0010 [✅]

## 3. 卷身份铁律

- 卷身份 = `disk_guid + partition_guid`；**盘符、卷序列号都不能当卷身份**（WinRE 内盘符会漂移重排：T:→D:、H:→E:；`format quick` 会把卷序列号改掉）—— 20261014-0945/20261013-0945 [✅]
- 卷 API 吃 `\\?\Volume{GUID}\`，设备 API 吃 `\\.\Volume{GUID}`，混用报 161/123 —— 多篇 [✅]
- 磁盘编号跨重启必变（`rebind_after_boot` 只重绑编号），格式化/应用/引导修复前仍严格核验本次绑定 —— 20261009-0620 [✅]

## 4. 在线态 BCD 备份手段（2026-10-15 实测定论，推翻旧记载）

| 手段 | SYSTEM（`prlctl exec` 默认身份） | 未提权用户（`--current-user`） |
| --- | --- | --- |
| `bcdedit /export <普通路径>` | ✅ **成功**（实测 651,264 字节完整件，`/store <文件> /enum {bootmgr}` 离线可读、可回导） | ❌ 拒绝访问（连 `bcdedit /enum {bootmgr} /v` 只读都被拒，且与目标路径 ACL 无关） |
| `copy` ESP 原文件 `S:\EFI\Microsoft\Boot\BCD` | ❌「另一个程序正在使用」（内核独占） | ❌ 拒绝访问 |

- **优先级：`/export` 首选（提权即可）→ 抄 `{bootmgr}` 的 default/displayorder/resumeobject 三行兜底（只能手抄、无完整性校验）→ ESP 原文件永远拷不走，别再试**——20261015-0708 [✅ 实测矩阵]
- 本项目常态就是 SYSTEM 通道（`prlctl exec`），**"在线态 /export 不可用"是旧记载**（当年用了未提权会话）——AGENTS.md 已改口径 [✅]

## 5. BCD 手术规则

- 三步：**先摘引用（`/deletevalue`）→ 再删对象（`bcdedit /delete {guid} /f`）→ 必要时 `/cleanup`**；删前 `/enum all /v` 全量存证；`/cleanup` 连带清从属对象（报"找不到指定文件"属正常）—— 20261014-2107/2345 [✅]
- **删 BCD 对象后等 60-90 秒再重启**——否则软重启因 ESP 写入未 flush 而"复活"（删完 /enum 干净、重启后对象回来）—— 20260929-2040 [✅]
- 清理判定=**"有引用才留"**：`tools/bcd-audit.ps1` 建被引用集合（recoverysequence/resumeobject/{bootmgr} 全文）只删无人引用条目；**MEMORY 点名保护的休眠恢复入口（`{f53cce28}` 类）与 `[unknown]`/`[F:]`/`[Y:]` WinRE 条目禁止按"看着多了"清理**——20261014-2345/20261012-1520 [✅]
- 最贵误判复盘：凭 GUID 时间戳断言正常对象是"历史残留"建议删除，实为主系统休眠恢复入口（`resumeloadersettings`+hiberfil.sys）——看见局部证据就下结论的典型 —— 20261012-1520 [✅]
- 知名别名白名单只放 `{bootmgr}` 一个；`{default}`/`{current}` 是**别名**（指向当前/默认项），执行后可能已重新绑定，**必须核对具体 GUID** —— 20260929-2040/20260927-2015 [✅]
- 本地化字段名必须同时识别：`device`/`设备`、`os 设备`（带空格）、`path`/`路径`；loader 判据 **OR 语义 + 显式排除 ramdiskdevice**——猜错一个标签只少一项证据，AND 会让中文机器全线失效 —— 20261012-1950/1510 [✅]
- **`bcdedit /enum` 枚举已删除对象返回退出码 0** + 本地化空文案（"没有匹配的对象…"）→ 唯一稳的判据是**结构性判据：全文没有任何 `{GUID}` 即对象不存在=被取代**；别再靠退出码/文案 —— 20261013-0152 [✅]
- 取证别用关键字过滤 bcdedit 输出：`displayorder` 的续行只有缩进+GUID、无字段名，一过滤就像"只剩一项" —— 20261014-2030 [✅]
- **绝不对真实 ESP 跑 `bcdboot X:\Windows /s S: /f UEFI`**（会改写 default/displayorder，VM 下次直接进那个卷）；PoC 只在"假 ESP"上做 —— 20260927-2015 §9.4（事故实据）[✅]
- bcdboot 会把 Boot Manager 的 default 改指目标卷；`set default {default}` 等于没恢复。修法：bcdboot 前记录原 default 的 `partition=X:`，bcdboot 后全枚举按 partition 找回真实 GUID —— 20260911-0102 §10.3 [✅]
- 本项目所有 bcdboot 都带 `/s <ESP>` → **`/d` 和 `/addlast` 可能被静默忽略**（BFSVC Error: options ignored when custom volume specified）——`/d` 不能当承重防线 —— 20261013-1222 [✅]

## 6. default 规则（用户 2026-10-15 拍板）+ v2.5.8 落地

- 用户三条规则：①**还原本系统**（目标=当前系统分区）→ default 必须指向还原后的自己（不管镜像原本备份自哪个分区）；②**新增第二系统** → default 保持原主系统、新项 `/addlast` 追加菜单末尾；③**禁止**「本次还原的 Windows」到哪个分区都一律设默认 —— 20261015-0725 [✅ 用户拍板]
- 规则①的路由判据 = 目标分区 vs 活动系统分区的**同一性**（`windows_prepare.rs` 落 `is_active_system`；RE 内 `%SystemDrive%` 是 RAM 盘 X: 探测不到，故离线侧只读该结论），与镜像来源分区无关 —— 20261015-0725 [✅]
- v2.5.8 已落地：`BootMenuPlan.default_policy: DefaultPolicy`（PointAtRestoreTarget/PreserveCurrentDefault）+ `set_default_policy` 唯一写入入口 + apply 唯一决策点 + 全分支日志；在线通道结构上固定 PreserveCurrentDefault（活动系统盘被顶部拒绝）—— 20261015-0725/commit 4b64c94 [✅]
- A 场景 bcdboot 不带 `/d`；C 场景四道防线保 default：bcdboot `/d /addlast` + `assert_boot_default_preserved`（bcdboot 后立即回读+自愈）+ `preserve_primary_boot_manager` + apply 的 `selected.contains(&previous.default)` —— 20261015-0725 [✅]
- S3/S5 待用户裁定：previous.default 不在 selected 时应报错拒绝而非 WARN；RestoreExisting 到非活动 Windows 卷且用户未起名时菜单项硬编码「本次还原的 Windows」（应读 WIM 索引名预填+撞名追加时间戳）—— 20261015-0725 [⚠️ 未实施]

## 7. 两个真实 bug 史（错误签名对照，防再归错因）

| 任务/轮次 | 失败错误串 | 真因 | 修复 |
| --- | --- | --- | --- |
| 阶段 3 `95ab0a7f`（10-14 19:00） | `Boot Manager default or secondary display order could not be preserved` | **resumeobject 不对称**：快照无该字段而 BCDBoot 为新系统凭空建一个（与新 loader 是兄弟 GUID），旧代码"有才写回、从无删除分支"，三项联合校验把 (live=Some, 快照=None) 判失配 → BootRepaired 确定性失败、重试必然同样失败 | v2.5.5 补 `/deletevalue {bootmgr} resumeobject` 分支 |
| 任务 `0fa52c1d`（10-14 14:26） | `invalid task: bcdedit.exe failed with exit code: 1` | **cleanup 容错缺失**：`cleanup_orphaned_secondary_resume_objects` 删已不存在的孤儿 resume object 得 exit 1，被 `?` 当致命错误 → 实质全成功的任务判失败 → 保留 boot entry 与载荷 → 下次启动 `0xc000000e` | v2.5.7 改 match + 日志继续，对齐 `boot_entry.rs::remove_entry_objects` 的"删除容错+枚举回读" |

- **通用语义**：`bcdedit /delete`、`/deletevalue`、`Remove-Item` 的"让它不存在"类操作失败要反过来看——目标已不存在 = 目的达成 —— 20261014-2315 [✅]
- `--skip-format` 解析分支曾漏接（GUI/usage/PrepareOptions 全接了，唯独参数解析 match 缺一支，兜底是硬报错 `unknown prepare option`；`unused_mut` 警告就是指纹）—— v2.5.5 补 —— 20261014-2030 [✅]
- `resume` 复用旧载荷陷阱：resume 只 rearm 一次性启动项 + shutdown，**复用 prepare 时暂存的 WIM（内含旧 Recovery.exe）** → 改 RE 侧代码后必须**全新 prepare** 才能验证；`build-win.sh --deploy` 只刷运行目录不动已暂存载荷 —— 20261014-2105/2107 [✅]

## 8. 绝对禁止清单（踩错机器起不来）

- 对真实 ESP 跑 `bcdboot … /s S:`；复用/改写 ReAgent **已登记**的 BCD 对象（无 /boottore 时 8 秒内被 bootmgr 丢弃）；`ramdisk=` 值拼错（`]` 位置）；载荷注入完成**前**武装 bootsequence（`--no-reboot` 也会改 bootmgr）；先删 BCD 对象再清 bootsequence；`bcdedit /delete` 后立即重启；夺走不属于本任务的 bootsequence；在 WinRE **入口**立即翻干注册位（与断电续跑冲突）—— 20261011-2350 汇编 [⚠️ 单一汇编来源，逐条有多源]

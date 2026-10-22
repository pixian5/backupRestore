# wimlib --no-acls 覆盖式还原通道集成（任务 #110，v2.6.25）

> **2026-10-22 20:30 更新**：本文的实机验收已由 `docs/20261022-2030-wimlib覆盖式还原e2e实机通过-Step-5-Preview.md` 完成——六条判据全过 + 三项判据零新增 + 环境已恢复，任务 #110 收口。两处口径以 2030 文档为准：实机部署走虚拟机内 `build-windows.ps1 -Deploy`（非 macOS 侧 `build-win.sh --deploy`，后者部署交叉编译产物）；覆盖式判据是「哨兵**存活** + 序列号不变 + 镜像内覆写」（非格式化路径的「哨兵消失」）。

- 时间：2026-10-22 17:19
- 模型：Step-5-Preview
- 依据：用户 2026-10-22 针对「覆盖式应用（取消格式化）打到已含 Windows 的卷被 DISM 拒绝」明确指示**「换 wimlib --no-acls」**；前置概念验证与对照实验见 `docs/20261022-1427-wimlib-no-acls覆盖式还原概念验证与DISM对照-Step-5-Preview.md`，任务背景与项目铁律见 `docs/20261022-1433-接手AI交接-wimlib覆盖式还原集成与项目铁律-Hy3.md`。
- 结论：**代码集成完成，构建与单测全绿；实机验收（小规模 F: → 恢复环境内端到端 → 三项判据）待跑**，清单见文末 §7。

---

## 1. 这次解决了什么

**能力缺口**：DISM `/Apply-Image` 做「覆盖式应用」（`--skip-format`，不格式化直接把镜像铺到已含 Windows 的卷）时，要还原 `Program Files\WindowsApps\...`（微软商店应用包目录，属主=`NT SERVICE\TrustedInstaller`、带微软商店应用专用访问控制列表）的**目录安全描述符/属主**；系统拒绝对这类目录改写元数据 → `RestoreDirMetadata 0x80070005` → DISM 整体放弃（退出码 5，事务性，不留半成品）。v2.6.7 起的对策是在 prepare/online/图形用户界面三处**硬拒**——安全但也是能力缺口（用户想覆盖重灌第二系统/坏 Windows 卷而不先格式化时会丢该卷其他数据）。

**方案**：集成 wimlib `--no-acls` 通道。两个决定性性质（概念验证 E3/E4/E5 实证）：
1. `--no-acls` 整个跳过安全描述符还原 → 对 RestoreDirMetadata 失败**结构性免疫**；
2. wimlib 自行获取 SeRestore/SeBackup 提权（禁用提权的父进程下仍退出码 0，同父进程 icacls 子进程被拒退出码 5）→ **恢复环境/预安装环境上下文同样可用**，而这正是 DISM 会败的环境。

**代价**（图形用户界面确认框明示）：镜像里的目录权限（访问控制列表/属主）不还原，微软商店应用权限可能异常。

---

## 2. 代码改动全清单（11 个文件 + 2 个新入库二进制）

| # | 文件 | 改动 |
| --- | --- | --- |
| 1 | `crates/backuprestore-core/src/lib.rs` | `TargetSpec` 新增 `overlay_no_acl: bool`（`#[serde(default)]` 兜底 false=旧路径），含路由语义中文注释；6 处测试字面量同步补字段 |
| 2 | `crates/backuprestore-cli/src/wimlib_apply.rs` | **新模块**：`wimlib_args`（纯函数，锁死 `apply <wim> <index> <dir> --no-acls` 形态）、`locate_wimlib`（executable_dir 定位，缺失给可操作报错、**绝不静默回退 DISM**）、`apply_no_acl`（cfg(windows)，经 `windows_command::run_program` 统一执行 + `dism_timeout::wait_ms()` 超时）；3 个单测 |
| 3 | `crates/backuprestore-cli/src/main.rs` | ① mod 声明；② **恢复环境任务链 apply（原 3677 行）按 `task.target.overlay_no_acl` 路由**——这是恢复环境上下文 DISM 退出码 5 的真实路径（A 入口主链），交接文档 §3.2 未列出，本次补上 |
| 4 | `crates/backuprestore-cli/src/windows_prepare.rs` | `validate_operation_inputs` 返回 `Option<ImageAcceptance>` → 新 struct `ValidationOutcome { acceptance, overlay_no_acl }`；命中「已含 Windows + 取消格式化」**不再 return Err**，置标记 + 日志；`TargetSpec` 落盘 `overlay_no_acl`；Probe/Backup 分支恒 false |
| 5 | `crates/backuprestore-cli/src/online_operation.rs` | 在线闸门（原 508）命中不再 Err（记日志）；`restore_to_windows_volume` 的 apply（原 690）按 overlay 路由；STEP 3/4 日志文案按通道区分。其余 dism 调用（capture/delete/单测）不涉路由 |
| 6 | `crates/backuprestore-cli/src/pe_operation.rs` | `Request` 新增 `overlay_no_acl`（`#[serde(default)]`）；`new_request` 加参数；`apply()` 按 `self.request.overlay_no_acl` 路由；5 处单测同步 |
| 7 | `crates/backuprestore-cli/src/native_gui.rs` | 图形用户界面预检（原 6793）**MB_OK 拦截 → MB_YESNO 确认框**（中英双语自包含文案：明示目标含 Windows + wimlib 通道 + 「目录权限不还原、微软商店应用权限可能异常」代价）；IDYES 放行并记图形用户界面日志，否则维持拦截。两处预安装环境入口 `new_request` 传 false（预安装环境三入口恒格式化，零行为变化） |
| 8 | `crates/backuprestore-cli/src/winre_payload.rs` | `OPTIONAL_RUNTIME_FILES` 增加 `wimlib-imagex.exe` + `libwim-15.dll`——复用现成 `copy_required` + sha256 校验注入链进恢复环境的 `Windows\System32`（恢复环境内 current_exe 就在那里，`locate_wimlib` 按 executable_dir 找即得） |
| 9 | `windows/wimlib-aarch64/` | **新入库**：`wimlib-imagex.exe`（125,440 B，sha256 `fddb8c21…`）+ `libwim-15.dll`（432,640 B，sha256 `b34549c6…`），取自概念验证基线 `.test-artifacts/recon-38-20261022/wimlib-aarch64/`（wimlib 1.14.5 EXPERIMENTAL 官方 Windows ARM64 预编译） |
| 10 | `.gitignore` | 「保留」段加两条 `!` 反否定（`*.exe`/`*.dll` 宽通配之后），否则这两个交付必需二进制被静默忽略 |
| 11 | `build-win.sh` | `--deploy` 段拷贝两文件到包目录与 `H:\brwork`；SHA 校验循环改「名称:期望:实际」三元组，wimlib 四副本（两目录 × 两文件）逐一比对仓库基线哈希；DEPLOYED 行带 wimlib 哈希 |

**许可合规**（已读 COPYING 核实）：wimlib 整体 GPLv3+，libwim 库部分可选 LGPLv2.1+（Windows 构建不链接 libntfs-3g）。以**独立进程**调用官方预编译二进制、不链接库 = 合法聚合，不传染产品代码。

---

## 3. 四个 apply 点的路由总表（关键：交接文档只列了三处，漏了恢复环境主链）

| apply 执行点 | 触发场景 | 路由来源 | 本版状态 |
| --- | --- | --- | --- |
| `main.rs` 恢复环境任务链 apply（原 3677） | A/B/C 入口走 prepare 的任务（`--skip-format` 落盘 format_target=false） | `task.target.overlay_no_acl`（prepare 判定落盘） | ✅ 已路由 |
| `online_operation.rs` apply（原 690） | 在线 create-secondary/restore 到非活动 Windows 卷 + 取消格式化 | 同函数内 `overlay_no_acl` 局部量（闸门处判定） | ✅ 已路由 |
| `pe_operation.rs` apply（原 672） | 预安装环境入口 `data` 操作（DataOverlay 跳过格式化）打到含 Windows 卷 | `Request.overlay_no_acl`（现恒 false——预安装环境三入口无 data 用户路径，防御性预留） | ✅ 结构就绪 |
| 数据卷在线覆盖（online :415） | B 入口数据镜像到数据卷 | 不涉及（目标无 Windows，overlay 判据不命中；DISM 覆盖数据卷 2026-10-14 已在 T: 验证） | 不变 |

---

## 4. 实现期发现与决策（相对交接文档的修正/补充）

1. **交接文档 §3.1 的引号坑在 `run_program` 通道下不存在**：概念验证在虚拟机里 cmd 直跑遇到 `"F:\"` 吞 `--no-acls`，但 `windows_command::quote` 已实现标准 MSVCRT 转义（`quote("C:\\")` → `"\"C:\\\\\""`，单测 `quoted_arguments_keep_trailing_slash` 锁定）——**无需对目标路径特殊处理**，照常传参即可。
2. **恢复环境链 apply 是第四个点**（见 §3 表）：交接文档 §3.2 只列 prepare/online/pe_operation 三处闸门与 apply，漏掉 `main.rs:3677`。若只改它列的三处，A 入口（还原系统到 C:）prepare 放行后会在恢复环境里撞 DISM 退出码 5。已补。
3. **预安装环境入口（pe_operation）恒格式化**：`run_restore` 只对 `DataOverlay` 跳过 format，而 data 操作的请求只来自单测与在线 B 入口（在线链自己路由）；预安装环境桌面三入口（backup/restore/secondary）无 data。故 `Request.overlay_no_acl` 现有调用点全传 false，结构预留。
4. **图形用户界面确认框不复用 `overlay_restore_on_windows_volume_error` 的旧文案**：该文案结尾是硬拒时代的操作引导（「请勾选格式化目标分区」），与确认框语义冲突；新文案只图形用户界面一处，无分叉。纯函数保留作「是否命中组合」的判定（`is_some()`）。
5. **wimlib 超时与 DISM 同源**：`dism_timeout::wait_ms()`（默认不限制），wimlib apply 71GiB 实测约 22 分钟，同属长任务。
6. `err()` 签名坑（本机）：命令行  crate 的 `err` 期望 `&str`，`err(format!(...))` 编译不过——三处新代码都用 `err(&format!(...))`。

---

## 5. 验证状态（截至 17:19）

| 项 | 结果 |
| --- | --- |
| macOS 单测 `cargo test --workspace` | **255 通过 / 0 失败**（cli 211 + core 44；新增 wimlib_apply 3 个：args 形态、缺失报错可操作性、存在即接受） |
| macOS 交叉编译 `./build-win.sh` | **成功**，产物 2,869,248 B（较 2.6.24 的 2,862,080 B +7,168 B，wimlib_apply 模块 + 路由逻辑 + 文案） |
| 二进制内含新代码核验 | `strings` 命中 13 处：`wimlib-imagex.exe`/`libwim-15.dll`（OPTIONAL_RUNTIME_FILES 串块）、`--no-acls`、图形用户界面确认框中英文案、路由日志串 |
| 产物 SHA-256 | `1423beba30256b4371669bae2640538a1f1645856be640ed7d4e67f0bd1a091d` |
| LNK4099 PDB 警告 | 既有（libcmt/libvcruntime 的 PDB 缺失），非本次引入 |

---

## 6. 实机验收计划（未跑，按 §4.3 测试环境分级纪律）

前置：`./build-win.sh --deploy`（会 taskkill 旧进程、拷贝 8 个文件、SHA 八联比对）；部署后按纪律以 SHA 确认虚拟机内 exe = 本机产物。

1. **小规模（F:，350GB 可牺牲卷，当前序列号 D47D8E54，概念验证后是完整 Windows 树）**：
   - `prepare create-secondary --source-drive C --target-drive F --image-path H:\BRImages\br-2619-noRE.wim --wim-index 1 --boot-menu-name WIMLIB-E2E --allow-destructive --skip-format --no-reboot`（SYSTEM 通道；noRE 镜像 = 只删 `reagentc.exe` 的副本，10-22 已构造验证 ABSENT）
   - 期望：prepare **成功**（命中 overlay 路由不再硬拒），`task.json` 里 `overlay_no_acl=true`、`format_target=false`
   - `resume` 进恢复环境 → `wimlib-imagex apply … --no-acls` 跑完 → 任务 success
   - 验证：哨兵文件消失 + F: 序列号不变（D47D8E54）+ F: 有完整 Windows 树 + `reagentc.exe` 仍缺席（--no-acls 不还原安全描述符，但文件数据照写；缺件镜像本来就没有它）+ 事件标识符 1000/WerFault 零新增
   - ⚠️ 若修 `/ImageFile:`→`/WimFile:` 门禁缺陷（`wim_mount_markers` 参数错，v2.6.25 未修），noRE 镜像将在准备期镜像完整性预检被硬拒——**两者互斥，先完成本端到端再修门禁**（已记入下一步待实现）。
2. **恢复环境内端到端（真实场景）**：对含原生微软商店应用保护目录的卷跑 --no-acls 全链（概念验证只在完整 Windows 里跑过 wimlib；恢复环境内只验证了自带提权性质）。F: 小规模跑通后，用 C: 源镜像 + F: 目标重复一次即可覆盖。
3. **三项判据验收**：哨兵法 + 卷序列号 + 事件标识符 1000 / WerFault 零新增（判据口径见 `经验/05` §7；`@($null).Count=1` 是 Windows 错误报告假阳性，先判 `-is [EventLogRecord]`）。

---

## 7. 与既有文档的关系

- 概念验证五实验 + F1/F2/F3/F4 发现：`docs/20261022-1427-…`（不变，本文不重复其内容）
- 任务背景 + 项目铁律 + 虚拟机操作手册：`docs/20261022-1433-接手AI交接-…-Hy3.md`（不变）
- 本文 = **集成实施的完整记录**（改了什么、为什么这么改、验证到哪、剩什么）
- 下一步待实现（实机验收 + 门禁缺陷修复 + 用户拍板项）：`docs/20261022-1719-下一步待实现-Step-5-Preview.md`

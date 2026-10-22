# wimlib --no-acls 覆盖式还原概念验证与 DISM 对照实验记录

- 时间：2026-10-22 13:20–14:30
- 模型：Step-5-Preview
- 背景：用户针对「覆盖式应用（取消格式化）打到已含 Windows 的卷被 DISM 拒绝（RestoreDirMetadata 0x80070005）」提问后，指示 **「换 wimlib --no-acls」**。本文记录落地前的对照概念验证。
- 结论先行：**wimlib --no-acls 完整达成了目标能力**（可向 SYSTEM 拒绝的目录写入、不还原任何安全描述符、71GiB 全量覆盖应用退出码 0）；同时实验推翻了两个此前的想当然认知（详见发现 F1/F2/F3）。

---

## 1. 结论摘要

| # | 实验 | 结果 | 意义 |
| --- | --- | --- | --- |
| E1 | 预置「合成限制性目录」+ icacls 金丝雀 | 金丝雀退出码 5（SYSTEM 改属主被拒） | 复现条件成立 |
| E2 | DISM /Apply-Image 覆盖应用（完整 Windows、特权禁用父进程） | **100% 成功**（意外） | 完整 Windows 里 wimgapi 自带提权，合成访问控制列表挡不住它 |
| E3 | wimlib-imagex apply --no-acls 全量覆盖应用（71GiB / 634,091 条目） | **退出码 0、100%、序列号不变** | 核心能力成立 |
| E4 | 小镜像机制测试：写入限制性目录 + 安全描述符不动 | 退出码 0；文件逐字节落盘；安全描述符原样恢复 | 「写入」与「不碰安全描述符」两点直接证明 |
| E5 | wimlib 在**禁用提权的父进程**下运行 | 退出码 0（同父进程的 icacls 子进程被拒退出码 5） | wimlib 自带 SeRestore/SeBackup 提权——恢复环境/预安装环境路径的关键性质 |

## 2. 实验设计

- **牺牲卷**：F:（350GB，#38 后可牺牲卷；实验前含 #38 ⑤ 的预安装环境硬盘安装内容，覆盖前已无其他用途）。
- **镜像**：`H:\BRImages\br-38sys-20261022.wim`（29.76GB，#38 ②a 捕获，索引名「Windows 11 26200.9550 20261022」）。
- **合成限制性目录**：在 `F:\Program Files\WindowsApps\` 下按**镜像里真实存在的路径**建 4 个目录（`Microsoft.DesktopAppInstaller_1.29.289.0_arm64__8wekyb3d8bbwe` 等，名字取自 10-16 实验的精确失败路径），安全描述符合成：属主=`NT SERVICE\TrustedInstaller`、DACL 仅 TrustedInstaller(FA)+RESTRICTED(RX)、**无任何 SYSTEM ACE**、protected。
- **特权禁用上下文**：PowerShell 内 P/Invoke `AdjustTokenPrivileges` 禁用 SeRestorePrivilege/SeTakeOwnershipPrivilege，DISM/wimlib 以 `Start-Process` 子进程启动（子进程继承禁用态）。金丝雀 = 同父进程的 `icacls /setowner` 必须退出码≠0。
- **wimlib 版本**：1.14.5 官方 Windows ARM64 二进制（`wimlib-imagex.exe` + `libwim-15.dll`，EXPERIMENTAL 标记），与本项目目标平台 aarch64-pc-windows-msvc 一致。

## 3. 五个实验详情

### E1 预置与门禁（13:46）

- 4 个目录安全描述符克隆成功（属主=TrustedInstaller、protected=True）。
- icacls 金丝雀：`exit code = 5`（拒绝访问）→ 复现条件成立，放行后续步骤。

### E2 DISM 对照：**意外成功**（13:46–14:01，约 15 分钟）

- 启动前证据：whoami 子进程与 icacls 子进程均证实禁用态（icacls 退出码 5）。
- 结果：**DISM 退出码 = 0，100%，「操作成功完成」**。
- 事后取证（`poc-sdstate-20261022.txt`）：4 个合成目录的安全描述符 **全部被 DISM 改写为镜像里的安全描述符**（与 C: 现有安全描述符逐字节一致，属主变回 SYSTEM）→ wimgapi 在完整 Windows 里**自行提权**，合成访问控制列表完全没挡住它。
- 附带证据：F: 序列号 D47D8E54 不变（未格式化），写入约 70GB。

### E3 wimlib --no-acls 全量覆盖应用（14:01–14:23，约 22 分钟）

- `wimlib-imagex apply br-38sys-20261022.wim 1 F:\ --no-acls` → **退出码 0**，`Extracting file data: 71 of 71 GiB (100%)`，`Done applying WIM image.`
- 验证（`poc-verify-20261022.txt`）：F: 文件数 515,205（与镜像 634,091 条目中文件部分吻合，差额为目录）；`ntoskrnl.exe`(11,612,624B)/`SYSTEM`(13,107,200B)/`Explorer.exe` 等关键文件均在；卷序列号 D47D8E54 不变。

### E4 小镜像机制测试（14:07–14:08）：写入 + 安全描述符不动，双点直接证明

- 用 wimlib 自 capture 一个 36 字节小镜像（`Program Files\WindowsApps\PocTestPkg\{file.txt, sub\nested.txt}`），预置 `F:\...\PocTestPkg` 为限制性目录（金丝雀 icacls 退出码 5 确认 SYSTEM 被拒）。
- `apply --no-acls` → 退出码 0。最终铁证（`poc-final-proof-20261022.txt`，临时授权读取后原样恢复 SDDL）：
  - `PocTestPkg\file.txt` len=19 content=`hello-from-image`
  - `PocTestPkg\sub\nested.txt` len=17 content=`nested-content`
  - 目录安全描述符恢复后 `sddlMatch=True`（属主=TrustedInstaller、protected）→ **--no-acls 确实一个字节的安全描述符都没还原**。

### E5 禁用特权父进程下的 wimlib（14:10）

- 同一脚本内：禁用两个提权 → icacls 子进程退出码 5（被拒）→ `Start-Process` 起 wimlib apply --no-acls → **退出码 0，36/36 字节提取完成**。
- 结论：**wimlib 自行获取 SeRestore/SeBackup 提权**，不依赖启动上下文。这是恢复环境/预安装环境路径的决定性性质。

## 4. 关键发现（改写此前的认知）

### F1：10-16 DISM 失败需要**两个条件同时成立**，缺一不可

重读 `docs/20261016-1413` §2.2 原始记录：同一预安装环境、同一镜像、同一 DISM 版本下，**C:（原生 Setup 安装的微软商店应用保护目录）秒退退出码 5、F:（DISM 洗过的目录）100% 成功**——变量只剩目标卷。结合本次 E2：完整 Windows 里连合成限制性访问控制列表都能被 wimgapi 提权改写。所以：

- 失败条件 = **预安装环境/恢复环境上下文**（wimgapi 的提权在该环境不生效）+ **原生 Setup 的微软商店应用专用访问控制列表**（属主=TrustedInstaller + app-package SID）。
- 推论：**在线路径（完整 Windows）的优先级 2 闸门可能过严**——在线态 wimgapi 有提权，覆盖式应用到非活动 Windows 卷应当能过（尚需独立实机验证，勿凭本条直接拆闸门）。

### F2：本虚拟机的原生微软商店应用保护已被「镜像回滚效应」洗掉

当前 C: 的 186 个 WindowsApps 直接子目录**全部**是属主=SYSTEM/Administrators + SYSTEM Full Control（`poc-wa-children-20261022.txt` 全量清单）。原因：C: 历经 10-19 C 入口、10-22 V1、#38 ④ 多次 DISM 还原，每次 format+apply 都把这些目录**新建**为 SYSTEM 所有——原生 Setup 的 TrustedInstaller 保护在一次还原后即永久消失。**这是「镜像回滚效应」的一个新副作用，此前未记录。**

### F3：--no-acls 对 RestoreDirMetadata 失败结构性免疫

0x80070005 发生在「还原目录安全描述符/属主」这一步；`--no-acls` 整个跳过安全描述符还原 → 该失败模式在代码路径上不可能发生。E4/E5 补齐了其余风险点（写入受限目录靠 wimlib 自带提权解决）。

### F4：工具层坑（5 条，均非产品缺陷）

1. **cmd 的 `"F:\"` 反斜杠转义引号**：`"F:\" --no-acls` 会把 `--no-acls` 吞进目标参数（报 `Can't open or create directory "F:" --no-acls "`）。修正：目标无空格时不要加引号。
2. **PS 5.1 无 BOM 脚本按 GBK 解析**：UTF-8 无 BOM 的 .ps1 里中文字面量会变乱码，`-match "拒绝访问"` 永远不匹配。修正：脚本内判据一律用退出码等语言无关量。
3. **AdjustTokenPrivileges 结构体 Pack=1 错位**：`TOKEN_PRIVILEGES` 需自然对齐（int+pad+long+int=24B），Pack=1 会让原生侧读到垃圾 LUID，「禁用」假成功。
4. **whoami /priv 的「状态」列不可信**：同一会话里它显示「已启用」而子进程 icacls 行为证明确被禁用。判权限只用行为探针（icacls 退出码）。
5. **同进程 .NET Set-Acl 与子进程行为不一致**：禁用后进程内 Set-Acl 仍成功、子进程 icacls 被拒——提权类自测必须用子进程探针。

## 5. 产品集成方案（待实施，任务 #110）

1. **新增 `wimlib_apply` 封装**：定位 `wimlib-imagex.exe`（在线态=程序同目录；恢复环境态=System32），经 `windows_command::run_program` 统一执行+超时+日志；缺失时明确报错（不得静默回退 DISM）。
2. **三处闸门从「硬拒」改「路由」**：
   - `windows_prepare.rs:2556`（离线/prepare）：不再返回 Err，改为在任务计划里记录 `overlay_no_acl=true`；
   - `online_operation.rs:508`（在线）：直接走 wimlib apply；
   - `native_gui.rs:6793`（图形用户界面）：改为确认框（明示「目录权限将不还原，微软商店应用权限可能异常」）后放行。
3. **恢复环境载荷**：`winre_payload.rs` 的 `OPTIONAL_RUNTIME_FILES` 增加 `wimlib-imagex.exe`+`libwim-15.dll`（复用现有 sha256 校验注入链），部署脚本 `build-win.sh --deploy` 同步拷贝到包目录与 H:\brwork。
4. **许可证事实**（已核实 COPYING）：wimlib 整体 GPLv3+，**libwim 库部分可选 LGPLv2.1+（Windows 构建不链接 libntfs-3g，可选 LGPL）**；以独立进程调用官方二进制属合法聚合，不传染产品代码。
5. **验收**：小规模（测试卷）→ 恢复环境内端到端（预安装环境里对含微软商店应用目录的卷跑 --no-acls 全链）→ 哨兵+序列号+事件标识符/WerFault 三项判据。

## 6. 环境终态与证据清单

- F:=`D47D8E54`（概念验证后为「DISM 应用 + wimlib --no-acls 再应用」的完整 Windows 树 + 旧测试残留，可随时再格式化）；C:=`BCD4EAAB` 未受影响；H: 工作卷不变。
- 证据（`.test-artifacts/recon-38-20261022/`）：`poc-prep-20261022.txt`、`poc-dism-20261022.txt`、`poc-verify-20261022.txt`（80MB 含 wimdir 全清单）、`poc-wa-children-20261022.txt`（186 目录安全描述符清单）、`poc-sdstate-20261022.txt`、`poc-tiny-prep/verify-20261022.txt`、`poc-wimlib-disabled-20261022.txt`、`poc-final-proof-20261022.txt`、`poc-run.log`/`poc-tiny.log`（虚拟机内）+ 全部脚本（poc-prep/poc-dism/poc-verify/poc-tiny*/poc-sdstate/poc-wimlib-disabled/poc-final-proof.ps1、poc-run.cmd、poc-tiny.cmd）+ `wimlib-aarch64/`（官方二进制解包）。

# BackupRestore 当前执行基线（v1.6.6）

更新时间：2026-09-16。此文优先于按日期保存的历史进度和 PoC 记录。

## 当前产品边界

- 正常 Windows GUI、任务准备和 WinRE 恢复入口均为 Rust；PowerShell 只用于 ADK
  构建和开发测试，绝不作为产品备份、还原或 WinRE 启动链。
- 程序所在目录就是工作目录。任务、载荷、状态、日志和 BCD 快照都保存在
  `<程序目录>\tasks` 或 `<程序目录>\logs`，不依赖 `C:\ProgramData\BackupRestore`，
  没有用户可见的“任务卷”或 `TaskDrive`。
- 备份允许程序目录与源分区相同。单系统还原和新增第二系统若程序目录所在分区等于
  还原目标，则在任何卷身份查询、UAC、任务创建、WinRE/BCD 修改或重启请求之前停止。
  程序不会自动复制、自动选盘或后台迁移；用户必须手动移动整个程序目录后重试。
- 正常任务 WinRE 入口固定为
  `%SYSTEMROOT%\System32\Recovery.exe,recover-env %SYSTEMROOT%\System32\RecoveryTask.env`。
  该内容内嵌在 Rust 二进制中，创建任务时直接生成 `payload\winpeshl.ini`，不依赖
  程序目录中的外部模板。自定义 PE 桌面仍使用独立的 `windows/winpe-winpeshl.ini`。

## v1.6.1 部署修复

- `winre_payload.rs` 是唯一的 WinRE 静态载荷映射和 shell 契约来源。任务暂存和 WIM 注入使用同一映射，
  每个复制文件均有 SHA-256 核对，提交前后都验证完整载荷。
- 任务 WIM 会删除遗留的 `BackupRestore.exe`、`RecoveryLauncher.cmd` 和
  `winpeshl-boot.cmd`，因而不能再以旧入口启动。
- 已删除自动重定位与 `--relocated` 参数，防止同卷还原偷偷产生副本或继续执行。
- GUI 的“创建桌面快捷方式”由 Rust 通过 Shell 已知文件夹 API 写入 `.url` 启动快捷方式，
  不生成或调用 PowerShell 脚本。
- Windows ARM64 部署脚本使用登录用户会话中的 Parallels `X:` 共享目录复制文件；只有
  Rust 二进制与 PE 模板复制成功，并且客体两个 EXE 的 SHA-256 与宿主构建一致时才输出
  `DEPLOYED`。WinRE shell 不再作为外部文件部署。
- 部署前会删除客体包中旧的 `RecoveryLauncher.cmd`、共享 `winpeshl.ini` 和
  `winpeshl-boot.cmd`，避免旧入口残留。

## 2026-09-16 验证结果

- `cargo fmt --all`、`cargo test --workspace --all-targets --offline`（32 项）和
  `bash scripts/audit-runtime-boundaries.sh` 通过。
- macOS 与 Windows ARM64 的严格 Clippy（`-D warnings`）通过；Windows ARM64 单元测试
  二进制已用 `rust-lld` 与 VM 提取的 SDK import library 完成 `--no-run` 编译。
- `./build-win.sh --deploy` 成功生成并部署 `BackupRestore.exe`（1,518,080 bytes），宿主、
  客体 `BackupRestore.exe` 与 `Recovery.exe` 的 SHA-256 均为
  `b44ba334c03418b826ac2cd7e66c111b8686615714498d04d23a2b522200d578`。
- 静态审计确认 GUI 在提权前、CLI 在 UAC 前阻止工作目录卷覆盖，且产品运行时源码没有
  PowerShell、`TaskDrive` 或 `C:\ProgramData\BackupRestore` 依赖。

## 2026-09-17 真实 WinRE probe

- 任务 `74d805aa-a192-474d-8c86-72b60a26bed3` 已完成
  `Windows -> reagentc /boottore -> WinRE -> Recovery.exe recover-env -> wpeutil reboot -> Windows`。
- `Recovery.log` 记录 Rust Recovery 从任务 env 启动、probe 不执行磁盘操作、原始注册
  WinRE 恢复并校验成功、清理完成；最终 `status.json` 为 `success / 100%`。
- 这证明当前 Rust 自包含入口、任务工作目录重新定位、WinRE 原镜像恢复和返回 Windows
  链路可用；不代表备份 Capture、还原 Apply、格式化或 BCDBoot 已由本次 probe 验证。

## 未验证边界

- 本次已排除“Parallels ARM 无法启动注册 WinRE RAMDISK”的旧结论。仍未由本次 probe
  覆盖的是实际 Capture/Apply、格式化、BCDBoot、第二系统启动和故障注入矩阵。
- 2026-09-17 C:→E: 真实备份首次实测在 WinRE“正在准备恢复环境”阶段无响应；根因是
  恢复程序对 `C:` 到 `Z:` 逐个同步调用 `mountvol <盘符> /L` 查找卷，任一盘符阻塞都会
  永久卡住。v1.6.2 改为直接调用 Windows 卷 API 查询卷 GUID；仅实际分配盘符时调用
  `mountvol.exe`，并增加 30 秒超时、子进程终止和可诊断日志。
- 2026-09-18 v1.6.3 增强 WinRE 早期诊断：`Recovery-early.log` 现在记录每个卷角色的
  GUID、盘符扫描数量、Win32 卷 API 错误码、`mountvol`/`diskpart` 耗时及挂载身份复核
  结果。该日志只增加可观测性，不改变卷身份校验、目标保护或备份/还原状态机。
- 2026-09-18 v1.6.4 的实测表明，WinRE 中按卷 GUID 查询挂载路径仍可能无响应。v1.6.5
  进一步收紧恢复路径：不再扫描盘符或调用 GUID 挂载路径枚举，只查询当前角色的固定
  临时盘符；`mountvol /L` 查询限制为 5 秒，`mountvol` 分配和 `diskpart` 分配限制为
  30 秒，超时会终止子进程并写入早期日志。身份复核使用任务中已保存的卷 GUID，避免
  在 WinRE 再次调用可能阻塞的卷 GUID API。

## 2026-09-23 WinRE 自动重启熔断

- Parallels 历史任务 `task-04bf4a16` 的 `prepare.log` 明确记录同一任务在
  `BootRequested` 阶段连续从正常 Windows 启动，再次请求 WinRE 并立即重启；这确认了
  应用恢复逻辑造成的无限重启循环。该日志不能说明最初为什么没有成功进入 WinRE。
- 当前 VM 的 `reagentc /info` 为 Disabled，活动 `tasks` 目录为空；历史归档任务不代表
  当前仍有待恢复任务。本轮未修改 VM、BCD、WinRE、磁盘或任务文件。
- CLI 对每个持久化任务阶段只自动请求一次 WinRE 重试，并以任务目录中的独立标记
  持久化熔断；阶段推进后仍允许对新阶段进行一次恢复尝试。`reagentc` 或重启命令明确
  失败时释放本次标记，允许之后重试；已经执行过一次自动重试而仍回到 Windows 时，停止
  自动重启并在 `prepare.log` 提示人工检查。
- 真实备份尚未在当前 v1.6.6 包上复测。后续需先确认 VM 有安全快照/回滚方案，再验证
  `Windows -> WinRE -> Recovery.exe` 和 C:→E: 备份；不得把历史循环修复等同于首次 WinRE
  启动故障已解决。

## 2026-09-24 缺陷修复与测试盲区收口

本轮先做静态分析，再修复分析中确认的两处真实缺陷，并把历史回归高发的纯逻辑
提取为跨平台模块。未改动 VM、BCD、WinRE 或任务文件。

### 已修复缺陷

1. **`mountvol` 查询把「超时」误当「未挂载」**（原 `mountvol_guid_with_timeout`）。
   旧代码把「盘符未挂载」、「查询超时 5 秒」和「mountvol 无输出」一律返回
   `Ok(None)`，`mount_env_volume` 收到 `None` 就继续 `mountvol.exe <盘符> <GUID>`
   与 diskpart `assign letter=`，于是同函数中
   "volume {letter}: is already mounted to {actual}, refusing to replace it"
   这条显式拒绝在超时路径下被静默绕过。超时正是 v1.6.2–v1.6.5 处理的场景，不是假设。
   现改为三态 `VolumeMountQuery{Mounted, Unmounted, Unknown}`
   （`text_parsing.rs:37` 分类，`main.rs:1513` 查询）：超时返回 `Unknown`，
   `mount_env_volume` 对 `Unknown` 直接停止并报
   "mount state could not be determined, refusing to assign it"（`main.rs:1311`），
   不再对状态未证实的盘符执行分配。`verify_mounted_volume` 同样区分
   「确实没挂载」与「查询没应答」两种失败。
   本修复不移除任何可用路径：分配后的身份复核原本就会在查询持续阻塞时失败，
   改动只是把难读的 diskpart 报错换成明确的身份冲突诊断。
2. **Win32 错误码被丢弃**（`windows_prepare.rs:1439`）。`mounted_volume_guid`
   原先用 `.ok().flatten()` 丢掉诊断函数的 Win32 错误码，而那个诊断函数没有其它调用方，
   于是 v1.6.3 记载的「`Recovery-early.log` 记录 Win32 卷 API 错误码」在该路径上
   已不成立。现在签名改为 `Result<Option<String>, u32>`：`Ok(None)` 表示盘符确实未分配，
   `Err(code)` 携带错误码，`volume_identity`（`windows_prepare.rs:1374`）据此区分
   「此处没有卷」与「卷 API 调用失败」，文档与代码重新对齐。

保留 `mountvol.exe` 子进程边界，没有改用进程内卷 GUID API：v1.6.4 记录该类 API
在 WinRE 中仍可能无响应，该区域跨四个版本不稳定且尚未实机复验，本轮只修超时语义。

### 测试盲区收口

`native_gui.rs`(7721 行)、`recovery_progress.rs` 与恢复挂载路径都由 `#[cfg(windows)]`
门控，macOS 上 `cargo test` 触及不到——历史上昂贵的回归正出自这里（DISM 索引名
带空格的 `exit=87`、bcdedit 的 GBK/UTF-16 输出、把「盘符空闲」和「查询没应答」混为一谈）。
新增 `crates/backuprestore-cli/src/text_parsing.rs`（494 行，**不带** `#[cfg(windows)]`），
把 11 个纯函数集中到跨平台模块并补 14 个测试：卷挂载三态分类、DISM WIM 元数据三种
JSON 形状、字节格式化、命令行参数加引号、bcdedit UTF-8/UTF-16（有无 BOM）解码、
GUID 提取、DISM 百分比解析、日志行到进度阶段的映射。
`native_gui.rs` 与 `recovery_progress.rs` 改为复用该模块，删除各自的重复实现。
测试数 33 → 46（CLI 14 → 27，core 19 不变）。

### 本轮验证结果

- `cargo fmt --all`、`cargo test --workspace --all-targets --offline`（46 项）通过。
- macOS 与 Windows ARM64 两侧 `cargo clippy --workspace --all-targets -- -D warnings` 均通过。
  Windows 侧交叉 clippy 是 `native_gui.rs` / `windows_prepare.rs` / `recovery_progress.rs`
  唯一的类型检查手段，本轮三个文件都改过，必须跑。
- `bash scripts/audit-runtime-boundaries.sh` 通过。

### 工程状态整治（非代码缺陷）

- 文档基线统一为 `1.6.6`：`docs/project-status.md`、`docs/verification-matrix.md`
  和 `docs/continuation-handoff.md` 原先仍写「当前版本 1.5.10」并指向
  `current-progress-2026-09-13.md`，与 `VERSION`=1.6.6 冲突，现已改为指向本文件。
  `current-progress-2026-09-13.md` 明确降级为历史快照。
- 新增 `.github/workflows/ci.yml`（此前仓库无任何 CI）：ripgrep + 运行时边界审计
  （含 `fmt --check`、test、clippy `-D warnings`）+ **Windows ARM64 交叉 clippy**
  + VERSION/两个 manifest/Cargo.lock 版本一致性检查。交叉 clippy 只做类型检查不链接，
  因此无需 Windows SDK import library 即可在 Linux runner 上覆盖全部 Windows-only 模块，
  这条盲区从此由 CI 兜住。
- `.gitignore` 收窄：根级 `/*.ps1`、`/*.bat`、`/*.txt`、`/*.xml`、`/*.out`、`/*.env`
  等宽通配确实在挡 40 多个一次性探测脚本，故保留，但已入库的
  `click-pe-disk.ps1`、`compare-pq.ps1` 等文件原先也被命中，只因早于规则加入才幸存；
  现在文件末尾加了显式 `!` 反否定段。`git ls-files -i -c` 已为空。
- 根目录 216 个一次性产物（png/ps1/log/txt/bat 等）移入 `.archive/root-temp-2026-09-24/`
  并加入忽略，未删除任何文件；构建目录一律未动。

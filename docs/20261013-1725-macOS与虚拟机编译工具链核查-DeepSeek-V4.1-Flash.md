# macOS 与虚拟机内编译工具链核查（2026-10-13 17:25）

> 模型：DeepSeek-V4.1-Flash
> 代码基线：HEAD `51e3657`，VERSION **2.4.4**
> 方法：**两侧都实跑了一次真实构建**，不只做存在性检查——"工具在不在"不等于"能不能编出东西"。
> 证据边界：本文只做环境核查与构建验证，**未部署（未跑 `-Deploy`）、未操作 WinRE/BCD、未改任何产品源码**。

---

## 一、结论先说

| 问题 | 结论 |
|---|---|
| 工具是否齐全 | **齐全**。macOS 交叉编译与 VM 内原生编译两条路径都实测构建成功。 |
| 是否需要额外下载 | **必需项为零**。唯一可下载的是 rustc 1.99.0 与 VM 侧 rustup 1.29.1（均为**可选**升级）。 |
| 是否最新 | **不是最新**，但**不阻塞开发**。macOS 与 VM 侧均落后一个 rustc 小版本（1.98.x → 1.99.0）。 |
| 有无阻塞项 | 有 1 项**与工具无关**的阻塞：**宿主空闲 163GiB < AGENTS.md 要求的 220GiB**，跑不了 87GB 级还原测试。 |

---

## 二、macOS 侧（交叉编译路径）

| 组件 | 实测结果 | 状态 |
|---|---|---|
| rustup | 1.29.1 (d95a37b6a 2026-08-13) | ✅ 已是最新 |
| rustc / cargo | 1.98.1 (48a229cea 2026-09-01) | 🟡 有 1.99.0 可用 |
| 默认工具链 | `stable-aarch64-apple-darwin` | ✅ |
| 已装 target | `aarch64-apple-darwin`、`aarch64-pc-windows-msvc` | ✅ 交叉目标已装 |
| 组件 | cargo / clippy / llvm-tools / rust-docs / rust-std(两个) / rustc / rustfmt | ✅ |
| `rust-lld` | `~/.rustup/toolchains/stable-aarch64-apple-darwin/lib/rustlib/aarch64-apple-darwin/bin/rust-lld`（5,711,312 B，09-16） | ✅ 在 `build-win.sh` 推导出的位置 |
| Windows SDK 导入库 | `/Users/x/win-sdk-arm64/`：`um/arm64` 439 个、`ucrt/arm64` 8 个、`vc/arm64` 57 个 | ✅ 三个目录都在 |
| crates.io 源 | `~/.cargo/config.toml` 走 **USTC 镜像**（`sparse+https://mirrors.ustc.edu.cn/crates.io-index/`） | ✅ 国内源，快 |

**实跑验证**：`./build-win.sh`（不带 `--deploy`）

```
Finished `release` profile [optimized] target(s) in 7.56s
>> 产物：2498560 字节
```

7.56 秒通过（增量），产物 2,498,560 字节。唯一输出是成片的 `LNK4099: Cannot use debug info for 'libvcruntime.lib(...)'`——**SDK 导入库没随附 PDB 的正常现象，无害**，不影响产物。

---

## 三、虚拟机侧（VM 内原生编译路径，当前主路径）

虚拟机 `Windows 11` 状态 running，GuestTools `state=installed version=26.4.2-57518`。

| 组件 | 实测结果 | 状态 |
|---|---|---|
| Visual Studio C++ Build Tools | `C:\BuildTools`，MSVC **14.44.35207** | ✅ |
| VsDevCmd.bat | `C:\BuildTools\Common7\Tools\VsDevCmd.bat` | ✅ 存在（`build-windows.ps1` 初始化 MSVC 环境要用） |
| vswhere | `%ProgramFiles(x86)%\Microsoft Visual Studio\Installer\vswhere.exe` | ✅ 返回 `C:\BuildTools` |
| MSVC 库 | `lib\arm64` ✅、`lib\x64` ✅ | ✅ ARM64 与 x64 都在 |
| Windows SDK | **10.0.26100.0**，`Lib\...\um\arm64\kernel32.Lib` ✅、`um\x64\kernel32.lib` ✅ | ✅ |
| MSVC 运行库 | `System32\VCRUNTIME140.dll`、`VCRUNTIME140_1.dll` | ✅（打包步骤要用） |
| Rust（**交互会话** `C:\Users\x\.cargo`） | rustc **1.98.0** (88d9e12ae 2026-08-18)，rustup **1.29.0** | 🟡 比另两处低一个补丁版 |
| Rust（**SYSTEM** `...\systemprofile\.cargo`） | rustc **1.98.1** (48a229cea) | ✅ |
| 已装 target（两处一致） | 仅 `aarch64-pc-windows-msvc` | ✅ 够用；x64 未装 |
| cargo 镜像源 | **两处都没有 config.toml** → 直连 crates.io | 🟡 无国内镜像 |

**实跑验证**（文档记录的标准命令，`-CargoTargetDir` 指到 VM 本地盘）：

```powershell
prlctl exec 'Windows 11' --current-user powershell -NoProfile -ExecutionPolicy Bypass `
  -File 'X:\windows\build-windows.ps1' -Architecture arm64 -CargoTargetDir 'C:\BackupRestoreBuild\target'
```

```
Building arm64 (aarch64-pc-windows-msvc) version 2.4.4
  Downloaded cfg-if v1.0.5 / unicode-ident v1.0.26 / thiserror v2.0.21 / thiserror-impl v2.0.21 / uuid v1.26.1 / syn v3.0.6
   Compiling ... backuprestore-core v2.4.4 / backuprestore-cli v2.4.4
    Finished `release` profile [optimized] target(s) in 55.89s
Package: X:\artifacts\windows\BackupRestore-windows-arm64-v2.4.4
```

55.89 秒通过，产出完整包（`BackupRestore.exe` / `Recovery.exe` 各 2,408,960 B + `VCRUNTIME140*.dll` + `build-manifest.json`）。`VERSION` 与两个 `Cargo.toml` 的版本一致性校验（脚本自带）也过了。

---

## 四、共享盘与通道（VM 构建的入口，易踩）

| 通道 | 结果 |
|---|---|
| `X:` → `\\Mac\backupRestore` | **只在交互会话存在**（`--current-user` 下 `net use` 可见，`X:\VERSION` 可读） |
| `Z:` → `\\Mac\Home` | 同上，交互会话可见 |
| `X:` 在 SYSTEM 会话 | ❌ `dir X:\VERSION` 报"系统找不到指定的路径" |
| UNC `\\Mac\backupRestore` 在 SYSTEM 会话 | ✅ 可读（`dir \\Mac\backupRestore\VERSION` 正常返回） |

**两条实操推论**：

1. 文档里的 `& X:\windows\build-windows.ps1 ...` **必须在 VM 的交互会话里跑**（人开的 PowerShell 窗口）。自动化 SYSTEM 通道要用 `\\Mac\backupRestore\windows\build-windows.ps1`（`build-windows.ps1` 用 `--manifest-path` 绑定仓库根，从任意目录都能跑，注释里就是这么设计的）。
2. **`if exist \\Mac\backupRestore\NUL` 这个判据是错的**（`NUL` 不是 UNC 根下的合法文件名，恒为假）。本次一开始就因此误判成"UNC 也不可达"。要判共享可达，用 `dir \\Mac\backupRestore\VERSION`。

---

## 五、本次发现的 5 个问题

### 5.1 VM 侧有两套 Rust，版本不一致（🟡 建议对齐）

| 位置 | rustc | rustup |
|---|---|---|
| VM 交互会话 `C:\Users\x\.cargo` | **1.98.0** | **1.29.0** |
| VM SYSTEM `C:\Windows\System32\config\systemprofile\.cargo` | 1.98.1 | — |
| macOS | 1.98.1 | 1.29.1 |

**影响**：文档记的标准命令在交互会话里跑，用的是 **1.98.0**；而 `build-win.sh` 的 `--deploy` 走 SYSTEM 通道（1.98.1）。同一个仓库、同一份源码，会因执行通道不同落到不同编译器上。Rust 补丁版之间通常兼容，但**产物哈希会不同**，将来核对"部署的是不是刚编的"时要留意这一点。

**建议**：在 VM 交互会话里 `rustup update stable`，把 1.98.0 提到与另两处一致（或统一升到 1.99.0）。

### 5.2 VM 侧没有 cargo 镜像源（🟡）

两处 `config.toml` 都不存在 → 直连 `crates.io`。本次构建正好印证：它现场下载了 6 个 crate（说明 VM 的 registry 缓存是**部分缺失**状态）。若哪天网络受限或要走热点，冷缓存重建依赖树会明显变慢。

**建议**：给 VM 配一个国内镜像（与 macOS 侧对齐，例如 USTC/清华），或在构建机上保留完整 `CARGO_HOME` 缓存。注意 macOS 用的是 `sparse+` 协议，Windows 侧同样支持。

### 5.3 `build-win.sh` 的注释与实测不符（🟢 文档问题）

- 第 42-43 行注释称 `prlctl exec` 会因 **GuestTools `state=outdated`** 返回 `Invalid argument`；实测 GuestTools 已是 `state=installed version=26.4.2-57518`，该注释的现场描述已过时。
- 本次另遇到**另一种**通道故障：SYSTEM 通道连续返回 `Unable to open new session in this virtual machine`（交互通道同时正常），**几分钟后自行恢复**。这与 `Invalid argument` 不是同一个错，现有重试逻辑（只对 `Invalid argument` 重试）**接不住它**——`build-win.sh --deploy` 遇到它会直接失败。建议把"新会话打不开"也纳入重试判据。

### 5.4 两条构建路径的产物大小差与文档说法不符（🟡 待复核）

| 路径 | 本次实测大小 |
|---|---|
| macOS 交叉编译（`build-win.sh`） | **2,498,560 B** |
| VM 内原生编译（`build-windows.ps1`） | **2,408,960 B** |
| 差值 | **89,600 B（约 3.7%）** |

而 `docs/20261012-1640-macOS交叉编译与VM内原生编译实测对比-Fable5.md` 记的是"**大小差 16KB**"。同一 VERSION（2.4.4）下差了 89.6KB，**该文档的这条数字已过时**。两者都是合法产物（不同链接器：`rust-lld` + SDK 导入库 vs `link.exe`），但"两条路径产物质量等同"的结论应重新核对，尤其**部署时不要混用**。

### 5.5 宿主磁盘不满足实机测试硬门槛（🔴 与工具无关，但会挡住下一步）

```
/dev/disk3s5   926Gi   736Gi   163Gi   82%   /System/Volumes/Data
```

AGENTS.md 的硬规则是"跑 87GB 级还原前，宿主空闲 **≥ 镜像大小 × 2.5（即 ≥220GB）**"。**当前 163GiB，差约 57GB**。VM 侧反而很宽裕（C: 169.9GB、E: 196.9GB、F: 349.9GB、H: 155.3GB 空闲）。

**推论**：任何 87GB 级的还原测试现在都跑不了，要先清理宿主（按 AGENTS.md：有快照时先删旧快照层再谈压缩）。这是**当前最该先处理的一项**，与工具链无关。

---

## 六、需要额外下载的项（按需，非必需）

| 项 | 用途 | 体积 | 判断 |
|---|---|---|---|
| rustc 1.99.0（macOS） | 跟进最新 stable | 约 200–300MB | 可选。当前 1.98.1 完全可用 |
| rustc 1.99.0 + rustup 1.29.1（VM 交互会话） | 消除 5.1 的版本偏斜 | 约 200–300MB | **推荐**，优先解决偏斜 |
| `rustup target add x86_64-pc-windows-msvc`（VM） | 将来要编 x64 包（`-Architecture x64`） | 约 30–60MB | 按需。VM 的 MSVC/SDK x64 库**已就绪**，只缺 Rust 侧 target |
| macOS 侧 x64 交叉所需的 x64 SDK 导入库 | 在 Mac 上交叉编 x64 | — | 按需。当前 `/Users/x/win-sdk-arm64` **只有 arm64** |

> 用户规则：单次下载超过 100MB 前要先确认网络。本次核查时网络为 **WiFi（192.168.0.0/24，网关 192.168.0.1）**，**不是热点**。上表任何一项都不需要立刻下载。

---

## 七、证据边界

- 两侧构建均为**真实执行**（macOS 7.56s / VM 55.89s），不是静态推断。
- **未跑 `-Deploy`**，因此"部署到 `C:\Users\Public\backupRestore-package` 与 `H:\brwork` + 四文件 SHA-256 核对"这一段**本次未验证**；它依赖共享盘（本次确认可用）与 SYSTEM 通道（本次出现过一次抖动）。
- 未跑 `cargo test`，未做任何实机（WinRE/BCD/格式化）验证。
- 版本与磁盘数字均为 **2026-10-13 17:15–17:25** 的时点快照，会随环境变动失效。
- 本次构建在 Mac 上留下了 `artifacts/windows/BackupRestore-windows-arm64-v2.4.4/`（已被 `.gitignore` 的 `artifacts/` 覆盖，不入库）。

# 构建路径唯一化为 VM 内 Windows + `build-windows.ps1` 幂等补齐 `+crt-static`（2026-10-15 08:44）

> 模型：Step-5-Preview｜触发：用户拍板「编译应该完全走 VM 内 Windows」
> 前置：经验库复核时发现构建路径口径冲突（README/ps1 称 VM 内为主，但 ps1 的 `Invoke-Cargo` 未设 `+crt-static`，且近几轮实际走 macOS `build-win.sh --deploy`）——本轮一并解决。

## 1. 改动清单

| 文件 | 改动 |
| --- | --- |
| `windows/build-windows.ps1` | `Invoke-Cargo` 调 cargo 前**幂等确保 `$env:RUSTFLAGS` 含 `-C target-feature=+crt-static`**（空则设、有其他 flag 追加、已有不动），附中文注释说明硬要求与 0xC0000135 症状 |
| `README.md` §三 | 「构建主路径在 VM 内」改为「**构建完全走 VM 内 Windows（唯一路径，macOS 侧不再是发布路径）**」；原「macOS 交叉编译（保留 sanity check）」小节改为「已废止为发布路径，仅限本地 cargo check」；crt-static 提示改为「VM 内脚本已幂等内置」 |
| `经验/06-开发构建部署.md` §1 | 改为「唯一路径=VM 内」+ crt-static 已补齐的记录；macOS 废止为发布路径 |
| `经验/07-墓碑与争议清单.md` | 未决项移除「VM 内构建缺 +crt-static / 构建主路径之争」，标注 2026-10-15 已解决 |
| `文档索引.md` | 收录本篇；横幅构建口径不变（未提构建路径，无需改） |

版本号：**未升**（`VERSION` 与两个 Cargo.toml 已被并行的 v2.5.9 源码轮占用，本轮为脚本+文档轮，按"仅源码轮升版本"规则不动）。

## 2. 修复的缺口（为什么必须补）

- `+crt-static` 原先只存在于 macOS 侧 `build-win.sh`；VM 内 `build-windows.ps1` 直接 `cargo build` 未设该 flag，仓库也无 `.cargo/config.toml`。
- 后果（理论）：VM 内产物动态链接 `VCRUNTIME140.dll`/`api-ms-win-crt-*`，进不了精简 WinRE/PE——症状=进程创建即被加载器终止，**零输出、零副作用、无日志无弹窗，退出码 0xC0000135**（20260925-0207 实测过的坑）。
- 之所以一直没爆雷：近几轮（2.5.5~2.5.8）实际构建走的是 macOS `build-win.sh --deploy`（2105/2107/0105 均有记录）。用户拍板唯一走 VM 内后，此 flag 成为必需项。

## 3. 验证（本轮已做）

1. **pwsh 语法解析**：`[Parser]::ParseFile` 0 错误（`SYNTAX_OK`）。
2. **幂等逻辑单测**（模拟四种初始 RUSTFLAGS）：空→只设 flag；已含 flag→不动；有其他 flag→正确追加；二次调用→不重复。全部符合预期。
3. `grep` 核对 README 无残留「主路径/sanity check/保留」矛盾表述；经验库 06/07 口径同步。

## 4. 未做实机验证（下一源码轮的首个动作）

- 本轮未在 VM 内实跑构建（会触发 RUSTFLAGS 变化导致的全量重编，且并行会话正在改源码）。
- **下一源码轮在 VM 内首次执行 `& X:\windows\build-windows.ps1 -Architecture arm64 -Deploy` 时**：构建后必须按静态 CRT 判据核对产物导入表（不得含 `VCRUNTIME140*`/`api-ms-win-crt-*`），再按四处 SHA-256 对账。若 host 侧原本有自定义 RUSTFLAGS，追加行为已被单测覆盖。

## 5. 后续规则

- 构建/部署/发布**只走** VM 内 `build-windows.ps1`；macOS 侧 `build-win.sh --deploy` 停用（本地 cargo check 除外）。
- 实机传递（`prlctl` 等）与部署经 UNC 的既有坑不变，见 `经验/02`、`经验/06`。

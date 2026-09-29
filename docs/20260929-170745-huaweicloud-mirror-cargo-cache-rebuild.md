# 镜像源切换华为云 + Cargo 缓存重建（2026-09-29 17:07）

## 背景

本机全局镜像源已切换到华为云开源镜像站：

- pip：`https://mirrors.huaweicloud.com/repository/pypi/simple/`（`~/.pip/pip.conf`）
- npm：`https://mirrors.huaweicloud.com/npm/`（`~/.npmrc`）
- ~~Homebrew API：`https://mirrors.huaweicloud.com/homebrew-bottles/api`~~ **华为云已下架 Homebrew 镜像，见下文，实际走 tuna**

## 关键结论一：华为云没有 crates.io 镜像

华为云开源镜像站仓库列表中**已无 Cargo 镜像**，以下地址均 404（2026-09-29 实测）：

- `https://mirrors.huaweicloud.com/repository/crates.io-index/config.json`
- `https://mirrors.huaweicloud.com/repository/crates/crates.io/...`

因此 **`~/.cargo/config.toml` 保持中科大源不变**：

```toml
[source.crates-io]
replace-with = "ustc"

[source.ustc]
registry = "sparse+https://mirrors.ustc.edu.cn/crates.io-index/"
```

备选可用源（实测 200）：清华 `mirrors.tuna.tsinghua.edu.cn/crates.io-index/`、rsproxy `rsproxy.cn`。

## 重建操作

```bash
# 1. 清空旧缓存（约 432M，全部可再生）
rm -rf ~/.cargo/registry/cache ~/.cargo/registry/src ~/.cargo/registry/index

# 2. 从 ustc 全量拉取（按 Cargo.lock 锁版本）
cargo fetch --locked

# 3. 验证（双目标）
cargo check --workspace
./build-win.sh   # aarch64-pc-windows-msvc release，产物约 1.78MB
```

## 顺带修复：macOS 编译缺 Windows 门控（v1.7.11 WIP 代码）

缓存重建后 `cargo check` 暴露 2 处遗漏，已修复并提交（`fb1835e`）：

1. `main.rs`：`Some("bcd-set") => bcd_set(...)` 分支没加 `#[cfg(windows)]`，而 `bcd_set` 本体是 Windows-only（新进程跑 `bcdedit` 的短生命周期子进程通道），macOS 报 E0425。
2. `boot_entry.rs`：`use std::os::windows::process::CommandExt;` 裸 import 未加 `#[cfg(windows)]`。

教训：EF0425 名称解析错误会**提前中止整个 crate 的类型检查**，同类门控遗漏会被掩盖，修完一个要继续 check 到底。

## npm 缓存现状（未处理，待用户决定）

`~/.npm` 占用 11G。npm 缓存（cacache）按下载 URL 寻址，源切换后旧条目自动失效，不影响新源命中，故未清理；如需回收磁盘可 `npm cache clean --force`（可再生，但其它 node 项目下次安装需重下）。

## 关键结论二：华为云已完全下架 Homebrew 镜像（2026-09-29 追加）

三个端点实测全部返回 **HTML 前端页面**（`Content-Type: text/html`），后端仓库不存在：

- `https://mirrors.huaweicloud.com/homebrew/brew.git/info/refs?service=git-upload-pack`
- `https://mirrors.huaweicloud.com/homebrew-bottles/api/formula.json`
- `https://mirrors.huaweicloud.com/homebrew-bottles/bottles/<包>.bottle.tar.gz`

**坑**：只看 HTTP 状态码会误判——这些路径都返回 200，但内容类型是网页不是 `application/json` / `application/octet-stream` / `x-git-upload-pack-advertisement`。判定镜像可用必须看 `Content-Type`（或 `file` 首字节）。

因此 `~/.zprofile` 的 Homebrew 三个变量改用清华 tuna（均已验证）：

- `HOMEBREW_BREW_GIT_REMOTE=https://mirrors.tuna.tsinghua.edu.cn/git/homebrew/brew.git`
- `HOMEBREW_API_DOMAIN=https://mirrors.tuna.tsinghua.edu.cn/homebrew-bottles/api`
- `HOMEBREW_BOTTLE_DOMAIN=https://mirrors.tuna.tsinghua.edu.cn/homebrew-bottles`

同时 `git -C /opt/homebrew remote set-url origin <tuna brew.git>`。`brew update` 实测通过。
（注意：另一个终端若在用 tuna 跑 brew update 会占锁且在命令末尾把 remote 改回去，改完 remote 后要复验。）

## 关键结论三：镜像源实测速率对比与最终版图（2026-09-29 追加）

统一测法：`curl -r 0-2MB` 小样本，不消耗多少流量。**同一厂商不同域名速率差巨大**：

| 源 | 对象 | 速率 | 结论 |
|---|---|---|---|
| `repo.huaweicloud.com` | pip whl / npm tgz | 0.18–0.34 MB/s | **快域** |
| `mirrors.huaweicloud.com` | 同上 | 0.01–0.06 MB/s | **慢域，勿用** |
| `mirrors.aliyun.com` | pip whl | 0.12 MB/s | 次选 |
| tuna / ustc | brew api | 100–400 KB/s | 打平 |
| ustc | cargo crate | 0.08 MB/s | 唯一好用 |

各工具最终配置：

| 工具 | 源 | 处理 |
|---|---|---|
| pip | `repo.huaweicloud.com/repository/pypi/simple/` | **已从 mirrors 域改到 repo 域**（配置在 `~/.config/pip/pip.conf`，不是 `~/.pip/pip.conf`） |
| npm | `repo.huaweicloud.com/repository/npm/` | 不变（已是快域） |
| Homebrew | 清华 tuna 全套 | 不变（华为云已无 brew 镜像） |
| Cargo | ustc | 不变（华为云无 crates；tuna crates download API 已 404） |

教训：`~/.npmrc` 早在用 repo 快域，而 pip 与 `~/.zshrc` 的 HOMEBREW_PIP_INDEX_URL 分居两个域——排查镜像源问题时要逐个文件核对，不能想当然。另 `~/.pip/pip.conf` 已不存在，pip 实际生效路径是 `~/.config/pip/pip.conf`。

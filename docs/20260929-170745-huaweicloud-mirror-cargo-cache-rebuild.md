# 镜像源切换华为云 + Cargo 缓存重建（2026-09-29 17:07）

## 背景

本机全局镜像源已切换到华为云开源镜像站：

- pip：`https://mirrors.huaweicloud.com/repository/pypi/simple/`（`~/.pip/pip.conf`）
- npm：`https://mirrors.huaweicloud.com/npm/`（`~/.npmrc`）
- Homebrew API：`https://mirrors.huaweicloud.com/homebrew-bottles/api`（环境变量 `HOMEBREW_API_DOMAIN`）

## 关键结论：华为云没有 crates.io 镜像

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

# 构建环境两个坑：镜像源与 target 所有权（2026-09-25）

两个坑都表现为「代码没动，突然构建不了」，而且报错都指向依赖，极易误判成代码或
`Cargo.lock` 有问题。**都不是代码问题。**

## 一、cargo 镜像源被换成没有本地索引的源

症状：

```
error: no matching package named `chrono` found
location searched: `huaweicloud` index (which is replacing registry `crates-io`)
note: offline mode (via `--offline`) can sometimes cause surprising resolution failures
```

排查要点：报错说 `huaweicloud` index，但

```bash
ls ~/.cargo/registry/index/
# index.crates.io-1949cf8c6b5b557f
# mirrors.ustc.edu.cn-38d0e5eb5da2abae
```

**根本没有 huaweicloud 的索引目录**——镜像在 `~/.cargo/config.toml` 里配了，
但从未拉取过索引。`--offline` 下没有索引就等于没有任何包。

注意换成官方 crates.io 也**不一定**能用：本机 `Cargo.lock` 锁定的
`js-sys 0.3.105` 的 `.crate` 未缓存（它是 chrono 的 wasm-only 依赖，从不参与构建，
但 cargo 解析整图时仍要求它可解析）。

修法：切回**本机确实有索引缓存**的源。本机是中科大：

```toml
[source.crates-io]
replace-with = "ustc"

[source.ustc]
registry = "sparse+https://mirrors.ustc.edu.cn/crates.io-index/"
```

判断该用哪个源的方法：`ls ~/.cargo/registry/index/` 看哪些源有缓存，
再 `find ~/.cargo/registry/index/<源>/.cache -name js-sys` 确认关键包在不在。

## 二、target/ 下有 root 拥有的产物

症状（换对镜像源之后才暴露出来）：

```
error: failed to write `.../target/debug/.fingerprint/backuprestore-cli-*/test-bin-backuprestore-cli`
Caused by: Permission denied (os error 13)
```

原因：曾经用 `sudo cargo ...` 跑过，留下 root 拥有的中间产物。本轮实测 370 个：

```bash
find target ! -user "$(id -un)" | wc -l   # 370
```

修法（这些都是可重建的中间产物，收回所有权即可，不必 `cargo clean`）：

```bash
sudo chown -R "$(id -u):$(id -g)" target
```

**不要再用 `sudo cargo`。** 本项目所有构建、测试、clippy 都不需要 root；
需要提权的只有客体 Windows 内的操作（经 `prlctl exec` 提权通道）。

## 复查清单

构建莫名失败时，按顺序查这三样，再怀疑代码：

1. `cat ~/.cargo/config.toml` + `ls ~/.cargo/registry/index/` —— 源与索引是否匹配
2. `find target ! -user "$(id -un)" | wc -l` —— 是否有 root 产物
3. `git status --short Cargo.lock` —— lock 是否被意外改动

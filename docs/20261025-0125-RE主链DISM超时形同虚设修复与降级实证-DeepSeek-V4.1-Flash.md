# RE 主链 DISM 根本没有超时：`--dism-timeout-secs` 此前形同虚设（v2.6.44）

- **时间**：2026-10-25 01:25
- **版本**：v2.6.44（基线 commit `6eb3ef0`，上一版 `1e2003d`）
- **模型**：DeepSeek-V4.1-Flash
- **性质**：**实机挖出的真缺陷** + 修复 + 端到端实证

## 一句话结论

`--dism-timeout-secs` 在恢复环境里**只写了一行说明日志**，apply 实际是无条件 `child.wait()`——设置形同虚设，而且**超时类失败永远走不到分类**，D1 的 wimlib 自动降级在 RE 主链上永不触发。这不是静态审查能看出来的：要真跑一次「1 秒上限 + 38GB 镜像」才会暴露。

---

## 怎么发现的（这条最重要）

旧待办里有一项「RE 主链降级未单独实机验证」。为了补它，我用

```
prepare --operation create-secondary --source-drive C --target-drive F \
        --image-path H:\BRImages\br-2629-20261023.wim --dism-timeout-secs 1
```

跑了 38GB 镜像到 F:，预期 DISM 1 秒就超时、立刻降级 wimlib。

**结果：任务 success/100%，用时约 6 分钟，日志里一条降级行都没有。**

查日志只看到：

```
DISM wait limit: 1 sec (0 = unlimited)      <- 只是启动时记的"说明"
```

也就是说：**上限只被"记录"了，没有被"使用"**。

### 根因

RE 主链 apply 走的是：

```rust
} else if let Err(error) = run_logged("dism.exe", &["/English", "/Apply-Image", ...], log) {
    fallback_after_dism_failure(...)?;
}
```

而 `run_logged` 里是：

```rust
let status = child.wait()?;   // 无超时
```

`dism_timeout::wait_ms()` 从来没被传进来。后果是双重的：

1. 用户以为配了逃生阀，卡死时照样卡死；
2. 因为没有 TimedOut 失败，`classify_apply_failure` 永远归不到 `TimedOut`，
   `fallback_after_dism_failure` 里的降级判据也就永远不会因超时触发。

---

## 修法

```rust
fn run_logged(program, args, log) { run_logged_timeout(program, args, log, u32::MAX) }

fn run_logged_timeout(program, args, log, wait_ms) { ... 用 wait_child(&mut child, wait_ms) ... }

fn wait_child(child, wait_ms) -> Result<Option<ExitStatus>, TaskError> {
    // u32::MAX = 等到底（旧行为）；否则 200ms 轮询，超时 kill + 回收，返回 None
}
```

三个要点：

- **零回归**：`run_logged` 保留，`wait_ms = u32::MAX` 与原行为完全一致，其它 11 个调用点（bcdboot / mountvol / diskpart / wpeutil …）不受影响。
- **失败仍可归因**：超时后先 `kill` + `wait`，**再 join 两个读线程**，输出尾部照样落进日志——分类判据要的就是这段文本。
- RE 主链的 `/Apply-Image` 传 `crate::dism_timeout::wait_ms()`。

---

## 端到端实证（同一条命令，修前修后对比）

**修前（v2.6.43）**：任务 success，6 分钟，日志零降级行 → 上限没生效。

**修后（v2.6.44）**，同一命令、同一镜像，`Recovery.log`：

```
01:08:53 [ERROR] DISM apply 失败分类：TimedOut——应用镜像超过等待上限被终止
                            （上限可在设置中调整，默认不限制）…
01:08:53 [WARN] DISM apply 失败（…），自动降级 wimlib 重试一次（等待上限：不限制）
01:08:53 wimlib 开始（覆盖式应用，还原安全描述符）：
         X:\windows\System32\wimlib-imagex.exe apply …\br-2629-20261023.wim 1 F:\
01:13:42 wimlib 结果：退出码 0
01:13:42 降级成功：wimlib 完成 apply，任务继续
```

任务 `49a99148` **success / 100%**。RE 内进度窗截图同步显示 wimlib 的
`正在展开系统文件：24 GiB of 79 GiB (31%)`——DISM 1 秒被终止、wimlib 接着写完 79GB/4 分 49 秒。

★ 这也是**恢复环境主链的降级第一次端到端实证**（此前只验过在线数据卷那层）。

---

## 现场处置（如实记录）

create-secondary 会把还原出的系统设为 default，重启就进新系统：

| 轮次 | 现象 |
|---|---|
| 第 1 次（修前） | 重启后进了还原出的系统：原 C:（249GB）变 **E:**，原 F:（349GB）变 **C:** |
| 恢复 | `bcdedit /default {af5ee73f-…}`（原系统条目）→ 重启 → 盘符回到原位 |
| 第 2 次（修后） | 同样进新系统，同样改回 |

最终现场：**C: 249.47GB 原系统**、F: 349.98GB 测试系统、T: 空卷、H: 正常；启动菜单多一项
`Windows Backup-202610250107`（对着 F: 的测试条目，命名清晰、留着下次测试直接可用）。

---

## 验收

| 项 | 结果 |
|---|---|
| 单测 | cli **240**（+1 ignored）+ core **45** |
| 交叉编译部署 | `2.6.44`，SHA256 `5b96f9cb…` |
| 修前 | `--dism-timeout-secs 1` + 38GB → 6 分钟正常完成、**零降级行** |
| 修后 | 同命令 → `TimedOut` → 降级 wimlib → 79GB 写完 → **success/100%** |
| RE 主链降级 | **首次端到端实证** |

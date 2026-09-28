# 方案 D「还原干净 WinRE」提前到 WinRE 入口（2026-09-28）

> 背景：用户要求把"备份前还原干净 WinRE"这一步从「DISM 捕获之前」挪到「刚进入 WinRE、
> 清理完一次性启动项、确认 WinRE 加载完成后」。本文记录分析、改动与验证。
>
> ⚠️ **重要：本文描述的「入口立即翻转」方案已于同日回退，不再是现行做法。**
> 原因：该改动与断电续跑存在真实冲突（注册位在入口被换成干净原版后，续跑重启会落进
> 微软原版 WinRE、没有 `winpeshl` 钩子、`Recovery.exe` 不会自动跑 → 任务续不起来）。
> 已证实这条暗契约：续跑**隐式依赖「注册位 = 注入件」**。
> 详见 [20260928-073809-resume-vs-clean-winre-conflict.md](20260928-073809-resume-vs-clean-winre-conflict.md)（冲突定位）
> 与 [20260928-074516-registered-winre-policy-and-resume-design.md](20260928-074516-registered-winre-policy-and-resume-design.md)（最终状态机设计）。
> **现行做法**：不在入口翻转；注册位在会话期间保持注入件，仅「DISM 捕获前」翻转为干净原版，
> 另在「断电续跑重武装之前」新增 `ensure_registered_is_payload` 保证续跑能落地。
> 下文其余部分（澄清、代码位置梳理）作为历史记录保留。

## 1. 用户诉求澄清

用户记忆里有两个相关但不同的"坑/功能"：

1. **进入 WinRE 后重启回不了 Windows（boot loop）** → 当时做了"进入 WinRE 后立即清理一次性
   启动项"，重启就能回 Windows。
2. **备份会把脏的（注入后的）WinRE 一起捕获进镜像（F3 污染）** → 方案 D 在捕获前把注册 WIM
   覆写回干净原件。

用户想把第 2 个操作（"替换回修改前的 Windows RE"）**挪到刚进入 WinRE、确认加载完成之后**。

### 关于"清理一次性启动项"的纠正

代码核实后：**WinRE 恢复路径里并没有一个独立的"清理一次性启动项"函数**。
- WinRE 路径保证"重启回 Windows"靠两道：
  - 进入 WinRE 之前由 `reagentc /boottore` 设置的一次性启动项（bootstatus）被 bootmgr 自身消费；
  - 任务收尾时 `WinreRestoreGuard`（`Drop`）调 `restore_original_winre` 还原干净 WinRE。
- "进入 WinRE 后立即清理一次性启动项"那套实现是 **PE 路径**的 `pe_self_clean_bootsequence`
  （`native_gui.rs:7117-7250`），不是 WinRE 路径。
- 因此在 WinRE 路径里，"清理完一次性启动项之后"对应的代码锚点就是 **`WinreRestoreGuard` 建立
  之后、task 加载完成**（`main.rs` 约 744–752 行）。

## 2. 改动（main.rs，WinRE 入口）

在 `recover-env` 入口、`WinreRestoreGuard` 建立且 `task` 加载完成之后（约 753 行后）新增：

```rust
// 方案 D（入口提前版）
if let Operation::Backup = task.operation {
    if let Some(recovery) = recovery_volume_from_env(&values) {
        if let Some(source) = task.source.as_ref() {
            if backuprestore_core::capture_source_hosts_registered_winre(source, Some(&recovery)) {
                if let Err(e) =
                    restore_original_winre(&values, &task_dir, &early_log, recovery_letter)
                {
                    return Err(e); // 失败即硬失败终止，绝不静默产脏镜像
                }
            }
        }
    }
}
```

### 为什么安全 / 正确

- **WinRE 已跑在内存（X: RAM 盘）里**：磁盘上 `<注册宿主卷>:\Recovery\WindowsRE\Winre.wim`
  不再被读取，覆写它不影响当前运行会话——这正是方案 D 的设计前提。
- **`recovery_letter`（R:）就是承载注册 WinRE 的卷**：入口处 `RECOVERY` 卷已挂载为
  `recovery_letter`，而 `R:\Recovery\WindowsRE\Winre.wim` 正是注册 WIM 本体（结尾
  `restore_original_winre` 也是写回这个位置）。直接用 `recovery_letter` 写回即可，不依赖
  source 是否挂盘符。
- **闸门只看 GUID，不看盘符**：`capture_source_hosts_registered_winre` 用 `same_partition`
  比 GUID，入口处 `task.source` 虽未挂盘符但 GUID 已就绪，可正确判定"备份源 == 注册 WinRE
  宿主卷"（F3 条件）。非备份任务 / 布局不命中则不动注册位。
- **复用已验证的 `restore_original_winre`**：它与结尾还原是同一个函数（拷贝
  `original/Winre.wim` → 注册位置 + 校验 `ORIGINAL_WINRE_SHA256`），逻辑经过实机验证。

### 保留的安全网（关键）

- **捕获前的 `restore_clean_winre_before_capture` 保留不动**：入口提前还原后，它会在捕获前
  走「已匹配干净原件」早返回（`sha256_file == expected`），是幂等兜底——万一入口被绕过
  （如断电后续跑跳过了入口段），捕获前这道仍能拦住脏镜像。
- **结尾 `restore_original_winre`（经 `WinreRestoreGuard` Drop）保留不动**：保证任务结束后
  活着的系统注册 WinRE 是干净的（方案 D 的"非侵入"设计：产品载荷只在任务窗口内临时注入）。

也就是说：入口还原 = 主动作（尽早闭合 F3 污染窗口）；捕获前 + 结尾 = 两道幂等兜底。

## 3. 验证

- ✅ 交叉编译 `cargo build --release --target aarch64-pc-windows-msvc -p backuprestore-cli`
  通过（仅 LNK4099 缺 PDB 警告，无害）。
- ✅ 核心 crate 单测 28 全绿（本次改动不触及 core）。
- ⏳ **VM 端到端实机验证未做**：完整备份周期（prepare → 重启 WinRE → 入口还原 → 捕获 →
  抽检镜像内 WIM 哈希 == `ORIGINAL_WINRE_SHA256`）建议在**下一次真实备份周期**顺带复测；
  改动是"把已验证安全的 `restore_original_winre` 调用提前"，风险低，但仍需一次实机闭环确认
  入口还原时序不干扰后续捕获。

## 4. 与既有结论的关系

- 阶段 2 文档（`docs/winre-task-wim-phase2-2026-09-27.md`）第 9.5 节描述的方案 D 逻辑不变，
  仅"还原干净 WinRE"的**触发时机**从"捕获前"扩展到"入口 + 捕获前 + 结尾"三道，干净窗口
  覆盖整个 WinRE 会话。
- `README.md` / `docs/project-status.md` 中"方案 D：捕获前换回干净原件"的描述建议补一句
  "已在 WinRE 入口提前执行"。

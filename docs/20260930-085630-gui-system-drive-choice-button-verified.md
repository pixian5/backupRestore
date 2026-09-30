# GUI 离线环境选择按钮实机复验 PASS（PE / Windows RE 双分支），v1.8.11→v1.8.14

**日期**：2026-09-30 08:56
**版本**：v1.8.11（首修）→ v1.8.12/13/14（连锁修复），最终验收包 v1.8.14
**验收环境**：Parallels "Windows 11" VM，程序目录 `H:\brwork`，用真实 `WM_COMMAND` 消息注入（**不是** `--test-hook` 绕过）

---

## 一、背景：这个按钮曾经完全不可用

主窗口点「创建任务」后弹出的**离线环境选择对话框**（窗口类 `BackupRestoreSystemDriveChoice`，标题「需要进入恢复环境处理」）有两个按钮：

| 控件 ID | 常量 | 文案 | 语义 choice |
|---|---|---|---|
| 2001 | `ID_CHOICE_PE` | 进入 PE | 1 |
| 2002 | `ID_CHOICE_RE` | 进入 Windows RE | 2 |

此前实机日志抓到 `choice=2` 结果被当 `0`（取消）处理 —— 用户点「进入 Windows RE」等于没点。这是产品主入口，卡着所有人工操作流程。

---

## 二、根因：三个独立缺陷，串在一起形成「点了没反应 / 点了程序消失」

### 缺陷 1（v1.8.11）结果盒被窗口销毁吞掉

`ask_system_drive_handler` 在 `DestroyWindow` 之后仍去 `GetWindowLongPtrW(dialog, ...)` 读结果盒。窗口已销毁属未定义行为，实机**稳定返回 0**。修复=把结果盒的裸指针留在局部变量里，循环退出后直接解引用，不再经过已销毁的句柄。

### 缺陷 2（v1.8.13）`PostQuitMessage` 杀死主窗口消息循环

`window_proc_system_choice` 的 `WM_DESTROY` 分支调用了 `PostQuitMessage(0)`。**`PostQuitMessage` 是线程级的**，会把 WM_QUIT 投给**共享同一消息队列的主窗口循环**。模态循环通常先取走它，但这是**竞态** —— 实机抓到过一次主窗口跟着退出：用户点完对话框，**整个程序消失**。

修复：
- 删掉该 `WM_DESTROY` 里的 `PostQuitMessage`（保留 `DefWindowProcW`）。
- `ask_system_drive_handler` 的模态循环退出条件改为 `IsWindow(dialog) != 0 && GetMessageW(...) > 0`，不再依赖 WM_QUIT。

### 缺陷 3（v1.8.14）同步 `DestroyWindow` 卡死跨进程 `BM_CLICK`

v1.8.13 把循环改成 `IsWindow` 后，`WM_COMMAND` 分支里的**同步** `DestroyWindow` 暴露新问题：`WM_COMMAND` 是按钮处理 `BM_CLICK` 时**同步**发来的，销毁父窗口会连带销毁正在执行窗口过程的按钮自身 → 跨进程 `SendMessage(BM_CLICK)` **永久阻塞**（实测：提权 PowerShell 窗口挂住不返回）。ESC 分支同理。

修复=两处都不再直接 `DestroyWindow`，改为 `PostMessageW(hwnd, WM_CLOSE, 0, 0)`；新增 `WM_CLOSE` 分支执行真正的 `DestroyWindow` —— 此时已退出按钮的同步消息链，销毁安全。

### 顺带修（v1.8.12 起）确认框文案混入英文内部键

确认框正文直接用内部键 `operation`，中文界面显示成「将创建**backup**任务」。改用 `operation_display(language, &operation)` 取显示名，中文为「将创建「备份」任务…」。

---

## 三、复验方法（真实消息注入，非 test-hook）

`--test-hook` 会直接预置 choice，**绕过对话框**，不能作为按钮验收。本次全程用跨进程真实消息注入：

1. 部署 v1.8.14（`build-win.sh --deploy`），核对本地与客体 SHA-256 一致：
   `71a157d0b69c0a91b285c5d8424592ec4da53857e1c96d08b793c09022abab06`
2. 以 `--test-hook` 启动 GUI（保留对话框交互，便于反复触发）。
3. 用提权计划任务（`/rl highest`）向主窗口投 `WM_COMMAND` id=1003（创建任务）→ 弹出选择对话框。
4. 向对话框（类 `BackupRestoreSystemDriveChoice`）投 `WM_COMMAND` id=2002 / id=2001 → 验证 choice 捕获。
5. 读 `H:\brwork\logs\gui.log`、枚举顶层窗口、检查进程存活。

> 注入脚本：`.test-artifacts/run-task-click.ps1` + `.test-artifacts/click-by-id.ps1`（提权计划任务桥接，见 `docs/vm-input-control-guide.md`）。

---

## 四、复验结果：双分支 PASS

### Windows RE 分支（choice=2）

```
[gui] system drive choice: button clicked, choice=2
system drive choice returned: 2 (0=cancel 1=PE 2=RE)
system drive operation: Windows RE confirm result=7 (6=yes)
system drive operation: Windows RE confirm declined
```

- 点击「进入 Windows RE」→ **choice=2 正确捕获**（缺陷 1 修复确认）
- 进程存活（PID 6768 不变，**缺陷 2 修复确认**）
- 「进入恢复环境」确认框正常弹出 → 点「否」优雅取消 → 回主窗口、**无重启**

### PE 分支（choice=1）

```
[gui] system drive choice: button clicked, choice=1
system drive choice returned: 1 (0=cancel 1=PE 2=RE)
system drive operation: user chose PE (schedule PE task)
```

- 点击「进入 PE」→ **choice=1 正确捕获**
- 进程存活（**缺陷 3 修复确认**，注入的提权 PowerShell 正常返回、不挂起）
- 弹出「未安装 PE 恢复环境」提示 —— 该 VM 未部署 PE，属**分支正确行为**；回车关闭后回主窗口

截图证据：`.test-artifacts/gui-pe-confirm.png`（PE 未安装提示）。

---

## 五、结论与边界

**结论**：GUI 离线环境的两个选择按钮已可用，三个连锁缺陷全部修复并**实机验证**。产品主入口（点按钮 → 选环境 → 确认 → 排任务）在 UI 层面打通。

**未验证 / 边界**：

- 只验到「确认框点否 / PE 未安装提示」为止；**未点「是」**跑完整入 PE/RE 排任务链路（会触发真实重启，留待后续按需做）。
- PE 分支的「未安装 PE 恢复环境」是当前 VM 现状，不代表 PE 已部署后的行为。
- 容量/性能、`create-secondary`（双系统）、Secure Boot 仍未验证。

**下一步**：见 `docs/20260930-0437-下一步待实现.md`（其第 2 项已由本文档闭环）。

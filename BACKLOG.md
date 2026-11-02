---
topics: [backlog]
doc_kind: note
created: 2026-10-13
---

# Feature Roadmap

## 界面（下一步待办）

**B-2691 ✅ 已完成（2.6.96）主窗口按实测宽度布局（18 号字）**：`Secondary boot name` 100/189（−89）、`Refresh environment` 120/178（−58）、`Cancel pending reboot` 150/198（−48）、`Language` 55/85（−30）、`Refresh task status` 140/160（−20）、`Browse…` 60/76（−16）、`Keep latest N` 100/115（−15）（中文 18 号另有三处）。这些控件仍是「按 14 号字调出的固定宽度」——2.6.84 的按实测算宽只铺到了设置窗口与两个辅助窗口。修法：改用 `measure_text_width(text).max(固定下限)` 并同步挪动相邻控件；判据现成（`dlg-fit.ps1` 量裁切、`overlap-all.ps1` 量重叠），验收组合 = `中文18 / 英文18 × 100% / 200%` 全 0。


> **Rules**: Only active Features (idea/spec/in-progress/review). Move to done after completion.
> Details in `docs/features/Fxxx-*.md`.

| ID | Name | Status | Owner | Link |
|----|------|--------|-------|------|
| B-2691 | 主窗口 18 号字下 7 处固定宽度控件被切（英文）/ 2 处（中文） | done (2.6.96) | | `docs/20261030-2045-100%缩放实测跑通并揪出18号字8处被切-v2.6.91-DS41F.md` |
| B-2692 | **提权探测改成常驻助手（根治孤儿控制台窗口）** | **done (2026-11-01)** | | `docs/20261101-0005-提权探测改常驻助手-根治828个孤儿窗口-DS41F.md` |
| B-2693 | **`choice_dialogs` 的两个弹窗按钮宽度改为实测（v2.7.5）**：`BackupRestoreSystemDriveChoice` / `BackupRestoreInvalidPathChoice`（系统盘处理选择、镜像路径非法）消除固定 400px 写死宽度，改为按候选按钮文字 measure_text_width 实测算宽 | done (2.7.5) | | `docs/20261102-1715-弹窗实测算宽与去unwrap安全锁-v2.7.5-Gemini.md` |
| B-2694 | **探测调用本身会漏控制台宿主（根治完成）**：提权交互（click/aux-open/aux-close/errcode/launch）与 ini 写入全流程改走 SYSTEM 通道，由 Session 1 桌面常驻 helper 代理继承控制台执行，彻底根除高频调用 `--current-user powershell` 引发的孤儿 conhost/OpenConsole 泄漏 | **done (2026-11-02)** | | `docs/20261102-2330-探测泄漏宿主彻底根治闭环B2694-Gemini.md` |
| B-2695 | **`recovery_progress` 引入 SafeLock 消除 42 处 Mutex unwrap**：锁毒化时取 `into_inner()` 保障数据继续读取，防止辅助线程异常导致主进程连锁 panic 与 `bcd_rollback` 挂死 | done (2.7.5) | | `docs/20261102-1715-弹窗实测算宽与去unwrap安全锁-v2.7.5-Gemini.md` |
| B-2696 | **全仓跨平台编译告警与死代码门控清理**：消除 `unused_mut`，规范 macOS 视角 `cfg_attr` 死代码标注，实现跨平台 0 warning 与 404 项单测全绿 | done (2.7.5) | | `docs/20261102-1715-弹窗实测算宽与去unwrap安全锁-v2.7.5-Gemini.md` |
| B-2697 | **PE 安装镜像结构完整性门禁**：新增 `BR-PE-059`（缺 winpeshl.ini）与 `BR-PE-060`（启动目标程序不存在）错误码与弹窗拦截，杜绝坏镜像开机死循环 | done (2.7.5) | | `docs/20261102-1715-弹窗实测算宽与去unwrap安全锁-v2.7.5-Gemini.md` |
| B-2698 | **准备阶段纯计算逻辑与 I/O 解耦抽离**：拆出 `prepare_plan.rs` 纯逻辑模块并补齐 17 个跨平台单测，加固核心路径健壮性 | done (2.7.5) | | `docs/20261102-1905-准备阶段纯逻辑抽离与单测加固-v2.7.5-Gemini.md` |
| B-2699 | **`native_gui.rs` 巨石解耦（七批次彻底重构）**：抽离 `task_admin_window.rs`、`pe_desktop.rs`、`pe_installer.rs`、`operation_tabs.rs`、`drive_selection.rs`、`task_executor.rs`、`environment.rs` 共 11 个自治子模块，代码自 11,067 行降至 **2,540 行**（净减 8,527 行，降幅 **77.1%**） | done (2.7.5) | | `docs/20261102-1915`、`2020`、`2030`、`2040`、`2045`、`2100` |

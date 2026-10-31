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
| B-2693 | **`choice_dialogs` 的两个弹窗从未被量过，且按钮宽度写死**：`BackupRestoreSystemDriveChoice` / `BackupRestoreInvalidPathChoice`（系统盘处理选择、镜像路径非法）不属于 GUI 矩阵的四个辅助窗口，`created` 时用的也是固定宽高。触发它们需要构造特定错误流（镜像路径非法 / 系统盘需人工选择），比开一个模态窗贵。**下一步**：先用 `listwin` + `dlg-fit` 在真实错误流下量一次 18 号字，再决定是否把按钮改成实测宽度 | idea | | `docs/20261101-0005`/`经验/05`（判据覆盖面的教训） |
| B-2694 | **探测调用本身会漏控制台宿主（根因已定，危害已压到 1/20）**：受控实验证明**每调用一次** `prlctl exec --current-user powershell` 就漏一对 `conhost`/`OpenConsole`（提权与否一样，走 SYSTEM 通道不漏）。**已做的两级缓解**：① `run.sh` 收尾用 SYSTEM 通道清扫；② `batch.ps1` 把每个组合的多次探测合并成一次调用（主窗口 7→4、辅助窗口 5→3），泄漏量 完整矩阵 **65→3**、`--quick` **41→0**。**仍待根治**：让 prlctl 不起控制台（GUI 子系统客户端/别的通道），否则提权那三次（click/aux-open/aux-close）仍是每次一个 | in-progress | | `docs/20261101-0005`、`docs/20261101-0130` |

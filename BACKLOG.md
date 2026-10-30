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

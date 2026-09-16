# BackupRestore 文档索引

当前执行基线： [current-status-2026-09-16.md](current-status-2026-09-16.md)。
WinRE/WinPE 入口隔离与载荷闭包修复见 [winre-payload-contract-2026-09-16.md](winre-payload-contract-2026-09-16.md)。
`current-progress-2026-09-13.md` 及更早的按日期进度文档是历史测试记录，不能覆盖当前执行基线。

按下面顺序阅读，避免把离线检查误解成 Windows 恢复已验收：

1. [project-status.md](project-status.md)：当前进度、用户决策、已完成与未完成项目、继续条件；
2. [verification-matrix.md](verification-matrix.md)：每一项需求的代码、离线和实机证据；
3. [continuation-handoff.md](continuation-handoff.md)：后续开发者或 AI 的实现边界和执行顺序；
4. [implementation-notes.md](implementation-notes.md)：实现细节、历史问题和验证边界；
5. [windows-build.md](windows-build.md)：Windows ARM64/x64 构建和产物规则。
6. [systematic-audit-2026-08-25.md](systematic-audit-2026-08-25.md)：全项目架构审计、已修复根因和当前验证边界。
7. [testing-plan.md](testing-plan.md)：备份/还原的分层测试方案与用例矩阵（离线、准备、WinRE 实机、故障注入）。
8. [winre-payload-contract-2026-09-16.md](winre-payload-contract-2026-09-16.md)：WinRE 与 WinPE 模板分离、载荷契约和回归防线。

产品需求原文位于仓库根目录的 `Windows 一键系统备份还原 V1——完整开发需求.md`。发生冲突时，优先级为：用户最新指令、源代码与已保存的实机证据、本文档；静态检查或历史记录不能替代实机证据。

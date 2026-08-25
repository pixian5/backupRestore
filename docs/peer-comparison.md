# 与同类备份还原程序的能力差距

对比基线选择 Windows 原生 DISM/WinRE、Clonezilla/Rescuezilla、Macrium Reflect、Veeam Agent、Acronis 类产品。对比只描述当前仓库代码和已取得的 ARM64 实机证据，不把设计目标当成已实现功能。

| 能力 | 当前程序 | 同类成熟工具通常具备 | 当前差距/风险 |
|---|---|---|---|
| 指定分区备份还原 | 已支持任意卷身份；U: fixture 非 C 真实 Capture 和 Index 2 Apply 已成功 | 支持整盘、分区、磁盘布局和多盘映射 | 还没有整盘分区表/EFI/MSR/Recovery 一体化镜像；EFI 需单独处理 |
| WIM 多索引 | 已生成两个索引并实际使用 Index 2 还原成功 | 多索引浏览、导出、删除、合并、索引元数据管理 | 备份本身总是生成单索引；GUI 不能创建/合并多索引，只能选择已有索引 |
| 增量/差异备份 | 不在 V1 | Macrium/Veeam/Acronis 普遍支持全量+增量/差异和保留策略 | 每次都是完整 Capture，空间和时间成本高 |
| 在线一致性 | 依赖离线 WinRE Capture | VSS 快照、应用一致性、SQL/Exchange 处理 | 当前不提供 Windows 在线热备；运行中应用一致性未覆盖 |
| 镜像格式 | WIM + SHA-256 + metadata | WIM、专有块镜像、压缩分卷、去重 | 无块级稀疏/去重、无镜像分卷、无密码加密 |
| 加密与密钥 | BitLocker 开启时拒绝，不自动改状态 | 支持 BitLocker 识别、密钥托管、加密镜像 | 没有镜像加密、密钥托管、恢复密钥流程 |
| 目标容量与身份 | GUID/磁盘/分区/偏移/容量校验，目标不足拒绝 | 还会提供缩小/扩展分区、布局迁移 | 不支持自动调整分区大小，不支持跨磁盘布局规划 |
| WinRE/启动 | 自动 `winpeshl.ini` 入口、一次性启动、原始 WinRE hash 恢复；独立 EFI E: + Z: 临时挂载实测 | 自带可启动 Rescue Media、启动修复、PXE/USB | 尚未证明从独立 EFI 实际启动进入 U:；双系统 `/addlast` 尚未实机验收 |
| 断电/失败恢复 | 状态机、`.partial`、WinRE/BCD 快照、阶段续跑代码 | 更成熟的事务日志、自动回滚和介质自检 | 每个阶段断电注入、BCD 失败回滚尚未完整实机验证 |
| 驱动/硬件迁移 | 目标必须是相同 Windows 语义的 NTFS 分区 | 通常支持异机还原、驱动注入、HAL/启动关键驱动处理 | 没有异机还原和驱动注入；当前主要验证同 VM/同架构 |
| 网络/云目标 | 不在 V1，目标是本地绝对路径 | SMB、NFS、S3、云存储、远程代理 | 无网络备份、断点传输、远端凭据和带宽控制 |
| 调度/保留 | 不支持 | 计划任务、保留点、自动清理、通知 | 只能手动启动 PowerShell/GUI |
| 恢复粒度 | 整个目标分区 Apply；可选 Index | 文件级恢复、单文件浏览、粒度化还原 | 无文件级浏览/恢复，无快照挂载 |
| 校验与演练 | SHA-256、DISM `/CheckIntegrity`、metadata、payload hash | 介质自检、定期恢复演练、可启动性验证 | 缺少自动化定期演练和从目标 EFI 实际启动回归 |
| UI/运维 | Rust Win32 原生 GUI、多语言、管理员自提升、模式字段显隐 | 更丰富的向导、队列、进度、取消、历史任务 | 当前长操作主要由隐藏 PowerShell/WinRE 执行，GUI 进度/取消/历史浏览仍较弱 |

## 当前结论

当前程序已经超过“仅能调用 DISM 的脚本”范围：它具备卷 GUID 安全模型、WinRE 自动入口、WIM/metadata/hash、状态机、原始 WinRE/BCD 保护、多语言 GUI，并已在 ARM64 VM 完成非 C 分区真实备份和多索引 Index 2 还原。

它仍不是 Macrium/Veeam/Acronis 级别的通用灾备产品。最重要的未覆盖项按优先级是：

1. 从独立 EFI 实际启动 U:，并完成正常 Windows 回归；
2. `create-secondary`/`/addlast` 双系统真实验收；
3. 断电注入、BCDBoot 失败回滚、BitLocker 拒绝和身份篡改的实机矩阵；
4. 完整磁盘/分区布局备份、增量/差异、VSS 在线一致性；
5. 异机驱动注入、网络/云目标、镜像加密、调度和文件级恢复。


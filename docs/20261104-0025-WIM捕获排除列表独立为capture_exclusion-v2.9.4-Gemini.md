# WIM捕获排除列表独立为capture_exclusion (v2.9.4)

> **时点**：2026-11-04 00:25  
> **版本**：v2.9.3 -> **v2.9.4**（改源代码升版 +0.0.1）  
> **模型**：Gemini  
> **主题**：架构演进第九阶段之核心库 WIM 备份排除列表独立为 `capture_exclusion.rs`，`backuprestore-core/src/lib.rs` 缩减至 2,842 行  

---

## 一、背景与动机

在 Windows 系统的 WIM 镜像捕获与在线备份过程中，排除策略直接决定了备份速度、镜像体积与后续还原成功率：
1. **系统根级可再生文件与独占锁文件**：如 `\$Recycle.Bin`、`\Temp`、`\Windows\SoftwareDistribution\Download` 以及 Parallels 卷根独占占位符 `\Mac disk`（若不排除会导致还原时以 0x80070020 破坏性中断）；
2. **根级自引用联接点动态排查**：如 `Documents and Settings` 指向 `Users` 的联接点（通过 `is_reparse_point` 判断，避免捕获时子树遍历文件数凭空翻倍导致预扫假死）；
3. **真实用户浏览器媒体与网络缓存**：递归枚举真实用户目录下的 Chrome、Edge、Firefox、Opera 等多 profile 缓存目录并输出字面路径（DISM 不支持中间通配符）；
4. **程序自身部署件与半成品任务过滤**：同卷部署时排除 5 件套部署文件，并过滤未完成的孤儿任务工作树（`pending_task_names`，避免 1.4GB 临时 WinRE 镜像进包与幽灵弹窗）。

此前这整套约 350 行的生产代码及 270 行的配套单测全部堆叠在 `crates/backuprestore-core/src/lib.rs` 底部，导致核心模型库体积膨胀至 3,463 行。

本轮重构将 WIM 捕获排除生成子系统及其完整测试集合抽离为自治子模块 `capture_exclusion.rs`。

---

## 二、核心改动与重构设计

### 1. 新建 `crates/backuprestore-core/src/capture_exclusion.rs`（453 行）

- **排除常量与路径辅助**：
  - `SELF_DEPLOY_FILES`：自身部署件文件名数组；
  - `LEGACY_PACKAGE_DIR`：历史分发包目录排除项；
  - `volume_key` / `relative_to_volume_root`：跨平台统一卷键提取与卷内相对路径规范化。
- **重解析点探测与任务过滤**：
  - `is_reparse_point`：基于 `FILE_ATTRIBUTE_REPARSE_POINT` 的目录联接点/符号链接判断；
  - `task_stage_is_terminal` / `pending_task_names`：识别未处于终态的工作区任务目录，实现从源头阻断半成品任务进入备份镜像。
- **核心排除配置生成器**：
  - `build_capture_exclusions`：组装包含固定项、联接点、浏览器缓存、自身部署件与孤儿任务的 `[ExclusionList]` 配置；
  - `write_capture_exclusion_config`：统一写入临时 `BackupRestore-exclusions.ini` 供 CLI、GUI 在线与 PE 三大入口共用。
- **单测自治归位**：
  - 迁入 6 组完整覆盖排除项判定、联接点排查、无用户 fallback、配置文件落地、同卷/跨卷部署件过滤以及未完成任务排除的单元测试。

### 2. 精简 `crates/backuprestore-core/src/lib.rs`

- 注册并导出：`pub mod capture_exclusion; pub use capture_exclusion::*;`，对外契约完全保持兼容；
- 从 `lib.rs` 中完整剥离 350 行生产代码与 270 行单测代码；
- `crates/backuprestore-core/src/lib.rs` 行数由 3,463 行降至 **2,842 行**（单次净减 621 行）。

---

## 三、质量门禁与验证

1. **单元测试与跨平台编译**：
   - 宿主机 `cargo test --workspace`：405 项单测全部 PASS；
   - 宿主机 `./build-win.sh`：交叉编译 0 warning 0 error；
   - 代码风格 `cargo clippy --all-targets`：0 warnings。
2. **源码静态守卫**：
   - `./tools/test/source-guard.sh`：`RESULT=PASS`，白名单与租约逃生口受控。
3. **陈旧断言扫描**：
   - `./tools/doc/stale-claims.sh` 结果全绿。
4. **生产代码健壮性**：
   - 严守生产代码 **0 unwrap & 0 expect** 纪律。

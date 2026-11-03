# WIM元数据与wimlib进度解析独立为wim_parse (v2.9.6)

> **时点**：2026-11-04 00:50  
> **版本**：v2.9.5 -> **v2.9.6**（改源代码升版 +0.0.1）  
> **模型**：Gemini  
> **主题**：架构演进第十一阶段之 WIM 镜像元数据与 wimlib 还原进度解析独立为 `wim_parse.rs`，`text_parsing.rs` 缩减至 2,810 行  

---

## 一、背景与动机

在 BackupRestore 的系统捕获与镜像还原执行体系中，针对 DISM 与 wimlib-imagex 工具链的输出解析扮演着感知控制中枢的角色：
1. **DISM WIM 镜像元数据反序列化（`parse_wim_images`）**：支持数组、单对象或 `images` 包装层等多种不规则 JSON 结构，并兼容多版本大小写字段命名（`ImageIndex`/`imageIndex`/`index`、`ImageSize`/`imageSize`、`ImageName`/`name`）；
2. **WIM 捕获全卷预扫与写入阶段心跳判据**：根据 DISM 早期固定 208 字节 WIM 头判定 `pre_scan` 与 `writing` 阶段，输出结构化固定 ASCII 心跳与停滞行（`capture_heartbeat_line` / `capture_stall_line` / `capture_summary_line`），杜绝全盘预扫静默引发假死误判；
3. **还原自检行结构化判据（`restore_selfcheck_line`）**：以非翻译固定 ASCII 形式记录前后目标卷序列号与 `ntoskrnl` 存在性；
4. **wimlib 还原三阶段进度折算与中文可读详情**：将 wimlib-imagex 的 `Creating files` (10%)、`Extracting file data` (80%)、`Applying metadata to files` (10%) 三阶段单调折算为 0–100 全局进度条，并生成友好的中文进度详情。

此前，上述逻辑及其实测夹具全量回放断言（`replay_probe`）全部堆积在 `crates/backuprestore-cli/src/text_parsing.rs` 内部，导致该文件高达 3,477 行，成为全仓行数最多的单文件巨石。

本轮重构将 WIM 镜像解析、阶段心跳判据、wimlib 进度折算以及夹具回放测试系统独立解耦为 `wim_parse.rs`。

---

## 二、核心改动与重构设计

### 1. 新建 `crates/backuprestore-cli/src/wim_parse.rs`（676 行）

- **WIM 镜像元数据解析**：
  - `WimImageInfo` 数据结构；
  - `wim_image_items` 与 `parse_wim_images`，完整保留多格式、多别名解析与防御校验；
  - 通用 JSON 文本提取工具 `json_text`。
- **捕获与还原结构化判据行**：
  - `CAPTURE_HEADER_BYTES_MAX`（1024 字节阈值）；
  - `capture_phase`、`capture_heartbeat_line`、`capture_stall_line`、`capture_summary_line`；
  - `restore_selfcheck_line`。
- **wimlib 还原进度与阶段折算**：
  - `WimlibPhase` 三阶段枚举及其标签；
  - `parse_wimlib_progress`、`wimlib_overall_percent`、`wimlib_detail_line`、`parse_percent`；
  - 人类可读容量格式化 `format_bytes`。
- **单测自治与入库夹具回放**：
  - 迁入 11 组 WIM 元数据、还原自检、捕获心跳、阶段单调折算及百分比解析单测；
  - 迁入 `mod replay_probe` 测试套件，包含 9 KB 真实夹具回放与单调性校验（`replay_wimlib_progress_fixture_is_monotone_and_completes`）及全量取证用例（`replay_real_wimlib_log`）。

### 2. 精简 `crates/backuprestore-cli/src/text_parsing.rs`

- 注册并导出：`pub(crate) use crate::wim_parse::*;`，对所有既有调用点（GUI、CLI、recovery）保持零改动兼容；
- 在 `main.rs` 中注册 `mod wim_parse;`；
- 从 `text_parsing.rs` 中完整移除 315 行生产代码与 352 行单测代码；
- `crates/backuprestore-cli/src/text_parsing.rs` 行数由 3,477 行降至 **2,810 行**（单次净减 667 行）。

---

## 三、质量门禁与验证

1. **单元测试与跨平台编译**：
   - 宿主机 `cargo test --workspace`：406 项单测全部 PASS；
   - 宿主机 `./build-win.sh`：交叉编译 0 warning 0 error；
   - 代码风格 `cargo clippy --all-targets`：0 warnings。
2. **源码静态守卫**：
   - `./tools/test/source-guard.sh`：`RESULT=PASS`。
3. **陈旧断言扫描**：
   - `./tools/doc/stale-claims.sh`：0 命中全绿。
4. **生产代码健壮性**：
   - 严守生产代码 **0 unwrap & 0 expect** 铁律。

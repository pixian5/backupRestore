# P1-c 备份文件管理 UI 立项实现：GUI 对话框 + CLI 双命令 + 全链路实机验收（2026-10-25 07:40，Step-5-Preview）

> 用户拍板「P1-c 备份文件管理 UI 立项」后的实现轮。需求依据：需求 §3 明确 `BackupRestore.exe`
> 负责「管理备份文件」。部署 SHA `87270388…`，HEAD 见文末。

---

## 1. 做了什么

### 1.1 核心模块 `crates/backuprestore-cli/src/image_admin.rs`（新建）

- `list_images(dir, log)`：列目录下 WIM + 每索引元数据（DISM 取真实索引号，sidecar 取名称/时间/源卷/大小；sidecar 缺失不致命）。
- `delete_oldest_indexes(image, keep, log)`：删最旧索引只保留最新 N。与 capture 保留逻辑**同源**
  （DISM `/Delete-Image /Index:1` 循环 + 逐次回读索引集合，不一致即中止并留半删现场），
  删完一次性 `renumber_sidecars`（内部重算 sha256/尺寸并逐份回读）。
- `clamp_keep(keep, total)`：纯函数，钳到 [1, total]；total=0 返回 0（坏镜像走「无可删」）。
  模块按项目惯例门控：纯逻辑跨平台可测，DISM 相关函数 `#[cfg(windows)]`。

### 1.2 CLI 两个新命令（main.rs）

- `list-images [dir]`（缺省 `H:\BRImages`，不存在退回程序目录）：人类可读清单 / `--json` 每镜像一行。
- `delete-old-indexes <wim> --keep <n>`：破坏性；删除前后各记日志，日志落
  `BackupRestore-image-admin-*`（`online_log` 白名单新增该标记）。

### 1.3 GUI「备份文件管理…」对话框（native_gui.rs）

- 顶栏新按钮（ID 1009，中英双语「备份文件管理…」/ "Backup files..."），进 `layout_operation`
  右对齐链条与 tooltip（2026-10-17 教训：顶栏按钮漏重排会压掉邻接控件）。
- 模态对话框（沿用设置窗口的「自定义类 + 静态量传值 + 嵌套消息循环」模式）：
  镜像下拉框 + 只读索引清单 + 「保留最近 N 个」输入（默认=当前索引数，即默认不删）
  + 刷新 / 删除旧索引… / 打开所在文件夹 / 关闭。
- 异步形态与在线任务一致：`OperationGate` 忙标志（独立于 `ONLINE_BUSY`）+ worker 线程
  （`catch_unwind` 兜底 panic）+ `WM_APP_IMAGE_LIST_DONE` / `WM_APP_IMAGE_DELETE_DONE` 回窗。
- 删除前**原生确认框逐项列出将删的索引**（号+名称）+ 不可撤销警示；删完弹结果并自动刷新列表。
- 新错误码 `BR-IMG-007`（删除后回读不一致；清单已用 `gen-error-codes.py` 重生成，213 条）。

## 2. 验收（全过）

### 2.1 构建与单测

- mac 单测 **249+45**（含 `clamp_keep` 2 条新测试）；`./build-win.sh` 交叉编译通过（3,133,440 B）。
- VM 内部署 SHA `87270388…`（两 exe 一致，wimlib 两件不变）。

### 2.2 CLI 链路（T: 5G 测试卷）

1. 三次在线备份造 3 索引镜像（内容 one/two/three）→ 均 success；
2. `list-images` 正确列出 3 索引（名称/时间/大小/源卷 `T: (7E8FD9A9) 磁盘0/分区5`）；
3. `delete-old-indexes --keep 1` → success，wim-info 剩 1 索引；
4. 还原该索引 → success 且内容 = **three**（最旧两个已删、最新保留；副档哈希预检通过）。

### 2.3 GUI 全点击链（br-s1.sh + br-click-id.ps1，新 helper `br-set-dlg-edit.ps1` 设输入框）

截图证据 `.test-artifacts/p1c-*.png`：

1. 点「备份文件管理…」→ 对话框打开，列表/按钮/提示行布局正确；
2. 再造 two 索引 → 点刷新 → 列表变 3 索引，「保留最近」自动填 3；
3. keep=3 点删除 → 「无需删除」分支正确；
4. 设 keep=1 点删除 → **确认框逐项列出将删的 1、2 号索引** + 不可撤销警示；
5. 点是 → 完成提示「已删除 2 个旧索引，保留最新 1 个」→ **列表自动刷新为 1 索引**；
6. 还原验证内容 = **five**（最新索引保留）。
7. 关对话框；T: 已 diskpart 清回空卷（新序列号 `CCF1-1AEE`）。

## 3. 现场与产物

- 测试镜像 `H:\BRImages\admin-test-20261025.wim`（17.9 KiB，1 索引）按「证据全留」保留。
- GUI 运行中（v2.6.50 新部署）；default 指原系统；快照 0 层。
- 改动文件：`image_admin.rs`（新）、`main.rs`（mod+两命令+usage）、`online_operation.rs`
  （dism/indexes 放行 pub(crate)）、`error_code.rs`（+BR-IMG-007）、`online_log.rs`（+image-admin 标记）、
  `native_gui.rs`（按钮+对话框+布局+语言+审计+tooltip）、`docs/ERROR-CODES.md`（重生成为 213 条）、
  `tools/win-clicker/br-set-dlg-edit.ps1`（新 helper）。

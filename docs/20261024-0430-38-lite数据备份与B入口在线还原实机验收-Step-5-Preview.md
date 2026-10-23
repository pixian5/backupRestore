# 20261024-0430-38-lite 数据备份与 B 入口在线还原实机验收-Step-5-Preview

> 版本时点：v2.6.32（HEAD `88295ab` 起手，本轮无代码改动，纯实机验收 + 文档）。
> 接手来源：`docs/20261024-0350-接手AI交接说明-v2.6.32-Step-5-Preview.md` §4.2 步骤 1–4。

## 0. 结论

**#38-lite 两步全部实机通过，零缺陷**：

| 步 | 内容 | 结果 |
| --- | --- | --- |
| ① | T: 数据备份（在线）→ `H:\BRImages\br-tdata-2629.wim` | ✅ success，索引名 `Windows (T:) 20261024` 符合降级命名规则 |
| ② | B 入口在线还原到 T:（勾选格式化） | ✅ success，哨兵重现 + 序列号 `E880494B→143E3564` + 载荷字节级一致 + BCD 未动 |

崩溃判据收口：EventID 1000 零新增（最新仍 2026-09-30）、WerFault 未运行。
环境终态：T: 已按「测试分区用后即删」纪律清回 5GB 空卷（序列号 `3CF3-1261`）。

## 1. ① 数据备份（在线，GUI 真实点击）

操作路径（全部为真实 GUI 点击，无 CLI 替跑）：

1. `tools/win-clicker/br-s1.sh _launch-gui-backup-2629.ps1`（`--tab 0` + 环境变量预置镜像路径）。
2. `br-combo-apply.ps1 -Source 4` 把源卷切到 `T:`（下拉索引 4，已回读验证：
   0=C: 1=D: 2=F: 3=H: 4=T:；脚本发 `CB_SETCURSEL` + `WM_COMMAND/CBN_SELCHANGE`，界面与内部状态一致）。
3. `br-click-id.ps1 -Id 1003` 点「创建任务」→ 弹「破坏性确认」→ `_click-dialog.ps1` 点「是」。
4. 等「执行任务完成」框 → `_click-dlgbtn.ps1 -BtnId 2` 点「确定」。

任务日志铁证（`H:\brwork\logs\BackupRestore-online-7012-1792786114975585600\Online.log`）：

```
STEP 2/4 捕获镜像：DISM /Capture-Image /CaptureDir:T:\ /Name:Windows (T:) 20261024
           /CheckIntegrity /Compress:fast → 100%，退出码 0
Get-WimInfo：Index 1, Name "Windows (T:) 20261024"
STEP 3/4：sha256=4722ef4c374b6015af1630ea7680117f4437d6873e179d9a23afb5aa4c7434fe
          bytes=857352 保留=1 删除=0
STEP 4/4：镜像及副档已发布并回读；候选目录已清理
```

CLI 交叉验证（`wim-info`）：索引名 / sha256 / `capturedUsedBytes=127774720` /
`minimumTargetSize=5368709120`（= T: 5GB）/ `programVersion=2.6.32` 全部吻合。

**索引名规则闭环**：源卷 T: 非系统盘 → `default_backup_index_name` 走降级分支
`Windows (T:) 20261024`，与 §4.2 判据一致。

## 2. ② B 入口在线还原（GUI 真实点击）

操作路径：

1. `_launch-gui-bentry-2629.ps1`（`--tab 2` = 还原数据到非 C 盘 + 预置上一步 WIM 路径）。
   启动即自动 `read_image`：WIM 索引下拉回读出 `索引 1 | Windows (T:) 20261024 | … | 大小 837.3 KiB`。
2. `br-combo-apply.ps1 -Target 4` 目标卷切 `T:`（目标下拉 5 项：0=C: 1=D: 2=F: 3=H: 4=T:）。
3. 「格式化目标分区」默认勾选、「在线执行（不重启）」默认选中——均截图确认。
4. `br-click-id.ps1 -Id 1003` → 「破坏性确认」（模式=还原数据到非 C 盘 / 目标卷 T: / WIM 索引 1）
   → 点「是」→ 「执行任务完成」→ 点「确定」。

任务日志铁证（`BackupRestore-online-6896-1792786954857160800\Online.log`）：

```
STEP 1/4 校验参数和卷身份；VolumeIdentity{ volume_serial: "E880494B", drive_letter: T, … }
STEP 2/4 格式化目标卷（diskpart quick）并校验：
         格式化校验通过：序列号 E880494B -> 143E3564，分区身份不变
STEP 2.5/4 应用镜像（数据卷已按选项格式化后写入）：
         DISM /Apply-Image /ImageFile:H:\BRImages\br-tdata-2629.wim /Index:1
              /ApplyDir:\\?\Volume{950bd694-…}\ /CheckIntegrity /Verify → 100%，退出码 0
STEP 3/4 回读目标卷身份；DISM 已执行完整性和应用校验
STEP 4/4 完成在线还原（已格式化后写入）；还未执行系统引导修复
操作完成：命令、回读和副档簿记均已通过
```

## 3. 判据逐条核对（§4.2 第 3 条）

| 判据 | 结果 | 证据 |
| --- | --- | --- |
| 还原后 T: 上哨兵**重新出现** | ✅ `BR-BENTRY-SENTINEL.txt`（42B，内容 `BR-BENTRY-SENTINEL data-restore-canary`） | `bentry-post-20261024.txt` |
| T: 序列号**改变** | ✅ `E880494B → 143E3564`（格式化生效） | 日志「格式化校验通过」+ `Get-CimInstance` 实测 |
| 任务 success/100 | ✅ 四步全过 + 「操作完成」 | `Online.log` |
| 100MB 载荷字节级一致 | ✅ sha `20492A4D…` 还原前后相同 | `bentry-pre/post-20261024.txt` |
| BR38Data 完整 | ✅ marker-pre/file1/file2/sub/nested 全在 | `bentry-post-20261024.txt` |
| BCD default/displayorder **不受影响** | ✅ `default {current}`；`displayorder {current} + {177f3ea3-…}`；device partition=S: 不变 | `bentry-final.txt` |
| 崩溃判据 | ✅ EventID 1000 零新增（最新 09-30）、WerFault 未运行 | `bentry-final.txt` |

「镜像带回旧状态」效应按预期工作：T: 备份时含哨兵/载荷/BR38Data，format+apply 后
这些内容随镜像落回 T:——这正是判据「哨兵重新出现」的机理。

## 4. 过程中发现并解决的三个问题（均为工具层，非产品缺陷）

1. **GUI 启动脚本预置镜像路径「看似失效」**：dump 读出的镜像路径/索引名/源卷详情全为空，
   一度误判为预置失败。根因：`_dump-now.ps1` 的 `Txt()` 用 `SendMessage(WM_GETTEXTLENGTH)`
   取长度（成功）+ `GetWindowTextW` 取文本（跨进程被 UIPI 静默拦成空串）。
   探针（`editprobe2.ps1`）三方法对照证实：`WM_GETTEXT` 消息读到正确文本、两个 API 返回空。
   **修法**：dump 类脚本统一改用 `SendMessage(WM_GETTEXT)`；本次验证改以截图 + 消息探针为准。
   界面状态实际全部正确（源卷 T:、路径、索引名 `Windows (T:) 20261024`）。
2. **确认框「是」按钮点击偶发不生效**：第一次 `PostMessage(BM_CLICK)` 后对话框仍在
   （无新日志、无新任务目录、T: 序列号未变可证伪「已执行」）；重点一次即通过。
   疑为对话框刚创建尚未进入消息循环时投递丢失。**对策**：点击后必须用
   「dump 对话框标题 / 查新任务目录 / 查 T: 序列号」三选一复验，不能以点击回执为准。
3. **完成框「确定」按钮 ID 是 2 不是 1**：本程序的 `show_message` 完成框按钮为 `IDCANCEL(2)`，
   `GetDlgItem(dlg, 1)` 返回空。新增 `_click-dlgbtn.ps1 -BtnId N` 按 ID 点击 + 只认
   「可见且有标题」的 #32770 窗口，避免枚举到隐藏 IME 类窗口误点。

另记两笔历史信息（接手时从 gui.log 考古所得，避免重复踩）：
10-23 22:49 / 23:26 两次 `create task; operation=backup` 后日志全无后续——
当时镜像路径为空、流程卡在空路径引导框；本次预置成功后不再复现。

## 5. 收尾与现场

- **T: 清理**：按「测试分区用后即删」纪律 `Format-Volume -DriveLetter T -FileSystem NTFS`，
  清回 5GB 空卷（序列号 `3CF3-1261`，仅剩 `System Volume Information`）。
  证据已全部落盘（文本 + 8 张截图），无需保留现场数据。
- **两个历史启动项结案**（§5.2）：`{c76d3c5a}` / `{ef55d218}` 指向活动卷 `C:` 的
  `C:\Recovery\WindowsRE\Winre.wim` 确实存在、无任何对象引用、当前恢复链在 `Y:`——
  **不是悬空条目，删除无收益还有风险，结论：不动、结案**。
- **H: 卷**：`br-2629-20261023.wim`（37.8GiB 系统镜像）+ `br-tdata-2629.wim`（837KB）
  两个镜像保留，`H:` 余 ~85GB。

## 6. 证据目录

`.test-artifacts/recon-38-lite-20261024/`：

- 备份链：`vm-after-create/vm-after-yes/vm-done-20261024.png`（确认框→完成后→干净界面）
- 还原链：`bentry-pre-20261024.txt`（还原前 T: 全量状态）/ `bentry-post-20261024.txt`
  （还原后 + 完整任务日志）/ `bentry-final.txt`（BCD + 崩溃判据）/
  `vm-bentry…vm-final-20261024.png`（B 入口页→目标卷 T:→确认框→进行→终态）
- 工具探针：`editprobe2.ps1`（文本读取三方法对照）、`_yes-check.ps1`、`_click-dlgbtn.ps1`、
  `_dump-dlgch.ps1`（均已随 `tools/win-clicker/` 提交）
- 脚本：`bentry-pre/post/final.ps1`、`format-t.txt`、`prev-log*.ps1`

## 7. 待办状态（接手文档 §5 对账）

- §5.1 需用户拍板三项（#38 全链专场是否彻底不做 / tasks 证据目录边界 / TaskStore 是否搬离 C:）——
  **未动，仍等用户**。
- §5.2：#38-lite 第②步 **已结清（本文档）**；历史启动项 **已结案（不动）**；
  T: 5GB 空卷 **已按纪律清理**。
- §5.3 已结清项维持原结论。

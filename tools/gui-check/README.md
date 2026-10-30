# tools/gui-check —— GUI 几何 / 文字 / 提示覆盖判据

## 这是什么

一条命令跑完 GUI 的自动化判据，输出**一行结论**：

```
GUI_CHECK combos=40 clipped=0 overlaps=0 below=0 untipped=0 skipped=0 candidates_min=37 errcode_na=0 errcode_missing=0
RESULT=PASS
```

## 为什么需要它

GUI 占了全项目 **22%** 的代码（`native_gui.rs` 11,010 行），但只有 **3 个单测**；
而且它是 `cfg(windows)`，在 macOS 上**编译为零信息**。历史上这一批缺陷全靠临时脚本发现：

| 缺陷 | 当时怎么发现的 |
|---|---|
| 字体静默回退（写雅黑、显示宋体） | 临时写 `GetTextFaceW` 探针 |
| 18 号字下 8 处控件被切 | 临时写量字宽脚本 |
| 按钮行改顺序排布后压住 PE 按钮（重叠 208×56） | 临时写重叠判据 |
| 气泡不折行、安装 PE 页漏挂提示 | 临时写 `TTM_GETMAXTIPWIDTH` 探针 + 审计 |

这些脚本用完就散在 `.test-artifacts/`（已 gitignore）——**下次得重造**。
本目录就是把它们固定下来：脚本进仓库、判据可复跑、结论可对比。

## 判据（都是外部独立测量，不读程序自陈）

| # | 判据 | 来源脚本 | 变红意味着 |
|---|---|---|---|
| 1 | 控件两两矩形相交数 | `ps/overlap-all.ps1` | 两个控件压在了一起 |
| 2 | 单行文字被裁切数 / 多行块高度不够 | `ps/dlg-fit.ps1` | 文字显示不全（独立重建字体量宽） |
| 3 | 最低控件是否越出窗口底边 | `ps/fit-bottom.ps1` | 有控件在窗口外，点不到 |
| 4 | 提示覆盖：候选数 / 未挂数 | `ps/tipaudit.ps1` | `untipped>0` 有控件没提示 |
| 5 | 错误码在界面上是否真可见 | `ps/errcode.ps1` | 任务记录有码、界面却看不到（**真缺陷**） |

★ **判据 4 必须同时看候选数**：只报 `untipped=0` 可能是「枚举根本没找到控件」的空集通过
（2.6.98 第一版就吃了这个亏）。`candidates_min>0` 且 `untipped=0` 才算真覆盖。

★ **判据 5 只在「任务记录里真的有错误码」时才算数**：`RECORD_CODE=none`（没有失败任务）
记为 `errcode_na=1`，**不算失败**；`RECORD_CODE=BR-...` 却 `VERDICT=MISSING` 才是真缺陷。
运行前提：`H:\brwork\last-task.json` 要指向一个**带 `errorCodeText` 的失败任务**
（VM 上现成的夹具是任务 `a3359054-1edc-4467-a459-57b3543a900a`，码 `BR-PE-039`）。

## 用法

```bash
tools/gui-check/run.sh              # 全矩阵：5 页签 × 中英 × 12/14/16/18 号（约 17 分钟）
tools/gui-check/run.sh --quick      # 冒烟：1 个组合 + 错误码判据（约 1 分钟）
BR_VM='Windows 11' tools/gui-check/run.sh
```

前置条件：VM 里 `H:\brwork\BackupRestore.exe` 已部署（即 `build-win.sh --deploy` 跑过）。

## ★ 凡是要「点」的判据，必须提权跑（UIPI）

程序是 **提权** 运行的。未提权进程向更高完整性级别的窗口投递鼠标输入会被系统
**静默丢弃**：`SetCursorPos` / `mouse_event` 全部正常返回，坐标也算得对，但点击
根本没送达。

2026-10-30 的实测证据（用「可见控件 id 集合」的前后差异判断页签到底切没切）：

| 点击方式 | 结果 |
|---|---|
| 未提权 | **DIFF=0**：一个页签都没切，而脚本照样报 `CLICK=OK` |
| 提权（经 `ps/runas.ps1`） | 点 1101 → 集合 17 个变 11 个（**DIFF=14**）；点 1106 → 14 个（**DIFF=11**） |

所以 `run.sh` 里 `click.ps1` 走 `vmps_elev`（提权执行）；只读取窗口信息的脚本
（`overlap-all` / `dlg-fit` / `fit-bottom` / `tipaudit`）不受 UIPI 影响，保持普通调用。

> 同一个坑的另一种形态：跨权限**发消息**（如 `SendMessage(TTM_GETMAXTIPWIDTH)`）
> 也会被静默丢弃并返回 0，曾让我误判成「设置没生效」。见 `经验/05`。

## 退出码

| 码 | 含义 |
|---|---|
| 0 | 全绿（`RESULT=PASS`） |
| 1 | 有真判据变红（`RESULT=FAIL`） |
| 2 | 通道抽风，有组合记 SKIP（`RESULT=SKIP`）—— **不能当通过** |

★ 为什么区分 1 和 2：`prlctl` 通道有约 1/8 的概率不执行命令。
「读不到」和「判据不满足」必须分开报——把通道抽风记成红，会训练人忽略红判据；
记成绿，则是假通过。这里一律记 `SKIP` 并以退出码 2 明确昭示。

## ★★ 读 EDIT 控件必须发 `WM_GETTEXT`，不能用 `GetWindowTextW`

这是本目录**最贵的一条**。跨进程 `GetWindowTextW` 拿到的是窗口管理器缓存的文本，
这份缓存只有 `DefWindowProc` 处理 `WM_SETTEXT` 时才更新；而 **EDIT 控件自己处理
`WM_SETTEXT`、不走 `DefWindowProc`**，于是跨进程读到的永远是**控件创建时的文本**。

实测：状态框（EDIT，id 1300）屏幕上明明是 8 行、含 `Error code: BR-PE-039`，
`GetWindowTextW` 却稳定返回 **31 字符**（创建时的中文占位提示语）——
据此连判三轮「界面藏了错误码」，差点去改产品代码。
改用 `SendMessage(WM_GETTEXT)` 后读到 **343 字符、含码**，并与整屏截图肉眼复核一致。

- 判据里对应 `GetTextLive()`；输出中 `STATUS_CACHED_len=31` 是**故意保留**的对照组，
  两个数字打架就说明有人改回去了。
- `Static` / `Button` 的缓存**会**被更新（实测读到切语言后的英文），所以 `dlg-fit.ps1` 不受影响。
- 判据 5 点「刷新任务状态」会顺手弹出**手工模态**的「任务与证据」窗口并禁用主窗口，
  所以脚本必须先关遗留模态、读完再关，并把 `MAIN_ENABLED=` 打进输出。
- 点击一律 `PostMessage(BM_CLICK)` + 轮询日志条数：`SendMessage` 在嵌套模态循环里会卡几十秒。

## 三个实现细节（都是踩过的坑）

1. **不依赖 `X:` 盘符**：那个映射每次 VM 重启都会丢。脚本用 `robocopy \\Mac\backupRestore\...`
   把 `ps/` 同步到 `H:\brwork\gui-check` 再本地执行。
2. **每个脚本必须打出 `__END__`，且 `run.sh` 还要校验结果值**：`__END__` 只证明
   「命令被执行」，结果值才证明「事情做成了」。2026-10-30 的教训：`click.ps1` 因为
   类名 `GC` 撞上 .NET 的 `System.GC` 而全程失败，输出 `CLICK=NO_WINDOW ... __END__`
   —— 旧逻辑看到 `__END__` 就判定「跑过了」，于是 40 个组合其实全在测同一个页签。
   现在每一步都校验具体结果（`COMBO font=` / `PID=<数字>` / `CLICK=OK` /
   `OVERLAPS=` / `SINGLE_LINE_CLIPPED=` / `MARGIN=`），缺一个就记 `SKIP`——
   **绝不允许落到默认值 0 去「通过」**，那会把「没量到」伪装成「量到 0 个问题」。

## 覆盖范围（如实声明）

- 已覆盖：主窗口 **5 个页签 × 中英 × 12/14/16/18 号**；每个组合量重叠 / 裁切 / 底边 / 提示覆盖；
  末尾再跑一次错误码可见性（判据 5）。
- **未覆盖**：中间档缩放（125/150/175%），这台 VM 只能在 100% 与 200% 之间切；
  设置对话框、备份文件管理、查看日志三个独立窗口（各自的一轮判据还没收编）；
  视觉观感（颜色、对齐是否好看）——判据只管「有没有被切/压/漏」。

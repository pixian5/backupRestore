# 启动配置数据历史 osloader 条目清理（12 个残留对象，重启验证通过）

- 时间：2026-10-23 20:35–20:52
- 模型：DeepSeek-V4.1-Flash
- 背景：交接文档 §8.2 里「启动配置数据历史恢复环境 / 预安装环境 osloader 条目清理」
  长期挂账（备案待清理，历史残留非回归）。本轮按 `AGENTS.md` 的空间纪律执行：
  **先 `bcdedit /export` 导出回滚材料，再动手**。
- 结论：删掉 **12 个对象**（6 个历史 `Windows Recovery Environment` 启动加载器 + 6 个
  只被各自加载器引用的设备选项对象），启动管理器默认项 / 显示顺序 / 恢复对象**零改动**，
  重启后原系统正常启动。**本轮未改源码，版本号保持 2.6.30。**

## 一、为什么这些是残留（判定依据）

枚举全量启动配置数据（548 个对象）后，启动加载器共 11 个。其中 6 个满足**三条同时成立**：

| 条目 | 设备指向 | 全文出现次数 |
|---|---|---|
| `{411d20e9-…}` | `ramdisk=[F:]\Recovery\WindowsRE\Winre.wim` | 1 |
| `{47803045-…}` | `ramdisk=[F:]\Recovery\WindowsRE\Winre.wim` | 1 |
| `{65760e6c-…}` | `ramdisk=[F:]\Recovery\WindowsRE\Winre.wim` | 1 |
| `{6a9a5eff-…}` | `ramdisk=[F:]\Recovery\WindowsRE\Winre.wim` | 1 |
| `{78af450d-…}` | `ramdisk=[F:]\Recovery\WindowsRE\Winre.wim` | 1 |
| `{a220bfb7-…}` | `ramdisk=[F:]\Recovery\WindowsRE\Winre.wim` | 1 |

1. **指向 `F:`**——而 `F:` 是本项目历轮测试的还原目标卷，现在已被格式化成 350 GB **全空卷**，
   文件根本不存在（可证明的悬空条目）；
2. **全文只出现 1 次**（只有自己的 `标识符`）——不被 `default` / `displayorder` /
   `toolsdisplayorder` / `bootsequence` / 任何对象的 `recoverysequence` / `resumeobject` 引用；
3. 都是 `Windows Recovery Environment` 型 ramdisk 加载器——来自历轮「还原到 F:」时
   在目标卷上注册恢复环境留下的痕迹，属本项目测试残留。

配套的 6 个设备选项对象（`{411d20ea}` 等）各出现 3 次 = 自身 `标识符` + 所属加载器的
`device` + `osdevice`，即**只被对应加载器引用**，随之一起删除。

## 二、保留下来的 5 个（一个都没动）

| 条目 | 说明 | 为什么留 |
|---|---|---|
| `{077a3904-…}` | 本次还原的 Windows（`C:`） | 原系统本体，当前默认项 |
| `{177f3ea3-…}` | Windows PE (BackupRestore) 内存启动 | 安装的预安装环境，在 `displayorder` 里 |
| `{264caa29-…}` | Windows Recovery Environment（`ramdisk=[Y:]\Recovery\…`） | 被 `{077a3904}.recoverysequence` 引用，是原系统的恢复链 |
| `{c76d3c5a-…}` | Windows Recovery Environment（`ramdisk=[C:]\Recovery\…`） | 被恢复对象 `{c46f6055-…}` 的 `recoverysequence` 引用；**且指向活动系统卷 C:**，需单独分析 |
| `{ef55d218-…}` | Windows Recovery Environment（`ramdisk=[C:]\Recovery\…`） | 虽无引用，但同样指向活动系统卷，本轮不动 |

## 三、执行与验证

| 步 | 动作 | 结果 |
|---|---|---|
| 1 | `bcdedit /export` 到 `.test-artifacts/bcd-rollback-before-osloader-cleanup.bcd` | 503,808 B，导出成功 |
| 2 | `bcdedit /delete {GUID} /f` × 12（6 加载器 + 6 设备选项） | 12 × 「操作成功完成」 |
| 3 | 回读：12 个 GUID 在全量枚举里逐个比对 | **12/12 `GONE`** |
| 4 | 回读启动管理器 | `default={current}`、`resumeobject={077a3903}`、`displayorder={current}+{177f3ea3}`、`timeout=10`——**与清理前逐字一致** |
| 5 | 启动加载器计数 | **11 → 5**（只少了那 6 个指向 `F:` 的） |
| 6 | `shutdown /r` 重启 | 原系统正常启动进桌面（`C:` = `BCD4-EAAB`，图形界面 v2.6.30） |
| 7 | `bcdedit /export` 清理后状态 | `bcd-rollback-after-osloader-cleanup.bcd`，导出成功 |

## 四、回滚方法（万一）

```cmd
bcdedit /import \\Mac\backupRestore\.test-artifacts\bcd-rollback-before-osloader-cleanup.bcd
```

回滚材料保留最近两份（`before` / `after` 各一份，均在 `.test-artifacts/`，**不入库**）。

## 五、本轮新增的环境坑（第三次撞见，已固化）

**从 macOS 写进虚拟机的 `.cmd` 必须 CRLF**——本轮清理脚本第一版又是 LF 写出，
`for %%G in ( … )` 的多行列表被 cmd 拼成一行，报错是「`0-23` 不是内部或外部命令」
（GUID 片段）与「`6` 不是…」。这正是昨天写进经验 02 / 交接文档 §6 的那条，
**今天又犯了一次**，说明必须靠工具而不是记忆：以后写 `.cmd` 一律
`open(p,'wb').write(s.replace('\n','\r\n').encode('ascii'))`。

另一个观察：`.cmd` 里 `bcdedit /enum osloader /v | find /c "标识符"` 报
「FIND: 参数格式不正确」——管道后的 `find` 在该上下文里不可靠；
计数改用 PowerShell 的 `-match` 全量比对（本轮 `DELETED_CHECK` 用的就是这个，可靠）。

## 六、剩余待分析（本轮有意不动）

- `{c76d3c5a}` / `{ef55d218}`：都指向**活动系统卷 `C:`** 的 `Recovery\WindowsRE\Winre.wim`。
  前者还被一个恢复对象引用。要清得先确认 `C:\Recovery\WindowsRE\Winre.wim` 是否存在、
  以及那个恢复对象是否也是残留——涉及活动系统卷，值得单开一轮，不混在批量清理里。
- 恢复对象（`从休眠状态恢复`）里可能也有同类残留，同理单开一轮。

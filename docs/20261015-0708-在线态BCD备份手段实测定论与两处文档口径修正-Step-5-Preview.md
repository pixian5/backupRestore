# 在线态 BCD 备份手段实测定论：`/export` 提权即可用 + 两处文档口径修正（2026-10-15 07:08）

> 模型：Step-5-Preview
> 触发：用户质疑 AGENTS.md 自相矛盾——第 23 行「优先 `bcdedit /export`」vs 第 9-12 行「`/export` 在线态也不可用，抄三行才是实际可行手段」，要求实测定论。
> 环境：Parallels `Windows 11` 在线态（Windows 11 26200.9550），ESP 挂载 `S:\`，主系统 default `{41326e72}`、resumeobject `{41326e71}`、displayorder = `{2a9bc012}` + default。

## 0. 一句话结论

**AGENTS.md 第 23 行（优先 `/export`）是对的；第 9-12 行「在线态不可用、只能抄三行」不准确，实为「未提权会话才不可用」。**
实测矩阵：提权会话 `/export` **成功**（651,264 字节完整备份）；未提权会话连 `bcdedit /enum {bootmgr} /v` 只读都被拒；ESP 原文件被内核独占，任何身份拷不走（旧记载此条准确）。

## 1. 实测矩阵

| 手段 | SYSTEM（`prlctl exec`，即提权通道） | 未提权用户（`prlctl exec --current-user`，账户 `x`） |
| --- | --- | --- |
| `bcdedit /export C:\xxx.bcd`（普通路径） | ✅ **成功** 651,264 字节 | ❌「拒绝访问」 |
| `bcdedit /enum {bootmgr} /v`（抄三行） | ✅ 正常 | ❌「无法打开启动配置数据存储。拒绝访问」 |
| `copy /B S:\EFI\Microsoft\Boot\BCD …`（ESP 原文件） | ❌「另一个程序正在使用这个文件」 | ❌「拒绝访问」 |

## 2. 逐条证据

1. **`/export` 成功且文件有效**：
   `bcdedit /export C:\br-bcdtest\export-test.bcd` → 退出码 0，`dir` 确认 651,264 字节；
   `bcdedit /store C:\br-bcdtest\export-test.bcd /enum {bootmgr}` 离线回读完整——
   `default {41326e72}` / `resumeobject {41326e71}` / `displayorder {2a9bc012}+default` / `timeout 10` 全在。
   **可回导、可校验，优于手抄三行。**
2. **未提权失败卡在 BCD 存储权限，不是目标路径**：用户身份先后导出到
   `C:\br-bcdtest\…`（SYSTEM 建的目录）与 `C:\Users\x\…`（自己的用户目录）**均报「拒绝访问」**——
   排除了目标文件 ACL 因素。同身份 `bcdedit /enum {bootmgr} /v` 同样被拒（只读也不行）。
   10-12 记载的「报‘请求的操作需要提升’」与本次「拒绝访问」是同一权限现象的**不同文案**
   （26200 中文版措辞变了），性质一致：需要提权。
3. **ESP 原文件锁是内核级、与身份无关**：SYSTEM `copy /B` 报「另一个程序正在使用」，
   用户身份报「拒绝访问」——文件句柄被系统独占，**任何身份拷不走**，此条旧记载仍成立。
4. **本项目常态就是提权通道**：`prlctl exec` 默认以 `nt authority\system` 执行（`whoami` 实证），
   所以项目语境下 `/export` 从来都该可用——**旧记载「在线态不可用」是把未提权会话的结果误当成了 `bcdedit` 的限制**。

## 3. 定论（回滚手段优先级）

1. **首选 `bcdedit /export <普通路径>`**：在线态提权即可，文件完整、可离线校验、必要时 `/store … /import` 可回导。
2. **兜底：提权抄 `{bootmgr}` 的 `default`/`displayorder`/`resumeobject` 三项值**（`bcdedit /enum {bootmgr} /v`，改坏 `bcdedit /set` 写回）。
   提权下可用，但只能手抄、无完整性校验，仅 `/export` 失败时用。
3. **ESP 原文件直接复制永远不可行**（内核独占锁），不要再尝试四种拷贝姿势。
4. **未提权会话什么都做不了**（枚举都被拒）——VM 交互会话里操作前先确认提权，或直接用宿主 `prlctl exec`（SYSTEM）。

## 4. 附带新坑：中文 cmd 管道会静默漏字段行

`prlctl exec … cmd /c "bcdedit /enum {bootmgr} /v | findstr /R default displayorder resumeobject"`
只命中 `default`，`displayorder`/`resumeobject` 报「无法显示」——**字段名本身是英文**
（`chcp 437` 后输出确认三个字段全在），问题是 GBK 代码页下字节流与 findstr 模式不匹配。
**根治：管道前先 `chcp 437`**。此前项目记录的「中文 bcdedit 字段名本地化」坑（`20261012-1510`）
在 prlctl 简体会话下未复现，真正会咬人的是管道代码页。

## 5. 本轮文档修正记录

| 文件 | 修正 |
| --- | --- |
| `AGENTS.md` 第 9-12 行 | 「`/export` 在线态不可用、抄三行才是实际可行手段」→「`/export` 提权首选 / 抄三行兜底 / ESP 原文件锁死」 |
| `AGENTS.md` 第 23 行 | **不变**（原口径正确，现与空间纪律口径对齐） |
| `README.md` 第 26 行「最新实机」 | `20261012-1935` 的「根因：bootmgr 只解析系统卷 C: 盘符…暂存改放系统卷」已作废 → 改引 `20261014-0003` 定论（变量是 ramdisk 规格形态：设备路径形式系统卷/数据卷都能引导） |
| `README.md`【下一步待实现】 | 「拍快照 + 抄 BCD 三行」→「拍快照 + `bcdedit /export` 备份（首选，`prlctl exec` SYSTEM 即可）+ 抄三行兜底」 |
| `docs/20261015-0021-…交接说明` §5 | **勘误（未改原文，时点快照）**：其「引导类改动前抄 `{bootmgr}` 三行；`bcdedit /export`/`/import` 联机态不可靠」的后半句已被本节推翻——`/export` 联机态（提权）可靠且应优先 |

## 6. 遗留

- 测试目录 `C:\br-bcdtest` 已删、用户目录两个测试残留已清，VM 无残留。
- 阶段 5 执行时按 §3 优先级做回滚准备：`prlctl exec "Windows 11" cmd /c "bcdedit /export <共享盘路径>\bcd-<时间戳>.bcd"`。

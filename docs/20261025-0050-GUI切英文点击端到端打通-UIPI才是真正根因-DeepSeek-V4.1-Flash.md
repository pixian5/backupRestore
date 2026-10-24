# GUI 切英文点击端到端打通：真正的拦路虎是 UIPI，不是会话隔离

- **时间**：2026-10-25 00:50
- **版本**：v2.6.41（基线 commit `c496473`，产品代码本轮**零改动**，只补测试工具与结论）
- **模型**：DeepSeek-V4.1-Flash
- **性质**：实机验证 + 工具补齐（任务 #121 结清）

## 一句话结论

`#121`（GUI 点一下语言下拉框切英文）**打通了**。之前记在案的两个「坑」里，**会话隔离根本不是问题**（`prlctl exec --current-user` 直接就在 session 1），`ExecutionPolicy` 也只是挡脚本文件的执行、`-Command` 完全不受限。真正的原因只有一个：**GUI 是提权进程，我们的通道是标准用户，UIPI 把所有 `SendMessage` 静默吃掉**。

---

## 三重铁证

切英文（`target=1`）与切回中文（`target=0`）双向都跑过：

```
RUN_START ... target=1
SELF_ELEVATED=True          <- 关键：这次是提权进程
SELF_SESSION=1
SELF_USER=x
BTN=66388 COMBO=66276
BEFORE=[刷新环境]
COUNT=2 CURSEL=0            <- 组合框确实有两项
ITEM[0]=[中文]
ITEM[1]=[English]
SETCURSEL target=1 now=1
AFTER=[Refresh environment]  <- 界面文案真的换了
RUN_END
```

反向：`BEFORE=[Refresh environment]` → `AFTER=[刷新环境]`。

截图：整窗英文（`Operation / Backup / Restore C: / Restore data / Add 2nd system (non-C:) / Install PE / Cancel pending reboot… / Settings… / Language: English / Source (backup) / Refresh environment / Create task / Refresh task status`）。

`gui.log`：

```
GUI action: control=ID_LANGUAGE id=1005 notification=0x1
GUI action completed: language changed
```

`notification=0x1` 就是 `CBN_SELCHANGE`，正是这次点击（对，是 `PostMessage` 模拟的，走的是同一条消息路径）。

---

## 之前为什么一直「跑不通」

### ① 假象：组合框看起来是空的

用标准用户身份读控件状态：

```
COMBO id=1005 rect=975,83,1155,103 style=0x50010203 count=0 cursel=0 text=[]
```

`count=0`，而界面上明明白白显示「中文」。差一点就当成「产品 bug：语言下拉框从来没填过项」。

**真相**：`SendMessageTimeout(CB_GETCOUNT)` 返回 0 且 `GetLastError=5 ERROR_ACCESS_DENIED`。UIPI 拒绝低完整性进程向高完整性窗口发消息，返回值一律是 0 —— 看起来像「空」，其实是「被拒」。

同一时刻量两侧完整性：

```
GUI_elevated=True
SELF_elevated=False
```

换提权执行体后同一个查询变成 `COUNT=2`、`ITEM[0]=[中文]`、`ITEM[1]=[English]`。

**教训**：跨进程读控件状态前，先确认双方完整性级别。`COUNT=0` + 界面明明有内容 = 先怀疑 UIPI，不要怀疑产品。

### ② `text=[]` 是另一回事，别混为一谈

标准用户读按钮 `GetWindowText` **能**拿到「刷新环境」，读 combo 却是空。

因为 `GetWindowText` 跨进程只可靠地拿**窗口标题（caption，存在内核）**；控件内部的文本要发 `WM_GETTEXT`，那就回到 UIPI 的问题上了。按钮文字是 caption 所以读得到，combo 的选中文本不是。

### ③ 输出丢失被误读成「脚本没跑」

早前的记录写着「session 1 执行返回 True 但无输出」。真相：`CreateProcessAsUser` 起的进程**不继承 stdout**，脚本里 `Write-Output` 全丢。日志里只剩启动器那句 `launch_result=True`，看上去像脚本没执行。

**改法**：执行体一律把结果 `Add-Content` 写文件（`H:\brwork\lang-switch-result.txt`），不依赖 stdout。

### ④ 两个具体的 API 坑

- `WTSQueryUserToken` + `TokenLinkedToken` 提权：`DuplicateTokenEx` 报 `err=6 ERROR_INVALID_HANDLE`。改为**直接借 GUI 进程自己的令牌**（`OpenProcess` + `OpenProcessToken` + `DuplicateTokenEx`），SYSTEM 通道有 `SeDebugPrivilege`，一次成功。
- 老 helper 把 `lpCurrentDirectory` 写成 `C:\Users\Public\backupRestore-package` —— **该目录 v2.6.34 起已不存在**，`CreateProcessAsUser` 会以 `267 ERROR_DIRECTORY` 失败。改用 `C:\Windows\Temp`。

---

## 现在的工具（两件套 + 一个判定器）

| 文件 | 作用 |
|---|---|
| `tools/run-elevated-session1.ps1` | **启动器**。SYSTEM 通道执行；找 GUI 进程借其提权令牌，`CreateProcessAsUser` 到 session 1 起执行体。日志 `H:\brwork\lang-switch-launch.txt` |
| `tools/switch-gui-language.ps1` | **执行体**。读 `COUNT`/`CURSEL`/各项文本 → `CB_SETCURSEL` → `PostMessage(WM_COMMAND, notification=CBN_SELCHANGE)` → 回读按钮文案。结果 `H:\brwork\lang-switch-result.txt`；目标索引从 `H:\brwork\lang-switch-target.txt` 读（默认 1 = 英文，0 = 中文） |
| `tools/_diag-uipi.ps1` | **判定器**。一条命令看出「GUI 是否提权 / 我是否提权 / SendMessage 是否被 UIPI 拦（err=5）」 |

用法：

```bash
# 1) 需要时先判定是谁的问题
prlctl exec "Windows 11" --current-user powershell -NoProfile -Command \
  'iex (Get-Content -Raw -Encoding UTF8 "\\Mac\Home\code\backupRestore\tools\_diag-uipi.ps1")'

# 2) 切换（SYSTEM 通道，不要加 --current-user：SYSTEM 才有 SeDebugPrivilege 拿 GUI 令牌）
prlctl exec "Windows 11" powershell -NoProfile -Command \
  'iex (Get-Content -Raw -Encoding UTF8 "\\Mac\Home\code\backupRestore\tools\run-elevated-session1.ps1")'

# 3) 读结果
prlctl exec "Windows 11" cmd /d /c 'type H:\brwork\lang-switch-result.txt'
```

注意 `CB_SETCURSEL` **只改选中项、不发通知**，必须显式补一条 `CBN_SELCHANGE`，否则 `apply_language` 根本不会被调用——这是纯发 `CB_SETCURSEL` 时「界面不变」的第二个陷阱。

---

## 顺带确认的会话事实

```
prlctl exec "Windows 11" --current-user powershell -NoProfile -Command '(Get-Process -Id $PID).SessionId'
→ 1
```

也就是说：**要进 session 1，不需要任何令牌借用脚本**。`--current-user` 就够了（用户 `x`，session 1）。令牌借用在本轮的唯一用途是**提权**，与 session 无关。这条订正了 `20261024-2355` 里「会话隔离卡住 #121」的说法。

---

## 没做的事（如实写）

- **界面语言仍不持久化**：每次启动 GUI 都是中文（`CB_SETCURSEL 0` 写死在初始化里）。这是功能缺口，不是 bug，用户没提过要求，本轮不动。
- 「切英文后日志也跟着切英文」的 i18n 效果，本轮**没有**重新实测——它在 `20261024-2345` 已用真实还原实测过（`DISM start (wait limit unlimited)` / `Operation complete`）。本轮只证明「点击真的能切界面」。

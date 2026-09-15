# 操作通道备忘（如何驱动测试 VM「Windows 11」）

> 结论先行：**Parallels 会丢弃 macOS 合成的鼠标事件**（computer-use / AX 的
> `click` 派不到客户机），所以不能靠"点鼠标"驱 VM 里的 GUI。可靠通道是
> **键盘注入 + 命令行**，这也是 BackupRestore GUI 做成 v1.5.9 全键盘导航
> （Ctrl+字母切页 / Tab 遍历 / 方向键切单选 / 回车确认）的原因。

## 一、直接操作 VM 的（首选）

| 工具 | 作用 | 示例 |
|---|---|---|
| `prlctl send-key-event` | 宿主导入键盘/组合键到 VM，绕过鼠标限制 | `prlctl send-key-event "Windows 11" -k 36 -e press`（回车） |
| `tools/win-clicker/vmkey.sh` | 一行注入任意键，封装键码 | `./vmkey.sh ctrl+p` / `./vmkey.sh enter` / `./vmkey.sh f5` |
| `prlctl capture` | 对 VM 截屏，用于"看"画面 | `prlctl capture "Windows 11" --file out.png` |
| `prlctl exec` | 在 VM 里以指定用户执行命令 | `prlctl exec <UUID> --user x --password 1 cmd /c "..."` |
| `tools/win-clicker/run-in-session.ps1` | 在 VM 交互桌面（Session 1）启动程序，避开 Session 0 无桌面 | `powershell -File run-in-session.ps1 -SessionId 1 -Command "..."` |
| `tools/win-clicker/diag2.ps1` | 查当前焦点控件 ID / 类名（键盘导航定位用） | 输出到 `diag2.txt` |
| `tools/win-clicker/activate.ps1` | 把后台窗口拉回前台 | `powershell -File activate.ps1 -Title BackupRestore` |

## 二、`prlctl exec` 关键坑（勿再踩）

1. **语法**：必须不加 `--`，且名字含空格时用 **UUID**（不是名字）。
   正确：`prlctl exec <UUID> --user x --password 1 cmd /c "echo HI"`。
   加 `--` 或用含空格的名字 → 静默 exit 2。
2. **引号剥除**：`prlctl exec` 会剥掉命令里的双引号 → 不要依赖 `-Command "..."`
   里的双引号。可靠做法：把 PS 脚本写成本地文件，用 `-EncodedCommand`
   （UTF-16LE → base64）传参。
3. **exec 身份**：`p8b6\x` 的令牌是 **UAC 过滤**（Medium，Admins deny-only，
   仅 SeShutdown）。`bcdedit /enum`、`reagentc`、挂 `S:`、`schtasks /create
   /rl HIGHEST`、`runas` 全部失败，无法非交互提权。
   → 需要系统级 prep（BCD/RE/ESP）时用 **run-in-session / 计划任务 Highest**。

## 三、需要交互桌面（Session 1）的场景

`prlctl exec` 跑在 Session 0（无桌面），GUI 起不来也看不见。要把程序显示在
用户可见桌面，用 `run-in-session.ps1`（WTS QueryUserToken + CreateProcessAsUser，
需 SYSTEM/SeTcbPrivilege，能调）。

## 四、操作 macOS 本机的

- **Bash 命令行**（git / 构建 / 文件处理）
- **Read / Write / Edit / Grep** 文件工具
- **prlctl**（Parallels 官方 CLI：管 VM、挂卸载光驱、拍快照）
- 特殊情况才用 macOS 辅助功能（AX）桌面控制（有专门技能）。

## 五、测试本项目当前状态（2026-09-15）

- 测试 VM：`Windows 11`（UUID `{caee9cb3-bac7-41e2-85f2-32b3a7369114}`）。
- 目标链路：**真实 C:（当前运行系统卷）离线备份 → 篡改 → 还原**，带防假标记。
- 防假标记：`C:\br-test\marker.txt`（`CDRIVE-VERIFY-MARKER`）+
  `marker-nonce.txt`（`N3ZJ4Gav7MIlkAugWh8pXJ0W`）+ `keepfile.txt`。
- 备份目标：`E:\br-cdrive-v1.wim`（E: = br.hdd 独立盘，卷标 `br`）。
- 配置：`C:\br-test.json` 已指向 `backup / source C / image E:\br-cdrive-v1.wim`。
- 活跃安装：`C:\Users\Public\backupRestore-package\BackupRestore.exe`（v1.5.11）。

## 六、WinRE / BCD 修复通道（2026-09-16 新增）

当 VM 进不了 Windows（引导循环 / BCD 丢条目标）时，用 **Windows 安装 ISO 引导
WinPE + bcdboot 重建**（详见 [winre-bcd-loop-fix-2026-09-16.md](winre-bcd-loop-fix-2026-09-16.md)）：

1. ISO 在 `~/Downloads/Win11_25H2_Chinese_Simplified_Arm64_v2.iso`。
2. 停 VM → `--device-set cdrom0 --image <iso> --connect` → `--device-bootorder "cdrom0 hdd0"`。
3. 启动，出现 "Press any key..." 时 `./vmkey.sh space` 进安装程序。
4. `Shift+F10` 开 cmd（`./vmkey.sh shift+f10`）→ diskpart 把 ESP 挂为 S:。
5. `bcdboot C:\Windows /s S: /f UEFI`（**必须带 /s 和 /f UEFI**）重建 Windows 引导条目。
6. 验证 `bcdedit /store S:\EFI\Microsoft\Boot\BCD /enum {default}` 显示
   `device partition=C:` / `description Windows 11`。
7. 卸 ISO、恢复 `hdd0 cdrom0`，重启。

### 关键结论：WinRE RAMDISK 引导在 Parallels ARM 上不可行
- `ramdisk=[C:]\...\Winre.wim`(803MB) 引导恒失败（黑屏~2min 回落）；8GB 内存、
  注入版/原始 WIM、C:/Y: 位置均无效；但**从 ISO(UDF) 引导同体积 WinPE 成功**。
- 锁定为 **bootmgr 对该 VM 的 ntfs RAMDISK 大 WIM 读取失败**（环境限制，非软件 bug）。
- VM 内存当前为 **6GB**（8GB 曾诱发引导异常；如需改动 `--memsize` 须先停 VM）。
- `type_text.sh` 支持小写→大写自动 shift、含 `\` `/` `:` 等符号的路径输入。
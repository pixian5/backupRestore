# 开发残留清理与测试盘重建（2026-09-13）

## 一、本次清理内容

### 1. VM 内 C:\Users\Public 开发历史残留（已删）
- `backupRestore-package`、`backupRestore-package-v2~v11`（历史构建包，共 ~21MB）
- `backupRestore-src`、`backupRestore-src-v2~v11`（源码+构建产物备份，12 份 × ~1.5GB ≈ **18GB**）
- 保留：`backupRestore-package`（当前部署 v1.5.3，build-win.sh 固定覆盖目标）

### 2. 宿主侧测试盘（已删，释放 ~44GB 虚拟磁盘）
- `backuprestore-full-e2e.hdd`（24GB，磁盘 1：E:BREFI 测试 ESP + F:BRSource 源卷 + G: 目标 + H:BRImages 镜像）
- `backuprestore-blank-compare.hdd`（20GB，磁盘 2：P:/Q: 目标卷）
- 删除命令（先停 VM）：
  ```bash
  prlctl stop "Windows 11"
  prlctl set "Windows 11" --device-del hdd1 --destroy-image-force   # 快照引用导致普通删除失败，必须 force
  prlctl set "Windows 11" --device-del hdd2 --destroy-image-force
  prlctl start "Windows 11"
  ```

## 二、踩坑记录（重要）

1. **运行中的 VM 不能增删设备**：`--device-del` 报 "cannot be added or removed while the virtual machine is running"，必须先 `prlctl stop`。
2. **设备名不是 hdd1/hdd2**：`--device-del` 的参数既不是文件名也不是 boot order 名，直接报 "device does not exist"；**实际可用名就是 `hdd1`/`hdd2`**（boot order 显示名），之前失败是 VM 未停止导致的误导性报错。
3. **快照引用阻塞删除**：报 "This disk is in use by the following snapshots: ..."，普通 `--device-del --destroy-image` 失败；必须用 `--destroy-image-force`（同时清掉快照对该镜像的引用）。
4. **删除 src 目录失败的根因**：`Remove-Item -Recurse -Force` 和 `rd /s /q` 都会因目录内存在**保留设备名文件 `nul`**（C:\Users\Public\backupRestore-src\nul，0 字节）而失败，报"目录不是空的"；用 `\\?\` 前缀可绕过：
   ```powershell
   [System.IO.Directory]::Delete("\\?\C:\Users\Public\backupRestore-src", $true)
   ```
   （脚本：tools/cleanup-src.ps1）
5. **保留设备名 `nul` 的来源**：历史构建/测试脚本在 Windows 下执行时把输出重定向到了 `nul` 却忘了加 `>`（如 `copy ... nul` 而非 `>nul`），导致真创建了一个名为 nul 的文件。

## 三、后续更新是否产生残留？

**不会**。`build-win.sh` 已固定部署目标：
```
C:\Users\Public\backupRestore-package\BackupRestore.exe
```
每次 `./build-win.sh --deploy` 是 taskkill 旧进程 + copy 覆盖同一路径，不新建版本目录。
（目录名带 v12 只是命名习惯，不会累积 v13/v14。）

## 四、测试盘重建（5 分钟）

见 `tools/create-test-disks.md`：
1. 宿主：`prlctl set "Windows 11" --device-add hdd --image ... --size ...`（或 GUI 添加硬盘）
2. VM：运行 `tools/create-test-disks.ps1`（按型号匹配，自动 GPT 初始化 + 分区 + 卷标）
3. 重建结果：磁盘1 = MSR + E:BREFI(FAT32 ESP) + F:BRSource + G: + H:BRImages；磁盘2 = MSR + P: + Q:

## 五、部署目录去版本号（2026-09-13 追加）

确认 build-win.sh 固定覆盖部署后，不再产生版本号目录，故去掉 v12 字样：

- VM 内：`C:\Users\Public\backupRestore-package-v12` → **`C:\Users\Public\backupRestore-package`**（整目录重命名）
- `build-win.sh` 部署目标同步改为 `C:\Users\Public\backupRestore-package\BackupRestore.exe`
- 桌面快捷方式 `BackupRestore.lnk` 目标同步更新（WScript.Shell 改 TargetPath）
- 删除测试残留 `BR-new.exe`；删除 8 个历史计划任务
  （BackupRestoreInteractiveSessionProbe / LatestInteractive / RustUiInteractiveTest / UiAction / V084/V127/V128/V129Interactive）
- 验证：Session 1 启动 GUI v1.5.3 正常，C 盘可用 145 GiB
- **PE 启动项不依赖部署目录**：BCD 的 BackupRestore PE 指向 `ramdisk=[unknown]\BackupRestorePE\sources\boot.wim`，迁移无影响
- 本地 tools/*.ps1、docs 已批量替换 v12 路径（历史文档 current-progress-2026-09-11.md 保留原样）

## 六、快照清理（2026-09-13）

VM 全部 11 个历史测试快照已删除（叶→根逐个 `prlctl snapshot-delete -i <id>`）：
before-current-efi-secondary-test/boot-20260908、before-v131-*、v131/v132/v133-stage-fault-baseline(-2)、
before-bcd-menu-boot、before-pe-boot-test、pe-click-test-1——均为 9 月 PE/BCD/断电测试残留，功能已收口，无回退价值。
删除后快照树为空，释放快照差异文件空间（.pvm 237G→208G）。

## 七、C 盘 100G 占用分析与清理（2026-09-13）

### 清理前构成（已用 ~109GB / 254.5GB）

| 目录 | 大小 | 判断 |
|---|---|---|
| C:\BackupRestoreBuild | 39.2 GB | 🔴 Windows 侧历史构建产物（BackupRestore-windows-arm64-v1.1.1~v1.3.3 发布包 + package-v1.2.7~v1.3.1 + target*） |
| C:\Windows | 31 GB | 系统本体（正常，含 WinSxS） |
| C:\Program Files (x86) | 16 GB | Microsoft Visual Studio + Windows Kits（交叉编译 SDK 来源，保留） |
| C:\BRTest\tasks | 9 GB | 🔴 旧版测试任务存储（TaskStore root，当前程序已改用 exe 目录） |
| C:\Program Files | 8.2 GB | WindowsApps（UWP 系统应用）+ Google，保留 |
| C:\System Volume Information | 5.8 GB | 系统还原点，已清 |
| C:\Users\x | 5.5 GB | AppData 2G + .rustup 1.3G + Downloads 1.1G（构建缓存备份）+ .codex 0.8G |
| C:\ProgramData | 5.1 GB | Package Cache 3G（VS 安装缓存，保留）+ Microsoft |
| C:\BuildTools | 4 GB | VS Build Tools（构建用，保留） |
| C:\BackupRestorePE | 1.2 GB | PE media/work（PE 恢复功能，保留） |
| C:\brsrc | 0.1 GB | 🔴 源码构建缓存副本 |

### 已清理（释放 ~48GB）
- 删除：C:\BackupRestoreBuild（39.2G）、C:\BRTest（9G）、C:\brsrc（0.1G）、空测试目录（Quick Scan C/ESPRead/PEMount/pewimmount/PEVerify/DiskGenius_WinPE）
- 系统还原点：`vssadmin delete shadows /all /quiet`（SYSTEM 计划任务提权执行，SVI 5830MB→0）
- DISM：`Dism /online /Cleanup-Image /StartComponentCleanup`（SYSTEM 提权）
- 结果：C 盘可用 145.0 → **192.6 GB**（已用降至 ~61.9GB），GUI v1.5.3 验证正常

### 踩坑：prlctl exec 非管理员上下文
`vssadmin`/`DISM` 在线命令在 prlctl exec 下无权限（输出版权信息即未执行），
必须用 `schtasks /create /tr "<cmd>" /ru SYSTEM /rl HIGHEST /f` + `/run` 提权执行，用完删任务。

### 仍可考虑（未动，等确认）
- C:\Users\x\Downloads 下 backupRestore-cargo-v2~v11 / toolchain(-cache) / build-v2~v4（~1.1GB，早期 Windows 侧构建缓存的手动备份，mac 侧构建已稳定后可删）
- C:\Users\x\.codex（~0.8GB，Codex CLI 数据）
- AppData 内浏览器/系统缓存（保守未动）

## 八、孤儿快照层 .hds 回收（2026-09-13，最大头 206G→62G）

### 现象
快照树已空（GUI「无可用的快照」+ `prlctl snapshot-list` 空），但
`Windows 11.pvm/harddisk.hdd/` 下仍残留 **12 个 .hds 层文件 ≈206GB**
（含 135G 根层 + 各快照差异层），Parallels 配置页显示「快照 210.58GB / 可回收 0KB」，
`.pvm` 整体 208G。磁盘「回收...」按钮仅提示实时优化，不清理层。

### 根因
历史 11 个快照删除时 VM 处于运行态，Parallels 只删了快照树节点，
**差异层 .hds 文件未被合并/回收**，且 DiskDescriptor.xml 仍把 12 层全部
列为 Storage 链——快照树空但层无法再通过 GUI/CLI 删除，形成「孤儿层」。

### 解决（官方工具，非手动删文件）
```bash
# VM 必须停止
prlctl stop "Windows 11"
# 1) 合并所有快照层 → 单层（12 层 206G → 单层 139G）
"/Applications/Parallels Desktop.app/Contents/MacOS/prl_disk_tool" merge --hdd "/Users/x/Parallels/Windows 11.pvm/harddisk.hdd"
# 2) 实际回收已删数据块（139G → 62G = 实际数据量）
"/Applications/Parallels Desktop.app/Contents/MacOS/prl_disk_tool" compact --hdd "/Users/x/Parallels/Windows 11.pvm/harddisk.hdd"
```
结果：.pvm 208G→62G，宿主可用 176→254Gi，Windows 11 启动验证正常（桌面/此电脑/C 盘数据完整）。

### 踩坑记录
- `prl_disk_tool merge` 语法是 `--hdd <路径>`，**不带 -i**；带 `-i` 报「The 'i' option cannot be used for the 'merge' operation」。
- `compact -i --buildmap` 估算「无空洞」时 compact 无效；**必须先 merge 再 compact** 才有回收空间（merge 后 Parallels 才标出「可回收 83GB」）。
- GUI 快照管理「删除」按钮需先选中左侧快照卡片，但卡片是自定义绘制控件、AX 树不可见，只能坐标点击且无法确认选中状态——**CLI 删除（prlctl snapshot-delete）更可靠**。
- 删除快照建议在 VM 停止状态进行，删完用 `ls .../harddisk.hdd/ | grep -c .hds` 核对层文件数，防止孤儿残留。
- DiskDescriptor.xml 的 `<UID>{e54cbaf2...}</UID>` 是磁盘自身 UID 不是层文件；层文件只认 `<Image><GUID>` 列表。

# ARM64 物理机（高通骁龙 X Elite / Surface）实机引导与部署验证 Checklist

> 时点：2026-11-03 15:10
> 适用版本：v2.7.8+
> 目标硬件：基于高通骁龙平台（Snapdragon X Elite / X Plus / 8cx Gen 3）的 ARM64 物理机（如 Surface Pro 11 / Surface Laptop 7 / ThinkPad T14s Gen 6 ARM / Dell XPS 13 ARM 等）。
> 编制依据：`AGENTS.md` 硬约束、`经验/07-墓碑与争议清单.md`、`crates/backuprestore-cli/src/`（主模块解耦后 5 大自治子系统）。

---

## 一、背景与环境差异分析（VM vs 物理机）

在 Parallels Desktop 虚拟机中，开发团队已全面打通了全套单测（404 项）、GUI 自动化布局（40 组无遮挡无截断）、无控制台静默外部调用（B-2694 根治）、0 unwrap 安全加固以及 `main.rs` 彻底解耦。

但在将产物部署至真实 ARM64 物理机时，存在以下**核心固件与硬件环境差异**：

| 关注维度 | Parallels Desktop VM 现状 | ARM64 物理机（高通 SoC）现实 | 潜在风险与防御手段 |
| :--- | :--- | :--- | :--- |
| **BitLocker / 设备加密** | 默认未启用加密（状态 0） | 出厂现代待机（Modern Standby）默认开启全盘加密 | **硬规则**：BackupRestore 绝不自动修改 BitLocker。必须在还原前由用户手动挂起（Suspend）或完全解密，严防抹盘后密钥丢失变砖。 |
| **`bootsequence` 一次性启动** | Parallels UEFI 固件**不消费**该字段（`经验/07` 表格 58 条目） | 标准 UEFI 固件严格消费 `bootsequence`，单次重启生效后自动清空 | 物理机上可完美利用原生 `bootsequence` 路由，减少对 `displayorder` 临时置顶的侵入性修改。 |
| **ESP 分区盘符状态** | 测试脚本有时临时指派 `S:` 盘符 | 物理机默认 ESP 是**隐藏无盘符卷** | 核心逻辑必须 100% 依赖 verbatim 路径 `\\?\Volume{GUID}\EFI\...`，禁止假定 ESP 具有盘符。 |
| **外设与输入驱动** | 宿主机 VirtIO 虚拟鼠标/键盘即插即用 | WinPE / WinRE 默认缺高通专有触摸屏/触控板 I2C 驱动 | 物理机离线恢复阶段必须备妥 USB 物理外接键盘或鼠标，防止因触摸失效无法确认。 |
| **存储控制器驱动** | 标配虚拟 NVMe 控制器 | 高通 UFS 4.0 / PCIe 4.0 NVMe 控制器专有驱动 | 还原第三方镜像前必须确保驱动集或提前注入，防止蓝屏 `INACCESSIBLE_BOOT_DEVICE`。 |
| **屏幕物理 DPI** | 典型 100% ~ 150% 虚拟分辨率 | 物理屏多为 2.8K/3K 视网膜屏（200% ~ 250% 缩放） | 严格遵守 `dpi::init()` 系统感知声明，禁止出现截断与重叠。 |

---

## 二、部署前准备与环境准入审查（Pre-deployment Gate）

在真实物理机上运行 BackupRestore 之前，必须逐一核验以下先决条件：

### 1. BitLocker 保护状态核验
- [ ] **执行只读查询**：
  ```powershell
  manage-bde -status
  ```
- [ ] **核验 Protection Status**：
  - 若为 `Protection On`：
    - **备份操作**：只读卷不受写保护限制，但还原需要解密目标。
    - **还原/擦除操作**：必须在 Windows 提升管理员命令行中执行挂起：
      ```powershell
      Suspend-BitLocker -MountPoint "C:" -RebootCount 0
      ```
    - 或在 Windows 设置中彻底关闭“设备加密”，等待解密完成。
- [ ] **48 位恢复密钥备份**：
  - 确认用户已导出并离线妥善保存目标机器的 BitLocker Recovery Key（打印纸质件或存入安全 USB 盘）。

### 2. UEFI 安全启动与 BCD 基线导出
- [ ] **确认 Secure Boot 状态**：
  ```powershell
  Confirm-SecureBootUEFI
  # 返回 True 即为正常；BackupRestore 部署的引导程序均基于官方微软签名 boot loader。
  ```
- [ ] **在线态提权导出 BCD 基线（按 AGENTS.md §二）**：
  ```cmd
  bcdedit /export C:\bcd-baseline.bak
  ```
  - 核验判据：文件非空（> 400KB）且 `bcdedit /store C:\bcd-baseline.bak /enum {bootmgr}` 能成功枚举。
- [ ] **手抄 `{bootmgr}` 关键配置**：
  - 抄录 `default`、`displayorder`、`resumeobject` 三个 GUID 留存备查。

### 3. 物理机驱动备份（灾备底线）
- [ ] **导出当前系统已安装的高通 SoC 专有驱动**：
  ```powershell
  Export-WindowsDriver -Online -Destination "H:\QualcommDrivers"
  ```
  - 确保还原通用或异机 Windows 镜像时，可通过 DISM 随时离线注入驱动：
    ```cmd
    dism /image:C:\ /add-driver /driver:H:\QualcommDrivers /recurse
    ```

---

## 三、离线恢复环境（WinRE / WinPE）实机核查（Offline Gate）

### 1. 物理机 WinRE 注册完整性
- [ ] **执行 `reagentc /info`**：
  - 确认 `Windows RE status: Enabled`；
  - 确认 `Windows RE location` 指向合法的 GPT 恢复卷；
  - 验证 `winre_registration.rs` 探测组件：确保系统内包含合法的 `reagentc.exe` 与 `SYSTEM` 配置单元。

### 2. 独立 WinPE 部署与 Shell 契约核验（v2.6.75 守卫）
- [ ] 若使用安装独立 WinPE 桌面：
  - 检查镜像壳（`wimlib-imagex dir` 确认 `\Windows\System32\winpeshl.ini` 存在）；
  - 确认 `winpeshl.ini` 指向 `Recovery.exe --pe-desktop` 且内容严格匹配 61 字节权威规范；
  - 确认镜像内包含 `BackupRestore.exe`、`Recovery.exe`、`wimlib-imagex.exe` 及 `libwim-15.dll` 全套四件套。

### 3. 物理外设接入与显示确认
- [ ] **接入物理外设**：由于多数 ARM64 机器在最小 WinPE 下缺少触摸固件，备妥 USB-C 扩展坞与物理键盘/鼠标；
- [ ] **DPI 与布局核验**：进入 WinPE 恢复桌面后，确认标题栏、操作选项卡（备份、还原到C盘、第二系统、安装PE）与按钮文字无任何截断或溢出。

---

## 四、执行与状态机转移验证（Execution Gate）

### 1. 挂起引导任务与断电自愈（`pending_task.rs`）
- [ ] **阶段原子转移**：
  - 观察 `status.json` 中的 `Stage` 转移顺序：`BootRequested` → `RecoveryStarted` → `TargetErased` → `ImageApplied` → `BootRepaired` → `RecoveryComplete`；
- [ ] **单阶段防打转与总尝试限制**：
  - 在物理机冷重启时，验证 `claim_boot_resume_attempt` 确保同一阶段仅允许自愈 1 次；跨阶段总尝试次数不超过 3 次；
  - 若人为模拟断电或硬件重启，系统应能自动识别断点并继续续跑，而不是无限死循环。

### 2. 目标分区 DiskPart 格式化与卷身份核验（`recovery_volume.rs`）
- [ ] 格式化脚本生成核验：
  ```text
  select disk <verified_disk>
  select partition <verified_partition>
  format fs=ntfs quick
  ```
  - 严禁假定卷号，必须经由 `PARTITION_INFORMATION_EX` 的 GPT `PartitionId` GUID 严密双向对齐后方可执行。

### 3. wimlib 还原流转与引导修复（`recovery_engine.rs` + `recovery_bcd.rs`）
- [ ] 还原执行：
  - 验证 `wimlib-imagex.exe apply` 是否成功保留全部 Windows 安全描述符（ACL）与符号链接；
- [ ] 引导初始化：
  - 验证 `bcdboot <Target>:\Windows /s <Verbatim_EFI> /f UEFI` 的正确执行；
  - 双系统场景下，验证 `preserve_primary_boot_manager` 严格恢复主系统 `{default}` 并将新加载器追加至 `displayorder` 末尾。

---

## 五、首次开机与收尾核验（Post-Flight & Verification）

### 1. 首次引导 Windows 正常登入
- [ ] 恢复完成后自动或手动重启，观察固件加载画面；
- [ ] 确认系统顺利通过驱动即插即用（PnP）阶段，无蓝屏（如 `INACCESSIBLE_BOOT_DEVICE` 或 `SYSTEM_THREAD_EXCEPTION_NOT_HANDLED`）；
- [ ] 成功进入登录界面或 OOBE 桌面。

### 2. 盘符泄漏与临时租约清扫（`letter_trace` & `mount-leases.log`）
- [ ] 检查 `%ProgramData%\BackupRestore\logs\mount-leases.log`：
  - 确认所有由 `hold_for_process()` 临时申请的盘符已全部标记 `RELEASED`；
- [ ] 检查注册表 `HKLM\SYSTEM\MountedDevices`：
  - 确认无残留的孤儿 `\DosDevices\<Letter>:` 幽灵盘符（彻底免疫 Explorer 登录弹窗）。
- [ ] 运行 `./tools/vm/postflight.sh`（或物理机对应检查脚本），确保：
  ```text
  ITEM=ghost_letter status=pass
  ITEM=orphan_console status=pass count=0
  POSTFLIGHT=clean
  ```

---

## 六、紧急逃生与故障回滚手册（Escape Protocol）

若物理机还原过程中出现意外中断或引导失败，按以下流程快速恢复：

1. **若机器无法引导进入 Windows**：
   - 插入预先制作的官方 Windows 11 ARM64 安装 U 盘或救援 PE；
   - 启动进入“命令提示符”；
2. **还原 BCD 引导存储**：
   ```cmd
   bcdedit /import C:\bcd-baseline.bak
   ```
3. **若需重新写入 EFI 引导文件**：
   ```cmd
   diskpart
   list volume
   select volume <ESP_VOLUME_NUMBER>
   assign letter=S
   exit
   bcdboot C:\Windows /s S: /f UEFI
   mountvol S: /d
   ```
4. **检查日志定位根因**：
   - 查看目标盘中的 `BackupRestore-logs\` 或 `tasks\<id>\recovery.log`，提取 `[BR-xxx-xxx]` 错误代码进行精准排查。

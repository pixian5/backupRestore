# 宿主↔VM 共享盘根因、修复与 build-win.sh deploy 方案落地（2026-09-28）

## 0. 一句话结论

Parallels 共享文件夹（Shared Folders / `prl_fs` 虚拟通道）只在**客体有交互登录会话**时才建立到宿主共享服务的连接。之前"坏"是 **Tools 降级 + 无登录会话**叠加导致。用户重装 Tools + 重启 + 登录后通道恢复。据此把 `build-win.sh --deploy` 从「`--current-user` + 盘符 `X:`」改成「SYSTEM 通道 + UNC `\\Mac\backupRestore`」，端到端验证通过。

---

## 1. 现象（坏期）

- `prlctl exec` 的 **SYSTEM 通道本身正常**（`whoami` = `nt authority\system`），所以坏的是「客体↔宿主共享端点」这条链，不是 exec 通道。
- `net view \\Mac` → **1702（找不到网络路径）**；`ping Mac` → 主机名解析不到；`X:` 盘（映射 `\\Mac\backupRestore`）在 SYSTEM exec 下不可见。
- 宿主侧 `prl_disp_service` / `prl_naptd` / `prl_vm_app` 都在跑。关键点：**Parallels 共享文件夹走虚拟通道（`prl_fs` 驱动 ↔ 宿主 `prl_disp_service`），不是标准 TCP 445 SMB** → `lsof` 看不到 445 监听是正常，不是病因；且宿主**没有可独立重启的共享守护**，共享是 `prl_disp_service` 内的一部分。
- 客体侧 `prl_fs`（共享文件夹 FS 驱动）**已加载**，但 `prl_tools_service.exe` 在预期路径（`C:\Program Files (x86)\Parallels\Parallels Tools\`）找不到 → 客体 Parallels Tools 安装处于**不一致状态**（用户之前降级 Tools 到 26.4.2）。
  - 注：后来发现真实二进制在 `C:\Program Files\Parallels\Parallels Tools\Services\prl_tools_service.exe`（native ARM64 + `Services` 子目录），前面查 `(x86)` 路径是假阴性；Tools 二进制其实是装好的。决定性信号仍是 `\\Mac` 重定向器连不上。

---

## 2. 根因

- Parallels Shared Folders 的 **`prl_fs` 重定向器只在客体有用户登录 console 时**，才会去和宿主共享服务建立虚拟通道。
- 之前坏期 = **Tools 降级致用户态桥接不全 + 无交互登录会话** → `\\Mac` 不可达。
- 三个关键事实（判共享、写脚本必须遵守）：
  1. **`--current-user` 在无登录会话时回传占位 `p8b6\x`**（rc=0 但命令没真跑）→ 不能用来判共享或跑真实命令。不加 `--current-user` 即 SYSTEM，永远可用。
  2. **盘符（`X:`/`Z:`）是会话级**，属于交互用户，SYSTEM 的 `prlctl exec` 看不到；必须用 **UNC `\\Mac\backupRestore`**（SYSTEM 下可读写）。
  3. **`net view \\Mac` 报 1702 是假警报**：Parallels 虚拟共享的「服务器名枚举」常返回 1702，但显式 UNC 路径 `\\Mac\backupRestore` 是通的。**判共享好坏别用 `net view`，直接测显式 UNC。**

---

## 3. 修复（用户操作）

用户重装 Parallels Tools（→26.4.2）、重启 VM、并以用户 **x 在 console 会话登录**后：

- `query user` 确认有活跃会话；`net use` 列出 `X:→\\Mac\backupRestore`、`Z:→\\Mac\Home`（会话级，SYSTEM 看不到）。
- **正确测法用 UNC 而非盘符**：
  - `dir \\Mac\backupRestore` → 正常列出项目目录；
  - `dir \\Mac\backupRestore\target\aarch64-pc-windows-msvc\release\BackupRestore.exe` → 读出 **1,697,792 字节**；
  - guest→host 写 `\\Mac\backupRestore\__br_share_test.txt` → **WRITE_OK**，已清理 ✅。
- 结论：host→guest 读、guest→host 写都通，共享盘问题已解决。

---

## 4. build-win.sh --deploy 方案决策与落地

### 4.1 两个选项

- **A（HTTP/curl）**：宿主 `python3 -m http.server --bind 10.211.55.2` + 客体 `curl` 拉 exe。
  - 优点：**不依赖登录会话**（headless / 无登录也通），因为只依赖「VM 运行（虚拟网卡 UP）+ SYSTEM exec 通道」。
  - 缺点：多起一个进程；走虚拟网卡 TCP 栈，比共享盘直通道多一层。
- **B（共享盘 UNC）**：继续用共享盘，但把第 81 行 `X:\target\...` 改 UNC、`--current-user` 去掉。
  - 优点：走 Parallels 半虚拟化共享直通道，**比 A 更快**、步骤更短、少一个进程。
  - 缺点：**依赖 VM 有登录会话**（headless 时 UNC 会再掉）。

### 4.2 用户决策：选 B

理由：用户通常都会登录 VM，headless 不掉这条约束不成立；且 B 比 A 快、更简单。A 保留为"真要无人值守自动部署"时的备选。

### 4.3 改动（build-win.sh 第 73–98 行 deploy 段）

- 第 81 行（真正拷贝 exe 那步）：
  - 去掉 `--current-user` → 统一用 **SYSTEM 通道**（`prlctl exec "Windows 11" cmd /d /c "..."`）；
  - `X:\target\aarch64-pc-windows-msvc\release\...` → `\\Mac\backupRestore\target\aarch64-pc-windows-msvc\release\...`；
  - `X:\windows\winpe-winpeshl.ini` → `\\Mac\backupRestore\windows\winpe-winpeshl.ini`。
- 转义（bash 双引号内）：UNC 前缀写 `\\\\Mac`（→ 输出 `\\Mac`），路径分隔写 `\\`（→ 输出单 `\`）。
- 注释同步订正：说明 SYSTEM 通道 + UNC 的取舍，以及"共享盘能否用仍取决于是否登录"。

### 4.4 验证

`./build-win.sh --deploy` 端到端通过：
- 构建产物 **1,697,792 字节**；
- 经 UNC 拷到 `C:\Users\Public\backupRestore-package\`（BackupRestore.exe + Recovery.exe + winpe-winpeshl.ini）；
- SHA-256 比对一致，打印 `>> DEPLOYED SHA256=c248bfb3a1f5c5d9ba86c051aa43305174c9f71f403ad650c07a3f37d67b2d80` 并走到 `>> 完成` ✅。
- 构建期的 `LNK4099`（缺 PDB 调试符号）每次都有，无害。

---

## 5. 遗留约束

- **B 方案依赖 VM 有登录会话**；headless / 无登录时 UNC 会再掉。真要无人值守自动部署再切 A（HTTP/curl）。
- `prl_exec_retry` 对 `Invalid argument` 抖动重试（见 build-win.sh 顶部注释）；真正的成功判据只有 **DEPLOYED 标记 + SHA-256 比对**，重试不会放过真实失败。

---

## 6. 相关文件

- `build-win.sh`（第 73–98 行 deploy 段）
- `.workbuddy/memory/2026-09-28.md`（同主题日志，含验证 SHA 与决策记录）
- `docs/vm-input-control-guide.md` 第十节（SYSTEM 多参数姿势、p8b6 占位说明、共享盘坏改 stdout 回传）

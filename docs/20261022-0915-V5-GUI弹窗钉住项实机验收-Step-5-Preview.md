# 2026-10-22 09:15 V5（GUI 弹窗取消勾选主系统）实机验收（Step-5-Preview）

> **结论一句话**：V5 实机通过——PE 恢复进度窗按 `M` 打开「引导修复」编辑器，
> 对「当前默认启动项·不可取消」的主系统按**空格取消勾选被拒绝**（√ 不丢失），
> 对「正在还原中的系统」同样拒绝；最终 BCD `default` 指向本次还原的新系统、
> 老系统完整留在 `displayorder`、**菜单名保持不变**——全程没有任何静默的 default 篡改。
> **#40 的 V 系列（V1/V2/V3/V4/V5/V6）全部结清。**

---

## 一、用例与通过判据

用例定义（`docs/20261015-0725` §6 验收矩阵 V5 行）：
**弹窗取消勾选主系统（第二系统场景）→ 明确报错，不静默改 default。**

| # | 判据 | 实测 | 结论 |
|---|---|---|---|
| 1 | 编辑器明确告知钉住规则（A4 裁定） | 静态文本：「正在还原的系统不可取消；『当前默认启动项』也不可取消，但还原完成后默认启动的是本次还原的新系统，原系统仍留在菜单里可选。空格勾选，Shift+↑↓排序，F2改名，Esc关闭。」 | ✅ |
| 2 | 列表项逐项标注 | `[√] 1. 本次还原的 Windows (C:) [当前默认启动项·不可取消，还原后由新系统接管]` / `[√] 2. V5Test (F:) [当前正在还原中的系统·Step 4生效]` | ✅ |
| 3 | **取消勾选主系统被拒** | 光标移到第 1 项按空格 → **√ 保持**（`boot_menu::toggle` 对 `is_pinned` 直接返回 false） | ✅ |
| 4 | 取消勾选还原目标被拒 | 光标移到第 2 项按空格 → **√ 保持** | ✅ |
| 5 | 不静默改 default | 终态 `default={75a727f5}`（V5Test，本次还原系统）；老系统 `{2ef03cd4}` 仍在 displayorder；日志「引导菜单名称保持不变：本次还原的 Windows」 | ✅ |
| 6 | 任务终态 | ✅ `742cfd28` create-secondary `success/100%`（01:04:07Z） | ✅ |
| 7 | 哨兵法 + 序列号 | ✅ `F:\BR-v5-sentinel.txt` 消失；F: `38E7-CC30` → 新系统卷 `387A-0D37` | ✅ |
| 8 | 三件套 | ✅ EventID=1000 最近 3 条全部 ≤ 09-30（镜像自带）；WerFault 零进程；新系统实机启动进桌面 | ✅ |

---

## 二、实机操作记录（2026-10-22 08:57–09:10 本地）

| 时间 | 事件 |
|---|---|
| 08:57:13 | F: 放哨兵 `BR-v5-sentinel.txt`（F: `38E7-CC30` 空卷）；`prepare --operation create-secondary --source-drive C --target-drive F --image-path H:\BRImages\br-2619-cur.wim --wim-index 1 --boot-menu-name V5Test --allow-destructive --no-reboot` → 任务 `742cfd28` |
| 08:58:07 | 查载荷在 → 立即 `resume`（EXIT=0） |
| 08:59:09 | 进 PE，STEP 2 还原镜像 3%，「引导修复(M)」按钮已在位 |
| 09:00:3x | **按 `M` 打开引导修复编辑器**（截图 v5-02）：两条钉住项、静态说明、光标默认落在还原目标项 |
| 09:00:5x | 按 `↑` → 高亮移到第 1 项（主系统），证明 LIST 已获焦点 |
| 09:01:0x | **按空格 → √ 保持，取消勾选被拒**（截图 v5-04） |
| 09:01:3x | 按 `↓` 回第 2 项 → 按空格 → √ 保持，再次被拒（截图 v5-05） |
| 09:01:4x | 按 `Esc` 关闭编辑器，还原继续（83%） |
| 09:04:07 | `Recovery completed` → STEP 4 清理 → 重启 |
| 09:05 | 新系统（V5Test）成为 default 并启动，**盘符重排**：新系统抢 C: `387A-0D37`，老系统退 E: `8E2F-5FC1`；GUI 自启弹镜像带回的 02138f14 待续跑框（与 P5 轮同型），按 Esc=否 |
| 09:08 | 逐项取证（判据表全部 ✅） |
| 09:09–09:11 | 清理：`bcdedit /default {2ef03cd4}` → 重启回老系统（C: `8E2F-5FC1`）→ `displayorder /remove` + `/delete {75a727f5} /f` → `format F: /FS:NTFS /Q /Y`（新序列号 `3452-FE6C`） |

---

## 三、关键日志（`H:\BRImages\Recovery.log`，UTC）

```
01:04:03 第二系统克隆注册配置已在目标卷内备份并重置；原系统恢复对象未禁用
01:04:05 WinRE registration verified: Enabled; target location, recoverysequence, ramdisk and original hashes match
01:04:06 Secondary Windows loader {75a727f5-cdb4-11f1-9713-a945446d0ef5} named V5Test
01:04:06 Restored 1 original display-order entries and appended secondary loader {75a727f5…} (default left to the boot-menu plan)
01:04:06 引导菜单名称保持不变：本次还原的 Windows
01:04:06 默认启动项裁决：{75a727f5-cdb4-11f1-9713-a945446d0ef5}；指向本次还原出来的系统
01:04:07 引导修复完成：菜单 2 项，显示 10 秒；现有 PE 入口保留
01:04:07 Recovery completed → STEP 4/4 → one-shot bootsequence cleared → staging removed and verified
```

---

## 四、实现机理（为什么不可能静默改 default）

两层防护，V5 实机走的是第一层：

1. **交互层**（`boot_menu.rs::toggle`）：`is_pinned(entry)`（`is_restore_target` 或 `is_current_default`）直接返回 false，勾选框根本不翻转——实机表现就是「按空格没反应」。
2. **持久层**（`BootMenuPlan::validate`）：任何途径落盘的方案若含未勾选的钉住项，报
   「当前默认启动的系统不能从启动菜单中移除」/「本次还原系统必须且只能出现一次，不能取消」，
   `BootMenuSession::edit` 把该错误写进 `state.error`，`freeze()`（STEP 4 前）直接失败——
   防止绕过 GUI 直接改 `boot-menu-plan.json`。
3. **界面告知**（2026-10-15 用户裁定 A4）：静态说明文本 + 每条钉住项的行尾标注，
   把「还原后默认启动项归新系统、原系统仍可选」说死，不让用户误以为原系统会被踢出。

单测侧 `boot_menu.rs` 已有 `validate_requires_exactly_one_selected_target` /
`validate_rejects_incomplete_identity_and_duplicates` 等锁死判据；本轮补的是 GUI 层实机。

---

## 五、环境终态（09:15）

| 项 | 值 |
|---|---|
| 运行系统 | C: `8E2F-5FC1`（V1 还原的系统，`本次还原的 Windows`，default） |
| F: | 350GB 空卷 `3452-FE6C`（V5 测试系统已删） |
| BCD | `default={2ef03cd4}`、displayorder 单条、`bootsequence` 空 |
| 二进制 | 老系统包目录 + H:\brwork 均 v2.6.24 `CD401F92…`（未受第二轮「镜像回滚」影响——V5 只动 F:） |
| 任务 | `H:\brwork\tasks` 11 个（新增 `742cfd28` V5 success） |
| 宿主 | ~140Gi 空闲；无快照 |

> 备注：V5 重启后新系统自带的包目录又是镜像里的 v2.6.19 + 02138f14（Capturing）——
> 但该系统已被删除，无需处理；**当前运行的老系统包目录未被动过**（V5 只格式化 F:）。

---

## 六、证据文件（`.test-artifacts/recon-20261020/`）

| 文件 | 内容 |
|---|---|
| `v5-prepare-20261022.txt` | 哨兵 + prepare 输出与任务 id |
| `v5-resume-20261022.txt` | 载荷检查 + resume |
| `v5-session-log.txt` | Recovery.log 本轮 49 行（含 default 裁决与菜单名保持不变） |
| `v5-verify-20261022.txt` / `v5-verify2-20261022.txt` | 任务状态/哨兵/序列号/BCD/WinRE + 崩溃信号 |
| `/tmp/v5-02…09.png` | 编辑器全貌 / ↑移动 / 对主系统空格被拒 / 对目标空格被拒 / 关闭 / 回老系统 |

---

## 七、#40 结语与下一步

**V 系列（V1–V6）按新 default 规则全部实机结清**：V1（`b4cec607`，A 入口动 C:）、
V2（`79445787`）、V3（10-15）、V4（`189e0b2d`）、V5（`742cfd28`）、V6（随各例三件套）。

**剩余唯一大项：#38**——A/B 入口 + 备份 + PE 安装全链复跑（需先拍快照，合并专场一轮）。

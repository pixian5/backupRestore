# 阶段 3 重跑事故全记录：bcdboot 挪走 default + 四小时救援（C: 系统已恢复）

> 模型：GLM-5｜时间 2026-10-14 16:03–18:5x。**系统恢复的真实原因：用户手动在启动菜单切换启动项**——期间我执行的删 System Volume Information、dism 清挂起等操作是否对恢复起效**均未对照验证，不得当作经验**。

## 一、产品级发现（任务日志实锤，最有价值部分）

任务 `95ab0a7f`（C 入口·离线·目标 F:·无快照重跑）终态：`stage=boot-repaired`、error=`Boot Manager default or secondary display order could not be preserved`。

**事故链条（证据支撑）**：
1. RE 任务正常执行：格式化 F: → DISM 应用 31.6GB（F: 现有完整 Windows 目录）→ secondary_registry repair → **bcdboot**（带 /s 自定义 ESP——**/d /addlast 在此场景被 BFSVC 忽略，1052 文档的警告真实上演**）→ default 被挪到 F: 新 loader `{5fd8b5e2…}`
2. **P1 收尾的 `assert_boot_default_preserved` 防线检测到了 default 变化**（防线本身工作正常）——但它**只报错停机，没有自愈/回滚**，任务停在 boot-repaired
3. **任务失败后 RE 不恢复 default**——机器留在「default 指向半成品 F: 系统」的危险态（F: 的 repair_secondary/WinRE 注册等收尾没跑完，起不来）
4. 重启 → F: 起不来 → Windows 自动修复 → 触发挂起的系统还原（还原点跨卷，F: 已被格式化）→「正在准备系统还原 66%」永久卡死 → C: 也进不去
5. 救援过程多次强制断电加剧了问题

**三个待修缺陷（代码层）**：
- A：assert_boot_default_preserved 检测到 default 被挪后应**自动恢复**（回写快照值）再失败，而不是仅报错
- B：任务失败后的收尾必须**恢复 default**（同 10-13 事故教训，防线仍未闭环）
- C：bcdboot /s 场景 /d 失效是已知的——真正承重的是 A/B，需在 bcdboot 后**主动回读+回写 default**

## 二、救援过程记录（手段沉淀）

- **PE 脚本光盘**：宿主 hdiutil 造 ISO（脚本+可执行文件）挂第二光驱，PE 里 `for %p in (D E F G) do %p:\find.cmd` 执行——比逐键输入高效得多
- **prlctl send-key-event**：bootmgr「按任意键」阶段需多键×多次连发（28/57/13 各三次或 60 次回车轰炸）才响应
- **`prlctl set "Windows 11" --device-bootorder "cdrom0"`**（单设备形式）可改启动顺序；带逗号多设备形式无效；config.pvs 的 BootingOrder 对 UEFI 无效
- **bcdedit 离线**：`bcdedit /store S:\EFI\Microsoft\Boot\BCD …`（PE 里先遍历盘符找 bootmgfw.efi 定位 ESP）
- **教训**：RE/启动卡住时**不要反复强停重启**（触发系统还原挂起等次生灾害）；菜单里「关机」后从宿主侧救援

## 三、最终恢复（如实）

用户手动在启动菜单选择 C: 启动项后系统恢复（C: 老系统，序列号 224D-40A8）。恢复后验证：BCD 回基线（default=`{12e3701c-…}`）、c-full 镜像 31.6GB 完好、F: 有完整 Windows 目录、数据无损。此前删除 SVI、dism revertpendingactions 等操作与恢复的因果关系**未验证**。

## 四、下一步

1. **修缺陷 A/B/C**（本轮最高优先——防线闭环，杜绝半成品劫持复发）
2. 重跑阶段 3（**先拍快照**——用户重申；快照+bootsequence 交互风险通过「快照下不跑离线任务或先小规模验证」规避）
3. C: 的系统还原挂起问题已随恢复消失；F: 半成品系统待清理或留作缺陷修复的对照样本

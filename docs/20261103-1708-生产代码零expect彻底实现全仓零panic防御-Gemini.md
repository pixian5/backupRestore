# 生产代码零 expect 彻底实现全仓零 panic 防御

- 时点：2026-11-03 17:08
- 模型：Gemini
- 对应版本：`v2.8.2`
- 关联提交：HEAD

---

## 1. 演进背景与审计

在 `v2.7.6` 中，全仓完成了对生产代码（非 test / 非断言）`.unwrap()` 的清零工作，消灭了 42 处 Mutex unwrap 隐患并建立 `SafeLock` 机制。
然而对全仓生产代码的深度静态扫描发现，工程内仍残存 **9 处 `.expect(...)`** 调用：
1. `recovery_fault.rs`（3 处）：`ACTIVE.lock().expect("验收配置锁")` 在故障注入配置与检查点直接 expect，若其他线程发生异常将引发锁毒化并造成二次 panic；
2. `pending_task.rs`（1 处）：`pending.pop().expect("pending length checked above")`；
3. `windows_prepare.rs`（2 处）：`image_path.as_ref().expect("restore image path validated")` 与 `boot_entry.expect("checked above")`；
4. `recovery_engine.rs`（2 处）：`task.image.as_ref().expect("image path implies image")` 与 `previous_boot_manager.as_ref().expect("secondary BCD state")`；
5. `native_gui/task_executor.rs`（1 处）：`tasks.into_iter().next().expect("tasks 非空")`。

虽然这些 expect 的上下文在正常逻辑分支下均有前置长度或状态检查，但从防御性编程（Defensive Programming）和系统恢复环境的高可靠性要求来看，**任何在离线恢复链或后台调度主循环中留存的 panic 隐患，都会在异常边缘场景下破坏回滚与自愈能力**。

---

## 2. 核心重构实施

### 2.1 彻底消灭 9 处 `.expect(...)`
1. **故障注入锁毒化免疫（`recovery_fault.rs`）**：
   - 3 处锁获取改造为 `ACTIVE.lock().unwrap_or_else(|e| e.into_inner())`，确保锁毒化时依然能取出配置数据，不发生级联 panic；
2. **安全模式匹配与早期返回（`pending_task.rs` & `task_executor.rs`）**：
   - 将 `pending.pop().expect(...)` 改造为 `let Some((mut task, task_dir)) = pending.pop() else { return Ok(None); };`；
   - 将 `tasks.into_iter().next().expect(...)` 改造为 `let Some(task) = tasks.into_iter().next() else { return None; };`；
3. **准备阶段路径与启动项强类型解构（`windows_prepare.rs`）**：
   - 镜像路径提取改造为安全错误码分流：`let Some(path) = image_path.as_ref() else { return Err(crate::errc(ErrCode::ImageFileMissing)); };`；
   - 补偿检查后的启动项解包改造为地道的错误传递：`let mut boot_entry = boot_entry?;`；
4. **恢复引擎状态契约解绑与错误码闭环（`recovery_engine.rs`）**：
   - 镜像路径与镜像对象解构改造为复合模式匹配：`if let (Some(path), Some(image)) = (&image_path, task.image.as_ref())`；
   - BCD 引导管理器快照提取缺失时拒绝 panic，安全返回稳定错误码：`return Err(crate::errc(ErrCode::BootManagerUnknownContinue));`。

---

## 3. 验证指标

1. **静态扫描验证（双零指标）**：
   - `Total unwraps in non-test code`: **0**；
   - `Total expects in non-test code`: **0**；
   - 生产代码达成 100% 零裸露 panic 风险调用。
2. **单测与门禁**：
   - 全仓 **405 项单元测试全部通过**；
   - `cargo clippy --all-targets` 达成 **0 warnings**；
   - Windows ARM64 交叉编译成功（`build-win.sh` 产物 3,461,632 字节，0 告警）；
   - `tools/doc/stale-claims.sh` 结果 `STALE_HITS=0, RESULT=PASS`。

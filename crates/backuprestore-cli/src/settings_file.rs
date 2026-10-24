//! 设置文件名的**单一来源**。
//!
//! 为什么单独一个模块：`BackupRestore.settings.ini` 有三个写者（`failure_policy`
//! 的失败策略、`dism_timeout` 的等待上限、`native_gui` 的界面语言），而读者里
//! 有个特殊角色——`i18n` 是**无条件编译**的（`text_parsing` 要用它的判据文案），
//! 它也要读这个文件来取语言偏好。
//!
//! 于是这个常量不能住在 `failure_policy`：那是 `cfg(any(windows, test))`，
//! 一旦 `i18n` 在非 Windows 构建下引用它，`cargo check` 直接 E0433
//! （2026-10-25 第二次踩到同一类门控坑，第一次是 `MessageLanguage`）。
//!
//! 放在这里：无平台依赖、无条件编译，谁都能引用。

pub(crate) const SETTINGS_FILE_NAME: &str = "BackupRestore.settings.ini";
